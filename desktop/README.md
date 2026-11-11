# Wasl desktop

The tray app for Windows, macOS and Linux: a window and a tray icon in front of `wasld`, the background service that
owns the sync engine. The app never syncs by itself; every action goes to `wasld` over the local protocol in
`engine/crates/wasl-ipc`, and quitting the tray leaves `wasld` syncing. Built with Tauri 2 (Rust) and a small
Vite + TypeScript frontend, in English and Arabic.

## Build and run

Needs Node 20.19 or newer and stable Rust. On Windows, WebView2 (included in Windows 11); on Linux, the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/); on macOS, the Xcode Command Line Tools.

The UI uses `@kahf/design`, Kahf Labs' private design package. **The desktop app can't be built from a public
clone**: `npm ci` fetches that package from a private GitHub repository. The engine, the `wasl` CLI and the Android app
build from this repository alone. Kahf Labs builds (with a read-only token for the package, as the CI secret
`KAHF_DESIGN_READ_TOKEN`, set up for git so npm can reach it):

```sh
cd desktop
npm ci
npm run tauri dev             # Vite on :1420 plus the app, reloads on change
npm run build                 # type-check and build the frontend into dist/
npx tauri build --no-bundle   # release executable: src-tauri/target/release/wasl-desktop
npm run package               # installer bundle (macOS: Wasl.app and a DMG), includes wasld and wasl
```

The app finds this user's `wasld` at start and starts it if it is not running. `--background` starts the app in the
tray without showing the window.

## Tests

```sh
npm test                      # vitest: translations, formatting helpers
npm run test:layout           # window layout in headless Chrome or Edge, light and dark, en and ar
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

## Layout

- `src/`: the frontend (`screens/` holds folder pairs, add pair, preview, problems, history, recycle bin, settings; `i18n/` holds English and Arabic).
- `src-tauri/`: the Rust side (tray, the connection to `wasld`, the commands the window calls). It is its own Cargo workspace and uses the `engine/` crates by path.

How the app, the daemon and the engine fit together: [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md). Using the app:
[docs/USER_GUIDE.md](../docs/USER_GUIDE.md).

## Licence

Wasl is source-available under the [PolyForm Shield License 1.0.0](../LICENSE). It is not open source by the OSI definition.

## Contributing, security, trademarks

- Contributing: [CONTRIBUTING.md](../CONTRIBUTING.md). Contributors sign a one-time [CLA](../CLA.md).
- Security: report problems privately, see [SECURITY.md](../SECURITY.md).
- Trademarks: [TRADEMARKS.md](../TRADEMARKS.md).
