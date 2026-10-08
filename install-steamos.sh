#!/bin/bash
# Install omakeyd on SteamOS (Steam Deck), without root. Run it in Desktop
# Mode, in Konsole:
#
#   curl -fsSL https://raw.githubusercontent.com/gladimdim/omakey-omarchy-plugin/main/install-steamos.sh | bash
#
#   ... | bash -s -- --uninstall    remove it again (keeps ~/.config/omakey)
#   OMAKEY_VERSION=v1.2.0 ...       a given release instead of the latest
#
# SteamOS keeps its system read-only and has no compiler, so this takes the
# static binary from the GitHub release and puts everything in your home
# folder, where SteamOS updates leave it alone:
#   ~/.local/bin/omakeyd                                   the daemon and CLI
#   ~/.config/systemd/user/omakeyd.service                 runs it, in Game Mode too
#   ~/.local/share/applications/omakey-pair.desktop        "Omakey: pair a phone"
#
# No udev rule: Steam's own lets you create the virtual keyboard, as Steam
# Input does for controllers. No firewall rules: SteamOS has no firewall.

set -euo pipefail

REPO="gladimdim/omakey-omarchy-plugin"
VERSION="${OMAKEY_VERSION:-latest}"
ASSET="omakeyd-x86_64-linux-musl"
BIN="$HOME/.local/bin/omakeyd"
UNIT="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/omakeyd.service"
APP="${XDG_DATA_HOME:-$HOME/.local/share}/applications/omakey-pair.desktop"

if [[ -n ${OMAKEY_BASE:-} ]]; then
  BASE="$OMAKEY_BASE" # a build of your own, for testing
elif [[ $VERSION == latest ]]; then
  BASE="https://github.com/$REPO/releases/latest/download"
else
  BASE="https://github.com/$REPO/releases/download/$VERSION"
fi

die() { echo "omakey: $*" >&2; exit 1; }

if [[ ${1:-} == --uninstall ]]; then
  systemctl --user disable --now omakeyd 2>/dev/null || true
  rm -f "$UNIT" "$BIN" "$APP"
  systemctl --user daemon-reload
  echo "omakeyd removed. Paired phones are kept in ~/.config/omakey; delete it to forget them."
  echo "If you added \"Omakey: pair a phone\" to Steam, remove it from the library."
  exit 0
fi

[[ $(uname -m) == x86_64 ]] || die "this installer has a build for x86_64 only (this is $(uname -m))"
command -v systemctl >/dev/null && systemctl --user show-environment >/dev/null 2>&1 \
  || die "needs a systemd user session; run it in Desktop Mode, in Konsole"
command -v curl >/dev/null || die "needs curl"
command -v sha256sum >/dev/null || die "needs sha256sum"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading omakeyd ($VERSION)..."
for f in "$ASSET" "$ASSET.sha256" omakeyd.service.in; do
  curl -fsSL --retry 3 -o "$tmp/$f" "$BASE/$f" \
    || die "couldn't download $f from $BASE (no release with a SteamOS build yet?)"
done
(cd "$tmp" && sha256sum --quiet -c "$ASSET.sha256") || die "the download doesn't match its checksum; try again"

first_install=true
[[ -e $APP ]] && first_install=false

install -D -m 0755 "$tmp/$ASSET" "$BIN"
mkdir -p "$(dirname "$UNIT")"
sed "s|@BIN@|$BIN|" "$tmp/omakeyd.service.in" > "$UNIT"

# Pairing without the Omarchy bar: the QR code in Konsole. Steam's
# "Add a Non-Steam Game" lists it too, so it works from Game Mode.
mkdir -p "$(dirname "$APP")"
cat > "$APP" <<EOF
[Desktop Entry]
Type=Application
Name=Omakey: pair a phone
Comment=Show the QR code the Omakey phone app scans
Exec=konsole --hold -e $BIN pair
Icon=input-keyboard
Terminal=false
Categories=Utility;
EOF

systemctl --user daemon-reload
systemctl --user enable omakeyd >/dev/null
systemctl --user restart omakeyd

if [[ ! -w /dev/uinput ]]; then
  echo
  echo "omakeyd is installed, but this Deck doesn't let you create a virtual keyboard"
  echo "(/dev/uinput). Steam's controller rule usually does; to add it yourself, set a"
  echo "password with 'passwd' if you haven't, then run once:"
  echo
  echo "  echo 'KERNEL==\"uinput\", SUBSYSTEM==\"misc\", TAG+=\"uaccess\", OPTIONS+=\"static_node=uinput\"' \\"
  echo "    | sudo tee /etc/udev/rules.d/70-omakey-uinput.rules"
  echo "  sudo udevadm control --reload && sudo udevadm trigger --sysname-match=uinput"
  echo "  systemctl --user restart omakeyd"
  exit 1
fi

sleep 0.5
if ! "$BIN" status >/dev/null 2>&1; then
  echo
  echo "omakeyd didn't start. Logs: journalctl --user -u omakeyd -n 20"
  exit 1
fi

# Offer it to Steam on the first install (SteamOS's own "Add to Steam").
if $first_install && command -v steamos-add-to-steam >/dev/null; then
  steamos-add-to-steam "$APP" >/dev/null 2>&1 || true
fi

echo
echo "Done. omakeyd runs in the background, in Game Mode too."
echo "Pair your phone: open \"Omakey: pair a phone\" (in the app menu, and in Steam"
echo "under Non-Steam), or run: $BIN pair"
