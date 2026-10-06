#!/bin/bash
# Build and install omakeyd, the service behind the Omakey Omarchy plugin.
#
#   ./install.sh              build and install (asks for sudo once)
#   ./install.sh --uninstall  remove it again (keeps ~/.config/omakey)
#
# Installs:
#   /usr/local/bin/omakeyd                      the daemon and CLI
#   /etc/udev/rules.d/70-omakey-uinput.rules    lets you (not root) create
#                                               the virtual keyboard
#   /etc/modules-load.d/omakey.conf             loads uinput at boot
#   ~/.config/systemd/user/omakeyd.service      runs it in your session
#   ufw rules for UDP 47800 and mDNS            only if ufw is active

set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
BIN="$PREFIX/bin/omakeyd"
RULE="/etc/udev/rules.d/70-omakey-uinput.rules"
MODULES="/etc/modules-load.d/omakey.conf"
UNIT="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/omakeyd.service"
PORT=47800
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Build outside the plugin folder: the shell watches that folder and would
# reload plugins on every file cargo writes.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/omakey/target}"

ufw_active() { command -v ufw >/dev/null && sudo ufw status 2>/dev/null | grep -q "^Status: active"; }

if [[ ${1:-} == --uninstall ]]; then
  systemctl --user disable --now omakeyd 2>/dev/null || true
  rm -f "$UNIT"
  systemctl --user daemon-reload
  sudo rm -f "$BIN" "$RULE" "$MODULES"
  sudo udevadm control --reload
  if ufw_active; then
    sudo ufw delete allow "$PORT/udp" >/dev/null 2>&1 || true
  fi
  echo "omakeyd removed. Paired phones are kept in ~/.config/omakey; delete it to forget them."
  exit 0
fi

command -v cargo >/dev/null || { echo "cargo is required: sudo pacman -S rust" >&2; exit 1; }

echo "Building omakeyd..."
cargo build --release --locked --manifest-path "$ROOT/omakeyd/Cargo.toml"

echo "Installing (needs sudo)..."
sudo install -D -m 0755 -o root -g root "$CARGO_TARGET_DIR/release/omakeyd" "$BIN"
sudo install -D -m 0644 -o root -g root "$ROOT/packaging/70-omakey-uinput.rules" "$RULE"
echo uinput | sudo install -D -m 0644 -o root -g root /dev/stdin "$MODULES"
sudo modprobe uinput 2>/dev/null || true
sudo udevadm control --reload
sudo udevadm trigger --action=change --sysname-match=uinput

if ufw_active; then
  echo "Opening UDP $PORT and mDNS in ufw..."
  sudo ufw allow "$PORT/udp" comment "Omakey phone keyboard" >/dev/null
  sudo ufw allow 5353/udp comment "mDNS" >/dev/null
fi

mkdir -p "$(dirname "$UNIT")"
sed "s|@BIN@|$BIN|" "$ROOT/packaging/omakeyd.service.in" > "$UNIT"
systemctl --user daemon-reload
systemctl --user enable omakeyd >/dev/null
systemctl --user restart omakeyd

sleep 0.5
if ! "$BIN" status >/dev/null 2>&1; then
  echo
  echo "omakeyd didn't start. Most likely /dev/uinput isn't yours yet:"
  echo "log out and back in, then run: systemctl --user restart omakeyd"
  echo "Logs: journalctl --user -u omakeyd -n 20"
  exit 1
fi

echo
echo "Done. Pair your phone from the ⌨ icon in the bar, or run: omakeyd pair"
