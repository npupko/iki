#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -p iki
app="dist/Iki.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp target/release/iki "$app/Contents/MacOS/iki"
cp app/Info.plist "$app/Contents/Info.plist"
# Apple Development keeps the same team, so an Accessibility grant survives later rebuilds.
# Ad-hoc signatures do not: each build is a new identity.
identity="${IKI_SIGN_ID:-Apple Development: lazynick7@gmail.com (PC7LYL8J8D)}"
codesign --force --sign "$identity" "$app"
echo "Built $app"
