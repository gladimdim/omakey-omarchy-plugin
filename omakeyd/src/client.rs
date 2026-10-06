//! A protocol client: used by `omakeyd test-client`, the tests, and to
//! generate cross-implementation test vectors. The phone apps implement the
//! same thing natively.

use crate::protocol::*;
use crate::store::{hex, unb64_key, unhex};
use aes_gcm::Aes256Gcm;
use std::collections::{BTreeSet, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairInfo {
    pub host_id: DeviceId,
    pub host_name: String,
    pub addrs: Vec<String>,
    pub port: u16,
    pub device_id: DeviceId,
    pub key: Key,
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                out.push(u8::from_str_radix(s.get(i + 1..i + 3)?, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

impl PairInfo {
    pub fn to_uri(&self) -> String {
        format!(
            "omakey://pair?v=1&h={}&n={}&a={}&p={}&d={}&k={}",
            hex(&self.host_id),
            percent_encode(&self.host_name),
            self.addrs.join(","),
            self.port,
            hex(&self.device_id),
            crate::store::b64(&self.key)
        )
    }

    pub fn parse(uri: &str) -> Option<PairInfo> {
        let query = uri.strip_prefix("omakey://pair?")?;
        let (mut h, mut n, mut a, mut p, mut d, mut k, mut v) = (None, None, None, None, None, None, None);
        for part in query.split('&') {
            let (key, val) = part.split_once('=')?;
            let val = percent_decode(val)?;
            match key {
                "v" => v = Some(val),
                "h" => h = unhex::<8>(&val),
                "n" => n = Some(val),
                "a" => a = Some(val.split(',').filter(|s| !s.is_empty()).map(String::from).collect()),
                "p" => p = val.parse().ok(),
                "d" => d = unhex::<8>(&val),
                "k" => k = unb64_key(&val),
                _ => {}
            }
        }
        if v.as_deref() != Some("1") {
            return None;
        }
        Some(PairInfo { host_id: h?, host_name: n.unwrap_or_default(), addrs: a?, port: p?, device_id: d?, key: k? })
    }
}

#[allow(dead_code)] // fields are part of the API the tests use
pub enum Reply {
    Welcome { host_name: String },
    Ack { client_time_ms: u32, last_eseq: u16 },
    Reject(u8),
}

struct Live {
    session_id: u32,
    c2s: Aes256Gcm,
    s2c: Aes256Gcm,
    max_server_counter: u64,
}

pub struct Client {
    device_id: DeviceId,
    key: Key,
    device_cipher: Aes256Gcm,
    client_random: [u8; 16],
    live: Option<Live>,
    counter: u64,
    eseq: u16,
    unacked: VecDeque<Event>,
    held: BTreeSet<u16>,
}

impl Client {
    pub fn new(device_id: DeviceId, key: Key) -> Client {
        Self::with_random(device_id, key, random_bytes())
    }

    pub fn with_random(device_id: DeviceId, key: Key, client_random: [u8; 16]) -> Client {
        Client {
            device_id,
            key,
            device_cipher: cipher(&key),
            client_random,
            live: None,
            counter: 0,
            eseq: 0,
            unacked: VecDeque::new(),
            held: BTreeSet::new(),
        }
    }

    pub fn connected(&self) -> bool {
        self.live.is_some()
    }

    pub fn hello_with_nonce(&self, name: &str, nonce: [u8; 12]) -> Vec<u8> {
        let h = Header { kind: T_HELLO, device_id: self.device_id, nonce };
        let body = Hello { client_random: self.client_random, name: name.into(), platform: 1 }.encode();
        seal(&self.device_cipher, &h, &body)
    }

    pub fn hello(&self, name: &str) -> Vec<u8> {
        self.hello_with_nonce(name, random_bytes())
    }

    pub fn on_packet(&mut self, pkt: &[u8]) -> Option<Reply> {
        let header = Header::decode(pkt)?;
        if header.device_id != self.device_id {
            return None;
        }
        match header.kind {
            T_REJECT => pkt.get(HEADER_LEN).map(|r| Reply::Reject(*r)),
            T_WELCOME => {
                let w = Welcome::decode(&open(&self.device_cipher, pkt)?)?;
                if w.client_random != self.client_random || self.live.is_some() {
                    return None;
                }
                let keys = derive_session_keys(&self.key, &w.client_random, &w.server_random);
                self.live = Some(Live {
                    session_id: w.session_id,
                    c2s: cipher(&keys.c2s),
                    s2c: cipher(&keys.s2c),
                    max_server_counter: 0,
                });
                Some(Reply::Welcome { host_name: w.name })
            }
            T_ACK => {
                let live = self.live.as_mut()?;
                let (sid, counter) = header.session_counter();
                if sid != live.session_id || counter <= live.max_server_counter {
                    return None;
                }
                let ack = Ack::decode(&open(&live.s2c, pkt)?)?;
                live.max_server_counter = counter;
                while self.unacked.front().is_some_and(|e| !eseq_newer(e.eseq, ack.last_eseq)) {
                    self.unacked.pop_front();
                }
                Some(Reply::Ack { client_time_ms: ack.client_time_ms, last_eseq: ack.last_eseq })
            }
            _ => None,
        }
    }

    /// Record a key change; returns the INPUT packet to send right away.
    pub fn key(&mut self, code: u16, down: bool, now_ms: u32) -> Option<Vec<u8>> {
        let changed = if down { self.held.insert(code) } else { self.held.remove(&code) };
        if !changed {
            return None;
        }
        self.eseq = self.eseq.wrapping_add(1);
        self.unacked.push_back(Event { eseq: self.eseq, code, down });
        while self.unacked.len() > MAX_EVENTS {
            self.unacked.pop_front();
        }
        self.input(now_ms)
    }

    pub fn has_unacked(&self) -> bool {
        !self.unacked.is_empty()
    }

    /// The current state as an INPUT packet (heartbeat / resend).
    pub fn input(&mut self, now_ms: u32) -> Option<Vec<u8>> {
        let live = self.live.as_ref()?;
        self.counter += 1;
        let body = Input {
            client_time_ms: now_ms,
            flags: 0,
            held: self.held.iter().copied().collect(),
            events: self.unacked.iter().copied().collect(),
        }
        .encode();
        let h = Header { kind: T_INPUT, device_id: self.device_id, nonce: session_nonce(live.session_id, self.counter) };
        Some(seal(&live.c2s, &h, &body))
    }

    pub fn bye(&mut self) -> Option<Vec<u8>> {
        let live = self.live.take()?;
        self.counter += 1;
        let h = Header { kind: T_BYE, device_id: self.device_id, nonce: session_nonce(live.session_id, self.counter) };
        Some(seal(&live.c2s, &h, &[]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_uri_round_trip() {
        let p = PairInfo {
            host_id: [1, 2, 3, 4, 5, 6, 7, 8],
            host_name: "Моя машина & co".into(),
            addrs: vec!["192.168.1.5".into(), "100.64.0.2".into()],
            port: 47800,
            device_id: [9; 8],
            key: [0xab; 32],
        };
        let uri = p.to_uri();
        assert!(!uri.contains(' '));
        assert_eq!(PairInfo::parse(&uri), Some(p));
        assert_eq!(PairInfo::parse("omakey://pair?v=2&h=00"), None);
    }
}
