//! `omakeyd run`: the UDP server, control socket, mDNS advert, state file.

use crate::bluetooth;
use crate::client::PairInfo;
use crate::clipboard::{Clipboard, DesktopClipboard};
use crate::keyboard::{Keyboard, KeySink, LogSink, Uinput};
use crate::net::UdpServer;
use crate::notify::Notifier;
use crate::protocol::{fingerprint, random_bytes, DeviceId, MAX_DATAGRAM};
use crate::server::{BtStatus, Pending, Server, SessionInfo, Shared, PAIRING_TTL};
use crate::store::{self, hex, unix_now, Config, Device, Devices};
use crate::theme::ThemeWatch;
use crate::hypr::KeyboardLayout;
use serde_json::json;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{IpAddr, Ipv4Addr};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SERVICE_TYPE: &str = "_omakey._udp.local.";
/// devices.json is rewritten at most this often for last_seen updates.
const SAVE_EVERY: Duration = Duration::from_secs(5);
/// `pair` hands out the open code again while it has this long left.
const PAIR_REUSE_MIN: u64 = 60;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }
}

pub fn control_socket() -> Result<PathBuf, String> {
    Ok(store::runtime_dir()?.join("ctl.sock"))
}

/// IPv4 addresses a phone can reach, LAN first, then VPNs like Tailscale.
pub fn addresses() -> Vec<Ipv4Addr> {
    let mut out: Vec<(u8, Ipv4Addr)> = Vec::new();
    for iface in if_addrs::get_if_addrs().unwrap_or_default() {
        let IpAddr::V4(ip) = iface.ip() else { continue };
        let name = iface.name.as_str();
        if ip.is_loopback()
            || ip.is_link_local()
            || ["docker", "br-", "veth", "virbr", "lxc", "podman", "vmnet"].iter().any(|p| name.starts_with(p))
        {
            continue;
        }
        let rank = if ip.is_private() { 0 } else { 1 };
        out.push((rank, ip));
    }
    out.sort();
    out.dedup();
    out.into_iter().map(|(_, ip)| ip).collect()
}

struct Context {
    shared: Arc<Mutex<Shared>>,
    host_id: DeviceId,
    host_name: String,
    port: u16,
    uinput: String,
    state_path: PathBuf,
}

pub fn run(port_override: Option<u16>, dry_run: bool) -> Result<(), String> {
    let config = Config::load();
    let port = port_override.unwrap_or(config.port);
    let host_id = store::host_id().map_err(|e| format!("can't create host id: {e}"))?;
    let host_name = store::host_name(&config);
    let runtime = store::ensure_runtime_dir()?;

    let sink: Box<dyn KeySink + Send> = if dry_run {
        Box::new(LogSink)
    } else {
        Box::new(Uinput::open().map_err(|e| {
            format!(
                "can't open /dev/uinput ({e}). Run install.sh to add the udev rule, then log out and back in."
            )
        })?)
    };

    let socket = UdpServer::bind(port)?;

    let shared = Arc::new(Mutex::new(Shared::new(Devices::load())));
    // Shared with the Bluetooth connection threads; the UDP loop below holds
    // it only while handling one packet.
    let server = Arc::new(Mutex::new(Server::new(shared.clone(), Keyboard::new(sink), host_name.clone())));
    // A real keyboard only: a dry run has no device for Hyprland to switch,
    // and only prints the keys a copy or paste would press.
    if !dry_run {
        let mut srv = server.lock().unwrap();
        srv.layout = Some(KeyboardLayout::new());
        if DesktopClipboard::available() {
            srv.clipboard = Some(Clipboard::start(Box::new(DesktopClipboard::new())));
        } else {
            eprintln!("omakeyd: no clipboard to reach (no wl-copy/wl-paste or D-Bus session); phones press copy and paste instead");
        }
    }
    let bluetooth = bluetooth::start(server.clone(), shared.clone());
    let ctx = Arc::new(Context {
        shared: shared.clone(),
        host_id,
        host_name: host_name.clone(),
        port,
        uinput: if dry_run { "dry-run".into() } else { "ok".into() },
        state_path: runtime.join("state.json"),
    });

    let listener = bind_control(&runtime.join("ctl.sock"))?;
    {
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("control".into())
            .spawn(move || control_loop(listener, ctx))
            .unwrap();
    }

    // The state file and devices.json are written on their own thread:
    // file I/O never delays a key.
    let (publish, publish_rx) = std::sync::mpsc::channel::<()>();
    let publisher = {
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("state".into())
            .spawn(move || {
                let mut last_save = None;
                use std::sync::mpsc::RecvTimeoutError;
                // Wakes for each publish, and every second for debounced saves.
                while let Ok(()) | Err(RecvTimeoutError::Timeout) = publish_rx.recv_timeout(Duration::from_secs(1)) {
                    while publish_rx.try_recv().is_ok() {}
                    save_devices(&ctx, &mut last_save, false);
                    write_state(&ctx, true);
                }
                last_save
            })
            .unwrap()
    };

    let mdns = advertise(&host_id, &host_name, port);
    install_signal_handlers();
    let mut notifier = Notifier::from_env();
    notifier.send("READY=1");
    eprintln!("omakeyd: listening on UDP {port} as \"{host_name}\" ({})", hex(&host_id));

    let mut buf = [0u8; MAX_DATAGRAM + 1];
    let mut last_tick = Instant::now();
    let mut last_publish = Instant::now() - Duration::from_secs(10);
    let mut theme_watch = ThemeWatch::new();

    while !STOP.load(Ordering::SeqCst) {
        match socket.recv(&mut buf) {
            Ok((n, from, local)) => {
                let now = Instant::now();
                let reply = server.lock().unwrap().handle(&buf[..n], from, now);
                if let Some(reply) = reply {
                    let _ = socket.send(&reply, from, local);
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(e) => eprintln!("omakeyd: recv: {e}"),
        }
        let now = Instant::now();
        notifier.tick(now);
        // Never hold both locks: take what the control thread left, then
        // work on the server, then hand its news back.
        let (forgotten, dirty) = {
            let mut sh = shared.lock().unwrap();
            sh.expire_pending(now);
            (std::mem::take(&mut sh.forgotten), std::mem::take(&mut sh.dirty))
        };
        let (seen, sessions) = {
            let mut srv = server.lock().unwrap();
            for id in &forgotten {
                srv.drop_device(id);
            }
            if now.duration_since(last_tick) >= Duration::from_millis(50) {
                srv.tick(now);
                last_tick = now;
            }
            if let Some(theme) = theme_watch.poll(now) {
                srv.set_theme(theme);
            }
            let due = dirty || srv.changed || now.duration_since(last_publish) >= Duration::from_secs(1);
            let sessions = due.then(|| {
                srv.changed = false;
                srv.session_infos(now)
            });
            (srv.take_seen(), sessions)
        };
        if !seen.is_empty() || sessions.is_some() {
            let mut sh = shared.lock().unwrap();
            sh.mark_seen(&seen);
            if let Some(s) = sessions {
                sh.sessions = s;
                drop(sh);
                let _ = publish.send(());
                last_publish = now;
            }
        }
    }

    eprintln!("omakeyd: shutting down");
    notifier.send("STOPPING=1");
    drop(bluetooth);
    let seen = {
        let mut srv = server.lock().unwrap();
        srv.shutdown();
        srv.take_seen()
    };
    {
        let mut sh = shared.lock().unwrap();
        sh.mark_seen(&seen);
        sh.sessions.clear();
    }
    drop(publish);
    let mut last_save = publisher.join().unwrap_or(None);
    if let Some(m) = mdns {
        let _ = m.shutdown();
    }
    save_devices(&ctx, &mut last_save, true);
    write_state(&ctx, false);
    let _ = std::fs::remove_file(runtime.join("ctl.sock"));
    Ok(())
}

/// Write devices.json if it changed: right away after pairing, forget or
/// rename, otherwise at most every SAVE_EVERY. Only the state thread (and
/// shutdown, after it ends) calls this, so saves never race.
fn save_devices(ctx: &Context, last_save: &mut Option<Instant>, force: bool) {
    let data = {
        let mut sh = ctx.shared.lock().unwrap();
        if !sh.devices_dirty || !(force || sh.save_now || last_save.is_none_or(|t| t.elapsed() >= SAVE_EVERY)) {
            return;
        }
        sh.devices_dirty = false;
        sh.save_now = false;
        serde_json::to_vec_pretty(&sh.devices).unwrap()
    };
    *last_save = Some(Instant::now());
    if let Err(e) = store::write_private(&Devices::path(), &data) {
        eprintln!("omakeyd: couldn't save devices.json: {e}");
        ctx.shared.lock().unwrap().devices_dirty = true;
    }
}

fn bind_control(path: &std::path::Path) -> Result<UnixListener, String> {
    if UnixStream::connect(path).is_ok() {
        return Err("omakeyd is already running".into());
    }
    let _ = std::fs::remove_file(path);
    let l = UnixListener::bind(path).map_err(|e| format!("can't bind {}: {e}", path.display()))?;
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    Ok(l)
}

/// The mDNS instance name, "<host> omakey <id6>". The id suffix makes it
/// unique on the network; the host part is cut on a character boundary so
/// the whole label fits DNS's 63 bytes.
fn instance_name(host_name: &str, id6: &str) -> String {
    let suffix = format!(" omakey {id6}");
    let mut end = host_name.len().min(63 - suffix.len());
    while !host_name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", host_name[..end].trim_end())
}

fn advertise(host_id: &DeviceId, host_name: &str, port: u16) -> Option<mdns_sd::ServiceDaemon> {
    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("omakeyd: mDNS unavailable ({e}); phones can still connect from the QR code");
            return None;
        }
    };
    let id = hex(host_id);
    let props = [("v", "1"), ("id", id.as_str()), ("n", host_name)];
    let instance = instance_name(host_name, &id[..6]);
    let mdns_host = format!("omakey-{id}.local.");
    let info = mdns_sd::ServiceInfo::new(SERVICE_TYPE, &instance, &mdns_host, "", port, &props[..])
        .map(|i| i.enable_addr_auto());
    match info.and_then(|i| daemon.register(i)) {
        Ok(()) => Some(daemon),
        Err(e) => {
            eprintln!("omakeyd: mDNS advert failed: {e}");
            None
        }
    }
}

/// What state.json shows, copied out of Shared so the lock is held only
/// for the copy.
struct Snapshot {
    sessions: Vec<SessionInfo>,
    devices: Vec<Device>,
    pending: Option<(String, u64, [u8; 32])>,
    paired: (u64, String),
    bluetooth: BtStatus,
}

fn snapshot(ctx: &Context) -> Snapshot {
    let sh = ctx.shared.lock().unwrap();
    Snapshot {
        sessions: sh.sessions.clone(),
        devices: sh.devices.devices.clone(),
        pending: sh.pending.as_ref().map(|p| (p.uri.clone(), p.expires_unix, p.key)),
        paired: sh.paired.clone(),
        bluetooth: sh.bluetooth.clone(),
    }
}

fn state_json(ctx: &Context, running: bool) -> serde_json::Value {
    let snap = snapshot(ctx);
    let devices: Vec<_> = snap
        .devices
        .iter()
        .map(|d| {
            let session = snap.sessions.iter().find(|s| s.device == d.id);
            json!({
                "id": d.id,
                "name": d.name,
                "paired_at": d.paired_at,
                "last_seen": d.last_seen,
                "connected": session.is_some(),
                "addr": session.map(|s| s.addr.clone()),
                "transport": session.map(|s| s.transport),
                "loss": session.map(|s| (s.loss * 10.0).round() / 10.0),
                "held": session.map(|s| s.held).unwrap_or(0),
            })
        })
        .collect();
    let pairing = snap.pending.as_ref().map(|(uri, expires, key)| {
        json!({ "uri": uri, "expires_at": expires, "fingerprint": fingerprint(key), "qr": crate::qr::rows(uri) })
    });
    let bt = &snap.bluetooth;
    json!({
        "version": 1,
        "running": running,
        "pid": std::process::id(),
        "daemon_version": env!("CARGO_PKG_VERSION"),
        "host_id": hex(&ctx.host_id),
        "host_name": ctx.host_name,
        "port": ctx.port,
        "addresses": addresses().iter().map(|a| a.to_string()).collect::<Vec<_>>(),
        "uinput": ctx.uinput,
        "bluetooth": {
            "state": bt.state,
            "address": bt.address.as_ref().map(bluetooth::address_text),
            "reason": bt.reason,
        },
        "connected": snap.sessions.len(),
        "devices": devices,
        "paired": { "count": snap.paired.0, "name": snap.paired.1 },
        "pairing": pairing,
    })
}

fn write_state(ctx: &Context, running: bool) {
    let path = &ctx.state_path;
    let data = serde_json::to_vec(&state_json(ctx, running)).unwrap();
    // Skip identical rewrites: the bar widget watches this file.
    if std::fs::read(path).ok().as_deref() == Some(&data[..]) {
        return;
    }
    if let Err(e) = store::write_private(path, &data) {
        eprintln!("omakeyd: can't write {}: {e}", path.display());
    }
}

/// Open a pairing window, or hand out the open one while it has a minute
/// left, so a second click (or `omakeyd pair` after the widget) shows the
/// same code instead of silently invalidating the first.
fn open_pairing(ctx: &Context) -> serde_json::Value {
    let addrs: Vec<String> = addresses().iter().map(|a| a.to_string()).collect();
    let mut shared = ctx.shared.lock().unwrap();
    let reply = |p: &Pending, reused: bool, count: u64| {
        json!({ "ok": true, "uri": p.uri, "expires_at": p.expires_unix, "fingerprint": fingerprint(&p.key),
                "reused": reused, "paired_count": count })
    };
    if let Some(p) = shared.pending.as_ref().filter(|p| p.expires_unix >= unix_now() + PAIR_REUSE_MIN) {
        return reply(p, true, shared.paired.0);
    }
    let device_id: DeviceId = random_bytes();
    let key = random_bytes();
    let bt = &shared.bluetooth;
    let info = PairInfo {
        host_id: ctx.host_id,
        host_name: ctx.host_name.clone(),
        addrs,
        port: ctx.port,
        device_id,
        key,
        bt: bt.address.filter(|_| bt.state == "on"),
    };
    let uri = info.to_uri();
    let pending = Pending {
        device_id,
        key,
        uri,
        expires: Instant::now() + PAIRING_TTL,
        expires_unix: unix_now() + PAIRING_TTL.as_secs(),
    };
    let out = reply(&pending, false, shared.paired.0);
    shared.pending = Some(pending);
    shared.dirty = true;
    out
}

fn handle_request(ctx: &Context, line: &str) -> serde_json::Value {
    let req: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return json!({ "ok": false, "error": format!("bad request: {e}") }),
    };
    match req["cmd"].as_str().unwrap_or("") {
        "status" => {
            let mut v = state_json(ctx, true);
            v["ok"] = json!(true);
            v
        }
        "pair" => open_pairing(ctx),
        "cancel-pair" => {
            let mut shared = ctx.shared.lock().unwrap();
            shared.pending = None;
            shared.dirty = true;
            json!({ "ok": true })
        }
        "forget" => {
            let id = req["id"].as_str().unwrap_or("").to_lowercase();
            let mut shared = ctx.shared.lock().unwrap();
            if shared.devices.remove(&id) {
                shared.forgotten.push(id);
                shared.dirty = true;
                shared.devices_dirty = true;
                shared.save_now = true;
                json!({ "ok": true })
            } else {
                json!({ "ok": false, "error": "no such device" })
            }
        }
        "rename" => {
            let id = req["id"].as_str().unwrap_or("").to_lowercase();
            let name = req["name"].as_str().unwrap_or("").trim().to_string();
            let mut shared = ctx.shared.lock().unwrap();
            let Some(d) = shared.devices.devices.iter_mut().find(|d| d.id == id) else {
                return json!({ "ok": false, "error": "no such device" });
            };
            if name.is_empty() {
                return json!({ "ok": false, "error": "empty name" });
            }
            d.name = name.chars().take(64).collect();
            shared.dirty = true;
            shared.devices_dirty = true;
            shared.save_now = true;
            json!({ "ok": true })
        }
        other => json!({ "ok": false, "error": format!("unknown command {other:?}") }),
    }
}

fn control_loop(listener: UnixListener, ctx: Arc<Context>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut writer = match stream.try_clone() {
                Ok(w) => w,
                Err(_) => return,
            };
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let resp = handle_request(&ctx, &line);
                if writeln!(writer, "{resp}").is_err() {
                    break;
                }
            }
        });
    }
}

/// Send one request to the running daemon.
pub fn request(req: serde_json::Value) -> Result<serde_json::Value, String> {
    let path = control_socket()?;
    let mut s = UnixStream::connect(&path).map_err(|_| {
        "omakeyd isn't running. Start it with: systemctl --user start omakeyd".to_string()
    })?;
    writeln!(s, "{req}").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    if v["ok"] == json!(false) {
        return Err(v["error"].as_str().unwrap_or("error").to_string());
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdns_instance_label_fits_63_bytes() {
        assert_eq!(instance_name("desk", "abcdef"), "desk omakey abcdef");
        let long = "Робоча станція Владислава на кухні біля вікна".to_string();
        let name = instance_name(&long, "abcdef");
        assert!(name.len() <= 63, "{} bytes", name.len());
        assert!(name.ends_with(" omakey abcdef"));
        assert!(long.starts_with(name.trim_end_matches(" omakey abcdef")));
        assert_eq!(instance_name(&"x".repeat(100), "abcdef").len(), 63);
    }
}
