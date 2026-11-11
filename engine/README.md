# Wasl engine

The Rust sync engine shared by the Android app, the desktop app and the `wasl` command-line tool. Apps provide the
UI and OS integration; every sync decision is made here. It is a Cargo workspace (`engine/crates/*`, Rust 2024
edition, toolchain pinned in `rust-toolchain.toml`).

## Crates

| Crate | What it holds |
|---|---|
| `wasl-core` | The planner (a pure function), data model, safety guards, name mapping |
| `wasl-ignore` | `.waslignore` and `.gitignore` rules |
| `wasl-store` | SQLite schema, migrations and the per-side baseline |
| `wasl-drive` | Google Drive v3 client |
| `wasl-fs` | Scanner, file watchers, atomic writes, recycle bin |
| `wasl-engine`, `wasl-pairs` | Runs plans and manages folder pairs |
| `wasl-net`, `wasl-devices`, `wasl-chunk` | Device-to-device sync |
| `wasl-crypto` | Vault encryption |
| `wasl-vfs`, `wasl-vfs-windows`, `wasl-vfs-fuse` | Files on demand |
| `wasl-ipc`, `wasld`, `wasl-cli` | Local protocol, the background daemon, the `wasl` CLI |
| `wasl-ffi` | Bindings used by the Android app |

How the pieces fit together: [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md).

## Build

Needs the pinned Rust toolchain (rustup picks it up) and a C compiler, because `wasl-store` builds SQLite from source
(MSVC Build Tools on Windows).

```sh
cd engine
cargo build --workspace --release
```

## Tests

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

The ignore engine's comparison tests use `git` and skip themselves when it is not on `PATH`. The parsers that read
outside data (peer frames, vaults, manifests, Drive JSON, IPC) also have robustness tests and cargo-fuzz targets
(`engine/fuzz`, nightly Rust, Linux or macOS); see [docs/SECURITY.md](../docs/SECURITY.md).

## Docs

[ARCHITECTURE](../docs/ARCHITECTURE.md) · [ENCRYPTION](../docs/ENCRYPTION.md) · [DEVICE_SYNC](../docs/DEVICE_SYNC.md) ·
[SECURITY notes](../docs/SECURITY.md) · [ERRORS](../docs/ERRORS.md)

## Licence

Wasl is source-available under the [PolyForm Shield License 1.0.0](../LICENSE). It is not open source by the OSI definition.

## Contributing, security, trademarks

- Contributing: [CONTRIBUTING.md](../CONTRIBUTING.md). Contributors sign a one-time [CLA](../CLA.md).
- Security: report problems privately, see [SECURITY.md](../SECURITY.md).
- Trademarks: [TRADEMARKS.md](../TRADEMARKS.md).
