//! Wire format and crypto for protocol v1. See docs/PROTOCOL.md.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;

pub const MAGIC: [u8; 2] = *b"OK";
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 24;
pub const TAG_LEN: usize = 16;
pub const MAX_DATAGRAM: usize = 1200;
pub const MAX_EVENTS: usize = 32;
pub const MAX_NAME: usize = 64;

pub const T_HELLO: u8 = 1;
pub const T_WELCOME: u8 = 2;
pub const T_INPUT: u8 = 3;
pub const T_ACK: u8 = 4;
pub const T_BYE: u8 = 5;
pub const T_REJECT: u8 = 6;

pub const REJECT_UNKNOWN_DEVICE: u8 = 1;

pub type DeviceId = [u8; 8];
pub type Key = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub kind: u8,
    pub device_id: DeviceId,
    pub nonce: [u8; 12],
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut h = [0u8; HEADER_LEN];
        h[0..2].copy_from_slice(&MAGIC);
        h[2] = VERSION;
        h[3] = self.kind;
        h[4..12].copy_from_slice(&self.device_id);
        h[12..24].copy_from_slice(&self.nonce);
        h
    }

    pub fn decode(buf: &[u8]) -> Option<Header> {
        if buf.len() < HEADER_LEN || buf[0..2] != MAGIC || buf[2] != VERSION {
            return None;
        }
        Some(Header {
            kind: buf[3],
            device_id: buf[4..12].try_into().unwrap(),
            nonce: buf[12..24].try_into().unwrap(),
        })
    }

    /// Session id and counter for nonces of types 3-5.
    pub fn session_counter(&self) -> (u32, u64) {
        (
            u32::from_be_bytes(self.nonce[0..4].try_into().unwrap()),
            u64::from_be_bytes(self.nonce[4..12].try_into().unwrap()),
        )
    }
}

pub fn session_nonce(session_id: u32, counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[0..4].copy_from_slice(&session_id.to_be_bytes());
    n[4..12].copy_from_slice(&counter.to_be_bytes());
    n
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("system random source");
    b
}

pub fn cipher(key: &Key) -> Aes256Gcm {
    Aes256Gcm::new_from_slice(key).expect("32-byte key")
}

/// Seal `plain` into a full datagram.
pub fn seal(cipher: &Aes256Gcm, header: &Header, plain: &[u8]) -> Vec<u8> {
    let aad = header.encode();
    let ct = cipher
        .encrypt(Nonce::from_slice(&header.nonce), Payload { msg: plain, aad: &aad })
        .expect("encryption cannot fail");
    let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
    out.extend_from_slice(&aad);
    out.extend_from_slice(&ct);
    out
}

/// Open a datagram whose header was already decoded. None if it doesn't verify.
pub fn open(cipher: &Aes256Gcm, datagram: &[u8]) -> Option<Vec<u8>> {
    if datagram.len() < HEADER_LEN + TAG_LEN {
        return None;
    }
    let (aad, ct) = datagram.split_at(HEADER_LEN);
    cipher
        .decrypt(Nonce::from_slice(&aad[12..24]), Payload { msg: ct, aad })
        .ok()
}

pub struct SessionKeys {
    pub c2s: Key,
    pub s2c: Key,
}

pub fn derive_session_keys(device_key: &Key, client_random: &[u8; 16], server_random: &[u8; 16]) -> SessionKeys {
    let mut salt = [0u8; 32];
    salt[..16].copy_from_slice(client_random);
    salt[16..].copy_from_slice(server_random);
    let hk = Hkdf::<Sha256>::new(Some(&salt), device_key);
    let mut c2s = [0u8; 32];
    let mut s2c = [0u8; 32];
    hk.expand(b"omakey v1 c2s", &mut c2s).unwrap();
    hk.expand(b"omakey v1 s2c", &mut s2c).unwrap();
    SessionKeys { c2s, s2c }
}

// ---- bodies ----

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|s| u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|s| u32::from_be_bytes(s.try_into().unwrap()))
    }
    fn arr16(&mut self) -> Option<[u8; 16]> {
        self.take(16).map(|s| s.try_into().unwrap())
    }
    fn name(&mut self) -> Option<String> {
        let len = self.u8()? as usize;
        let raw = self.take(len)?;
        Some(String::from_utf8_lossy(raw).chars().filter(|c| !c.is_control()).collect())
    }
}

fn push_name(out: &mut Vec<u8>, name: &str) {
    let mut end = name.len().min(MAX_NAME);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    out.push(end as u8);
    out.extend_from_slice(&name.as_bytes()[..end]);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub client_random: [u8; 16],
    pub name: String,
    pub platform: u8,
}

impl Hello {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + 2 + self.name.len());
        out.extend_from_slice(&self.client_random);
        push_name(&mut out, &self.name);
        out.push(self.platform);
        out
    }
    pub fn decode(buf: &[u8]) -> Option<Hello> {
        let mut r = Reader::new(buf);
        Some(Hello { client_random: r.arr16()?, name: r.name()?, platform: r.u8()? })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Welcome {
    pub client_random: [u8; 16],
    pub server_random: [u8; 16],
    pub session_id: u32,
    pub name: String,
}

impl Welcome {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(37 + self.name.len());
        out.extend_from_slice(&self.client_random);
        out.extend_from_slice(&self.server_random);
        out.extend_from_slice(&self.session_id.to_be_bytes());
        push_name(&mut out, &self.name);
        out
    }
    pub fn decode(buf: &[u8]) -> Option<Welcome> {
        let mut r = Reader::new(buf);
        Some(Welcome {
            client_random: r.arr16()?,
            server_random: r.arr16()?,
            session_id: r.u32()?,
            name: r.name()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub eseq: u16,
    pub code: u16,
    pub down: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Input {
    pub client_time_ms: u32,
    pub flags: u8,
    pub held: Vec<u16>,
    pub events: Vec<Event>,
}

impl Input {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(7 + self.held.len() * 2 + self.events.len() * 5);
        out.extend_from_slice(&self.client_time_ms.to_be_bytes());
        out.push(self.flags);
        out.push(self.held.len() as u8);
        for c in &self.held {
            out.extend_from_slice(&c.to_be_bytes());
        }
        out.push(self.events.len() as u8);
        for e in &self.events {
            out.extend_from_slice(&e.eseq.to_be_bytes());
            out.extend_from_slice(&e.code.to_be_bytes());
            out.push(e.down as u8);
        }
        out
    }
    pub fn decode(buf: &[u8]) -> Option<Input> {
        let mut r = Reader::new(buf);
        let client_time_ms = r.u32()?;
        let flags = r.u8()?;
        let held_count = r.u8()? as usize;
        let mut held = Vec::with_capacity(held_count);
        for _ in 0..held_count {
            held.push(r.u16()?);
        }
        let event_count = r.u8()? as usize;
        if event_count > MAX_EVENTS {
            return None;
        }
        let mut events = Vec::with_capacity(event_count);
        for _ in 0..event_count {
            let eseq = r.u16()?;
            let code = r.u16()?;
            let down = r.u8()? != 0;
            events.push(Event { eseq, code, down });
        }
        Some(Input { client_time_ms, flags, held, events })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ack {
    pub client_time_ms: u32,
    pub last_eseq: u16,
}

impl Ack {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(6);
        out.extend_from_slice(&self.client_time_ms.to_be_bytes());
        out.extend_from_slice(&self.last_eseq.to_be_bytes());
        out
    }
    pub fn decode(buf: &[u8]) -> Option<Ack> {
        let mut r = Reader::new(buf);
        Some(Ack { client_time_ms: r.u32()?, last_eseq: r.u16()? })
    }
}

/// Plaintext REJECT datagram.
pub fn reject(device_id: DeviceId, reason: u8) -> Vec<u8> {
    let h = Header { kind: T_REJECT, device_id, nonce: [0; 12] };
    let mut out = h.encode().to_vec();
    out.push(reason);
    out
}

/// True when `eseq` comes after `last` in wrapping 16-bit order.
pub fn eseq_newer(eseq: u16, last: u16) -> bool {
    (eseq.wrapping_sub(last) as i16) > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hkdf_rfc5869_case1() {
        let ikm = [0x0bu8; 22];
        let salt: Vec<u8> = (0x00..=0x0c).collect();
        let info: Vec<u8> = (0xf0..=0xf9).collect();
        let hk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
        let mut okm = [0u8; 42];
        hk.expand(&info, &mut okm).unwrap();
        assert_eq!(
            hex(&okm),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
    }

    #[test]
    fn seal_open_round_trip_and_tamper() {
        let key = [7u8; 32];
        let c = cipher(&key);
        let h = Header { kind: T_INPUT, device_id: [1; 8], nonce: session_nonce(9, 1) };
        let mut pkt = seal(&c, &h, b"hello");
        assert_eq!(open(&c, &pkt).unwrap(), b"hello");
        pkt[3] = T_HELLO; // header is AAD
        assert!(open(&c, &pkt).is_none());
    }

    #[test]
    fn input_round_trip() {
        let i = Input {
            client_time_ms: 123456,
            flags: 0,
            held: vec![125, 57],
            events: vec![
                Event { eseq: 65535, code: 125, down: true },
                Event { eseq: 0, code: 57, down: true },
            ],
        };
        assert_eq!(Input::decode(&i.encode()).unwrap(), i);
        assert!(Input::decode(&i.encode()[..9]).is_none());
    }

    #[test]
    fn eseq_wraps() {
        assert!(eseq_newer(1, 0));
        assert!(eseq_newer(0, 65535));
        assert!(!eseq_newer(5, 5));
        assert!(!eseq_newer(4, 5));
        assert!(!eseq_newer(65535, 0));
    }

    #[test]
    fn names_are_truncated_on_char_boundary() {
        let h = Hello { client_random: [0; 16], name: "ї".repeat(40), platform: 1 };
        let d = Hello::decode(&h.encode()).unwrap();
        assert!(d.name.len() <= MAX_NAME);
        assert!(d.name.chars().all(|c| c == 'ї'));
    }

    pub fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}
