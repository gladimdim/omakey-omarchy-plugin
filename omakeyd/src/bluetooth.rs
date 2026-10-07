//! The protocol over Bluetooth, for phones that can't reach us on Wi-Fi.
//!
//! BlueZ owns the RFCOMM socket and the SDP record: we register a Profile1
//! with our UUID and it hands us each phone's connection as a file
//! descriptor. RFCOMM is a reliable stream, so every datagram is framed
//! with a 2-byte big-endian length; the packets inside are exactly the UDP
//! ones, with the same encryption, so Bluetooth pairing adds no trust.

use crate::protocol::MAX_DATAGRAM;
use crate::server::{BtStatus, Peer, Server, Shared};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedFd, OwnedObjectPath, OwnedValue, Value};

/// The Omakey RFCOMM service. Phones find its channel by this UUID over SDP.
pub const SERVICE_UUID: &str = "4f4b6579-6d61-4b79-9000-6f6d616b6579";
const PROFILE_PATH: &str = "/org/omakey/rfcomm";
/// How often we check that bluetoothd and the adapter are still there.
const CHECK_EVERY: Duration = Duration::from_secs(3);
/// A connection that sends nothing this long is dead (phones send a
/// heartbeat every 100 ms).
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CONNECTIONS: usize = 8;

/// Open RFCOMM connections: id → (phone address, a handle to shut it down).
type Conns = Arc<Mutex<HashMap<u64, (Arc<str>, UnixStream)>>>;

/// Keeps the profile registered (re-registering when bluetoothd restarts)
/// while it lives.
pub struct Bluetooth {
    stop: Arc<AtomicBool>,
    conns: Conns,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Bluetooth {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        for (_, s) in self.conns.lock().unwrap().values() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// "14:18:C3:68:87:1E"
pub fn address_text(a: &[u8; 6]) -> String {
    a.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Start the thread that registers with BlueZ and keeps `server.bt_address`
/// and `shared.bluetooth` up to date. Never fails: without bluetoothd or an
/// adapter it keeps retrying.
pub fn start(server: Arc<Mutex<Server>>, shared: Arc<Mutex<Shared>>) -> Bluetooth {
    let stop = Arc::new(AtomicBool::new(false));
    let conns: Conns = Arc::default();
    let thread = {
        let (stop, conns) = (stop.clone(), conns.clone());
        std::thread::Builder::new()
            .name("bluetooth".into())
            .spawn(move || watch(server, shared, conns, stop))
            .map_err(|e| eprintln!("omakeyd: no Bluetooth fallback ({e})"))
            .ok()
    };
    Bluetooth { stop, conns, thread }
}

/// Our bus connection, and the bluetoothd instance the profile is registered with.
struct Link {
    conn: Connection,
    released: Arc<AtomicBool>,
    registered_with: Option<String>,
}

fn watch(server: Arc<Mutex<Server>>, shared: Arc<Mutex<Shared>>, conns: Conns, stop: Arc<AtomicBool>) {
    let mut link: Option<Link> = None;
    let mut last = BtStatus::default();
    while !stop.load(Ordering::SeqCst) {
        let status = check(&mut link, &server, &conns);
        if status != last {
            match status.state {
                "on" => eprintln!("omakeyd: Bluetooth fallback on {}", address_text(&status.address.unwrap_or_default())),
                _ => eprintln!("omakeyd: no Bluetooth fallback ({}); Wi-Fi only", status.reason),
            }
            // Only advertise a service phones can actually reach.
            server.lock().unwrap().bt_address = status.address.filter(|_| status.state == "on");
            let mut sh = shared.lock().unwrap();
            sh.bluetooth = status.clone();
            sh.dirty = true;
            drop(sh);
            last = status;
        }
        let until = Instant::now() + CHECK_EVERY;
        while Instant::now() < until && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if let Some(l) = link.filter(|l| l.registered_with.is_some()) {
        let _ = l.conn.call_method(
            Some("org.bluez"),
            "/org/bluez",
            Some("org.bluez.ProfileManager1"),
            "UnregisterProfile",
            &(ObjectPath::try_from(PROFILE_PATH).unwrap(),),
        );
    }
}

fn unavailable(reason: impl Into<String>) -> BtStatus {
    BtStatus { state: "unavailable", address: None, reason: reason.into() }
}

fn is_error(e: &zbus::Error, name: &str) -> bool {
    matches!(e, zbus::Error::MethodError(n, ..) if n.as_str() == name)
}

/// One round: connect, (re-)register if bluetoothd is new, read the adapter.
fn check(link: &mut Option<Link>, server: &Arc<Mutex<Server>>, conns: &Conns) -> BtStatus {
    if link.is_none() {
        let conn = match Connection::system() {
            Ok(c) => c,
            Err(e) => return unavailable(format!("no system bus: {e}")),
        };
        let released = Arc::new(AtomicBool::new(false));
        let profile = Profile { server: server.clone(), conns: conns.clone(), released: released.clone() };
        if let Err(e) = conn.object_server().at(PROFILE_PATH, profile) {
            return unavailable(format!("can't export the profile: {e}"));
        }
        *link = Some(Link { conn, released, registered_with: None });
    }
    let l = link.as_mut().unwrap();

    let owner = l.conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "GetNameOwner",
        &("org.bluez",),
    );
    let owner: String = match owner.and_then(|r| r.body().deserialize::<String>()) {
        Ok(o) => o,
        Err(e) if is_error(&e, "org.freedesktop.DBus.Error.NameHasNoOwner") => {
            l.registered_with = None;
            return unavailable("bluetoothd isn't running");
        }
        Err(e) => {
            // The bus itself went away; reconnect next time.
            *link = None;
            return unavailable(format!("system bus: {e}"));
        }
    };
    if l.released.swap(false, Ordering::SeqCst) {
        l.registered_with = None;
    }
    if l.registered_with.as_deref() != Some(owner.as_str()) {
        match register(&l.conn) {
            Ok(()) => l.registered_with = Some(owner),
            Err(e) if is_error(&e, "org.bluez.Error.AlreadyExists") => l.registered_with = Some(owner),
            Err(e) => return unavailable(format!("BlueZ didn't register the profile: {e}")),
        }
    }
    match adapter(&l.conn) {
        Ok(Some((address, true))) => BtStatus { state: "on", address: Some(address), reason: String::new() },
        Ok(Some((address, false))) => BtStatus { state: "off", address: Some(address), reason: "Bluetooth is turned off".into() },
        Ok(None) => unavailable("no Bluetooth adapter"),
        Err(e) => unavailable(e),
    }
}

fn register(conn: &Connection) -> zbus::Result<()> {
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("Name", Value::from("Omakey"));
    options.insert("Role", Value::from("server"));
    // Our own AES-GCM authenticates every packet; BlueZ pairing isn't needed
    // on top, and asking for authorization would prompt on every connect.
    options.insert("RequireAuthentication", Value::from(false));
    options.insert("RequireAuthorization", Value::from(false));
    conn.call_method(
        Some("org.bluez"),
        "/org/bluez",
        Some("org.bluez.ProfileManager1"),
        "RegisterProfile",
        &(ObjectPath::try_from(PROFILE_PATH).unwrap(), SERVICE_UUID, options),
    )?;
    Ok(())
}

type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// The first adapter BlueZ knows (by path, so hci0 before hci1): its
/// address and whether it is powered.
fn adapter(conn: &Connection) -> Result<Option<([u8; 6], bool)>, String> {
    let reply = conn
        .call_method(Some("org.bluez"), "/", Some("org.freedesktop.DBus.ObjectManager"), "GetManagedObjects", &())
        .map_err(|e| format!("can't list Bluetooth adapters: {e}"))?;
    let objects: ManagedObjects = reply.body().deserialize().map_err(|e| e.to_string())?;
    let mut adapters: Vec<_> = objects.iter().filter_map(|(path, ifaces)| Some((path.as_str(), ifaces.get("org.bluez.Adapter1")?))).collect();
    adapters.sort_by_key(|(path, _)| *path);
    let Some((_, props)) = adapters.first() else { return Ok(None) };
    let text: &str = props.get("Address").and_then(|v| v.downcast_ref().ok()).ok_or("adapter has no address")?;
    let address = parse_address(text).ok_or_else(|| format!("bad adapter address {text:?}"))?;
    let powered: bool = props.get("Powered").and_then(|v| v.downcast_ref().ok()).unwrap_or(false);
    Ok(Some((address, powered)))
}

pub fn parse_address(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.split([':', '_']).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = [0u8; 6];
    for (o, p) in out.iter_mut().zip(parts) {
        *o = u8::from_str_radix(p, 16).ok()?;
    }
    Some(out)
}

/// "AA:BB:CC:DD:EE:FF" from /org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF.
fn device_mac(device: &OwnedObjectPath) -> String {
    device.as_str().rsplit('/').next().unwrap_or("").trim_start_matches("dev_").replace('_', ":")
}

struct Profile {
    server: Arc<Mutex<Server>>,
    conns: Conns,
    released: Arc<AtomicBool>,
}

#[zbus::interface(name = "org.bluez.Profile1")]
impl Profile {
    fn release(&self) {
        // BlueZ dropped our registration; the watch thread registers again.
        self.released.store(true, Ordering::SeqCst);
    }

    fn new_connection(&self, device: OwnedObjectPath, fd: OwnedFd, _properties: HashMap<String, OwnedValue>) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let mac: Arc<str> = device_mac(&device).into();
        let stream = UnixStream::from(std::os::fd::OwnedFd::from(fd));
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        {
            let mut conns = self.conns.lock().unwrap();
            if conns.len() >= MAX_CONNECTIONS {
                eprintln!("omakeyd: refusing Bluetooth connection from {mac}: {MAX_CONNECTIONS} already open");
                return;
            }
            match stream.try_clone() {
                Ok(handle) => conns.insert(id, (mac.clone(), handle)),
                Err(e) => {
                    eprintln!("omakeyd: can't serve a Bluetooth connection: {e}");
                    return;
                }
            };
        }
        let (server, conns) = (self.server.clone(), self.conns.clone());
        let spawned = std::thread::Builder::new().name("bt-conn".into()).spawn(move || {
            eprintln!("omakeyd: Bluetooth connection from {mac}");
            serve_connection(stream, Peer::Bluetooth { mac: mac.clone(), conn: id }, &server);
            conns.lock().unwrap().remove(&id);
            eprintln!("omakeyd: Bluetooth connection from {mac} closed");
        });
        if let Err(e) = spawned {
            self.conns.lock().unwrap().remove(&id);
            eprintln!("omakeyd: can't serve a Bluetooth connection: {e}");
        }
    }

    fn request_disconnection(&self, device: OwnedObjectPath) {
        let mac = device_mac(&device);
        for (m, s) in self.conns.lock().unwrap().values() {
            if **m == *mac {
                // The reader thread sees EOF and cleans up.
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        }
    }
}

/// Serve one RFCOMM connection until it closes, then let go of the keys
/// held through it.
pub(crate) fn serve_connection(stream: UnixStream, peer: Peer, server: &Mutex<Server>) {
    // BlueZ hands the socket over non-blocking; we read it on a thread of its own.
    if let Err(e) = stream.set_nonblocking(false) {
        eprintln!("omakeyd: Bluetooth socket: {e}");
    } else {
        let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        serve(&stream, peer.clone(), server);
    }
    server.lock().unwrap().peer_gone(&peer);
}

/// Read framed datagrams until the phone hangs up; reply on the same stream.
fn serve<S: Read + Write>(mut sock: S, peer: Peer, server: &Mutex<Server>) {
    let mut buf = [0u8; MAX_DATAGRAM];
    let mut out = Vec::with_capacity(MAX_DATAGRAM + 2);
    loop {
        let mut len = [0u8; 2];
        if sock.read_exact(&mut len).is_err() {
            return;
        }
        let n = u16::from_be_bytes(len) as usize;
        if n == 0 || n > MAX_DATAGRAM || sock.read_exact(&mut buf[..n]).is_err() {
            return;
        }
        let reply = server.lock().unwrap().handle(&buf[..n], peer.clone(), Instant::now());
        if let Some(reply) = reply {
            // One write per frame, so it travels as one RFCOMM packet.
            out.clear();
            out.extend_from_slice(&(reply.len() as u16).to_be_bytes());
            out.extend_from_slice(&reply);
            if sock.write_all(&out).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_parse_from_bluez_and_device_paths() {
        assert_eq!(parse_address("14:18:C3:68:87:1E"), Some([0x14, 0x18, 0xc3, 0x68, 0x87, 0x1e]));
        assert_eq!(parse_address("14_18_c3_68_87_1e"), Some([0x14, 0x18, 0xc3, 0x68, 0x87, 0x1e]));
        assert_eq!(parse_address("14:18:C3:68:87"), None);
        assert_eq!(parse_address("zz:18:C3:68:87:1E"), None);
    }
}
