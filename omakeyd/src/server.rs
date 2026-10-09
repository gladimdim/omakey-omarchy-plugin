//! Session state machine. No sockets here: `handle` takes a datagram and
//! returns the reply, so the whole protocol is unit-testable.

use crate::clipboard::{ClipError, Clipboard, Contents, Done, Job, Text};
use crate::hypr::KeyboardLayout;
use crate::keyboard::{allowed, is_modifier, KeySink, Keyboard};
use crate::protocol::*;
use crate::store::{hex, unix_now, Device, Devices};
use aes_gcm::Aes256Gcm;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Keys held by a silent session are let go after this long.
pub const STUCK_KEY_TIMEOUT: Duration = Duration::from_millis(500);
/// A silent session is forgotten after this long.
pub const SESSION_TIMEOUT: Duration = Duration::from_secs(30);
pub const PAIRING_TTL: Duration = Duration::from_secs(300);
/// Sessions of one device still waiting for their first INPUT. A replayed
/// HELLO can only push out these, never the session the phone is using.
const MAX_PENDING_PER_DEVICE: usize = 2;
const MAX_HELD: usize = 64;
/// ACKs per session that carry the desktop theme; one is enough, the rest cover loss.
const THEME_REPEATS: u8 = 4;
/// HELLOs per source IP: a token bucket of this rate and burst.
const HELLO_RATE: f32 = 10.0;
const HELLO_BURST: f32 = 20.0;
const KEY_LEFTCTRL: u16 = 29;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_INSERT: u16 = 110;

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
    /// "wifi" (any UDP) or "bluetooth".
    pub transport: &'static str,
    pub held: usize,
    pub idle_ms: u64,
    pub packets: u64,
    /// Packets lost, in percent of the last LOSS_WINDOW the phone sent.
    pub loss: f32,
}

/// What the Bluetooth fallback is doing, for state.json.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtStatus {
    /// "on", "off" (adapter powered off) or "unavailable".
    pub state: &'static str,
    pub address: Option<[u8; 6]>,
    pub reason: String,
}

impl Default for BtStatus {
    fn default() -> Self {
        BtStatus { state: "unavailable", address: None, reason: "starting".into() }
    }
}

/// State shared with the control socket and state threads. Every holder
/// keeps it only for in-memory work: no file I/O or slow calls under it.
pub struct Shared {
    pub devices: Devices,
    pub pending: Option<Pending>,
    /// Bumped whenever a phone finishes pairing, with its name.
    pub paired: (u64, String),
    pub sessions: Vec<SessionInfo>,
    pub bluetooth: BtStatus,
    /// The virtual keyboard: "ok", "away" (another user's session is in
    /// front at the seat), "unavailable" (can't open /dev/uinput) or "dry-run".
    pub uinput: &'static str,
    /// Set when devices, pairing or Bluetooth changed: rewrite state.json.
    pub dirty: bool,
    /// devices.json needs saving; `save_now` skips the debounce (pairing).
    pub devices_dirty: bool,
    pub save_now: bool,
    /// Devices forgotten from the control socket whose sessions must go.
    pub forgotten: Vec<String>,
}

impl Shared {
    pub fn new(devices: Devices) -> Shared {
        Shared {
            devices,
            pending: None,
            paired: (0, String::new()),
            sessions: Vec::new(),
            bluetooth: BtStatus::default(),
            uinput: "",
            dirty: true,
            devices_dirty: false,
            save_now: false,
            forgotten: Vec::new(),
        }
    }

    /// Close an expired pairing window.
    pub fn expire_pending(&mut self, now: Instant) {
        if self.pending.as_ref().is_some_and(|p| p.expires <= now) {
            self.pending = None;
            self.dirty = true;
        }
    }

    /// Record that these devices were just connected.
    pub fn mark_seen(&mut self, ids: &[DeviceId]) {
        let now = unix_now();
        for id in ids {
            if let Some(d) = self.devices.find_mut(id) {
                d.last_seen = now;
                self.devices_dirty = true;
            }
        }
    }
}

/// Where a session's packets come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Peer {
    Udp(SocketAddr),
    /// One RFCOMM connection (`conn` is unique per connection) from the
    /// phone with Bluetooth address `mac`.
    Bluetooth { mac: Arc<str>, conn: u64 },
}

impl Peer {
    pub fn transport(&self) -> &'static str {
        match self {
            Peer::Udp(_) => "wifi",
            Peer::Bluetooth { .. } => "bluetooth",
        }
    }
}

impl From<SocketAddr> for Peer {
    fn from(a: SocketAddr) -> Peer {
        Peer::Udp(a)
    }
}

impl fmt::Display for Peer {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Peer::Udp(a) => write!(f, "{}", a.ip()),
            Peer::Bluetooth { .. } => write!(f, "Bluetooth"),
        }
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
    addr: Peer,
    max_counter: u64,
    server_counter: u64,
    last_eseq: u16,
    pressed: BTreeSet<u16>,
    last_rx: Instant,
    established: bool,
    packets: u64,
    loss: LossMeter,
    /// ACKs that still carry the desktop theme: the first few of a
    /// session, and again after the theme changes.
    theme_left: u8,
    /// The clipboard transfer under way or last finished (PROTOCOL.md, CLIP).
    clip: Option<Transfer>,
}

/// One clipboard transfer: its id and where it is.
struct Transfer {
    id: u32,
    op: u8,
    state: ClipState,
}

enum ClipState {
    /// Put: the text so far, until it has `total` bytes.
    Receiving { buf: Vec<u8>, total: u32, flags: u8 },
    /// Put: the clipboard is being set, then pasted with `paste`.
    Writing { total: u32, paste: bool },
    /// Get with copy first: what the clipboard had before Ctrl+Insert.
    Before,
    /// Get: the clipboard is being read.
    Reading,
    /// Finished: the status, and for a get the text.
    Done { status: u8, total: u32, text: Option<Text> },
}

pub const LOSS_WINDOW: usize = 256;

/// Which of the last LOSS_WINDOW client counters never arrived.
#[derive(Default)]
struct LossMeter {
    lost: [u64; LOSS_WINDOW / 64],
    pos: usize,
    filled: usize,
}

impl LossMeter {
    fn record(&mut self, lost: bool) {
        let (w, b) = (self.pos / 64, self.pos % 64);
        if lost {
            self.lost[w] |= 1 << b;
        } else {
            self.lost[w] &= !(1 << b);
        }
        self.pos = (self.pos + 1) % LOSS_WINDOW;
        self.filled = (self.filled + 1).min(LOSS_WINDOW);
    }

    /// A packet with `counter` arrived after `max`: the ones between were lost
    /// (or came too late to be used, which is the same to the user).
    fn on_counter(&mut self, max: u64, counter: u64) {
        for _ in 0..(counter - max - 1).min(LOSS_WINDOW as u64) {
            self.record(true);
        }
        self.record(false);
    }

    fn percent(&self) -> f32 {
        if self.filled == 0 {
            return 0.0;
        }
        let lost: u32 = self.lost.iter().map(|w| w.count_ones()).sum();
        lost as f32 * 100.0 / self.filled as f32
    }
}

/// Token buckets per source IP for HELLOs. Each costs a decrypt, and a
/// REJECT to a spoofed source would make us a reflector.
#[derive(Default)]
struct RateLimiter {
    buckets: HashMap<IpAddr, (f32, Instant)>,
}

impl RateLimiter {
    fn allow(&mut self, ip: IpAddr, now: Instant) -> bool {
        if self.buckets.len() >= 4096 {
            // A bucket idle this long is full again, as if it weren't there.
            let full = Duration::from_secs_f32(HELLO_BURST / HELLO_RATE);
            self.buckets.retain(|_, (_, t)| now.saturating_duration_since(*t) < full);
            if self.buckets.len() >= 4096 {
                self.buckets.clear();
            }
        }
        let (tokens, last) = self.buckets.entry(ip).or_insert((HELLO_BURST, now));
        *tokens = (*tokens + now.saturating_duration_since(*last).as_secs_f32() * HELLO_RATE).min(HELLO_BURST);
        *last = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

pub struct Server {
    shared: Arc<Mutex<Shared>>,
    kb: Keyboard,
    sessions: HashMap<u32, Session>,
    host_name: String,
    /// Sent in WELCOME so phones learn where to reach us over Bluetooth.
    /// Some only while the profile is registered and the adapter is on.
    pub bt_address: Option<[u8; 6]>,
    /// Set when sessions came or went, so the state file gets rewritten.
    pub changed: bool,
    /// Devices whose session just ended, for their last_seen.
    seen: Vec<DeviceId>,
    hello_limit: RateLimiter,
    /// The desktop theme as an ACK trailer (PROTOCOL.md, ACK `theme`), if any.
    theme: Option<Vec<u8>>,
    /// Switches the keyboard's layout when a phone asks (INPUT `layout`);
    /// None in tests and where there's no Hyprland to ask.
    pub layout: Option<KeyboardLayout>,
    /// The desktop clipboard for CLIP; None where there's none to reach.
    pub clipboard: Option<Clipboard>,
}

impl Server {
    pub fn new(shared: Arc<Mutex<Shared>>, kb: Keyboard, host_name: String) -> Server {
        Server {
            shared,
            kb,
            sessions: HashMap::new(),
            host_name,
            bt_address: None,
            changed: true,
            seen: Vec::new(),
            hello_limit: RateLimiter::default(),
            theme: None,
            layout: None,
            clipboard: None,
        }
    }

    /// The desktop theme changed: every session hears about it again.
    pub fn set_theme(&mut self, theme: Option<Vec<u8>>) {
        self.theme = theme;
        for s in self.sessions.values_mut() {
            s.theme_left = THEME_REPEATS;
        }
    }

    pub fn handle(&mut self, pkt: &[u8], from: impl Into<Peer>, now: Instant) -> Option<Vec<u8>> {
        let from = from.into();
        if pkt.len() > MAX_DATAGRAM {
            return None;
        }
        let header = Header::decode(pkt)?;
        match header.kind {
            T_HELLO => {
                if let Peer::Udp(a) = &from {
                    if !self.hello_limit.allow(a.ip(), now) {
                        return None;
                    }
                }
                self.on_hello(&header, pkt, from, now)
            }
            T_INPUT => self.on_input(&header, pkt, from, now),
            T_CLIP => self.on_clip(&header, pkt),
            T_BYE => {
                self.on_bye(&header, pkt);
                None
            }
            _ => None,
        }
    }

    fn on_hello(&mut self, header: &Header, pkt: &[u8], from: Peer, now: Instant) -> Option<Vec<u8>> {
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
                // The state thread saves it right away, outside our locks.
                shared.devices_dirty = true;
                shared.save_now = true;
                shared.paired = (shared.paired.0 + 1, name.clone());
                shared.dirty = true;
                eprintln!("omakeyd: paired {name} ({})", hex(&id));
            }
        }

        // A retried HELLO gets the same session back.
        if let Some(s) = self
            .sessions
            .values_mut()
            .find(|s| s.device_id == id && !s.established && s.client_random == hello.client_random)
        {
            s.last_rx = now;
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
            features: FEATURE_POINTER | if self.clipboard.is_some() { FEATURE_CLIPBOARD } else { 0 },
            bt_address: self.bt_address,
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
                loss: LossMeter::default(),
                theme_left: THEME_REPEATS,
                clip: None,
            },
        );
        Some(reply)
    }

    /// Make room for a new session of this device by dropping its oldest
    /// not-yet-used ones. The established session is never touched: replayed
    /// HELLOs must not kick the phone off.
    fn trim_sessions(&mut self, id: &DeviceId) {
        let mut waiting: Vec<(Instant, u32)> = self
            .sessions
            .iter()
            .filter(|(_, s)| &s.device_id == id && !s.established)
            .map(|(sid, s)| (s.last_rx, *sid))
            .collect();
        if waiting.len() < MAX_PENDING_PER_DEVICE {
            return;
        }
        waiting.sort();
        for (_, sid) in waiting.iter().take(waiting.len() + 1 - MAX_PENDING_PER_DEVICE) {
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
        s.loss.on_counter(s.max_counter, counter);
        s.max_counter = counter;
        Some((sid, plain))
    }

    fn on_input(&mut self, header: &Header, pkt: &[u8], from: Peer, now: Instant) -> Option<Vec<u8>> {
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
        // The new session's keys go down before an older session's come up,
        // so a key held across a Wi-Fi/Bluetooth switch never flickers.
        // The layout first, so this packet's keys are read with it.
        if let Some(l) = self.layout.as_mut() {
            l.apply(input.layout.as_deref(), !input.events.is_empty(), now);
        }
        self.apply(sid, &input);
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

        let leds = self.kb.leds();
        let s = self.sessions.get_mut(&sid).unwrap();
        s.server_counter += 1;
        let h = Header { kind: T_ACK, device_id: s.device_id, nonce: session_nonce(sid, s.server_counter) };
        let mut ack = Ack { client_time_ms: input.client_time_ms, last_eseq: s.last_eseq, leds }.encode();
        // The theme follows `leds`, so it can only go when that does.
        if let (Some(_), Some(theme)) = (leds, &self.theme) {
            if s.theme_left > 0 {
                s.theme_left -= 1;
                ack.extend_from_slice(theme);
            }
        }
        Some(seal(&s.s2c, &h, &ack))
    }

    /// Apply one INPUT's events, held set and pointer to its session.
    fn apply(&mut self, sid: u32, input: &Input) {
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
    }

    /// A clipboard request. It doesn't count as hearing from the phone for
    /// the stuck-key timeout: only INPUT says what's held.
    fn on_clip(&mut self, header: &Header, pkt: &[u8]) -> Option<Vec<u8>> {
        let (sid, plain) = self.authenticate(header, pkt)?;
        let clip = Clip::decode(&plain, false)?;
        if !self.sessions[&sid].established {
            return None;
        }
        // Answer with the newest state.
        self.drain_clipboard();
        let mut reply = Clip { op: clip.op, clip_id: clip.clip_id, ..Default::default() };
        match (&self.clipboard, clip.op) {
            (None, CLIP_PUT | CLIP_GET) => reply.status = CLIP_FAILED,
            (Some(cb), CLIP_PUT) => Self::clip_put(cb, sid, self.sessions.get_mut(&sid).unwrap(), &clip, &mut reply),
            (Some(cb), CLIP_GET) => Self::clip_get(cb, sid, self.sessions.get_mut(&sid).unwrap(), &clip, &mut reply),
            _ => return None,
        }
        let s = self.sessions.get_mut(&sid).unwrap();
        s.server_counter += 1;
        let h = Header { kind: T_CLIP_REPLY, device_id: s.device_id, nonce: session_nonce(sid, s.server_counter) };
        Some(seal(&s.s2c, &h, &reply.encode(true)))
    }

    /// The phone's text, piece by piece; set (and pasted) once it's all in.
    fn clip_put(cb: &Clipboard, sid: u32, s: &mut Session, clip: &Clip, reply: &mut Clip) {
        reply.total = clip.total;
        if s.clip.as_ref().is_none_or(|t| t.id != clip.clip_id || t.op != CLIP_PUT) {
            // A new transfer starts at 0; anything else is from one we don't have.
            if clip.offset != 0 {
                reply.status = CLIP_UNKNOWN;
                return;
            }
            let state = if clip.total == 0 || clip.total as usize > MAX_CLIP {
                let status = if clip.total == 0 { CLIP_EMPTY } else { CLIP_TOO_LARGE };
                ClipState::Done { status, total: 0, text: None }
            } else {
                ClipState::Receiving { buf: Vec::with_capacity(clip.total as usize), total: clip.total, flags: clip.flags }
            };
            s.clip = Some(Transfer { id: clip.clip_id, op: CLIP_PUT, state });
        }
        let t = s.clip.as_mut().unwrap();
        if let ClipState::Receiving { buf, total, flags } = &mut t.state {
            // Only the piece that carries on from what we have.
            if clip.offset as usize == buf.len() {
                let take = clip.data.len().min(*total as usize - buf.len());
                buf.extend_from_slice(&clip.data[..take]);
            }
            if buf.len() == *total as usize {
                let text = Text { bytes: std::mem::take(buf), sensitive: *flags & CLIP_SENSITIVE != 0 };
                let paste = *flags & CLIP_PASTE != 0;
                t.state = ClipState::Writing { total: *total, paste };
                cb.submit(Job::Write { tag: (sid, t.id), text });
            }
        }
        match &t.state {
            ClipState::Receiving { buf, .. } => reply.offset = buf.len() as u32,
            ClipState::Writing { total, .. } => {
                reply.status = CLIP_WORKING;
                reply.offset = *total;
            }
            ClipState::Done { status, .. } => {
                reply.status = *status;
                reply.offset = if *status == CLIP_OK { clip.total } else { 0 };
            }
            ClipState::Before | ClipState::Reading => unreachable!("a put never reads"),
        }
    }

    /// The desktop's text, from the asked offset; read (after a copy) first.
    fn clip_get(cb: &Clipboard, sid: u32, s: &mut Session, clip: &Clip, reply: &mut Clip) {
        if s.clip.as_ref().is_none_or(|t| t.id != clip.clip_id || t.op != CLIP_GET) {
            // With copy first, read what's there before pressing Ctrl+Insert,
            // to see when the copy lands.
            let copy = clip.flags & CLIP_COPY != 0;
            cb.submit(Job::Read { tag: (sid, clip.clip_id), changed_from: None });
            let state = if copy { ClipState::Before } else { ClipState::Reading };
            s.clip = Some(Transfer { id: clip.clip_id, op: CLIP_GET, state });
        }
        reply.offset = clip.offset;
        match &s.clip.as_ref().unwrap().state {
            ClipState::Done { status, total, text } => {
                reply.status = *status;
                reply.total = *total;
                if let Some(text) = text {
                    let from = (clip.offset as usize).min(text.bytes.len());
                    let to = (from + CLIP_CHUNK).min(text.bytes.len());
                    reply.data = text.bytes[from..to].to_vec();
                    reply.offset = from as u32;
                    if text.sensitive {
                        reply.flags |= CLIP_SENSITIVE;
                    }
                }
            }
            _ => reply.status = CLIP_WORKING,
        }
    }

    /// Take the clipboard worker's results: press the keys a copy or paste
    /// needs and move each transfer on.
    fn drain_clipboard(&mut self) {
        while let Some(done) = self.clipboard.as_ref().and_then(Clipboard::poll) {
            let ((sid, id), result) = match done {
                Done::Read { tag, contents } => (tag, Ok(contents)),
                Done::Write { tag, ok } => (tag, Err(ok)),
            };
            let Some(s) = self.sessions.get_mut(&sid) else { continue };
            let Some(t) = s.clip.as_mut().filter(|t| t.id == id) else { continue };
            let kb = &mut self.kb;
            t.state = match (std::mem::replace(&mut t.state, ClipState::Reading), result) {
                (ClipState::Before, Ok(before)) => {
                    tap(kb, KEY_LEFTCTRL, KEY_INSERT);
                    if before == Err(ClipError::Failed) {
                        // No clipboard to read (SteamOS's Game Mode): the copy
                        // happened on the desktop, there's nothing to send.
                        read_done(before)
                    } else {
                        self.clipboard.as_ref().unwrap().submit(Job::Read { tag: (sid, id), changed_from: Some(before) });
                        ClipState::Reading
                    }
                }
                (ClipState::Reading, Ok(contents)) => read_done(contents),
                (ClipState::Writing { total, paste }, Err(ok)) => {
                    if ok && paste {
                        tap(kb, KEY_LEFTSHIFT, KEY_INSERT);
                    }
                    ClipState::Done { status: if ok { CLIP_OK } else { CLIP_FAILED }, total, text: None }
                }
                // A result for a state that moved on: keep the state.
                (state, _) => state,
            };
        }
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
                self.seen.push(s.device_id);
                self.changed = true;
            }
        }
    }

    /// Devices whose session ended since the last call, for their last_seen.
    pub fn take_seen(&mut self) -> Vec<DeviceId> {
        std::mem::take(&mut self.seen)
    }

    /// End the sessions of a device that was forgotten.
    pub fn drop_device(&mut self, hex_id: &str) {
        let gone: Vec<u32> = self.sessions.iter().filter(|(_, s)| hex(&s.device_id) == hex_id).map(|(k, _)| *k).collect();
        for sid in gone {
            self.drop_session(sid);
        }
    }

    /// A transport connection closed: let go of the keys of the sessions
    /// last heard on it. The sessions stay, as the phone may carry on over
    /// another transport.
    pub fn peer_gone(&mut self, peer: &Peer) {
        for s in self.sessions.values_mut().filter(|s| &s.addr == peer) {
            Self::release_all(&mut self.kb, s);
        }
        self.changed = true;
    }

    /// Whether the seat's session in front is ours (see seat.rs).
    pub fn in_front(&self) -> bool {
        self.kb.in_front()
    }

    pub fn has_device(&self) -> bool {
        self.kb.has_device()
    }

    /// Swap the virtual devices: None while another user's session is in
    /// front. The sessions stay, holding nothing; the next INPUT presses
    /// what the phone still holds on the new device.
    pub fn set_device(&mut self, sink: Option<Box<dyn KeySink + Send>>) {
        for s in self.sessions.values_mut() {
            s.pressed.clear();
        }
        self.kb.set_device(sink);
        self.changed = true;
    }

    /// Timeouts. Call often (every ~50 ms). Also drains the LED events.
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
        // The kernel keeps only a few events for us; read them before they wrap.
        self.kb.leds();
        self.drain_clipboard();
    }

    pub fn session_infos(&self, now: Instant) -> Vec<SessionInfo> {
        let mut v: Vec<SessionInfo> = self
            .sessions
            .values()
            .filter(|s| s.established)
            .map(|s| SessionInfo {
                device: hex(&s.device_id),
                name: s.name.clone(),
                addr: s.addr.to_string(),
                transport: s.addr.transport(),
                held: s.pressed.len(),
                idle_ms: now.saturating_duration_since(s.last_rx).as_millis() as u64,
                packets: s.packets,
                loss: s.loss.percent(),
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

/// A shortcut on the virtual keyboard: `modifier` + `key`, pressed and let go.
fn tap(kb: &mut Keyboard, modifier: u16, key: u16) {
    kb.press(modifier);
    kb.press(key);
    kb.release(key);
    kb.release(modifier);
}

fn read_done(contents: Contents) -> ClipState {
    match contents {
        Ok(text) => ClipState::Done { status: CLIP_OK, total: text.bytes.len() as u32, text: Some(text) },
        Err(e) => {
            let status = match e {
                ClipError::Empty => CLIP_EMPTY,
                ClipError::TooLarge => CLIP_TOO_LARGE,
                ClipError::Failed => CLIP_FAILED,
            };
            ClipState::Done { status, total: 0, text: None }
        }
    }
}
