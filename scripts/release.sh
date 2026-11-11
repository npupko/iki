#!/bin/bash
# Builds a universal, Developer ID signed, notarized and stapled dist/Iki-<version>.dmg.
# Needs notary credentials saved once in the Keychain:
#   xcrun notarytool store-credentials iki-notary --apple-id <id> --team-id 84KFVZGANB
set -euo pipefail
cd "$(dirname "$0")/.."

profile="${IKI_NOTARY_PROFILE:-iki-notary}"
identity="${IKI_SIGN_ID:-Developer ID Application: Nikita Pupko (84KFVZGANB)}"
version=$(/usr/libexec/PlistBuddy -c "Print CFBundleShortVersionString" app/Info.plist)
dmg="dist/Iki-$version.dmg"

IKI_RELEASE=1 IKI_SIGN_ID="$identity" ./scripts/bundle.sh

rm -f "$dmg"
dmgbuild -s scripts/dmg-settings.py -D app=dist/Iki.app "Iki" "$dmg"
codesign --force --timestamp --sign "$identity" "$dmg"

xcrun notarytool submit "$dmg" --keychain-profile "$profile" --wait
xcrun stapler staple "$dmg"
spctl --assess --type open --context context:primary-signature --verbose "$dmg"
echo "Built $dmg"
