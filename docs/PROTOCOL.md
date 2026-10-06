# Omakey wire protocol, version 1

The phone app (client) sends key state to `omakeyd` (server) on the desktop
over UDP. The server replays it on a Linux `uinput` virtual keyboard, so the
compositor sees an ordinary keyboard.

Design goals, in order: never leave a key stuck, never let anyone else on the
network type, and add as little latency as possible on top of one Wi-Fi hop.

- **UDP, one datagram per change.** A key press leaves the phone the moment
  the finger lands. There is no connection setup per key, no Nagle, no
  head-of-line blocking.
- **Every packet carries the full held-key set.** A lost packet heals with the
  next one. Recent un-acknowledged events ride along too, so a quick tap is
  not lost when its press packet is.
- **Authenticated encryption with a key from the QR code.** AES-256-GCM.
  A packet that does not decrypt is dropped silently.

All integers are **big-endian**. Lengths are in bytes.

## Constants

| Name              | Value                        |
|-------------------|------------------------------|
| UDP port          | `47800` (configurable)       |
| mDNS service      | `_omakey._udp.local.`        |
| Magic             | `0x4F 0x4B` (`"OK"`)         |
| Protocol version  | `1`                          |
| Max datagram      | 1200 bytes                   |

## Identity and pairing

- **Host id:** 8 random bytes, created on the server's first run, shown as
  16 lowercase hex chars. Stable across restarts and IP changes.
- **Device id:** 8 random bytes the *server* assigns to one phone.
- **Device key `K`:** 32 random bytes the server assigns to one phone.

Pairing is a QR code shown by `omakeyd pair` or the bar widget:

```
omakey://pair?v=1&h=<host id hex>&n=<host name>&a=<ip>[,<ip>...]&p=<port>&d=<device id hex>&k=<K, base64url, no padding>
```

`n` is percent-encoded UTF-8. `a` lists the server's IPv4 addresses, LAN
addresses first. The server writes the device to
`~/.config/omakey/devices.json` the first time a HELLO with that device id
decrypts, and closes the pairing window; each QR code pairs one phone. If no
phone uses it, it expires after 5 minutes.

The link *is* that phone's credential: whoever has it can connect as that
phone until it is forgotten on the desktop. The QR code is only ever shown
on the desktop's screen.

The phone stores `{host id, host name, addresses, port, device id, K}`.

### Discovery and reconnecting

The server advertises `_omakey._udp` over mDNS with TXT records
`v=1`, `id=<host id hex>`, `n=<host name>`. A paired phone sends HELLO to the
last address that worked and, in parallel, browses mDNS for a service whose
`id` matches; whichever answers first wins. This keeps working when DHCP hands
the desktop a new address.

## Packet layout

```
offset  size  field
0       2     magic "OK"
2       1     version (1)
3       1     type
4       8     device id
12      12    nonce
24      n+16  AES-256-GCM ciphertext, then the 16-byte tag
```

The 24-byte header is the GCM additional authenticated data (AAD), so a
packet's type and device id cannot be changed in flight.

| type | name    | direction | key   | nonce                         |
|------|---------|-----------|-------|-------------------------------|
| 1    | HELLO   | c → s     | `K`   | 12 random bytes               |
| 2    | WELCOME | s → c     | `K`   | 12 random bytes               |
| 3    | INPUT   | c → s     | `Kcs` | session id ‖ client counter   |
| 4    | ACK     | s → c     | `Ksc` | session id ‖ server counter   |
| 5    | BYE     | c → s     | `Kcs` | session id ‖ client counter   |
| 6    | REJECT  | s → c     | none  | zeros; body is plaintext      |

For types 3-5 the nonce is the 4-byte session id followed by an 8-byte
counter. Each side starts its counter at 1 and adds 1 per packet it sends.

### HELLO (1) — start a session

```
client_random   16
name_len         1
name             name_len   UTF-8 phone name, at most 64 bytes
platform         1          1 = Android, 2 = iOS
```

The client resends HELLO every 250 ms (with a fresh nonce and the same
`client_random`) until WELCOME arrives.

### WELCOME (2)

```
client_random   16   echoed; the client ignores a WELCOME that doesn't match
server_random   16
session_id       4
name_len         1
name             name_len   UTF-8 host name
```

Both sides then derive two 32-byte keys with HKDF-SHA256:

```
salt = client_random ‖ server_random
Kcs  = HKDF(ikm = K, salt, info = "omakey v1 c2s", L = 32)
Ksc  = HKDF(ikm = K, salt, info = "omakey v1 s2c", L = 32)
```

A replayed HELLO gets a new `server_random`, so the attacker can't derive
the session keys. The server keeps a device's previous session alive until
the new one sends its first valid INPUT, so a replayed HELLO can't kick the
real phone off either.

### INPUT (3) — key state

```
client_time_ms   4    client's monotonic clock, ms, truncated to 32 bits
flags            1    reserved, 0
held_count       1
held             held_count × 2   key codes held right now, after the events below,
                                  in ascending order
event_count      1
events           event_count × 5:
  eseq           2    event sequence number, starts at 1 per session, wraps
  code           2    Linux key code (input-event-codes.h)
  value          1    1 = press, 0 = release
```

Events are in ascending `eseq` order. The client includes every event the
server has not acknowledged yet, newest last, at most 32 (drop the oldest
when over; the held set still repairs the state).

Sending rules:

- Send an INPUT immediately on every key press and release.
- While any event is un-acknowledged, resend every 20 ms.
- Otherwise send a heartbeat INPUT (no events) every 100 ms. The heartbeat
  keeps the phone's Wi-Fi radio out of power save and measures ping.

The server, for each INPUT:

1. Drops it unless `counter` is greater than the highest counter seen in this
   session (replay and reorder protection; a newer packet already carries
   everything an older one would).
2. Applies each event whose `eseq` is newer than the last one it applied,
   comparing as a wrapping 16-bit difference: `(int16)(eseq - last) > 0`.
3. Reconciles with `held`: releases keys it pressed that are not held, then
   presses held keys it does not have down, modifiers first.
4. Replies with ACK.

### ACK (4)

```
client_time_ms   4    echoed from the INPUT being acknowledged
last_eseq        2    newest event applied
```

The client drops events up to `last_eseq` from its resend queue and shows
`now - client_time_ms` as the ping.

### BYE (5)

Empty body. The server releases every key the session holds and forgets it.

### REJECT (6)

Sent unencrypted when a HELLO names a device id the server doesn't know
(forgotten or never paired). Body: `reason 1` (1 = unknown device).
Because it is not authenticated, the client only shows a "pair again" hint;
it never deletes its pairing because of a REJECT.

## Test vectors

`docs/test-vectors.json` (from `omakeyd test-vectors`) has fixed keys,
randoms and nonces and the exact datagrams they produce: HELLO, WELCOME,
two INPUTs (SUPER down, then SPACE down with both events un-acked), ACK and
BYE. Every implementation must reproduce them byte for byte.

## Safety rules on the server

- A session that holds keys and sends nothing for **500 ms** has all its keys
  released. The next INPUT presses them again through the held set.
- A session silent for **30 s** is dropped.
- Key codes are limited to `1..=255` and `0x160..=0x2bf`; anything else is
  ignored. Mouse buttons are never sent.
- Several phones may be connected; a key is down on the virtual keyboard
  while any session holds it.
- On shutdown the server releases everything before destroying the device.

## What the phone does with Fn

Fn is a layer on the phone, as on a laptop keyboard where Fn never reaches
the OS. While a layer key is held, keys send their layer code. The code a
finger pressed is the code its release sends, even if Fn is let go first.
