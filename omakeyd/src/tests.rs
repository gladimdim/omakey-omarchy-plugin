//! End-to-end protocol tests: a real Client talking to a real Server, with
//! a recording key sink in place of uinput.

use crate::client::{Client, Reply};
use crate::keyboard::test_sink::Recorder;
use crate::keyboard::Keyboard;
use crate::protocol::*;
use crate::server::*;
use crate::store::Devices;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

const META: u16 = 125;
const SPACE: u16 = 57;
const A: u16 = 30;

fn isolate_config() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("omakeyd-test-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", dir);
    });
}

struct Rig {
    server: Server,
    shared: Arc<Mutex<Shared>>,
    rec: Recorder,
    client: Client,
    addr: SocketAddr,
    t0: Instant,
}

impl Rig {
    /// A server with an open pairing window and a client holding its QR.
    fn new() -> Rig {
        isolate_config();
        let shared = Arc::new(Mutex::new(Shared::new(Devices::default())));
        let device_id: DeviceId = random_bytes();
        let key: Key = random_bytes();
        let t0 = Instant::now();
        shared.lock().unwrap().pending = Some(Pending {
            device_id,
            key,
            uri: String::new(),
            expires: t0 + PAIRING_TTL,
            expires_unix: 0,
        });
        let rec = Recorder::default();
        let server = Server::new(shared.clone(), Keyboard::new(Box::new(rec.clone())), "desk".into());
        Rig { server, shared, rec, client: Client::new(device_id, key), addr: "192.168.1.9:5000".parse().unwrap(), t0 }
    }

    fn send(&mut self, pkt: Vec<u8>, at_ms: u64) -> Option<Reply> {
        let reply = self.server.handle(&pkt, self.addr, self.t0 + Duration::from_millis(at_ms))?;
        self.client.on_packet(&reply)
    }

    fn connect(&mut self) {
        let hello = self.client.hello("Pixel");
        assert!(matches!(self.send(hello, 0), Some(Reply::Welcome { .. })));
        assert!(self.client.connected());
    }

    fn key(&mut self, code: u16, down: bool, at_ms: u64) -> Option<Reply> {
        let pkt = self.client.key(code, down, at_ms as u32).unwrap();
        self.send(pkt, at_ms)
    }
}

#[test]
fn pairing_commits_device_and_closes_window() {
    let mut r = Rig::new();
    r.connect();
    let s = r.shared.lock().unwrap();
    assert!(s.pending.is_none());
    assert_eq!(s.devices.devices.len(), 1);
    assert_eq!(s.devices.devices[0].name, "Pixel");
    assert_eq!(s.paired.0, 1);
}

#[test]
fn super_space_chord() {
    let mut r = Rig::new();
    r.connect();
    r.key(META, true, 10);
    r.key(SPACE, true, 20);
    r.key(SPACE, false, 30);
    r.key(META, false, 40);
    assert_eq!(r.rec.take(), vec![(META, true), (SPACE, true), (SPACE, false), (META, false)]);
}

#[test]
fn lost_press_packet_still_types_the_tap() {
    let mut r = Rig::new();
    r.connect();
    let _lost = r.client.key(A, true, 10).unwrap();
    let release = r.client.key(A, false, 20).unwrap();
    r.send(release, 20);
    assert_eq!(r.rec.take(), vec![(A, true), (A, false)]);
}

#[test]
fn ack_clears_resend_queue() {
    let mut r = Rig::new();
    r.connect();
    r.key(A, true, 10);
    assert!(!r.client.has_unacked());
}

#[test]
fn lost_events_beyond_tail_are_repaired_from_held_set() {
    let mut r = Rig::new();
    r.connect();
    // 40 events never arrive; only the last packet does. The held set still
    // says META is down.
    let mut last = None;
    for i in 0..20 {
        last = r.client.key(A, i % 2 == 0, i);
    }
    last = r.client.key(META, true, 30).or(last);
    for i in 0..20 {
        last = r.client.key(SPACE, i % 2 == 0, 40 + i).or(last);
    }
    r.send(last.unwrap(), 100);
    assert!(r.server.key_down(META));
    assert!(!r.server.key_down(SPACE));
    assert!(!r.server.key_down(A));
}

#[test]
fn replayed_packet_is_ignored() {
    let mut r = Rig::new();
    r.connect();
    let press = r.client.key(A, true, 10).unwrap();
    assert!(r.server.handle(&press, r.addr, r.t0).is_some());
    r.key(A, false, 20);
    r.rec.take();
    assert!(r.server.handle(&press, r.addr, r.t0).is_none());
    assert!(r.rec.take().is_empty());
}

#[test]
fn silent_phone_releases_keys_then_recovers() {
    let mut r = Rig::new();
    r.connect();
    r.key(META, true, 10);
    r.server.tick(r.t0 + Duration::from_millis(300));
    assert!(r.server.key_down(META));
    r.server.tick(r.t0 + Duration::from_millis(700));
    assert!(!r.server.key_down(META));
    // The phone comes back still holding SUPER: the heartbeat re-presses it.
    let hb = r.client.input(800).unwrap();
    r.send(hb, 800);
    assert!(r.server.key_down(META));
}

#[test]
fn bye_releases_everything() {
    let mut r = Rig::new();
    r.connect();
    r.key(META, true, 10);
    r.key(A, true, 20);
    let bye = r.client.bye().unwrap();
    r.server.handle(&bye, r.addr, r.t0);
    assert!(!r.server.key_down(META) && !r.server.key_down(A));
}

#[test]
fn unknown_device_is_rejected_in_plaintext() {
    let mut r = Rig::new();
    let stranger = Client::new(random_bytes(), random_bytes());
    let reply = r.server.handle(&stranger.hello("x"), r.addr, r.t0).unwrap();
    assert_eq!(reply[3], T_REJECT);
    assert_eq!(reply[HEADER_LEN], REJECT_UNKNOWN_DEVICE);
}

#[test]
fn wrong_key_is_dropped_silently() {
    let mut r = Rig::new();
    let id = r.shared.lock().unwrap().pending.as_ref().unwrap().device_id;
    let impostor = Client::new(id, random_bytes());
    assert!(r.server.handle(&impostor.hello("x"), r.addr, r.t0).is_none());
    assert!(r.shared.lock().unwrap().pending.is_some());
}

#[test]
fn retried_hello_gets_the_same_session() {
    let mut r = Rig::new();
    let h1 = r.client.hello("Pixel");
    let h2 = r.client.hello("Pixel");
    let w1 = r.server.handle(&h1, r.addr, r.t0).unwrap();
    let w2 = r.server.handle(&h2, r.addr, r.t0).unwrap();
    let c = cipher(&{
        let s = r.shared.lock().unwrap();
        s.devices.devices[0].key_bytes().unwrap()
    });
    let a = Welcome::decode(&open(&c, &w1).unwrap()).unwrap();
    let b = Welcome::decode(&open(&c, &w2).unwrap()).unwrap();
    assert_eq!(a.session_id, b.session_id);
}

#[test]
fn chord_after_lost_packets_keeps_order() {
    let mut r = Rig::new();
    r.connect();
    let _lost1 = r.client.key(META, true, 10);
    let _lost2 = r.client.key(SPACE, true, 11);
    // Simulate a packet with only the held set (events lost beyond the tail)
    // by sending a fresh heartbeat from a client whose queue was trimmed.
    let hb = r.client.input(12).unwrap();
    r.send(hb, 12);
    let keys = r.rec.take();
    assert_eq!(keys.first(), Some(&(META, true)));
}

#[test]
fn second_phone_holding_same_key_keeps_it_down() {
    let mut r = Rig::new();
    r.connect();
    // Pair a second phone.
    let (id2, key2): (DeviceId, Key) = (random_bytes(), random_bytes());
    r.shared.lock().unwrap().pending =
        Some(Pending { device_id: id2, key: key2, uri: String::new(), expires: r.t0 + PAIRING_TTL, expires_unix: 0 });
    let mut c2 = Client::new(id2, key2);
    let w = r.server.handle(&c2.hello("Tab"), r.addr, r.t0).unwrap();
    c2.on_packet(&w);
    r.key(A, true, 10);
    let p = c2.key(A, true, 10).unwrap();
    r.server.handle(&p, r.addr, r.t0);
    r.key(A, false, 20);
    assert!(r.server.key_down(A));
    let p = c2.key(A, false, 30).unwrap();
    r.server.handle(&p, r.addr, r.t0 + Duration::from_millis(30));
    assert!(!r.server.key_down(A));
}

#[test]
fn touchpad_buttons_and_motion_reach_the_mouse() {
    use crate::keyboard::BTN_LEFT;
    let mut r = Rig::new();
    r.connect();
    r.key(BTN_LEFT, true, 10);
    let p = Pointer { dx: 5, dy: -3, wheel: 0, hwheel: 0 };
    let pkt = r.client.pointer(p, 11).unwrap();
    r.send(pkt, 11);
    r.key(BTN_LEFT, false, 12);
    assert_eq!(r.rec.take(), vec![(BTN_LEFT, true), (BTN_LEFT, false)]);
    assert_eq!(*r.rec.1.lock().unwrap(), vec![p]);
    // Motion isn't resent with the next heartbeat.
    let hb = r.client.input(20).unwrap();
    r.send(hb, 20);
    assert_eq!(r.rec.1.lock().unwrap().len(), 1);
}

#[test]
fn a_held_button_is_released_when_the_phone_goes_quiet() {
    use crate::keyboard::BTN_LEFT;
    let mut r = Rig::new();
    r.connect();
    r.key(BTN_LEFT, true, 10);
    r.server.tick(r.t0 + Duration::from_millis(700));
    assert!(!r.server.key_down(BTN_LEFT));
}

#[test]
fn welcome_advertises_the_touchpad() {
    let mut r = Rig::new();
    let hello = r.client.hello("Pixel");
    let reply = r.server.handle(&hello, r.addr, r.t0).unwrap();
    let key = r.shared.lock().unwrap().devices.devices[0].key_bytes().unwrap();
    let w = Welcome::decode(&open(&cipher(&key), &reply).unwrap()).unwrap();
    assert_eq!(w.features & FEATURE_POINTER, FEATURE_POINTER);
}
