#!/usr/bin/env bash
# Build CcooNverter.app (universal arm64 + x86_64) and a DMG on macOS.
#
#   bash scripts/build_macos.sh
#
# Prerequisites: xcode-select --install, rustup (https://rustup.rs).
# Output: dist/macos/CcooNverter.app, dist/macos/CcooNverter_v<ver>_macos_universal.dmg
#         and dist/macos/SHA256SUMS.txt
# See docs/03_MAC_BUILD_PLAN.md.
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
APP=CcooNverter
BUNDLE_ID=com.ccoon.ccoonverter
DMG_NAME="${APP}_v${VERSION}_macos_universal.dmg"
export MACOSX_DEPLOYMENT_TARGET=11.0

echo "Building $APP v$VERSION (universal)"

rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release -p ccoonverter --target aarch64-apple-darwin
cargo build --release -p ccoonverter --target x86_64-apple-darwin

rm -rf dist/macos
mkdir -p "dist/macos/$APP.app/Contents/MacOS" "dist/macos/$APP.app/Contents/Resources"
lipo -create \
  "target/aarch64-apple-darwin/release/$APP" \
  "target/x86_64-apple-darwin/release/$APP" \
  -output "dist/macos/$APP.app/Contents/MacOS/$APP"
lipo -info "dist/macos/$APP.app/Contents/MacOS/$APP"
cp assets/icon.icns "dist/macos/$APP.app/Contents/Resources/icon.icns"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@BUNDLE_ID@/$BUNDLE_ID/g" \
  packaging/macos/Info.plist.template > "dist/macos/$APP.app/Contents/Info.plist"
printf 'APPL????' > "dist/macos/$APP.app/Contents/PkgInfo"
plutil -lint "dist/macos/$APP.app/Contents/Info.plist"

# Ad-hoc signature (no Developer ID). Unsigned arm64 binaries are refused on
# Apple Silicon, so this step is required.
codesign --force --deep --sign - "dist/macos/$APP.app"
codesign --verify --verbose "dist/macos/$APP.app"

# DMG with hdiutil only: the app plus an Applications symlink.
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
cp -R "dist/macos/$APP.app" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "$APP $VERSION" -srcfolder "$STAGE" -ov -format UDZO \
  "dist/macos/$DMG_NAME"

(cd dist/macos && shasum -a 256 "$DMG_NAME" > SHA256SUMS.txt)
cat dist/macos/SHA256SUMS.txt
echo "OK: dist/macos/$DMG_NAME"
