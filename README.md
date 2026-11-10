<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/iki-wordmark-dark.png">
    <img alt="Iki 息" src="assets/brand/iki-wordmark.png" width="280">
  </picture>
</p>

# Iki

Hold the Globe key (or fn) to dictate into the focused text field. Audio is transcribed with your Soniox key. Releasing the key pastes the transcript.

macOS only. No meetings, accounts, or formatting passes.

## Run

1. Add your Soniox key from **Soniox Key…** in the Iki menu bar item. Iki checks it with Soniox, stores it in the Keychain and uses it from the next take, with no relaunch. Without a saved key, Iki falls back to `SONIOX_API_KEY` from the environment, then to `fnox get SONIOX_API_KEY`. A Finder launch still finds the mise `fnox` shim. A key left in the old `~/Library/Application Support/Iki/soniox_api_key` file is moved into the Keychain at launch.
2. Build, sign and install into `/Applications`: `mise run install`. `mise run build` only builds `dist/Iki.app`.
3. Iki opens on its own after `install`.
4. Grant Microphone and Accessibility when macOS asks. The Globe key also needs Input Monitoring on some systems. Grant those to Iki, not to the terminal that built it.

A small pill follows the cursor's display while you hold the key, then while the transcript finishes. It hides at rest. Drag the pill to move it. Quit from the Iki menu bar item.
