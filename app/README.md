# Fantastic Team app packaging

`app/tauri.conf.json` packages the Fantastic Team UI as a desktop app for
macOS, Windows and Linux. The window loads the local web UI on
`127.0.0.1` only; no frontend is bundled.

## Build locally

Start the team server on the loopback port the window expects (8765), then
build from the repository root with the Tauri CLI:

- macOS (.app, .dmg): `cargo tauri build --bundles app,dmg`
- Windows (.msi, NSIS .exe): `cargo tauri build --bundles msi,nsis`
- Linux (.deb, AppImage): `cargo tauri build --bundles deb,appimage`

Artifacts land under `target/release/bundle/`.

## Signing and notarization (maintainer only)

Signing runs on the maintainer's laptop only. Nothing in this repository
stores an identity, team id, certificate, thumbprint or password.

- macOS: set the signing identity and notarization credentials in the
  maintainer's shell environment at build time, then build the dmg. The
  identifier is `com.aisixteen.team`.
- Windows: sign the msi/exe with the maintainer's code-signing certificate
  from the local certificate store.
- Linux: packages are unsigned by default; the maintainer adds a detached
  signature when publishing.

Never commit these values. The repository secret check (`scripts/check-secrets.sh`)
and `crates/team/tests/app_packaging.rs` guard against committed signing data.
