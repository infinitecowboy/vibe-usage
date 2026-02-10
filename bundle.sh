#!/bin/bash
# Build and assemble Vibe Usage.app bundle
set -e

cargo build --release

APP="target/release/Vibe Usage.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
mkdir -p "$APP/Contents/Resources"

cp target/release/vibe-usage "$APP/Contents/MacOS/"
cp Info.plist "$APP/Contents/"
cp AppIcon.icns "$APP/Contents/Resources/"

echo "Built: $APP"
