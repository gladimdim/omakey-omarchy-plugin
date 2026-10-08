# Omakey for Omarchy

**Your phone becomes a real keyboard for your Omarchy desktop.**

**Website and live demo: <https://gladimdim.github.io/omakey-omarchy-plugin/>** ·
Android app: [download the APK](https://github.com/gladimdim/omakey-mobile/releases/latest)

Pair the Omakey phone app with a QR code and the desktop gets a new
keyboard: Esc, F1–F12, Super, Ctrl, Alt, arrows, media keys, all of it.
Hold `SUPER + SPACE`, `CTRL + SHIFT + T` or any chord with as many fingers
as you like, just as on a Bluetooth keyboard, but over Wi-Fi.

- **A real keyboard, not a text box.** `omakeyd` creates a kernel virtual
  keyboard (`uinput`). Hyprland binds, games, terminals and the lock screen
  all see key presses, and your keyboard layout (US, Ukrainian, …) applies.
- **Fast.** One UDP packet per key, sent the moment your finger lands.
  Typically a few milliseconds on home Wi-Fi.
- **No stuck keys.** Every packet carries every held key, so lost packets
  heal themselves, and if the phone goes quiet for half a second its keys are
  released.
- **Private.** The QR code holds a 256-bit key; every packet is encrypted
  and authenticated (AES-256-GCM), with replay protection. Nobody else on
  the Wi-Fi can type on your machine.
- **Touchpad too.** Pull the touchpad down in the app to move the mouse,
  click, right-click and scroll. It is a second virtual device, "Omakey
  Mouse", next to "Omakey Keyboard".
- **Bar widget.** See which phones are connected, pair new ones, and forget
  old ones.
- **Bluetooth fallback.** Off Wi-Fi, the phone reaches `omakeyd` over
  Bluetooth instead, with the same encrypted packets. It needs `bluetoothd`
  running; nothing extra to pair. `omakeyd` picks it up when Bluetooth is
  turned on or `bluetoothd` restarts, and a held key or drag survives the
  switch between Wi-Fi and Bluetooth.
- **Lock lights.** Caps Lock, Num Lock and Scroll Lock state goes back to
  the phone, so the app can show it.
- **Shared clipboard.** Copy on the phone copies what's selected on the
  desktop and puts it on the phone's clipboard too; Paste pastes on the
  desktop, bringing the phone's clipboard along when it has something newer.
  Needs `wl-copy` and `wl-paste` (Omarchy has them).

The app can also be a plain Bluetooth keyboard for computers without
`omakeyd`: pair the phone in that computer's Bluetooth settings.

## Install

Needs Omarchy with the Omarchy shell (`omarchy plugin` available) and Rust
(`sudo pacman -S rust`).

```bash
omarchy plugin add https://github.com/gladimdim/omakey-omarchy-plugin --enable
~/.config/omarchy/plugins/gladimdim.omakey/install.sh
```

`install.sh` builds `omakeyd` and asks for sudo once to install:

| What | Why |
|------|-----|
| `/usr/local/bin/omakeyd` | the service and its CLI |
| `/etc/udev/rules.d/70-omakey-uinput.rules` | lets *you* (not root) create the virtual keyboard |
| `/etc/modules-load.d/omakey.conf` | loads `uinput` at boot |
| `~/.config/systemd/user/omakeyd.service` | runs it in your session |
| ufw: UDP 47800 and 5353 from private networks | only if ufw is active (10/8, 172.16/12, 192.168/16, and 100.64/10 for Tailscale) |

If the service doesn't start right after installing, log out and back in
once so the udev rule applies to your session.

## The bar widget

Click the ⌨ icon in the bar. The panel does everything:

- **Set up** — first time, *Set up Omakey* runs `install.sh` in a terminal
  (it asks for your password there). *Update the service* appears when the
  installed `omakeyd` is older than the widget.
- **Pair** — *Pair a phone* shows a QR code for the Omakey app to scan, with
  a countdown, a fingerprint the phone shows too, *Copy link* (for pasting
  into the app instead) and *Cancel*. Right-clicking the icon starts pairing
  straight away; opening it again shows the same code while it has more than
  a minute left. The copied link is marked sensitive, so clipboard managers
  skip it, and cleared after a minute.
- **Phones** — who is connected, over Wi-Fi (with the address) or
  Bluetooth, how many packets are being lost, how many keys they hold, and
  when the others were last seen. Rename (✎) or forget (⛓) each one.
- **Service** — start it, stop it, restart it, or open its logs. The footer
  says whether Bluetooth is on, and why not when it's off.
- **Settings** — the name your phone shows and the UDP port.

The QR code is good for one phone and 5 minutes. After pairing, the phone
finds the desktop by itself (mDNS), even when its IP address changes.

From a terminal: `omakeyd pair`.

## CLI

```
omakeyd pair [--no-wait]        show a QR code (and its fingerprint) to pair a phone
omakeyd cancel-pair             close the pairing window
omakeyd status [--json]         connection status
omakeyd devices                 list paired phones
omakeyd forget <device-id>      unpair a phone
omakeyd rename <device-id> <n>  rename a phone
omakeyd config [--name N] [--port P]  show or change settings
omakeyd run [--dry-run]         the service itself (systemd runs this)
```

Logs: `journalctl --user -u omakeyd -f`.

## Settings

`~/.config/omakey/config.json` (optional):

```json
{ "port": 47800, "name": "My desk" }
```

Or set them from the widget's Settings, or with `omakeyd config`.
Restart with `systemctl --user restart omakeyd` after editing the file.

## How it works

```
phone ──UDP 47800, AES-GCM──▶ omakeyd ──▶ /dev/uinput ──▶ libinput ──▶ Hyprland
      ◀────────── ACKs ──────
```

- [`docs/PROTOCOL.md`](docs/PROTOCOL.md): the wire protocol.
- [`docs/test-vectors.json`](docs/test-vectors.json): exact bytes every client
  implementation must reproduce.
- `omakeyd/`: the Rust daemon. `cargo test` runs the protocol tests: chords,
  lost and replayed packets, stuck-key timeout, two phones.
- `BarWidget.qml`: the bar widget. It reads `$XDG_RUNTIME_DIR/omakey/state.json`
  and calls the CLI, so it works with any bar and the keyboard keeps working
  across `omarchy restart shell`.
- The systemd unit is `Type=notify` with a watchdog, and sandboxed: the file
  system is read-only except `~/.config/omakey` and
  `$XDG_RUNTIME_DIR/omakey`, with a private `/tmp` and only IP, Unix and
  netlink sockets.

### Security notes

- Paired phones and their keys live in `~/.config/omakey/devices.json`
  (mode 600). The pairing link is that phone's key, so don't share it.
- The udev rule gives the logged-in user access to `/dev/uinput`, the same
  rule Steam uses for controllers. Any program running as you can then
  create input devices.
- `omakeyd` needs `XDG_RUNTIME_DIR` (any login session has it) and won't
  fall back to `/tmp`, where another user could plant its socket.
- The firewall rules only let in private and Tailscale addresses, and
  `omakeyd` answers at most 10 handshakes per second per address.
- Packets from unknown or tampered sources are dropped without a reply,
  except a plaintext "unknown device" notice so the app can suggest
  re-pairing.

## Development

```bash
cd omakeyd && cargo test
cargo run -- run --dry-run --port 47899   # prints keys instead of typing
scripts/dev-sync                          # copy this checkout over the installed plugin
```

The website lives in `docs/` (GitHub Pages, no build step). After the stock
layouts change in the studio repo, run `scripts/site-layouts` to refresh
`docs/assets/layouts.js`.

`omakeyd test-client '<omakey://pair link>' --addr 127.0.0.1 --text "hi"`
acts as a phone: it pairs, types the text, and prints the ping.

## Uninstall

```bash
~/.config/omarchy/plugins/gladimdim.omakey/install.sh --uninstall
omarchy plugin remove gladimdim.omakey
```

## License

MIT
