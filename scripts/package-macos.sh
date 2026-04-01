#!/bin/bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="Claude Count"
BIN_PATH="${1:-$ROOT_DIR/target/release/claude-code-usage-monitor}"
OUTPUT_DMG="${2:-$ROOT_DIR/dist/claude-count.dmg}"
VERSION="$(awk -F ' = ' '/^version = / { gsub(/"/, "", $2); print $2; exit }' "$ROOT_DIR/Cargo.toml")"
SIGN_IDENTITY="${SIGN_IDENTITY:--}"
BUILD_DIR="$ROOT_DIR/target/macos-package"
APP_DIR="$BUILD_DIR/$APP_NAME.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"
STAGING_DIR="$BUILD_DIR/dmg"
ICONSET_DIR="$BUILD_DIR/AppIcon.iconset"
ICNS_PATH="$RESOURCES_DIR/AppIcon.icns"

rm -rf "$BUILD_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR" "$ICONSET_DIR" "$(dirname "$OUTPUT_DMG")"

if [ ! -f "$BIN_PATH" ]; then
  echo "Missing release binary at $BIN_PATH" >&2
  exit 1
fi

cat > "$CONTENTS_DIR/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>Claude Count</string>
  <key>CFBundleExecutable</key>
  <string>Claude Count</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundleIdentifier</key>
  <string>com.deepakagarwal.claudecount</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Claude Count</string>
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
PLIST

cp "$BIN_PATH" "$MACOS_DIR/$APP_NAME"
chmod +x "$MACOS_DIR/$APP_NAME"

SOURCE_ICON="$ROOT_DIR/src/icons/256x256.png"
sips -z 16 16 -s format png "$SOURCE_ICON" --out "$ICONSET_DIR/icon_16x16.png" >/dev/null
sips -z 32 32 -s format png "$SOURCE_ICON" --out "$ICONSET_DIR/icon_16x16@2x.png" >/dev/null
sips -z 32 32 -s format png "$SOURCE_ICON" --out "$ICONSET_DIR/icon_32x32.png" >/dev/null
sips -z 64 64 -s format png "$SOURCE_ICON" --out "$ICONSET_DIR/icon_32x32@2x.png" >/dev/null
sips -z 128 128 -s format png "$SOURCE_ICON" --out "$ICONSET_DIR/icon_128x128.png" >/dev/null
sips -z 256 256 -s format png "$SOURCE_ICON" --out "$ICONSET_DIR/icon_128x128@2x.png" >/dev/null
iconutil -c icns "$ICONSET_DIR" -o "$ICNS_PATH"

xattr -cr "$APP_DIR" || true
codesign --force --deep --sign "$SIGN_IDENTITY" "$APP_DIR"

mkdir -p "$STAGING_DIR"
cp -R "$APP_DIR" "$STAGING_DIR/"
ln -s /Applications "$STAGING_DIR/Applications"

rm -f "$OUTPUT_DMG"
hdiutil create \
  -volname "$APP_NAME" \
  -srcfolder "$STAGING_DIR" \
  -ov \
  -format UDZO \
  "$OUTPUT_DMG" >/dev/null

echo "Created $OUTPUT_DMG"
