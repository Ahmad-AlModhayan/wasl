# Notice

Wasl (وصل) uses a decoupled open-core licensing split:

- The sync core, daemon, storage, cryptography, network/device sync, VFS, FFI (`wasl-ffi`), and relay/server components (`engine/` crates other than the protocol/IPC/CLI crates below) are licensed under GNU AGPL v3.0 (`AGPL-3.0-only`, [`LICENSE`](LICENSE)). Commercial dual-licences are available from Kahf Labs.
- The wire protocol (`wasl-proto`), local IPC client (`wasl-ipc`), and thin CLI (`wasl-cli`) are licensed under Apache License 2.0 (`Apache-2.0`, [`LICENSE-APACHE`](LICENSE-APACHE)) with zero AGPL dependencies.

Required Notice: Copyright Ahmad AlModhayan, trading as Kahf Labs (https://kahflabs.com)

Dependencies keep their own licences (see `engine/Cargo.lock`, `engine/deny.toml`, and `site/package-lock.json`). The names Kahf, Wasl, Qasd and Miqyas are covered by [TRADEMARKS.md](TRADEMARKS.md), not by the licence.

## Third-Party & Asset Notices

- Fonts under `docs/design/project/fonts` are licensed under the SIL Open Font License 1.1 with each font's own notice.
