//! Deterministic test vectors so other implementations (Kotlin, Swift) can
//! check they produce the same bytes. `omakeyd test-vectors` prints them.

use crate::client::{Client, PairInfo};
use crate::protocol::*;
use crate::store::hex;
use serde_json::json;

pub fn generate() -> serde_json::Value {
    let device_id: DeviceId = *b"\x01\x02\x03\x04\x05\x06\x07\x08";
    let host_id: DeviceId = *b"\xa0\xa1\xa2\xa3\xa4\xa5\xa6\xa7";
    let key: Key = core::array::from_fn(|i| i as u8);
    let client_random: [u8; 16] = core::array::from_fn(|i| 0x10 + i as u8);
    let server_random: [u8; 16] = core::array::from_fn(|i| 0x20 + i as u8);
    let session_id: u32 = 0x0badcafe;
    let hello_nonce: [u8; 12] = core::array::from_fn(|i| 0x30 + i as u8);
    let welcome_nonce: [u8; 12] = core::array::from_fn(|i| 0x40 + i as u8);

    let pair = PairInfo {
        host_id,
        host_name: "omarchy desk".into(),
        addrs: vec!["192.168.50.219".into(), "100.67.193.30".into()],
        port: 47800,
        device_id,
        key,
        bt: None,
    };

    let mut client = Client::with_random(device_id, key, client_random);
    let hello = client.hello_with_nonce("Pixel 9", hello_nonce);

    let welcome_body =
        Welcome { client_random, server_random, session_id, name: "omarchy desk".into(), features: FEATURE_POINTER, bt_address: None }
            .encode();
    let welcome = seal(
        &cipher(&key),
        &Header { kind: T_WELCOME, device_id, nonce: welcome_nonce },
        &welcome_body,
    );
    assert!(client.on_packet(&welcome).is_some());
    let keys = derive_session_keys(&key, &client_random, &server_random);

    // SUPER down (counter 1), then SPACE down (counter 2) with both events
    // still un-acknowledged.
    let input1 = client.key(125, true, 1000).unwrap();
    let input2 = client.key(57, true, 1016).unwrap();
    let ack = seal(
        &cipher(&keys.s2c),
        &Header { kind: T_ACK, device_id, nonce: session_nonce(session_id, 1) },
        &Ack { client_time_ms: 1016, last_eseq: 2, leds: None }.encode(),
    );
    // Left button down (event 3) with motion and a scroll in the same packet (counter 3).
    let input3 = {
        client.key(0x110, true, 1032);
        client.pointer(Pointer { dx: -12, dy: 34, wheel: 120, hwheel: -60 }, 1033).unwrap()
    };
    let bye = client.bye().unwrap();

    // The clipboard, in a session of its own with the same keys: a put of
    // "Hi ✓" that pastes (client counter 1), and the reply that it's done
    // (server counter 1).
    let mut clip_client = Client::with_random(device_id, key, client_random);
    assert!(clip_client.on_packet(&welcome).is_some());
    let text = "Hi ✓".as_bytes();
    let clip_put = Clip { op: CLIP_PUT, clip_id: 0x01020304, offset: 0, flags: CLIP_PASTE, total: text.len() as u32, data: text.to_vec(), ..Default::default() };
    let clip_put_packet = clip_client.clip(&clip_put).unwrap();
    let clip_reply = Clip { status: CLIP_OK, offset: text.len() as u32, flags: 0, data: vec![], ..clip_put.clone() };
    let clip_reply_packet = seal(
        &cipher(&keys.s2c),
        &Header { kind: T_CLIP_REPLY, device_id, nonce: session_nonce(session_id, 1) },
        &clip_reply.encode(true),
    );

    json!({
        "description": "Protocol v1 vectors. Hex unless noted. All packets are full datagrams.",
        "device_id": hex(&device_id),
        "host_id": hex(&host_id),
        "device_key": hex(&key),
        "client_random": hex(&client_random),
        "server_random": hex(&server_random),
        "session_id": session_id,
        "pair_uri": pair.to_uri(),
        "kcs": hex(&keys.c2s),
        "ksc": hex(&keys.s2c),
        "hello": { "name": "Pixel 9", "platform": 1, "nonce": hex(&hello_nonce), "packet": hex(&hello) },
        "welcome": { "host_name": "omarchy desk", "features": FEATURE_POINTER, "nonce": hex(&welcome_nonce), "packet": hex(&welcome) },
        "input1": { "counter": 1, "client_time_ms": 1000, "held": [125], "events": [[1, 125, 1]], "packet": hex(&input1) },
        "input2": { "counter": 2, "client_time_ms": 1016, "held": [57, 125], "events": [[1, 125, 1], [2, 57, 1]], "packet": hex(&input2) },
        "ack": { "counter": 1, "client_time_ms": 1016, "last_eseq": 2, "packet": hex(&ack) },
        "input4": { "counter": 4, "client_time_ms": 1033, "held": [57, 125, 272], "events": [[1, 125, 1], [2, 57, 1], [3, 272, 1]],
                    "pointer": { "dx": -12, "dy": 34, "wheel": 120, "hwheel": -60 }, "packet": hex(&input3),
                    "note": "counter 3 was the INPUT sent by the button press itself; this is the next one" },
        "bye": { "counter": 5, "packet": hex(&bye) },
        "clip_put": { "counter": 1, "op": CLIP_PUT, "clip_id": 0x01020304u32, "offset": 0, "flags": CLIP_PASTE, "total": text.len(),
                      "text": "Hi ✓", "body": hex(&clip_put.encode(false)), "packet": hex(&clip_put_packet),
                      "note": "a session of its own, after the same WELCOME: counters start again" },
        "clip_reply": { "counter": 1, "op": CLIP_PUT, "clip_id": 0x01020304u32, "status": CLIP_OK, "offset": text.len(), "flags": 0,
                        "total": text.len(), "body": hex(&clip_reply.encode(true)), "packet": hex(&clip_reply_packet) },
    })
}
