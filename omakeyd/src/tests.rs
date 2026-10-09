//! End-to-end protocol tests: a real Client talking to a real Server, with
//! a recording key sink in place of uinput.

use crate::client::{Client, Reply};
use crate::keyboard::test_sink::Recorder;
use crate::keyboard::Keyboard;
use crate::protocol::*;
use crate::server::*;
use crate::store::Devices;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// The seat watcher's flag: whether our session is in front.
    in_front: Arc<AtomicBool>,
    client: Client,
    addr: SocketAddr,
    t0: Instant,
    device_id: DeviceId,
    key: Key,
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
        let in_front = Arc::new(AtomicBool::new(true));
        let kb = Keyboard::gated(Some(Box::new(rec.clone())), in_front.clone());
        let server = Server::new(shared.clone(), kb, "desk".into());
        Rig {
            server,
            shared,
            rec,
            in_front,
            client: Client::new(device_id, key),
            addr: "192.168.1.9:5000".parse().unwrap(),
            t0,
            device_id,
            key,
        }
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

fn bt_peer(conn: u64) -> Peer {
    Peer::Bluetooth { mac: "AA:BB:CC:DD:EE:FF".into(), conn }
}

/// Send one framed datagram on a Bluetooth stream and read the framed reply.
fn exchange(phone: &mut std::os::unix::net::UnixStream, client: &mut Client, pkt: Vec<u8>) -> Option<Reply> {
    use std::io::{Read, Write};
    let mut frame = (pkt.len() as u16).to_be_bytes().to_vec();
    frame.extend(pkt);
    phone.write_all(&frame).unwrap();
    let mut len = [0u8; 2];
    phone.read_exact(&mut len).unwrap();
    let mut reply = vec![0u8; u16::from_be_bytes(len) as usize];
    phone.read_exact(&mut reply).unwrap();
    client.on_packet(&reply)
}

#[test]
fn bluetooth_stream_carries_the_same_packets() {
    use std::os::unix::net::UnixStream;
    let Rig { server, rec, mut client, .. } = Rig::new();
    let server = Arc::new(Mutex::new(server));
    let (mut phone, desk) = UnixStream::pair().unwrap();
    // BlueZ hands the socket over non-blocking; serving must still wait for data.
    desk.set_nonblocking(true).unwrap();
    let s2 = server.clone();
    let desk_thread = std::thread::spawn(move || crate::bluetooth::serve_connection(desk, bt_peer(1), &s2));
    std::thread::sleep(Duration::from_millis(50));

    let hello = client.hello("Pixel");
    assert!(matches!(exchange(&mut phone, &mut client, hello), Some(Reply::Welcome { .. })));
    let press = client.key(A, true, 1).unwrap();
    assert!(exchange(&mut phone, &mut client, press).is_some());
    assert_eq!(rec.take(), vec![(A, true)]);

    drop(phone);
    desk_thread.join().unwrap();
    let infos = server.lock().unwrap().session_infos(Instant::now());
    assert_eq!(infos[0].addr, "Bluetooth");
    assert_eq!(infos[0].transport, "bluetooth");
}

#[test]
fn bluetooth_eof_releases_keys_but_keeps_the_session() {
    use std::os::unix::net::UnixStream;
    let Rig { server, mut client, addr, .. } = Rig::new();
    let server = Arc::new(Mutex::new(server));
    let (mut phone, desk) = UnixStream::pair().unwrap();
    let s2 = server.clone();
    let desk_thread = std::thread::spawn(move || crate::bluetooth::serve_connection(desk, bt_peer(7), &s2));
    let hello = client.hello("Pixel");
    exchange(&mut phone, &mut client, hello);
    let press = client.key(META, true, 1).unwrap();
    exchange(&mut phone, &mut client, press);
    assert!(server.lock().unwrap().key_down(META));

    // The link drops: SUPER comes up at once, not after the stuck-key timeout.
    drop(phone);
    desk_thread.join().unwrap();
    assert!(!server.lock().unwrap().key_down(META));
    // The phone carries on over Wi-Fi in the same session.
    let hb = client.input(10).unwrap();
    let reply = server.lock().unwrap().handle(&hb, addr, Instant::now()).unwrap();
    assert!(matches!(client.on_packet(&reply), Some(Reply::Ack { .. })));
    assert!(server.lock().unwrap().key_down(META));
}

#[test]
fn replayed_hellos_cannot_evict_the_phones_session() {
    let mut r = Rig::new();
    r.connect();
    r.key(META, true, 10);
    // An attacker replays HELLOs it captured earlier, each with its own client_random.
    for i in 0..10 {
        let old = Client::new(r.device_id, r.key);
        assert!(r.server.handle(&old.hello("Pixel"), r.addr, r.t0 + Duration::from_millis(20 + i)).is_some());
    }
    assert!(r.server.key_down(META));
    assert!(matches!(r.key(A, true, 40), Some(Reply::Ack { .. })));
    assert!(r.server.key_down(A));
}

#[test]
fn switching_transport_does_not_flicker_held_keys() {
    use crate::keyboard::BTN_LEFT;
    let mut r = Rig::new();
    r.connect();
    // A drag over Wi-Fi with SUPER held...
    r.key(META, true, 10);
    r.key(BTN_LEFT, true, 20);
    r.rec.take();
    // ...continues over Bluetooth in a new session, keys still held.
    let mut bt = Client::new(r.device_id, r.key);
    let w = r.server.handle(&bt.hello("Pixel"), bt_peer(3), r.t0 + Duration::from_millis(30)).unwrap();
    assert!(matches!(bt.on_packet(&w), Some(Reply::Welcome { .. })));
    bt.key(META, true, 40);
    let p = bt.key(BTN_LEFT, true, 41).unwrap();
    r.server.handle(&p, bt_peer(3), r.t0 + Duration::from_millis(41)).unwrap();
    assert_eq!(r.rec.take(), vec![], "no key went up and down");
    assert!(r.server.key_down(META) && r.server.key_down(BTN_LEFT));
    // The Wi-Fi session is gone: its next packet isn't acknowledged.
    let old = r.client.input(50).unwrap();
    assert!(r.server.handle(&old, r.addr, r.t0 + Duration::from_millis(50)).is_none());
}

#[test]
fn first_acks_carry_the_desktop_theme() {
    let mut r = Rig::new();
    r.connect();
    *r.rec.2.lock().unwrap() = Some(0);
    let theme = crate::theme::Theme::parse("nord", "background = \"#2e3440\"\nforeground = \"#eceff4\"").unwrap();
    r.server.set_theme(Some(theme.encode()));
    let mut themed = 0;
    for i in 0..8 {
        match r.key(A, i % 2 == 0, 10 + i * 10) {
            Some(Reply::Ack { theme: Some(t), .. }) => {
                assert_eq!(t, theme);
                themed += 1;
            }
            Some(Reply::Ack { theme: None, .. }) => {}
            _ => panic!("no ACK"),
        }
    }
    assert_eq!(themed, 4);
    // A new theme goes out again.
    r.server.set_theme(Some(theme.encode()));
    assert!(matches!(r.key(A, true, 200), Some(Reply::Ack { theme: Some(_), .. })));
}

#[test]
fn ack_carries_the_lock_leds() {
    let mut r = Rig::new();
    r.connect();
    *r.rec.2.lock().unwrap() = Some(LED_CAPS | LED_NUM);
    match r.key(A, true, 10) {
        Some(Reply::Ack { leds, .. }) => assert_eq!(leds, Some(3)),
        _ => panic!("no ACK"),
    }
    // A sink that can't read LEDs sends no trailer.
    *r.rec.2.lock().unwrap() = None;
    match r.key(A, false, 20) {
        Some(Reply::Ack { leds, .. }) => assert_eq!(leds, None),
        _ => panic!("no ACK"),
    }
}

#[test]
fn hellos_are_rate_limited_per_source() {
    let mut r = Rig::new();
    let stranger = Client::new(random_bytes(), random_bytes());
    let from: SocketAddr = "10.0.0.66:4000".parse().unwrap();
    let answered = |r: &mut Rig, n: usize, at: Duration| {
        (0..n).filter(|_| r.server.handle(&stranger.hello("x"), from, r.t0 + at).is_some()).count()
    };
    assert_eq!(answered(&mut r, 30, Duration::ZERO), 20);
    // The bucket refills at 10 per second.
    assert_eq!(answered(&mut r, 30, Duration::from_millis(1000)), 10);
    // Another source isn't affected.
    let other = r.server.handle(&stranger.hello("x"), r.addr, r.t0 + Duration::from_millis(1000));
    assert!(other.is_some());
}

#[test]
fn loss_is_estimated_from_counter_gaps() {
    let mut r = Rig::new();
    r.connect();
    for i in 0..20u64 {
        let pkt = r.client.input(i as u32).unwrap();
        // Every fourth packet is lost.
        if i % 4 != 3 {
            r.send(pkt, i);
        }
    }
    let infos = r.server.session_infos(r.t0);
    assert_eq!(infos[0].transport, "wifi");
    // 19 counters seen through the last delivered one, 4 of them lost.
    assert!((infos[0].loss - 4.0 * 100.0 / 19.0).abs() < 0.01, "{}", infos[0].loss);
}

#[test]
fn input_layout_round_trips_after_the_pointer() {
    let with = Input { client_time_ms: 7, held: vec![30], layout: Some("ua".into()), ..Default::default() };
    let enc = with.encode();
    let back = Input::decode(&enc).unwrap();
    assert_eq!(back.layout.as_deref(), Some("ua"));
    // The pointer went as zeros to make room for it.
    assert_eq!(back.pointer, Some(Pointer { dx: 0, dy: 0, wheel: 0, hwheel: 0 }));
    // Without one, the packet is as before.
    let without = Input { client_time_ms: 7, held: vec![30], ..Default::default() };
    assert_eq!(Input::decode(&without.encode()).unwrap().layout, None);
    // Junk isn't taken as a layout name.
    let mut bad = enc.clone();
    let n = bad.len();
    bad[n - 1] = b'/';
    assert_eq!(Input::decode(&bad).unwrap().layout, None);
}


// ---- clipboard (PROTOCOL.md, CLIP) ----

const CTRL: u16 = 29;
const SHIFT: u16 = 42;
const INSERT: u16 = 110;

impl Rig {
    /// The rig with an in-memory clipboard, connected and past its first INPUT.
    fn with_clipboard() -> (Rig, crate::clipboard::fake::Fake) {
        let mut r = Rig::new();
        let fake = crate::clipboard::fake::Fake::default();
        r.server.clipboard = Some(crate::clipboard::Clipboard::start(Box::new(fake.clone())));
        r.connect();
        let input = r.client.input(5).unwrap();
        r.send(input, 5);
        (r, fake)
    }

    fn clip(&mut self, c: Clip) -> Option<Clip> {
        let pkt = self.client.clip(&c).unwrap();
        match self.send(pkt, 10) {
            Some(Reply::Clip(reply)) => Some(reply),
            _ => None,
        }
    }

    /// Ask again while the server says it's working, as the phone does.
    fn clip_until_done(&mut self, c: Clip) -> Clip {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let reply = self.clip(c.clone()).expect("a reply");
            if reply.status != CLIP_WORKING || Instant::now() > deadline {
                return reply;
            }
            std::thread::sleep(Duration::from_millis(5));
            self.server.tick(self.t0 + Duration::from_millis(10));
        }
    }

    /// Put [text] the way the phone does: in order, a chunk at a time.
    fn put(&mut self, id: u32, text: &[u8], flags: u8) -> Clip {
        let mut offset = 0usize;
        loop {
            let end = (offset + CLIP_CHUNK).min(text.len());
            let c = Clip { op: CLIP_PUT, clip_id: id, offset: offset as u32, flags, total: text.len() as u32, data: text[offset..end].to_vec(), ..Default::default() };
            let reply = self.clip_until_done(c);
            if reply.status != CLIP_OK || reply.offset as usize == text.len() {
                return reply;
            }
            offset = reply.offset as usize;
        }
    }

    /// Get the desktop's text the way the phone does.
    fn get(&mut self, id: u32, flags: u8) -> Result<(Vec<u8>, u8), u8> {
        let mut text = Vec::new();
        loop {
            let c = Clip { op: CLIP_GET, clip_id: id, offset: text.len() as u32, flags, ..Default::default() };
            let reply = self.clip_until_done(c);
            if reply.status != CLIP_OK {
                return Err(reply.status);
            }
            assert_eq!(reply.offset as usize, text.len());
            text.extend_from_slice(&reply.data);
            if text.len() >= reply.total as usize {
                return Ok((text, reply.flags));
            }
        }
    }
}

#[test]
fn welcome_advertises_the_clipboard_only_when_there_is_one() {
    let mut r = Rig::new();
    let hello = r.client.hello("Pixel");
    let reply = r.server.handle(&hello, r.addr, r.t0).unwrap();
    let w = Welcome::decode(&open(&cipher(&r.key), &reply).unwrap()).unwrap();
    assert_eq!(w.features & FEATURE_CLIPBOARD, 0);

    let mut r = Rig::new();
    r.server.clipboard = Some(crate::clipboard::Clipboard::start(Box::new(crate::clipboard::fake::Fake::default())));
    let hello = r.client.hello("Pixel");
    let reply = r.server.handle(&hello, r.addr, r.t0).unwrap();
    let w = Welcome::decode(&open(&cipher(&r.key), &reply).unwrap()).unwrap();
    assert_eq!(w.features, FEATURE_POINTER | FEATURE_CLIPBOARD);
}

#[test]
fn put_sets_the_clipboard_then_pastes() {
    let (mut r, fake) = Rig::with_clipboard();
    let text = "Привіт, desktop! ".repeat(150).into_bytes(); // several chunks
    assert!(text.len() > 2 * CLIP_CHUNK);
    let reply = r.put(7, &text, CLIP_PASTE);
    assert_eq!((reply.status, reply.offset as usize), (CLIP_OK, text.len()));
    let set = fake.contents.lock().unwrap().clone().unwrap();
    assert_eq!(set.bytes, text);
    assert!(!set.sensitive);
    assert_eq!(r.rec.take(), vec![(SHIFT, true), (INSERT, true), (INSERT, false), (SHIFT, false)]);
    // Asking again doesn't paste again.
    let again = r.clip(Clip { op: CLIP_PUT, clip_id: 7, offset: text.len() as u32, total: text.len() as u32, ..Default::default() }).unwrap();
    assert_eq!(again.status, CLIP_OK);
    assert!(r.rec.take().is_empty());
}

#[test]
fn put_keeps_only_the_piece_that_carries_on() {
    let (mut r, fake) = Rig::with_clipboard();
    let text = vec![b'x'; CLIP_CHUNK + 10];
    let first = Clip { op: CLIP_PUT, clip_id: 1, total: text.len() as u32, flags: CLIP_SENSITIVE, data: text[..CLIP_CHUNK].to_vec(), ..Default::default() };
    assert_eq!(r.clip(first.clone()).unwrap().offset as usize, CLIP_CHUNK);
    // A resent first piece changes nothing.
    assert_eq!(r.clip(first).unwrap().offset as usize, CLIP_CHUNK);
    // A piece past the end of what the server has is ignored.
    let gap = Clip { op: CLIP_PUT, clip_id: 1, offset: CLIP_CHUNK as u32 + 5, total: text.len() as u32, data: vec![b'x'; 5], ..Default::default() };
    assert_eq!(r.clip(gap).unwrap().offset as usize, CLIP_CHUNK);
    let last = Clip { op: CLIP_PUT, clip_id: 1, offset: CLIP_CHUNK as u32, total: text.len() as u32, flags: CLIP_SENSITIVE, data: text[CLIP_CHUNK..].to_vec(), ..Default::default() };
    assert_eq!(r.clip_until_done(last).status, CLIP_OK);
    let set = fake.contents.lock().unwrap().clone().unwrap();
    assert_eq!(set.bytes, text);
    assert!(set.sensitive);
    // Without the paste flag nothing is pressed.
    assert!(r.rec.take().is_empty());
}

#[test]
fn put_refuses_unknown_transfers_and_oversized_text() {
    let (mut r, _) = Rig::with_clipboard();
    let mid = Clip { op: CLIP_PUT, clip_id: 9, offset: 1024, total: 2000, data: vec![1; 10], ..Default::default() };
    assert_eq!(r.clip(mid).unwrap().status, CLIP_UNKNOWN);
    let huge = Clip { op: CLIP_PUT, clip_id: 10, total: MAX_CLIP as u32 + 1, data: vec![1; 10], ..Default::default() };
    assert_eq!(r.clip(huge).unwrap().status, CLIP_TOO_LARGE);
}

#[test]
fn get_with_copy_presses_ctrl_insert_and_waits_for_the_new_text() {
    let (mut r, fake) = Rig::with_clipboard();
    let old = crate::clipboard::Text { bytes: b"old".to_vec(), sensitive: false };
    let new = crate::clipboard::Text { bytes: "selected ✓".repeat(200).into_bytes(), sensitive: true };
    // Before the copy, then once unchanged, then the app has copied.
    fake.reads.lock().unwrap().extend([Ok(old.clone()), Ok(old), Ok(new.clone())]);
    let (text, flags) = r.get(3, CLIP_COPY).unwrap();
    assert_eq!(text, new.bytes);
    assert_eq!(flags & CLIP_SENSITIVE, CLIP_SENSITIVE);
    assert_eq!(r.rec.take(), vec![(CTRL, true), (INSERT, true), (INSERT, false), (CTRL, false)]);
}

#[test]
fn get_reads_without_pressing_anything_and_reports_empty() {
    let (mut r, fake) = Rig::with_clipboard();
    assert_eq!(r.get(1, 0), Err(CLIP_EMPTY));
    *fake.contents.lock().unwrap() = Some(crate::clipboard::Text { bytes: b"hi".to_vec(), sensitive: false });
    assert_eq!(r.get(2, 0), Ok((b"hi".to_vec(), 0)));
    assert!(r.rec.take().is_empty());
}

#[test]
fn clip_needs_a_session_in_use_and_a_clipboard() {
    let mut r = Rig::new();
    r.connect();
    // Before the first INPUT: ignored.
    assert!(r.clip(Clip { op: CLIP_GET, clip_id: 1, ..Default::default() }).is_none());
    let input = r.client.input(5).unwrap();
    r.send(input, 5);
    // No clipboard on this server: failed, not silence.
    assert_eq!(r.clip(Clip { op: CLIP_GET, clip_id: 1, ..Default::default() }).unwrap().status, CLIP_FAILED);
}

#[test]
fn copy_without_a_reachable_clipboard_still_presses_ctrl_insert_and_fails_fast() {
    let (mut r, fake) = Rig::with_clipboard();
    fake.reads.lock().unwrap().push_back(Err(crate::clipboard::ClipError::Failed));
    let t = Instant::now();
    assert_eq!(r.get(4, CLIP_COPY), Err(CLIP_FAILED));
    assert!(t.elapsed() < Duration::from_millis(500), "no waiting for a change that can't be seen");
    assert_eq!(r.rec.take(), vec![(CTRL, true), (INSERT, true), (INSERT, false), (CTRL, false)]);
}

#[test]
fn nothing_is_typed_while_another_users_session_is_in_front() {
    let mut r = Rig::new();
    r.connect();
    r.key(META, true, 0);
    assert_eq!(r.rec.take(), vec![(META, true)]);

    // Another user switches in: the watcher clears the flag first, and
    // from then on not a key or a motion reaches the old device.
    r.in_front.store(false, Ordering::SeqCst);
    r.key(A, true, 10);
    r.key(A, false, 20);
    let pkt = r.client.pointer(Pointer { dx: 5, ..Default::default() }, 30).unwrap();
    r.send(pkt, 30);
    assert!(r.rec.take().is_empty());
    assert!(r.rec.1.lock().unwrap().is_empty());
    assert_eq!(r.server.session_infos(r.t0).len(), 1, "the phone stays connected");

    // Then the main loop removes the device. Nothing is held on it any more.
    r.server.set_device(None);
    assert!(!r.server.has_device() && !r.server.key_down(META));
    r.key(SPACE, true, 40);
    assert!(r.rec.take().is_empty());
}

#[test]
fn back_in_front_the_new_device_gets_what_the_phone_still_holds() {
    let mut r = Rig::new();
    r.connect();
    r.key(META, true, 0);
    r.rec.take();
    r.in_front.store(false, Ordering::SeqCst);
    r.server.set_device(None);

    r.in_front.store(true, Ordering::SeqCst);
    let fresh = Recorder::default();
    r.server.set_device(Some(Box::new(fresh.clone())));
    r.key(SPACE, true, 50);
    let mut got = fresh.take();
    got.sort();
    assert_eq!(got, vec![(SPACE, true), (META, true)]);
    assert!(r.rec.take().is_empty(), "the old device is gone");
}
