# Wasl engine

The Rust sync engine shared across IPC (`wasl-ipc`) and UniFFI (`wasl-ffi`) by clients, the daemon (`wasld`), and the `wasl` command-line tool. Native apps provide UI and OS integration across that boundary; every sync decision is made here. It is a Cargo workspace (`engine/crates/*`, Rust 2024 edition, toolchain pinned in `rust-toolchain.toml`).

## Crates

| Crate | Licence | What it holds |
|---|---|---|
| `wasl-core` | `AGPL-3.0-only` | The planner (a pure function), data model, safety guards, name mapping |
| `wasl-ignore` | `AGPL-3.0-only` | `.waslignore` and `.gitignore` rules |
| `wasl-store` | `AGPL-3.0-only` | SQLite schema, migrations and the per-side baseline |
| `wasl-drive` | `AGPL-3.0-only` | Google Drive v3 client |
| `wasl-fs` | `AGPL-3.0-only` | Scanner, file watchers, atomic writes, recycle bin |
| `wasl-engine`, `wasl-pairs` | `AGPL-3.0-only` | Runs plans and manages folder pairs |
| `wasl-net`, `wasl-devices`, `wasl-chunk` | `AGPL-3.0-only` | Device-to-device sync |
| `wasl-crypto` | `AGPL-3.0-only` | Vault encryption |
| `wasl-vfs`, `wasl-vfs-windows`, `wasl-vfs-fuse` | `AGPL-3.0-only` | Files on demand |
| `wasld` | `AGPL-3.0-only` | Background sync daemon & offline `wasld vault decrypt` recovery helper |
| `wasl-ffi` | `AGPL-3.0-only` | UniFFI bindings for native mobile clients |
| `wasl-proto` | `Apache-2.0` | Shared IPC protocol types, statuses, op-log DTOs, preset IDs |
| `wasl-ipc` | `Apache-2.0` | Local socket client/server transport & user-only path helpers |
| `wasl-cli` | `Apache-2.0` | Thin `wasl` CLI (talks to `wasld` over `wasl-ipc` / spawns `wasld vault decrypt`) |

How the pieces fit together: [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md). For licensing details and boundary rules, see [docs/OPEN_MODEL.md](../docs/OPEN_MODEL.md).

## Build

Needs the pinned Rust toolchain (rustup picks it up) and a C compiler, because `wasl-store` builds SQLite from source
(MSVC Build Tools on Windows).

```sh
cd engine
cargo build --workspace --release
```

To package ready-to-run `wasl` and `wasld` release archives with SHA-256 checksums for the host target (from `engine/`):

```sh
python3 scripts/package-release.py
```

## Tests & benchmarks

```sh
cd engine
cargo fmt --all --check
python3 scripts/check-license-boundaries.py
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --lib --bins --examples -- -D warnings -D clippy::unwrap_used
cargo test --workspace
cargo test -p wasl-ffi --all-features
cargo test -p wasl-engine --test killpoints   # a run stopped at every point of the apply path, then run again
cargo run --release -p wasl-engine --example bench
```

`[workspace.lints.rust]` forbids `unsafe_code` everywhere except `wasl-vfs-windows` (where `undocumented_unsafe_blocks = "deny"` applies), and every shipping crate root enforces `#![cfg_attr(not(test), deny(clippy::unwrap_used))]`.

The kill-point tests use a fault plan (`wasl_engine::fault`), compiled only under `cfg(test)` or the
`test-support` feature, which the engine's own tests enable; a release build has no such point. The harness is
`crates/wasl-engine/tests/common/kill.rs`: it runs a scenario once to count the points it passes, then once per
point, stopped there as if the process had died, restarts the engine on the same data and runs again.

### The previous release next to this one

`engine/scripts/mixed-version-test.sh` builds `wasld` and `wasl` from the release named in
`engine/PREVIOUS_RELEASE` (a tag, or a commit until there is a tag; it is the only place that names it), in a
separate checkout under `engine/target/previous-release`, and runs `crates/wasld/tests/mixed_version.rs`: the old
and the current daemon, as two real processes on loopback, pair with an invitation (each version making it in
turn), sync a folder both ways and remove each other, and the old command line lists the current daemon's pairs.
It fails, never skips, when the build or the binaries are missing. `build-previous-release.sh` alone builds and prints
the binary paths (`WASL_PREVIOUS_WASLD`, `WASL_PREVIOUS_WASL`) that the test reads; the build is reused while the ref
is the same. The engine workflow runs it on Linux and caches the build by the ref.

The ignore engine's comparison tests use `git` and skip themselves when it is not on `PATH`. The parsers that read
outside data (peer frames, vaults, manifests, Drive JSON, IPC) also have robustness tests and cargo-fuzz targets
(`engine/fuzz`, nightly Rust, Linux or macOS); see [docs/SECURITY.md](../docs/SECURITY.md).

## Licence

Core engine, daemon, storage, crypto, network, VFS, and FFI crates are licensed under [GNU AGPL v3.0](../LICENSE) (`AGPL-3.0-only`), with commercial dual-licences available from Kahf Labs. `wasl-proto`, `wasl-ipc`, and `wasl-cli` are licensed under [Apache License 2.0](../LICENSE-APACHE) (`Apache-2.0`) and have zero AGPL dependencies.

## Contributing, security, trademarks

- Contributing: [CONTRIBUTING.md](../CONTRIBUTING.md). Contributors sign a one-time [CLA](../CLA.md).
- Security: report problems privately, see [SECURITY.md](../SECURITY.md).
- Trademarks: [TRADEMARKS.md](../TRADEMARKS.md).
