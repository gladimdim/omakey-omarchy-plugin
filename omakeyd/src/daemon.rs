//! `omakeyd run`: the UDP server, control socket, mDNS advert, state file.

use crate::client::PairInfo;
use crate::keyboard::{Keyboard, KeySink, LogSink, Uinput};
use crate::protocol::{random_bytes, DeviceId, MAX_DATAGRAM};
use crate::server::{Pending, Server, Shared, PAIRING_TTL};
use crate::store::{self, hex, unix_now, Config, Devices};
use serde_json::json;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SERVICE_TYPE: &str = "_omakey._udp.local.";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: i32) {
    STOP.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    extern "C" {
        fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
    }
    unsafe {
        signal(2, on_signal); // SIGINT
        signal(15, on_signal); // SIGTERM
    }
}

pub fn control_socket() -> PathBuf {
    store::runtime_dir().join("ctl.sock")
}

pub fn state_path() -> PathBuf {
    store::runtime_dir().join("state.json")
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
}

pub fn run(port_override: Option<u16>, dry_run: bool) -> Result<(), String> {
    let config = Config::load();
    let port = port_override.unwrap_or(config.port);
    let host_id = store::host_id().map_err(|e| format!("can't create host id: {e}"))?;
    let host_name = store::host_name(&config);

    let sink: Box<dyn KeySink + Send> = if dry_run {
        Box::new(LogSink)
    } else {
        Box::new(Uinput::open().map_err(|e| {
            format!(
                "can't open /dev/uinput ({e}). Run install.sh to add the udev rule, then log out and back in."
            )
        })?)
    };

    let socket = UdpSocket::bind(("0.0.0.0", port)).map_err(|e| format!("can't bind UDP port {port}: {e}"))?;
    socket.set_read_timeout(Some(Duration::from_millis(50))).unwrap();

    let shared = Arc::new(Mutex::new(Shared::new(Devices::load())));
    let ctx = Arc::new(Context {
        shared: shared.clone(),
        host_id,
        host_name: host_name.clone(),
        port,
        uinput: if dry_run { "dry-run".into() } else { "ok".into() },
    });

    std::fs::create_dir_all(store::runtime_dir()).map_err(|e| e.to_string())?;
    let listener = bind_control()?;
    {
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("control".into())
            .spawn(move || control_loop(listener, ctx))
            .unwrap();
    }

    let mdns = advertise(&host_id, &host_name, port);
    install_signal_handlers();
    eprintln!("omakeyd: listening on UDP {port} as \"{host_name}\" ({})", hex(&host_id));

    let mut server = Server::new(shared.clone(), Keyboard::new(sink), host_name);
    let mut buf = [0u8; MAX_DATAGRAM + 1];
    let mut last_tick = Instant::now();
    let mut last_publish = Instant::now() - Duration::from_secs(10);

    while !STOP.load(Ordering::SeqCst) {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                let now = Instant::now();
                if let Some(reply) = server.handle(&buf[..n], from, now) {
                    let _ = socket.send_to(&reply, from);
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(e) => eprintln!("omakeyd: recv: {e}"),
        }
        let now = Instant::now();
        if now.duration_since(last_tick) >= Duration::from_millis(50) {
            server.tick(now);
            last_tick = now;
        }
        let dirty = std::mem::take(&mut shared.lock().unwrap().dirty);
        if dirty || server.changed || now.duration_since(last_publish) >= Duration::from_secs(1) {
            server.changed = false;
            let sessions = server.session_infos(now);
            shared.lock().unwrap().sessions = sessions;
            write_state(&ctx, true);
            last_publish = now;
        }
    }

    eprintln!("omakeyd: shutting down");
    server.shutdown();
    if let Some(m) = mdns {
        let _ = m.shutdown();
    }
    write_state(&ctx, false);
    let _ = std::fs::remove_file(control_socket());
    Ok(())
}

fn bind_control() -> Result<UnixListener, String> {
    let path = control_socket();
    if UnixStream::connect(&path).is_ok() {
        return Err("omakeyd is already running".into());
    }
    let _ = std::fs::remove_file(&path);
    let l = UnixListener::bind(&path).map_err(|e| format!("can't bind {}: {e}", path.display()))?;
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    Ok(l)
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
    // Instance names must be unique on the network; the id suffix makes them so.
    let instance = format!("{host_name} omakey {}", &id[..6]);
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

fn state_json(ctx: &Context, running: bool) -> serde_json::Value {
    let shared = ctx.shared.lock().unwrap();
    let connected: Vec<&str> = shared.sessions.iter().map(|s| s.device.as_str()).collect();
    let devices: Vec<_> = shared
        .devices
        .devices
        .iter()
        .map(|d| {
            let session = shared.sessions.iter().find(|s| s.device == d.id);
            json!({
                "id": d.id,
                "name": d.name,
                "paired_at": d.paired_at,
                "last_seen": d.last_seen,
                "connected": connected.contains(&d.id.as_str()),
                "addr": session.map(|s| s.addr.clone()),
                "held": session.map(|s| s.held).unwrap_or(0),
            })
        })
        .collect();
    let pairing = shared.pending.as_ref().map(|p| {
        json!({ "uri": p.uri, "expires_at": p.expires_unix, "qr": crate::qr::rows(&p.uri) })
    });
    json!({
        "version": 1,
        "running": running,
        "pid": std::process::id(),
        "updated_at": unix_now(),
        "host_id": hex(&ctx.host_id),
        "host_name": ctx.host_name,
        "port": ctx.port,
        "addresses": addresses().iter().map(|a| a.to_string()).collect::<Vec<_>>(),
        "uinput": ctx.uinput,
        "connected": shared.sessions.len(),
        "devices": devices,
        "paired": { "count": shared.paired.0, "name": shared.paired.1 },
        "pairing": pairing,
    })
}

fn write_state(ctx: &Context, running: bool) {
    let path = state_path();
    let data = serde_json::to_vec(&state_json(ctx, running)).unwrap();
    // Skip identical rewrites: the bar widget watches this file.
    if std::fs::read(&path).ok().as_deref() == Some(&data[..]) {
        return;
    }
    if let Err(e) = store::write_private(&path, &data) {
        eprintln!("omakeyd: can't write {}: {e}", path.display());
    }
}

fn open_pairing(ctx: &Context) -> serde_json::Value {
    let device_id: DeviceId = random_bytes();
    let key = random_bytes();
    let info = PairInfo {
        host_id: ctx.host_id,
        host_name: ctx.host_name.clone(),
        addrs: addresses().iter().map(|a| a.to_string()).collect(),
        port: ctx.port,
        device_id,
        key,
    };
    let uri = info.to_uri();
    let expires_unix = unix_now() + PAIRING_TTL.as_secs();
    let mut shared = ctx.shared.lock().unwrap();
    shared.pending = Some(Pending { device_id, key, uri: uri.clone(), expires: Instant::now() + PAIRING_TTL, expires_unix });
    shared.dirty = true;
    json!({ "ok": true, "uri": uri, "expires_at": expires_unix, "paired_count": shared.paired.0 })
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
                shared.dirty = true;
                match shared.devices.save() {
                    Ok(()) => json!({ "ok": true }),
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                }
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
            let _ = shared.devices.save();
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
    let path = control_socket();
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
