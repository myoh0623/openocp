#!/usr/bin/env bash
set -Eeuo pipefail

PROJECT_DIR=$(cd "$(dirname "$0")" && pwd)
APP_NAME="Remote OCP"
APP_DIR="$PROJECT_DIR/dist/$APP_NAME.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"
BUILD_DIR="$PROJECT_DIR/.build"
ICONSET_DIR="$BUILD_DIR/AppIcon.iconset"
SDK_PATH=$(xcrun --sdk macosx --show-sdk-path)

mkdir -p "$MACOS_DIR" "$RESOURCES_DIR" "$BUILD_DIR" "$ICONSET_DIR"
rm -f "$MACOS_DIR/OpenOCPDashboard"

build_arch() {
  local arch=$1
  xcrun swiftc \
    -swift-version 5 \
    -parse-as-library \
    -O \
    -target "$arch-apple-macosx13.0" \
    -sdk "$SDK_PATH" \
    -framework SwiftUI \
    -framework AppKit \
    "$PROJECT_DIR/Sources/OpenOCPApp.swift" \
    -o "$BUILD_DIR/OpenOCPDashboard-$arch"
}

build_arch arm64
build_arch x86_64
xcrun lipo -create \
  "$BUILD_DIR/OpenOCPDashboard-arm64" \
  "$BUILD_DIR/OpenOCPDashboard-x86_64" \
  -output "$MACOS_DIR/OpenOCPDashboard"

cp "$PROJECT_DIR/Resources/Info.plist" "$CONTENTS_DIR/Info.plist"
cp "$PROJECT_DIR/Resources/ssh-askpass.sh" "$RESOURCES_DIR/ssh-askpass.sh"
chmod 755 "$MACOS_DIR/OpenOCPDashboard" "$RESOURCES_DIR/ssh-askpass.sh"

make_icon() {
  local size=$1
  local output=$2
  sips -z "$size" "$size" "$PROJECT_DIR/Resources/AppIcon.png" --out "$ICONSET_DIR/$output" >/dev/null
}

make_icon 16 icon_16x16.png
make_icon 32 icon_16x16@2x.png
make_icon 32 icon_32x32.png
make_icon 64 icon_32x32@2x.png
make_icon 128 icon_128x128.png
make_icon 256 icon_128x128@2x.png
make_icon 256 icon_256x256.png
make_icon 512 icon_256x256@2x.png
make_icon 512 icon_512x512.png
make_icon 1024 icon_512x512@2x.png
iconutil -c icns "$ICONSET_DIR" -o "$RESOURCES_DIR/AppIcon.icns"

codesign --force --deep --sign - "$APP_DIR"
ditto -c -k --sequesterRsrc --keepParent "$APP_DIR" "$PROJECT_DIR/dist/Remote-OCP-macOS.zip"

echo "$APP_DIR"
