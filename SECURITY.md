# Security policy

## Supported versions

Only the [latest release](https://github.com/npupko/iki/releases/latest) receives fixes.

## Reporting a vulnerability

Report it privately through [GitHub's private vulnerability reporting](https://github.com/npupko/iki/security/advisories/new). Do not open a public issue.

Include what an attacker could do, the steps to reproduce it, and the Iki and macOS versions. You should hear back within a week. Once a fix ships, the advisory is published with credit to you unless you ask otherwise.

Iki holds a Soniox API key in the Keychain, listens to the Globe key, records the microphone while that key is held, and pastes into other apps. Issues that expose the key, record or paste without the key held, or let another process drive Iki are in scope.
