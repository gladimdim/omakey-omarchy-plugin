//! Session state machine. No sockets here: `handle` takes a datagram and
//! returns the reply, so the whole protocol is unit-testable.

use crate::keyboard::{allowed, is_modifier, Keyboard};
use crate::protocol::*;
use crate::store::{hex, unix_now, Device, Devices};
use aes_gcm::Aes256Gcm;
use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Keys held by a silent session are let go after this long.
pub const STUCK_KEY_TIMEOUT: Duration = Duration::from_millis(500);
/// A silent session is forgotten after this long.
pub const SESSION_TIMEOUT: Duration = Duration::from_secs(30);
pub const PAIRING_TTL: Duration = Duration::from_secs(300);
const MAX_SESSIONS_PER_DEVICE: usize = 4;
const MAX_HELD: usize = 64;

pub struct Pending {
    pub device_id: DeviceId,
    pub key: Key,
    pub uri: String,
    pub expires: Instant,
    pub expires_unix: u64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct SessionInfo {
    pub device: String,
    pub name: String,
    pub addr: String,
    pub held: usize,
    pub idle_ms: u64,
    pub packets: u64,
}

/// State shared with the control socket thread.
pub struct Shared {
    pub devices: Devices,
    pub pending: Option<Pending>,
    /// Bumped whenever a phone finishes pairing, with its name.
    pub paired: (u64, String),
    pub sessions: Vec<SessionInfo>,
    /// Set by the control thread when devices or pairing changed.
    pub dirty: bool,
}

impl Shared {
    pub fn new(devices: Devices) -> Shared {
        Shared { devices, pending: None, paired: (0, String::new()), sessions: Vec::new(), dirty: true }
    }
}

struct Session {
    device_id: DeviceId,
    name: String,
    client_random: [u8; 16],
    welcome: Vec<u8>,
    device_cipher: Aes256Gcm,
    c2s: Aes256Gcm,
    s2c: Aes256Gcm,
    addr: SocketAddr,
    max_counter: u64,
    server_counter: u64,
    last_eseq: u16,
    pressed: BTreeSet<u16>,
    last_rx: Instant,
    established: bool,
    packets: u64,
}

pub struct Server {
    shared: Arc<Mutex<Shared>>,
    kb: Keyboard,
    sessions: HashMap<u32, Session>,
    host_name: String,
    /// Set when sessions came or went, so the state file gets rewritten.
    pub changed: bool,
}

impl Server {
    pub fn new(shared: Arc<Mutex<Shared>>, kb: Keyboard, host_name: String) -> Server {
        Server { shared, kb, sessions: HashMap::new(), host_name, changed: true }
    }

    pub fn handle(&mut self, pkt: &[u8], from: SocketAddr, now: Instant) -> Option<Vec<u8>> {
        if pkt.len() > MAX_DATAGRAM {
            return None;
        }
        let header = Header::decode(pkt)?;
        match header.kind {
            T_HELLO => self.on_hello(&header, pkt, from, now),
            T_INPUT => self.on_input(&header, pkt, from, now),
            T_BYE => {
                self.on_bye(&header, pkt);
                None
            }
            _ => None,
        }
    }

    fn on_hello(&mut self, header: &Header, pkt: &[u8], from: SocketAddr, now: Instant) -> Option<Vec<u8>> {
        let id = header.device_id;
        let (key, pairing) = {
            let shared = self.shared.lock().unwrap();
            if let Some(dev) = shared.devices.find(&id) {
                (dev.key_bytes()?, false)
            } else if let Some(p) = shared.pending.as_ref().filter(|p| p.device_id == id && p.expires > now) {
                (p.key, true)
            } else {
                return Some(reject(id, REJECT_UNKNOWN_DEVICE));
            }
        };
        let device_cipher = cipher(&key);
        let hello = Hello::decode(&open(&device_cipher, pkt)?)?;

        if pairing {
            let mut shared = self.shared.lock().unwrap();
            // Re-check: another HELLO may have consumed the window meanwhile.
            if shared.pending.as_ref().map(|p| p.device_id) == Some(id) {
                shared.pending = None;
                let name = if hello.name.trim().is_empty() { "Phone".to_string() } else { hello.name.clone() };
                shared.devices.devices.push(Device {
                    id: hex(&id),
                    name: name.clone(),
                    key: crate::store::b64(&key),
                    paired_at: unix_now(),
                    last_seen: unix_now(),
                });
                if let Err(e) = shared.devices.save() {
                    eprintln!("omakeyd: couldn't save devices.json: {e}");
                }
                shared.paired = (shared.paired.0 + 1, name.clone());
                shared.dirty = true;
                eprintln!("omakeyd: paired {name} ({})", hex(&id));
            }
        }

        // A retried HELLO gets the same session back.
        if let Some(s) = self
            .sessions
            .values()
            .find(|s| s.device_id == id && !s.established && s.client_random == hello.client_random)
        {
            let h = Header { kind: T_WELCOME, device_id: id, nonce: random_bytes() };
            return Some(seal(&s.device_cipher, &h, &s.welcome));
        }

        self.trim_sessions(&id);
        let server_random: [u8; 16] = random_bytes();
        let session_id = loop {
            let sid = u32::from_be_bytes(random_bytes());
            if sid != 0 && !self.sessions.contains_key(&sid) {
                break sid;
            }
        };
        let keys = derive_session_keys(&key, &hello.client_random, &server_random);
        let welcome = Welcome {
            client_random: hello.client_random,
            server_random,
            session_id,
            name: self.host_name.clone(),
            features: FEATURE_POINTER,
        }
        .encode();
        let h = Header { kind: T_WELCOME, device_id: id, nonce: random_bytes() };
        let reply = seal(&device_cipher, &h, &welcome);
        self.sessions.insert(
            session_id,
            Session {
                device_id: id,
                name: hello.name,
                client_random: hello.client_random,
                welcome,
                device_cipher,
                c2s: cipher(&keys.c2s),
                s2c: cipher(&keys.s2c),
                addr: from,
                max_counter: 0,
                server_counter: 0,
                last_eseq: 0,
                pressed: BTreeSet::new(),
                last_rx: now,
                established: false,
                packets: 0,
            },
        );
        Some(reply)
    }

    /// Keep at most a few sessions per device, dropping the oldest.
    fn trim_sessions(&mut self, id: &DeviceId) {
        let mut mine: Vec<(Instant, u32)> = self
            .sessions
            .iter()
            .filter(|(_, s)| &s.device_id == id)
            .map(|(sid, s)| (s.last_rx, *sid))
            .collect();
        if mine.len() < MAX_SESSIONS_PER_DEVICE {
            return;
        }
        mine.sort();
        for (_, sid) in mine.iter().take(mine.len() + 1 - MAX_SESSIONS_PER_DEVICE) {
            self.drop_session(*sid);
        }
    }

    /// Find and authenticate a session packet. Returns the session id.
    fn authenticate(&mut self, header: &Header, pkt: &[u8]) -> Option<(u32, Vec<u8>)> {
        let (sid, counter) = header.session_counter();
        let s = self.sessions.get_mut(&sid)?;
        if s.device_id != header.device_id {
            return None;
        }
        let plain = open(&s.c2s, pkt)?;
        // Strictly increasing: drops replays and stale reordered packets.
        if counter <= s.max_counter {
            return None;
        }
        s.max_counter = counter;
        Some((sid, plain))
    }

    fn on_input(&mut self, header: &Header, pkt: &[u8], from: SocketAddr, now: Instant) -> Option<Vec<u8>> {
        let (sid, plain) = self.authenticate(header, pkt)?;
        let input = Input::decode(&plain)?;

        let first = {
            let s = self.sessions.get_mut(&sid).unwrap();
            s.last_rx = now;
            s.addr = from;
            s.packets += 1;
            let first = !s.established;
            s.established = true;
            first
        };
        if first {
            // The phone now uses this session; older ones of the device go.
            let dev = header.device_id;
            let old: Vec<u32> = self
                .sessions
                .iter()
                .filter(|(k, s)| **k != sid && s.device_id == dev)
                .map(|(k, _)| *k)
                .collect();
            for k in old {
                self.drop_session(k);
            }
            self.changed = true;
        }

        let s = self.sessions.get_mut(&sid).unwrap();
        let kb = &mut self.kb;

        for e in &input.events {
            if !eseq_newer(e.eseq, s.last_eseq) {
                continue;
            }
            s.last_eseq = e.eseq;
            if !allowed(e.code) {
                continue;
            }
            if e.down {
                if s.pressed.insert(e.code) {
                    kb.press(e.code);
                }
            } else if s.pressed.remove(&e.code) {
                kb.release(e.code);
            }
        }

        // Repair with the held set: anything an event was lost for.
        let held: BTreeSet<u16> = input.held.iter().copied().filter(|c| allowed(*c)).take(MAX_HELD).collect();
        let stale: Vec<u16> = s.pressed.difference(&held).copied().collect();
        for &c in stale.iter().filter(|c| !is_modifier(**c)).chain(stale.iter().filter(|c| is_modifier(**c))) {
            s.pressed.remove(&c);
            kb.release(c);
        }
        let missing: Vec<u16> = held.difference(&s.pressed).copied().collect();
        for &c in missing.iter().filter(|c| is_modifier(**c)).chain(missing.iter().filter(|c| !is_modifier(**c))) {
            s.pressed.insert(c);
            kb.press(c);
        }

        // Touchpad motion and scroll. Not retransmitted: a lost packet's
        // motion is simply lost, which is what a real mouse does too.
        if let Some(p) = input.pointer {
            if p != Pointer::default() {
                kb.pointer(p);
            }
        }

        s.server_counter += 1;
        let h = Header { kind: T_ACK, device_id: s.device_id, nonce: session_nonce(sid, s.server_counter) };
        let ack = Ack { client_time_ms: input.client_time_ms, last_eseq: s.last_eseq }.encode();
        Some(seal(&s.s2c, &h, &ack))
    }

    fn on_bye(&mut self, header: &Header, pkt: &[u8]) {
        if let Some((sid, _)) = self.authenticate(header, pkt) {
            self.drop_session(sid);
        }
    }

    fn release_all(kb: &mut Keyboard, s: &mut Session) {
        // Non-modifiers first, so a held SUPER can't combine with a key
        // that is being let go.
        let keys: Vec<u16> = std::mem::take(&mut s.pressed).into_iter().collect();
        for &c in keys.iter().filter(|c| !is_modifier(**c)).chain(keys.iter().filter(|c| is_modifier(**c))) {
            kb.release(c);
        }
    }

    fn drop_session(&mut self, sid: u32) {
        if let Some(mut s) = self.sessions.remove(&sid) {
            Self::release_all(&mut self.kb, &mut s);
            if s.established {
                let mut shared = self.shared.lock().unwrap();
                if let Some(d) = shared.devices.find_mut(&s.device_id) {
                    d.last_seen = unix_now();
                    let _ = shared.devices.save();
                }
                self.changed = true;
            }
        }
    }

    /// Timeouts. Call often (every ~50 ms).
    pub fn tick(&mut self, now: Instant) {
        let mut expired = Vec::new();
        for (sid, s) in self.sessions.iter_mut() {
            let idle = now.saturating_duration_since(s.last_rx);
            if idle > SESSION_TIMEOUT {
                expired.push(*sid);
            } else if idle > STUCK_KEY_TIMEOUT && !s.pressed.is_empty() {
                Self::release_all(&mut self.kb, s);
            }
        }
        for sid in expired {
            self.drop_session(sid);
        }

        let gone: Vec<u32> = {
            let mut shared = self.shared.lock().unwrap();
            if shared.pending.as_ref().is_some_and(|p| p.expires <= now) {
                shared.pending = None;
                shared.dirty = true;
            }
            // Sessions whose device was forgotten.
            self.sessions
                .iter()
                .filter(|(_, s)| shared.devices.find(&s.device_id).is_none())
                .map(|(k, _)| *k)
                .collect()
        };
        for sid in gone {
            self.drop_session(sid);
        }
    }

    pub fn session_infos(&self, now: Instant) -> Vec<SessionInfo> {
        let mut v: Vec<SessionInfo> = self
            .sessions
            .values()
            .filter(|s| s.established)
            .map(|s| SessionInfo {
                device: hex(&s.device_id),
                name: s.name.clone(),
                addr: s.addr.ip().to_string(),
                held: s.pressed.len(),
                idle_ms: now.saturating_duration_since(s.last_rx).as_millis() as u64,
                packets: s.packets,
            })
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn shutdown(&mut self) {
        let sids: Vec<u32> = self.sessions.keys().copied().collect();
        for sid in sids {
            self.drop_session(sid);
        }
    }

    #[cfg(test)]
    pub fn key_down(&self, code: u16) -> bool {
        self.kb.is_down(code)
    }
}
