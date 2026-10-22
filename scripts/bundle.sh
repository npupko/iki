#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -p iki
app="dist/Iki.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/sounds"
cp target/release/iki "$app/Contents/MacOS/iki"
cp app/Info.plist "$app/Contents/Info.plist"
cp assets/sounds/dictation-start.wav assets/sounds/dictation-stop.wav "$app/Contents/Resources/sounds/"
cp assets/brand/AppIcon.icns assets/brand/StatusIcon.png assets/brand/StatusIcon@2x.png "$app/Contents/Resources/"
# Apple Development keeps the same team, so an Accessibility grant survives later rebuilds.
# Ad-hoc signatures do not: each build is a new identity.
identity="${IKI_SIGN_ID:-Apple Development: lazynick7@gmail.com (PC7LYL8J8D)}"
codesign --force --sign "$identity" "$app"
echo "Built $app"
