#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <target-triple> <output-dmg>" >&2
  exit 2
fi

TARGET="$1"
OUTPUT_DMG="$2"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DIST="$ROOT/dist"
VERSION="${GITHUB_REF_NAME:-0.1.0}"
VERSION="${VERSION#v}"
if [ "$VERSION" = "main" ] || [ -z "$VERSION" ]; then
  VERSION="0.1.0"
fi

BIN_DIR="$ROOT/target/$TARGET/release"
if [ ! -d "$BIN_DIR" ]; then
  BIN_DIR="$ROOT/target/release"
fi
MAIN_BIN="$BIN_DIR/lanclip"
CONTROL_BIN="$BIN_DIR/lanclip-control"
if [ ! -x "$MAIN_BIN" ] || [ ! -x "$CONTROL_BIN" ]; then
  echo "missing release binaries under $BIN_DIR" >&2
  exit 1
fi

WORK="$DIST/macos-$TARGET"
APP="$WORK/lanclip.app"
ICONSET="$WORK/lanclip.iconset"
ICNS="$WORK/lanclip.icns"
DMG_ROOT="$WORK/dmgroot"

rm -rf "$WORK" "$OUTPUT_DMG"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$DMG_ROOT"

python3 "$ROOT/scripts/make_icons.py" --iconset "$ICONSET"
iconutil -c icns "$ICONSET" -o "$ICNS"

cp "$MAIN_BIN" "$APP/Contents/MacOS/lanclip"
cp "$CONTROL_BIN" "$APP/Contents/MacOS/lanclip-control"
cp "$ICNS" "$APP/Contents/Resources/lanclip.icns"
cp "$ROOT/crates/lanclip-ui/assets/icons/lanclip.svg" "$APP/Contents/Resources/lanclip.svg"
cp "$ROOT/README.md" "$APP/Contents/Resources/README.md"
cp "$ROOT/LICENSE" "$APP/Contents/Resources/LICENSE"
chmod +x "$APP/Contents/MacOS/lanclip" "$APP/Contents/MacOS/lanclip-control"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>zh_CN</string>
  <key>CFBundleDisplayName</key>
  <string>lanclip</string>
  <key>CFBundleExecutable</key>
  <string>lanclip</string>
  <key>CFBundleIconFile</key>
  <string>lanclip.icns</string>
  <key>CFBundleIdentifier</key>
  <string>cn.apizero.lanclip</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>lanclip</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleVersion</key>
  <string>$VERSION</string>
  <key>LSMinimumSystemVersion</key>
  <string>12.0</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
EOF

if command -v codesign >/dev/null 2>&1; then
  codesign --force --deep --sign - "$APP" || true
fi

cp -R "$APP" "$DMG_ROOT/"
ln -s /Applications "$DMG_ROOT/Applications"
hdiutil create -volname "lanclip" -srcfolder "$DMG_ROOT" -ov -format UDZO "$OUTPUT_DMG"
rm -rf "$WORK"
