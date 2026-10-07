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
omakey://pair?v=1&h=<host id hex>&n=<host name>&a=<ip>[,<ip>...]&p=<port>&d=<device id hex>&k=<K, base64url, no padding>[&b=<bt address hex>]
```

`n` is percent-encoded UTF-8. `a` lists the server's IPv4 addresses, LAN
addresses first. `b`, when the server has Bluetooth, is its adapter
address as 12 hex digits (`1418c368871e` for `14:18:C3:68:87:1E`); see
[Bluetooth](#bluetooth). The server writes the device to
`~/.config/omakey/devices.json` the first time a HELLO with that device id
decrypts, and closes the pairing window; each QR code pairs one phone. If no
phone uses it, it expires after 5 minutes.

The link *is* that phone's credential: whoever has it can connect as that
phone until it is forgotten on the desktop. The QR code is only ever shown
on the desktop's screen.

Both screens show a **fingerprint** of the link so the user can check the
phone scanned this desktop's code: the first 4 bytes of SHA-256(`K`) as 8
uppercase hex digits in two groups, e.g. `7D75-B04F`.

While a pairing window has more than 60 s left, asking for a code again
(`omakeyd pair`, the bar widget) returns the same code rather than a new one.

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

The client resends HELLO (with a fresh nonce and the same `client_random`)
until WELCOME arrives: after 50 ms at first, doubling up to every 250 ms. It
sends one at once to any address it newly learns, e.g. over mDNS.

### WELCOME (2)

```
client_random   16   echoed; the client ignores a WELCOME that doesn't match
server_random   16
session_id       4
name_len         1
name             name_len   UTF-8 host name
features         1          optional; bit 0 = touchpad (pointer) support
bt_address       6          optional; the server's Bluetooth adapter
```

Servers before the touchpad send no `features` byte; read it as 0. A server
without Bluetooth sends no `bt_address`; phones paired before it existed
learn it here. Clients ignore any bytes after the fields they know.

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

Optional pointer trailer, after the events (clients without a touchpad omit it):

```
pointer_len      1    8; a server skips whatever follows its known fields
dx               2    signed relative motion, mouse counts
dy               2    signed
wheel            2    signed vertical scroll in 1/120 of a notch; positive scrolls up
hwheel           2    signed horizontal scroll in 1/120 of a notch; positive scrolls right
```

Optional layout trailer, after the pointer (a client sending it sends the
pointer too, as zeros when there's no motion):

```
layout_len       1    at most 16
layout           layout_len   ASCII xkb layout name: "us", "ua"
```

The layout the keys in this INPUT are meant in. A client typing with the
phone's own keyboard picks, for each character, a layout that has it, and
before a different one waits until every event so far is acknowledged, so
no key is read with the wrong layout. The server switches its own keyboard
device (only that one: the desktop's keyboards keep their layouts) to that
layout before applying the events, if it is one of the device's configured
layouts; otherwise it leaves it and logs once. While keys arrive it looks
again about every second, as a layout toggle on the desktop can move the
device too. An INPUT with events and no `layout` puts the device back on
the layout it had before a client first asked. On Omarchy this is
Hyprland's `switchxkblayout omakey-keyboard <index>`.

Motion and scroll since the previous INPUT, sent once and never repeated:
a lost packet loses that bit of motion, as a mouse would. Touchpad buttons
are not here: `BTN_LEFT` (0x110), `BTN_RIGHT` (0x111) and `BTN_MIDDLE`
(0x112) travel as ordinary held codes and events, so a click is never lost
and a held button is released like a key when the phone goes quiet. The
server sends them, the motion and the scroll to a separate virtual mouse
(`Omakey Mouse`) so the keyboard device never looks like a pointer.

Events are in ascending `eseq` order. The client includes every event the
server has not acknowledged yet, newest last, at most 32 (drop the oldest
when over; the held set still repairs the state).

Sending rules:

- Send an INPUT immediately on every key press and release.
- While any event is un-acknowledged, resend when its ACK is overdue:
  after 1.5 × the smoothed ping + 2 ms, kept within 5–20 ms.
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
leds             1    optional; lock LEDs of the virtual keyboard:
                      bit 0 = Num Lock, bit 1 = Caps Lock, bit 2 = Scroll Lock
theme_len        1    optional, only after leds; bytes of theme that follow
theme            theme_len
```

`theme` is the desktop's Omarchy theme, for phones that colour themselves
to match:

```
mode             1    0 = dark, 1 = light
name_len         1
name             name_len   UTF-8, the theme's id ("tokyo-night"), at most 32 bytes
count            1    colours that follow
colors           3 × count  RGB, in this order: background, lighter_background,
                      dark_background, foreground, muted, accent, selection,
                      red, yellow, green, cyan, blue, magenta, orange
```

The colours are the keys of Omarchy's `colors.toml`; one a theme lacks is
filled from a close one (lighter_background from background, muted from
foreground). The server reads `~/.local/state/omarchy/current/` and puts
`theme` in the first 4 ACKs of a session, and in the next 4 after the
desktop's theme changes; it looks every 2 s. Without Omarchy, or without
`leds`, there is no `theme`. A client takes the first `count` colours it
knows and ignores the rest.

The client drops events up to `last_eseq` from its resend queue and shows
`now - client_time_ms` as the ping.

`leds` is the state the compositor last set on the `Omakey Keyboard` device,
so the phone can show Caps Lock and friends. Servers before it, and servers
that can't read the LEDs (`--dry-run`), send no `leds` byte: the client then
shows no lock state, it doesn't assume "off". Clients ignore any bytes after
the fields they know.

### BYE (5)

Empty body. The server releases every key the session holds and forgets it.

### REJECT (6)

Sent unencrypted when a HELLO names a device id the server doesn't know
(forgotten or never paired). Body: `reason 1` (1 = unknown device).
Because it is not authenticated, the client only shows a "pair again" hint;
it never deletes its pairing because of a REJECT.
It also only believes a REJECT from an address it sent HELLO to.

## Bluetooth

When Wi-Fi can't reach the server, the same packets travel over Bluetooth
Classic RFCOMM. The server registers an RFCOMM service with UUID
`4f4b6579-6d61-4b79-9000-6f6d616b6579` (BlueZ assigns the channel; phones
look it up over SDP) at the address from `b` or `bt_address`.

- RFCOMM is a reliable stream, so each datagram is sent as a 2-byte
  big-endian length followed by the datagram, in one write. A length of 0
  or over 1200 closes the connection.
- Everything inside is unchanged: HELLO, WELCOME, INPUT, ACK and BYE with
  the same keys, nonces and counters. Bluetooth pairing and link encryption
  are not needed and not relied on; the phone connects with an unauthenticated
  ("insecure") socket.
- The stream doesn't lose packets, so the client doesn't resend events. It
  still sends the 100 ms heartbeat, for ping and the 500 ms stuck-key timeout.
- A phone uses one transport at a time. UDP runs all along; Bluetooth is
  added when no WELCOME has arrived over UDP after about 1.2 s, or when the
  session in use goes quiet. The first transport to be welcomed is used.
  UDP is preferred: a WELCOME over UDP while Bluetooth is in use moves the
  phone back to UDP and Bluetooth stops. The new session's first INPUT then
  replaces the old one on the server, keys held across the switch staying
  down.

On the server:

- It finds the adapter through BlueZ's ObjectManager (the first
  `org.bluez.Adapter1`), registers the profile, and checks every few
  seconds, registering again when `bluetoothd` restarts. It advertises
  `b` and `bt_address` only while the profile is registered and the
  adapter is powered.
- At most 8 RFCOMM connections are served at once. A connection silent for
  30 s is closed.
- When a connection closes, the keys of the sessions last heard on it are
  released at once. The session itself stays, so the phone can carry on
  over UDP with the same session.
- When a session's first INPUT arrives (e.g. after switching from Wi-Fi to
  Bluetooth), its held keys are pressed before the device's older session
  is dropped, so a key held across the switch never goes up.

### Bluetooth keyboard mode

Separately, the phone can be a standard Bluetooth HID keyboard and mouse
for any computer, without omakeyd. That uses the HID Device profile and
the computer's own Bluetooth pairing, and none of this protocol. Its
report descriptor is in the Android app (`protocol/.../Hid.kt`).

## Test vectors

`docs/test-vectors.json` (from `omakeyd test-vectors`) has fixed keys,
randoms and nonces and the exact datagrams they produce: HELLO, WELCOME
(with the touchpad feature), two INPUTs (SUPER down, then SPACE down with
both events un-acked), an INPUT with the left button held and a pointer
trailer, ACK (without the `leds` byte) and BYE. Every implementation must
reproduce them byte for byte. The fingerprint of their device key is
`630D-CD29`.

The server replies to a UDP packet from the address it was sent to, so a
desktop with several addresses answers from the one the phone knows.

## Safety rules on the server

- A session that holds keys and sends nothing for **500 ms** has all its keys
  released. The next INPUT presses them again through the held set.
- A session silent for **30 s** is dropped.
- A device keeps at most 2 sessions that haven't had an INPUT yet; a new
  HELLO pushes out the oldest of those, never the session in use.
- HELLOs are rate-limited per source IP (10 per second, bursts of 20).
  Others are dropped unread, so a flood can't burn CPU on decryption or
  turn REJECTs into a reflector.
- Key codes are limited to `1..=255`, `0x160..=0x2bf` and the touchpad
  buttons `0x110..=0x112`; anything else is ignored.
- Several phones may be connected; a key is down on the virtual keyboard
  while any session holds it.
- On shutdown the server releases everything before destroying the device.

## What the phone does with Fn

Fn is a layer on the phone, as on a laptop keyboard where Fn never reaches
the OS. While a layer key is held, keys send their layer code. The code a
finger pressed is the code its release sends, even if Fn is let go first.
