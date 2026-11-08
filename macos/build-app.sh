#!/bin/sh
# Build enoch.app for Apple Silicon and Intel Macs (one universal binary),
# sign it ad hoc, and zip it: dist/enoch.app and dist/enoch-macos.zip.
# Runs on macOS, with both Rust targets installed:
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin
set -eu
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
app=dist/enoch.app

for target in aarch64-apple-darwin x86_64-apple-darwin; do
	cargo build --release --locked --target "$target"
done

rm -rf dist
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/enoch" \
	target/aarch64-apple-darwin/release/enoch \
	target/x86_64-apple-darwin/release/enoch
sed "s/@VERSION@/$version/g" macos/Info.plist >"$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"

# Every size an .icns holds, scaled down from the 1024px drawing.
iconset=$(mktemp -d)/enoch.iconset
mkdir "$iconset"
for s in 16 32 128 256 512; do
	sips -z "$s" "$s" macos/icon.png --out "$iconset/icon_${s}x${s}.png" >/dev/null
	d=$((s * 2))
	sips -z "$d" "$d" macos/icon.png --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns -o "$app/Contents/Resources/enoch.icns" "$iconset"

# Ad hoc: enough to run, but not notarized (see the README).
codesign --force --sign - "$app"
codesign --verify --strict --verbose=2 "$app"

ditto -c -k --keepParent "$app" dist/enoch-macos.zip
echo "built $app ($version) and dist/enoch-macos.zip"
