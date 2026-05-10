#!/usr/bin/env bash
# scripts/build-mac-app.sh
#
# Wraps the release medasr-gui binary in a proper macOS .app bundle so it
# shows the icon in Finder/Spotlight/Dock and macOS treats it as a stable
# application identity (so permission grants don't reset on every rebuild).
#
# Usage:
#   bash scripts/build-mac-app.sh                  # uses target/release
#   bash scripts/build-mac-app.sh /path/to/binary  # uses given binary
#
# Output: dist/MedASR.app  (drag into /Applications when ready)
#
# This DOES NOT codesign or notarize. For distribution outside the local
# machine you still need to run `codesign --deep --sign "Developer ID..."`
# and `xcrun notarytool submit` — see docs/RELEASE.md.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:-$ROOT/target/release/medasr-gui}"
ICON="$ROOT/assets/icon/icon.icns"
DIST="$ROOT/dist"
APP="$DIST/MedASR.app"

if [[ ! -x "$BIN" ]]; then
    echo "error: binary not found at $BIN"
    echo "       build it first:  cargo build --release -p medasr-gui"
    exit 1
fi
if [[ ! -f "$ICON" ]]; then
    echo "error: icon not found at $ICON"
    echo "       generate it:  bash scripts/build-icon.sh"
    exit 1
fi

VERSION="$(grep -E '^version' "$ROOT/Cargo.toml" | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$BIN" "$APP/Contents/MacOS/medasr-gui"
chmod +x "$APP/Contents/MacOS/medasr-gui"
cp "$ICON" "$APP/Contents/Resources/icon.icns"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>            <string>en</string>
    <key>CFBundleDisplayName</key>                  <string>MedASR</string>
    <key>CFBundleName</key>                         <string>MedASR</string>
    <key>CFBundleExecutable</key>                   <string>medasr-gui</string>
    <key>CFBundleIconFile</key>                     <string>icon</string>
    <key>CFBundleIdentifier</key>                   <string>org.medasr.app</string>
    <key>CFBundlePackageType</key>                  <string>APPL</string>
    <key>CFBundleShortVersionString</key>           <string>${VERSION}</string>
    <key>CFBundleVersion</key>                      <string>${VERSION}</string>
    <key>CFBundleSignature</key>                    <string>????</string>
    <key>LSMinimumSystemVersion</key>               <string>12.0</string>
    <key>NSHighResolutionCapable</key>              <true/>
    <key>NSMicrophoneUsageDescription</key>         <string>MedASR records dictation locally on this device. Audio is processed on-device and never leaves your machine.</string>
    <key>NSAppleEventsUsageDescription</key>        <string>MedASR can type transcribed text into the focused window when enabled.</string>
</dict>
</plist>
EOF

echo "built $APP (version ${VERSION})"
echo "  drag into /Applications to install:"
echo "    cp -R \"$APP\" /Applications/"
