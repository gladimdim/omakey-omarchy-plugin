//! The layout Hyprland reads Omakey's keys with. A phone typing with its
//! own keyboard says which layout each key is meant in (INPUT `layout`), and
//! only the `omakey-keyboard` device is switched to it: the desktop's own
//! keyboards keep theirs. Talks to Hyprland's control socket, as hyprctl does.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const DEVICE: &str = "omakey-keyboard";
/// While typing, look again this often: a layout toggle on the desktop
/// (Alt+Alt) can move this device too.
const RECHECK: Duration = Duration::from_secs(1);
const TIMEOUT: Duration = Duration::from_millis(300);

pub struct KeyboardLayout {
    socket: Option<PathBuf>,
    /// The layout the phone asked for last.
    wanted: Option<String>,
    /// The device's layout before a phone first asked, to go back to when
    /// a phone sends keys without one (the app's own key layouts).
    original: Option<usize>,
    last_check: Option<Instant>,
    /// Layouts already reported as missing, so the log says it once.
    missing: Vec<String>,
}

impl KeyboardLayout {
    pub fn new() -> KeyboardLayout {
        KeyboardLayout { socket: socket_path(), wanted: None, original: None, last_check: None, missing: Vec::new() }
    }

    /// One INPUT's `layout`, before its events reach the keyboard.
    pub fn apply(&mut self, layout: Option<&str>, has_events: bool, now: Instant) {
        if self.socket.is_none() {
            return;
        }
        match layout {
            Some(l) => {
                let changed = self.wanted.as_deref() != Some(l);
                let stale = self.last_check.is_none_or(|t| now.duration_since(t) >= RECHECK);
                if changed || (has_events && stale) {
                    self.wanted = Some(l.to_string());
                    self.last_check = Some(now);
                    self.ensure(l);
                }
            }
            None if has_events && self.original.is_some() => {
                let original = self.original.take().unwrap();
                self.wanted = None;
                self.switch(original);
            }
            None => {}
        }
    }

    /// Switch the device to [layout] if it's one of its layouts and not on it already.
    fn ensure(&mut self, layout: &str) {
        let Some((layouts, active)) = self.device() else { return };
        let Some(index) = layouts.iter().position(|l| l == layout) else {
            if !self.missing.iter().any(|m| m == layout) {
                eprintln!("omakeyd: the phone types in \"{layout}\", which isn't among the keyboard layouts ({})", layouts.join(","));
                self.missing.push(layout.to_string());
            }
            return;
        };
        if index != active {
            self.original.get_or_insert(active);
            self.switch(index);
        }
    }

    fn switch(&self, index: usize) {
        let _ = self.request(&format!("switchxkblayout {DEVICE} {index}"));
    }

    /// The device's layouts and the active one's index.
    fn device(&self) -> Option<(Vec<String>, usize)> {
        let json: serde_json::Value = serde_json::from_str(&self.request("j/devices")?).ok()?;
        let kb = json["keyboards"].as_array()?.iter().find(|k| k["name"] == DEVICE)?;
        let layouts = kb["layout"].as_str()?.split(',').map(|s| s.trim().to_string()).collect();
        Some((layouts, kb["active_layout_index"].as_u64()? as usize))
    }

    fn request(&self, cmd: &str) -> Option<String> {
        let mut s = UnixStream::connect(self.socket.as_ref()?).ok()?;
        s.set_read_timeout(Some(TIMEOUT)).ok()?;
        s.set_write_timeout(Some(TIMEOUT)).ok()?;
        s.write_all(cmd.as_bytes()).ok()?;
        let mut out = String::new();
        s.read_to_string(&mut out).ok()?;
        Some(out)
    }
}

/// Hyprland's control socket: from the environment, else the newest instance.
fn socket_path() -> Option<PathBuf> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("hypr");
    if let Some(sig) = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE") {
        let p = runtime.join(sig).join(".socket.sock");
        if p.exists() {
            return Some(p);
        }
    }
    std::fs::read_dir(&runtime)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path().join(".socket.sock"))
        .filter(|p| p.exists())
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}
