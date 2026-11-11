#!/bin/bash
# Builds and signs dist/Iki.app.
# IKI_RELEASE=1 builds a universal (Apple silicon + Intel) binary, signs with
# Developer ID and a secure timestamp, as notarization requires.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${IKI_RELEASE:-}" == 1 ]]; then
    targets=(aarch64-apple-darwin x86_64-apple-darwin)
    rustup target add "${targets[@]}"
    for t in "${targets[@]}"; do cargo build --release -p iki --target "$t"; done
    mkdir -p target/universal
    lipo -create -output target/universal/iki \
        "target/aarch64-apple-darwin/release/iki" "target/x86_64-apple-darwin/release/iki"
    binary=target/universal/iki
    identity="${IKI_SIGN_ID:-Developer ID Application: Nikita Pupko (84KFVZGANB)}"
    timestamp=(--timestamp)
else
    cargo build --release -p iki
    binary=target/release/iki
    # Apple Development keeps the same team, so an Accessibility grant survives later rebuilds.
    # Ad-hoc signatures do not: each build is a new identity.
    identity="${IKI_SIGN_ID:-Apple Development: lazynick7@gmail.com (PC7LYL8J8D)}"
    timestamp=(--timestamp=none)
    if [[ -z "${IKI_SIGN_ID:-}" ]] && ! security find-identity -v -p codesigning | grep -qF "\"$identity\""; then
        echo "No '$identity' in the Keychain. Signing ad hoc; set IKI_SIGN_ID to keep permission grants across rebuilds." >&2
        identity=-
    fi
fi

app="dist/Iki.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/sounds"
cp "$binary" "$app/Contents/MacOS/iki"
cp app/Info.plist "$app/Contents/Info.plist"
cp assets/sounds/dictation-start.wav assets/sounds/dictation-stop.wav "$app/Contents/Resources/sounds/"
cp assets/brand/AppIcon.icns assets/brand/StatusIcon.png assets/brand/StatusIcon@2x.png "$app/Contents/Resources/"
# Hardened runtime blocks the microphone unless the entitlement asks for it.
codesign --force --options runtime --entitlements app/Iki.entitlements \
    "${timestamp[@]}" --sign "$identity" "$app"
echo "Built $app"
