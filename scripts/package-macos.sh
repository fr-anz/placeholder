#!/usr/bin/env bash
# Package a macOS release build as Placeholder.app inside a .dmg:
#   scripts/package-macos.sh VERSION BINARY [OUT_DIR]
# BINARY may be a universal binary (CI joins arm64 and x86_64 with lipo).
# The app is signed ad hoc, not with a Developer ID, and is not notarized:
# on first launch macOS asks the user to allow it in Privacy & Security.
set -euo pipefail

VERSION="${1:?usage: package-macos.sh VERSION BINARY [OUT_DIR]}"
BINARY="${2:?usage: package-macos.sh VERSION BINARY [OUT_DIR]}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="${3:-$ROOT/dist}"
APP_ID="io.github.fr_anz.placeholder"

[[ -x "$BINARY" ]] || { echo "No executable at $BINARY" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
app="$work/Placeholder.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
# The app runs this binary directly; see bundle_uses_monocode_data in main.rs.
install -m755 "$BINARY" "$app/Contents/MacOS/Placeholder"

# AppIcon.icns from the 1024px PNG.
iconset="$work/AppIcon.iconset"
mkdir -p "$iconset"
src="$ROOT/crates/monocode-gpui/assets/app-icon.png"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$src" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$src" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>Placeholder</string>
  <key>CFBundleExecutable</key>
  <string>Placeholder</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundleIdentifier</key>
  <string>$APP_ID</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Placeholder</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleVersion</key>
  <string>$VERSION</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key>
  <string>${MACOSX_DEPLOYMENT_TARGET:-11.0}</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key>
  <true/>
</dict>
</plist>
EOF
plutil -lint "$app/Contents/Info.plist" >/dev/null

codesign --force --deep --sign - "$app"

stage="$work/dmg"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
mkdir -p "$OUT_DIR"
out="$OUT_DIR/Placeholder-$VERSION-macos-universal.dmg"
rm -f "$out"
hdiutil create -volname "Placeholder $VERSION" -srcfolder "$stage" -ov -format UDZO "$out" >&2
echo "$out"
