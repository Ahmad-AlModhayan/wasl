# Wasl

Wasl syncs folders between Android, desktop (Windows, macOS, Linux) and Google Drive, using your own Drive
account. Every deletion or overwrite can be undone for 30 days, and folders can be encrypted on your device
before they reach Drive.

## What it does

- Per-folder sync methods: two-way, upload only, download only, mirror, and move.
- Instant sync (file watcher plus Drive change polling), schedules, and conditions (Wi-Fi, SSID, charging, battery, VPN).
- Safety: conflict copies, a recycle bin, Drive revisions with restore, a mass-delete guard, and a dry-run preview.
- Vaults: optional client-side encryption of names and contents, so Drive never sees them ([docs/ENCRYPTION.md](docs/ENCRYPTION.md)).
- Direct device-to-device sync, with or without Drive ([docs/DEVICE_SYNC.md](docs/DEVICE_SYNC.md)).
- Google Docs, Sheets and Slides exported as Office or PDF copies.
- Drive files on demand (browse without downloading, pin for offline).
- AI agent changes are held on your computer until you accept or revert them ([docs/AGENT_CHANGES.md](docs/AGENT_CHANGES.md)).
- No ads, no analytics, no Wasl servers: the app talks to Google Drive and to your own devices.

## Repository layout

| Folder | What it is |
|---|---|
| [`app/`](app/README.md) | Android app (Kotlin, Jetpack Compose) |
| [`engine/`](engine/README.md) | The Rust sync engine, shared by every platform |
| [`desktop/`](desktop/README.md) | Desktop tray app (Tauri 2) |
| [`site/`](site/README.md) | The Wasl website (needs the private `@kahf/design`) |
| [`docs/`](docs/) | Architecture, requirements, user guide, security notes |

## Build and install

Android needs a Google Cloud OAuth client for your own Drive account; [docs/SETUP.md](docs/SETUP.md) walks through it,
then:

```sh
./gradlew :app:assembleRelease
adb install -r app/build/outputs/apk/release/app-release.apk
```

Release builds need a signing key you provide; see [docs/SETUP.md](docs/SETUP.md#signing-keys). Engine build
instructions are in [`engine/`](engine/README.md).

The engine, the CLI and the Android app build from this repository alone. The desktop app and the website need
Kahf Labs' private design package (`@kahf/design`), so they can't be built from a public clone; contributors can
change their code, but the desktop and website CI jobs are skipped on pull requests from forks; Kahf Labs runs them before
merging (see [CONTRIBUTING.md](CONTRIBUTING.md)).

## Tests

```sh
./gradlew :app:testDebugUnitTest                      # Android, JVM tests
(cd engine && cargo test --workspace)                 # engine
```

## Docs

[SETUP](docs/SETUP.md) · [USER_GUIDE](docs/USER_GUIDE.md) · [ARCHITECTURE](docs/ARCHITECTURE.md) ·
[ENCRYPTION](docs/ENCRYPTION.md) · [AUTOMATION](docs/AUTOMATION.md) · [DEVICE_SYNC](docs/DEVICE_SYNC.md) ·
[PRD](docs/PRD.md) · [SRS](docs/SRS.md) · [security notes](docs/SECURITY.md)

## Open code

Wasl is **open code**: the full source is public so you can read it, run it, change it and send pull requests. It is
**source-available** under the [PolyForm Shield License 1.0.0](LICENSE), not open source by the OSI definition: you may
not use it to provide a product that competes with Wasl or with another Kahf Labs product built on it, free or paid.
Contributions need a one-time [CLA](CLA.md) (you keep your copyright); see [CONTRIBUTING.md](CONTRIBUTING.md). Forks may
not use the Kahf or Wasl names or logos ([TRADEMARKS.md](TRADEMARKS.md)). Report security problems privately
([SECURITY.md](SECURITY.md)). Everyone is expected to follow the [Code of Conduct](CODE_OF_CONDUCT.md).
The source is under PolyForm Shield (see [LICENSE](LICENSE)); the installed binaries carry no separate terms yet.

| Path | Licence |
|---|---|
| Everything not listed below | [PolyForm Shield 1.0.0](LICENSE) |
| Fonts under `docs/design/project` and the app | SIL Open Font License 1.1, with each font's own notice |
