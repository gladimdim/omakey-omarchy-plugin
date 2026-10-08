//! The desktop clipboard for phones (PROTOCOL.md, CLIP). Reading and
//! setting it means running wl-paste and wl-copy, which can take a while
//! (the app that owns the clipboard serves it), so a worker thread does it
//! and the server picks up the results on its tick: no key ever waits.

use crate::protocol::MAX_CLIP;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

/// A copy waits this long at most for the clipboard to change.
const COPY_WAIT: Duration = Duration::from_secs(1);
const COPY_POLL: Duration = Duration::from_millis(50);
/// wl-paste or wl-copy taking longer than this is given up on.
const TOOL_TIMEOUT: Duration = Duration::from_secs(2);
/// After wl-copy, how long to wait for the clipboard to read back the text.
const SET_WAIT: Duration = Duration::from_millis(300);

/// Clipboard text and whether the desktop marked it sensitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    pub bytes: Vec<u8>,
    pub sensitive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipError {
    /// Nothing copied, or nothing that is text.
    Empty,
    TooLarge,
    /// No clipboard to reach, or the tool failed.
    Failed,
}

pub type Contents = Result<Text, ClipError>;

/// Which transfer a job is for: session id and clip id.
pub type Tag = (u32, u32);

pub enum Job {
    /// Read the clipboard. With `changed_from`, read again until it differs
    /// from that, for at most [COPY_WAIT], and answer with the last read.
    Read { tag: Tag, changed_from: Option<Contents> },
    Write { tag: Tag, text: Text },
}

pub enum Done {
    Read { tag: Tag, contents: Contents },
    Write { tag: Tag, ok: bool },
}

/// Where the text actually goes: wl-clipboard, or a fake in tests.
pub trait Backend: Send + 'static {
    fn read(&mut self) -> Contents;
    fn write(&mut self, text: &Text) -> bool;
}

pub struct Clipboard {
    jobs: Sender<Job>,
    done: Receiver<Done>,
}

impl Clipboard {
    pub fn start(mut backend: Box<dyn Backend>) -> Clipboard {
        let (jobs, job_rx) = channel::<Job>();
        let (done_tx, done) = channel::<Done>();
        std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                for job in job_rx {
                    let done = match job {
                        Job::Read { tag, changed_from } => {
                            let deadline = Instant::now() + COPY_WAIT;
                            let mut contents = backend.read();
                            if let Some(before) = changed_from {
                                while contents == before && Instant::now() < deadline {
                                    std::thread::sleep(COPY_POLL);
                                    contents = backend.read();
                                }
                            }
                            Done::Read { tag, contents }
                        }
                        Job::Write { tag, text } => Done::Write { tag, ok: backend.write(&text) },
                    };
                    if done_tx.send(done).is_err() {
                        return;
                    }
                }
            })
            .expect("clipboard thread");
        Clipboard { jobs, done }
    }

    pub fn submit(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    /// A finished job, if any.
    pub fn poll(&self) -> Option<Done> {
        self.done.try_recv().ok()
    }
}

/// The Wayland clipboard through wl-clipboard's wl-copy and wl-paste.
pub struct WlClipboard;

impl WlClipboard {
    /// Both tools are installed (Omarchy has them).
    pub fn available() -> bool {
        ["wl-copy", "wl-paste"].iter().all(|t| on_path(t))
    }

    fn command(tool: &str) -> Command {
        let mut c = Command::new(tool);
        // A service started before the compositor exported its environment
        // has no WAYLAND_DISPLAY: use the compositor's socket.
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            if let Some(d) = wayland_display() {
                c.env("WAYLAND_DISPLAY", d);
            }
        }
        c
    }
}

impl Backend for WlClipboard {
    fn read(&mut self) -> Contents {
        let types = run(Self::command("wl-paste").arg("--list-types"), 64 * 1024);
        let types = match types {
            Ok(t) => String::from_utf8_lossy(&t).into_owned(),
            Err(e) => return Err(e),
        };
        let sensitive = types.lines().any(|t| t.trim() == "x-kde-passwordManagerHint");
        let bytes = run(Self::command("wl-paste").args(["--no-newline", "--type", "text"]), MAX_CLIP)?;
        if bytes.is_empty() {
            return Err(ClipError::Empty);
        }
        Ok(Text { bytes, sensitive })
    }

    fn write(&mut self, text: &Text) -> bool {
        let mut c = Self::command("wl-copy");
        // Text even when it looks like something else (a path, HTML).
        c.args(["--type", "text/plain;charset=utf-8"]);
        if text.sensitive {
            c.arg("--sensitive");
        }
        // wl-copy forks to serve the clipboard and returns once it's set.
        // The fork lives on, so it gets no pipes we'd wait on.
        let Ok(mut child) = c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
            return false;
        };
        let wrote = child.stdin.take().is_some_and(|mut i| i.write_all(&text.bytes).is_ok());
        if !(wait(&mut child).is_some_and(|s| s.success()) && wrote) {
            return false;
        }
        // A paste follows at once: make sure the clipboard serves the new text.
        let deadline = Instant::now() + SET_WAIT;
        while self.read().map(|t| t.bytes) != Ok(text.bytes.clone()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }
}

/// Wait for a tool at most TOOL_TIMEOUT; None (and killed) if it's slower.
fn wait(child: &mut std::process::Child) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + TOOL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Run a tool and read at most `max` bytes of its output.
fn run(cmd: &mut Command, max: usize) -> Result<Vec<u8>, ClipError> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ClipError::Failed)?;
    // Read on threads, so a tool that hangs can be killed after TOOL_TIMEOUT.
    let mut stdout = child.stdout.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        // One more than allowed, to tell "too large" from "just fits".
        let _ = (&mut stdout).take(max as u64 + 1).read_to_end(&mut buf);
        // Drain the rest, so a huge clipboard ends now instead of at the timeout.
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        buf
    });
    let mut stderr = child.stderr.take().unwrap();
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = (&mut stderr).take(4096).read_to_string(&mut s);
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        s
    });
    let status = wait(&mut child);
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    match status {
        Some(s) if s.success() => {
            if out.len() > max {
                Err(ClipError::TooLarge)
            } else {
                Ok(out)
            }
        }
        // wl-paste with nothing copied, or nothing that is text.
        Some(_) if err.contains("Nothing is copied") || err.contains("No suitable type") => Err(ClipError::Empty),
        // Killed for reading too much: the text is over the limit.
        None if out.len() > max => Err(ClipError::TooLarge),
        _ => {
            let msg = err.trim();
            if !msg.is_empty() {
                eprintln!("omakeyd: clipboard: {msg}");
            }
            Err(ClipError::Failed)
        }
    }
}

fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file()))
}

/// The compositor's Wayland socket: `wayland-N` in the runtime directory.
fn wayland_display() -> Option<String> {
    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("wayland-") && !n.ends_with(".lock"))
        .collect();
    names.sort();
    names.into_iter().next()
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// An in-memory clipboard. `reads` answers reads first, one each,
    /// to play an app copying a moment after Ctrl+Insert.
    #[derive(Clone, Default)]
    pub struct Fake {
        pub contents: Arc<Mutex<Option<Text>>>,
        pub reads: Arc<Mutex<VecDeque<Contents>>>,
    }

    impl Backend for Fake {
        fn read(&mut self) -> Contents {
            if let Some(r) = self.reads.lock().unwrap().pop_front() {
                return r;
            }
            self.contents.lock().unwrap().clone().ok_or(ClipError::Empty)
        }

        fn write(&mut self, text: &Text) -> bool {
            *self.contents.lock().unwrap() = Some(text.clone());
            true
        }
    }

    #[test]
    fn tools_are_killed_on_timeout_and_output_is_capped() {
        let mut sleepy = Command::new("sh");
        sleepy.args(["-c", "exec sleep 10"]);
        let t = Instant::now();
        assert_eq!(run(&mut sleepy, 10), Err(ClipError::Failed));
        assert!(t.elapsed() < Duration::from_secs(5));
        let mut chatty = Command::new("sh");
        chatty.args(["-c", "printf 0123456789AB"]);
        assert_eq!(run(&mut chatty, 10), Err(ClipError::TooLarge));
        let mut echo = Command::new("sh");
        echo.args(["-c", "printf abc"]);
        assert_eq!(run(&mut echo, 10), Ok(b"abc".to_vec()));
        let mut empty = Command::new("sh");
        empty.args(["-c", "echo 'Nothing is copied' >&2; exit 1"]);
        assert_eq!(run(&mut empty, 10), Err(ClipError::Empty));
    }
}
