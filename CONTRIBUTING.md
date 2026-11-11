# Contributing to Iki

Thanks for helping. Iki stays small on purpose: hold a key, dictate, paste. Open an issue before starting on a new feature, so we can agree it fits.

## Reporting a bug

Use the bug report form under [Issues](https://github.com/npupko/iki/issues/new/choose). Include your macOS version, your Mac's chip, the Iki version from the DMG name, and what you expected to happen.

Report security issues privately, as described in [SECURITY.md](SECURITY.md).

## Development

Set up as in [Build from source](README.md#build-from-source). Then:

| Command | What it does |
|---------|--------------|
| `mise run build` | Build and sign `dist/Iki.app` |
| `mise run install` | Build, copy into `/Applications` and relaunch |
| `mise run test` | Run the workspace tests |
| `mise run icons` | Regenerate the icon, menu bar glyph, wordmarks and DMG background |

The workspace has one app and three crates:

| Path | Role |
|------|------|
| `app` | Menu bar app, windows, key storage, history and dictionary |
| `crates/globe-input` | Globe key event tap and paste |
| `crates/soniox-audio` | Microphone capture and the Soniox realtime session |
| `crates/iki-bar` | The floating recording pill |

## Pull requests

- Keep each pull request to one change, and explain why it is needed.
- Run `cargo fmt` and `mise run test` before pushing. CI runs the tests and a release build.
- Write commit messages as one plain sentence in the imperative, such as "Snap the pill to the screen's centre line while dragging."

By contributing, you agree that your work is dual licensed under MIT or Apache-2.0, as stated in the [README](README.md#license).

## Releasing

Maintainers only. A release needs the Developer ID Application certificate and notary credentials saved once in the Keychain:

```sh
xcrun notarytool store-credentials iki-notary --apple-id <apple-id> --team-id 84KFVZGANB
```

1. Bump `CFBundleShortVersionString` and `CFBundleVersion` in `app/Info.plist`.
2. Commit and push `main`.
3. Run `mise run release`. It builds a universal binary, signs it with the hardened runtime, packs `dist/Iki-<version>.dmg` with a drag-to-Applications window, notarizes it and staples the ticket.
4. Run `mise run publish`. It creates the `v<version>` tag and GitHub release with the DMG attached.

`IKI_SIGN_ID` and `IKI_NOTARY_PROFILE` override the signing identity and notary profile. The DMG window is laid out by [dmgbuild](https://github.com/dmgbuild/dmgbuild) from `scripts/dmg-settings.py`, and mise installs it.
