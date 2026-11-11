<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/iki-wordmark-dark.png">
    <img alt="Iki 息" src="assets/brand/iki-wordmark.png" width="280">
  </picture>
</p>

<p align="center">
  <a href="https://github.com/npupko/iki/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/npupko/iki"></a>
  <a href="https://github.com/npupko/iki/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/npupko/iki/actions/workflows/ci.yml/badge.svg"></a>
  <a href="#license"><img alt="License: MIT or Apache-2.0" src="https://img.shields.io/badge/license-MIT%20or%20Apache--2.0-blue"></a>
</p>

# Iki

Hold the Globe key (or fn) to dictate into the focused text field. Audio is transcribed with your Soniox key. Releasing the key pastes the transcript.

macOS 12 or later, on Apple silicon or Intel. No meetings, accounts, or formatting passes.

- **Dictionary** keeps names and terms spelled your way, and sets the languages you speak.
- **Transcripts** lists past takes, so you can copy one again.
- **A small pill** follows the cursor's display while you hold the key, then while the transcript finishes. Drag it to move it.

## Install

1. Download `Iki-<version>.dmg` from the [latest release](https://github.com/npupko/iki/releases/latest).
2. Open the DMG and drag **Iki** onto **Applications**.
3. Open Iki from Applications. It lives in the menu bar and has no Dock icon.
4. Create a Soniox API key at [console.soniox.com](https://console.soniox.com).
5. Choose **Soniox Key…** from the Iki menu bar item and paste the key. Iki checks it with Soniox and stores it in your Keychain.
6. Grant the permissions macOS asks for, all to Iki:
   - **Microphone**, to hear you while you hold the Globe key.
   - **Accessibility**, so Iki can paste into the focused field. Recent macOS calls it **Device Control and Data Access**.
   - **Input Monitoring**, which some systems need for the Globe key.

   If a prompt was dismissed, enable Iki under **System Settings › Privacy & Security** and relaunch it.

To update, download the newer DMG and replace the app in Applications. Your key, settings and permissions stay.

## Privacy

Iki records only while you hold the Globe key. That audio streams to [Soniox](https://soniox.com) under your own key. Iki has no server, account or analytics of its own.

Your key is stored in the macOS Keychain. Transcript history, the dictionary, languages and the pill position stay on your Mac in `~/Library/Application Support/Iki`.

## Troubleshooting

**Iki is switched on in Privacy & Security but still asks for access.** The grant belongs to an older copy of Iki that was signed differently. Quit Iki and clear its grants, then open it and grant them again:

```sh
tccutil reset All com.pupko.iki
```

**Holding Globe does nothing, or opens the emoji picker.** Iki needs Accessibility and Input Monitoring to watch the Globe key. Check both, then relaunch Iki.

**Nothing is pasted.** Check that the cursor is in a text field. The transcript is also listed under **Transcripts** in the menu, ready to copy.

## Uninstall

Quit Iki from its menu bar item, then run:

```sh
rm -rf /Applications/Iki.app ~/Library/Application\ Support/Iki
security delete-generic-password -s com.pupko.iki -a soniox_api_key
tccutil reset All com.pupko.iki
```

## Build from source

You need the Xcode Command Line Tools, Rust via [rustup](https://rustup.rs) and [mise](https://mise.jdx.dev).

```sh
xcode-select --install
git clone https://github.com/npupko/iki.git
cd iki
mise run install
```

`mise run install` builds, signs and copies Iki into `/Applications`, then opens it. `mise run build` only builds `dist/Iki.app`. `mise run test` runs the tests.

Set `IKI_SIGN_ID` to one of your signing identities, as listed by `security find-identity -v -p codesigning`. Without one, the build is signed ad hoc. macOS then treats every rebuild as a new app, so the permissions must be granted again after each one.

```sh
IKI_SIGN_ID="Apple Development: Your Name (TEAMID)" mise run install
```

Besides the key saved from the menu, Iki reads `SONIOX_API_KEY` from the environment, then `fnox get SONIOX_API_KEY`. The saved key wins.

## Contributing

Bug reports and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md). Report security issues privately, as described in [SECURITY.md](SECURITY.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Iki by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
