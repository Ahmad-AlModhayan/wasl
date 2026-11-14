# Wasl

Wasl syncs folders between Android, desktop (Windows, macOS, Linux) and Google Drive, using your own Drive
account and your own devices. Every deletion or overwrite can be undone for 30 days, and folders can be encrypted on your device
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
- No ads, no analytics: Drive traffic goes directly to Google Drive, and P2P traffic goes directly between your devices (with optional public STUN, self-hosted relay, or Kahf managed relay/blind-cache fallback behind strict NATs).

## Repository layout

This public repository contains the open-source sync engine, daemon, FFI bindings, protocol, CLI, tools, specs, and website. Official Android and desktop UI apps talk to `wasl-core` across the strict FFI/IPC boundary and are shipped separately.

| Folder / Crate | What it is | Licence |
|---|---|---|
| [`engine/`](engine/README.md) (`wasl-core`, `wasl-engine`, `wasl-store`, `wasl-crypto`, `wasl-net`, `wasl-devices`, `wasl-drive`, `wasl-fs`, `wasl-ignore`, `wasl-pairs`, `wasl-chunk`, `wasl-vfs*`, `wasl-ffi`, `wasld`) | Rust sync engine, daemon (`wasld`), UniFFI bindings, self-hostable core | [AGPL-3.0-only](LICENSE) (commercial dual-licence available) |
| `engine/crates/wasl-proto`, `wasl-ipc`, `wasl-cli` | Wire protocol & DTOs, local IPC client, thin `wasl` CLI | [Apache-2.0](LICENSE-APACHE) |
| [`tools/`](tools/) | Vault recovery reference script & Miqyas test harnesses | [AGPL-3.0-only](LICENSE) |
| [`site/`](site/README.md) | The Wasl website (needs the private `@kahf/design`) | See [NOTICE.md](NOTICE.md) |
| [`docs/`](docs/) | Architecture, requirements, open-core model, user guide, security notes | See [LICENSE](LICENSE) |

## Build and tests

Engine, daemon, FFI, protocol, and CLI build from this repository alone (see [`engine/README.md`](engine/README.md)):

```sh
cd engine
cargo build --workspace --release
cargo test --workspace
cargo test -p wasl-ffi --all-features
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

## Docs

[OPEN_MODEL](docs/OPEN_MODEL.md) · [SETUP](docs/SETUP.md) · [USER_GUIDE](docs/USER_GUIDE.md) · [ARCHITECTURE](docs/ARCHITECTURE.md) ·
[ENCRYPTION](docs/ENCRYPTION.md) · [AUTOMATION](docs/AUTOMATION.md) · [DEVICE_SYNC](docs/DEVICE_SYNC.md) ·
[PRD](docs/PRD.md) · [SRS](docs/SRS.md) · [security notes](docs/SECURITY.md)

## Open-core model & licensing

Wasl uses a **decoupled open-core** model ([docs/OPEN_MODEL.md](docs/OPEN_MODEL.md)):

- **Core engine, daemon, FFI, and self-hostable relay (`wasl-core`, `wasld`, `wasl-ffi`, etc.):** [GNU AGPL v3.0](LICENSE) (`AGPL-3.0-only`), with B2B commercial dual-licensing available from Kahf Labs.
- **Wire protocol, IPC client, and thin CLI (`wasl-proto`, `wasl-ipc`, `wasl-cli`):** [Apache License 2.0](LICENSE-APACHE) (`Apache-2.0`), with zero AGPL dependencies enforced in CI.
- **Official native UI apps & Managed Relay / Blind-Cache service:** Shipped separately under Kahf Labs proprietary terms. **There is no Wasl Pro version** — all local, Drive, and P2P sync features are free; subscriptions apply only where Kahf incurs ongoing TURN relay bandwidth and blind-cache server costs.

Contributions require a one-time [CLA](CLA.md) (you keep your copyright); see [CONTRIBUTING.md](CONTRIBUTING.md). Forks may not use the Kahf or Wasl names or logos ([TRADEMARKS.md](TRADEMARKS.md)). Report security problems privately ([SECURITY.md](SECURITY.md)), and read third-party notices in [NOTICE.md](NOTICE.md). Everyone is expected to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

| Path | Licence |
|---|---|
| `engine/crates/wasl-proto`, `wasl-ipc`, `wasl-cli` | [Apache-2.0](LICENSE-APACHE) |
| All other `engine/crates/*`, `tools/`, core & relay code | [AGPL-3.0-only](LICENSE) |
| Fonts under `docs/design/project` | SIL Open Font License 1.1, with each font's own notice |
