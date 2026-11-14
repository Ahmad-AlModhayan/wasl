# How Wasl is licensed and offered

Wasl uses a **decoupled open-core** model that separates the verifiable sync engine and self-hostable relay code (`AGPL-3.0-only`) from the wire protocol, IPC client, and thin CLI (`Apache-2.0`), and keeps proprietary native UI clients and hosted relay services separate from this public repository.

## Code licences in this repository

| Layer | Crates / Paths | Licence | Why |
|---|---|---|---|
| **Core engine, daemon & FFI** | `wasl-core`, `wasl-store`, `wasl-crypto`, `wasl-chunk`, `wasl-drive`, `wasl-fs`, `wasl-ignore`, `wasl-engine`, `wasl-pairs`, `wasl-net`, `wasl-devices`, `wasl-vfs`, `wasl-vfs-windows`, `wasl-vfs-fuse`, `wasl-ffi`, `wasld`, and self-hostable relay components | GNU Affero General Public License v3.0 ([`AGPL-3.0-only`](../LICENSE)) | Copyleft keeps sync planning, cryptography, chunking, and relay behaviour auditable while preventing third parties from wrapping the engine or relay into a proprietary SaaS without complying with AGPL-3.0-only §13 or obtaining a commercial licence. |
| **Wire protocol, IPC client & CLI** | `wasl-proto`, `wasl-ipc`, `wasl-cli` | Apache License 2.0 ([`LICENSE-APACHE`](../LICENSE-APACHE)) | Permissive licence lets power users, homelabbers, Linux admins, and third-party tools script `wasl`, speak the local IPC protocol, and build integrations without copyleft obligations. |
| **Native UI clients & managed hosted relay** (separate products) | Outside this repository | Proprietary (from Kahf Labs) | Native Android/desktop UI polish, OS shell integrations, and Kahf-operated high-speed TURN relay / blind-cache servers monetize hosted infrastructure and UX without gating local sync engines. |

## Strict dependency boundary

Four rules keep the `Apache-2.0` interface crates free of `AGPL-3.0-only` obligations:

1. **Zero AGPL dependencies in `Apache-2.0` crates.** `wasl-proto`, `wasl-ipc`, and `wasl-cli` declare `license = "Apache-2.0"` explicitly in their `Cargo.toml` manifests and depend on no `AGPL-3.0-only` workspace crate (normal, build, or dev dependencies).
2. **Process / socket isolation.** `wasl-cli` (`Apache-2.0`) talks to `wasld` (`AGPL-3.0-only`) over the user-only local socket using `wasl-ipc` / `wasl-proto` types, and invokes `wasld vault decrypt` out-of-process for offline cryptographic recovery. `wasl-ipc` receives platform ACL checks from callers such as `wasld` rather than importing `wasl-vfs-windows`.
3. **CI verification.** `engine.yml` inspects `cargo metadata --format-version 1` on every CI run and fails if any `Apache-2.0` crate depends on any `AGPL-3.0-only` crate or if any crate fails to declare `AGPL-3.0-only` or `Apache-2.0`. `engine/deny.toml` continues to forbid strong-copyleft third-party dependencies across the workspace.
4. **Separate licence files.** Root [`LICENSE`](../LICENSE) carries GNU AGPL v3.0 (`AGPL-3.0-only`) and [`LICENSE-APACHE`](../LICENSE-APACHE) carries Apache License 2.0 (`Apache-2.0`). Notices live in [`NOTICE.md`](../NOTICE.md).

## Pricing model (no Wasl Pro version)

Local-first users should not pay subscriptions for software running on their own hardware, so **Wasl has no Pro tier**:

- **Free everywhere locally & P2P:** Unlimited folder pairs and devices, Google Drive sync, direct LAN and internet P2P sync, client-side encrypted vaults, 30-day undo/recycle bin/versioning, `.waslignore` and presets, desktop files on demand, Android battery/network scheduling, and full CLI access are all free.
- **Managed Relay & Blind-Cache Tier (optional subscription):** Direct P2P fails behind strict carrier NATs or when devices are not online together. Rate-limited public STUN/discovery nodes remain free (and relay servers can be self-hosted under `AGPL-3.0-only`); Kahf offers an optional paid subscription only where Kahf incurs server bandwidth and encrypted blind-cache storage costs.
- **Commercial Dual-Licensing (B2B):** Companies that want to embed `wasl-core` inside proprietary products or services without complying with `AGPL-3.0-only` may purchase a commercial licence from Kahf Labs.

## Governance & contributions

- Every external pull request requires signing [`CLA.md`](../CLA.md) so Kahf Labs retains the right to offer B2B commercial dual-licences and distribute official native clients across the FFI/IPC boundary.
- Maintainership and merge rights on core crates are restricted to protect deterministic conflict resolution and OS-specific filesystem edge cases.
- New automation integrations belong on top of local IPC event hooks rather than bloating `wasl-core`.

Questions or commercial dual-licensing inquiries: `hello@kahflabs.com`.
