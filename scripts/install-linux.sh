#!/usr/bin/env bash
# Build Placeholder in release mode and install it for the current user:
#   ~/.local/bin/placeholder, a launcher entry and its icon.
# Pass --no-build to install the last release build as it is.
set -euo pipefail

APP_ID="io.github.fr_anz.placeholder"
NAME="Placeholder"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="$ROOT/crates/monocode-gpui/Cargo.toml"
BUILT="$ROOT/crates/monocode-gpui/target/release/monocode-gpui"
BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"

if [[ "${1:-}" != "--no-build" ]]; then
  cargo build --locked --release --manifest-path "$MANIFEST" -p monocode-gpui
fi
[[ -x "$BUILT" ]] || { echo "No release build at $BUILT" >&2; exit 1; }

install -Dm755 "$BUILT" "$BIN_DIR/placeholder"

for size in 32 64 128 256 512; do
  dir="$DATA/icons/hicolor/${size}x${size}/apps"
  mkdir -p "$dir"
  if command -v magick >/dev/null; then
    magick "$ROOT/crates/monocode-gpui/assets/app-icon.png" -resize "${size}x${size}" "$dir/$APP_ID.png"
  else
    install -Dm644 "$ROOT/crates/monocode-gpui/assets/app-icon.png" "$dir/$APP_ID.png"
  fi
done

install -Dm644 /dev/stdin "$DATA/applications/$APP_ID.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=$NAME
Comment=Native desktop app for coding agents
Exec=$BIN_DIR/placeholder %U
Icon=$APP_ID
Terminal=false
Categories=Development;IDE;
StartupWMClass=$APP_ID
EOF

command -v update-desktop-database >/dev/null && update-desktop-database -q "$DATA/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$DATA/icons/hicolor" || true

echo "Installed $NAME: $BIN_DIR/placeholder"
