<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/iki-wordmark-dark.png">
    <img alt="Iki 息" src="assets/brand/iki-wordmark.png" width="280">
  </picture>
</p>

# Iki

Hold the Globe key (or fn) to dictate into the focused text field. Audio is transcribed with your Soniox key. Releasing the key pastes the transcript.

macOS 12 or later, on Apple silicon or Intel. No meetings, accounts, or formatting passes.

## Install

1. Download `Iki-<version>.dmg` from the [latest release](../../releases/latest).
2. Open the DMG and drag **Iki** onto **Applications**.
3. Open Iki from Applications. It lives in the menu bar and has no Dock icon.
4. Create a Soniox API key at [console.soniox.com](https://console.soniox.com).
5. Choose **Soniox Key…** from the Iki menu bar item and paste the key. Iki checks it with Soniox and stores it in your Keychain.
6. Grant the permissions macOS asks for, all to Iki:
   - **Microphone**, to hear you while you hold the Globe key.
   - **Accessibility**, so Iki can paste into the focused field.
   - **Input Monitoring**, which some systems need for the Globe key.

   If a prompt was dismissed, enable Iki under **System Settings › Privacy & Security** and relaunch it.

A small pill follows the cursor's display while you hold the key, then while the transcript finishes. It hides at rest. Drag the pill to move it. Quit from the Iki menu bar item.

To update, download the newer DMG and replace the app in Applications. Your key and permissions stay.

## Build from source

You need the Xcode Command Line Tools, Rust via [rustup](https://rustup.rs) and [mise](https://mise.jdx.dev).

```sh
xcode-select --install
git clone https://github.com/npupko/iki.git
cd iki
mise run install
```

`mise run install` builds, signs and copies Iki into `/Applications`, then opens it. `mise run build` only builds `dist/Iki.app`. `mise run test` runs the tests.

The build signs with the maintainer's Apple Development certificate by default. Set `IKI_SIGN_ID` to the name of your own signing identity, as listed by `security find-identity -v -p codesigning`. Use `IKI_SIGN_ID=-` for an ad-hoc signature if you have none. macOS then treats every rebuild as a new app, so Accessibility must be granted again after each one.

```sh
IKI_SIGN_ID="Apple Development: Your Name (TEAMID)" mise run install
```

Besides the key saved from the menu, Iki reads `SONIOX_API_KEY` from the environment, then `fnox get SONIOX_API_KEY`. The saved key wins.

## Release

`mise run release` builds a universal binary, signs it with Developer ID and the hardened runtime, packs `dist/Iki-<version>.dmg` with a drag-to-Applications window, notarizes it and staples the ticket. `mise run publish` uploads that DMG to a GitHub release tagged `v<version>`. The version comes from `CFBundleShortVersionString` in `app/Info.plist`, so bump it, and `CFBundleVersion`, before each release.

Notarization reads credentials saved once in the Keychain under the profile `iki-notary`. Create an app-specific password at [account.apple.com](https://account.apple.com), then run:

```sh
xcrun notarytool store-credentials iki-notary --apple-id <apple-id> --team-id 84KFVZGANB
```

`IKI_SIGN_ID` and `IKI_NOTARY_PROFILE` override the identity and profile. mise installs [dmgbuild](https://github.com/dmgbuild/dmgbuild), which lays out the DMG window from `scripts/dmg-settings.py`.
