# Wasl Architecture

> Audited against `main` commit `094d3dc` plus the shipping `unwrap_used` lint policy in this branch (`engine/Cargo.toml`, 19 workspace crates, Rust 2024 edition).
> Earlier releases ran a Kotlin-only Android engine ("Drive Sync"); that implementation is retired and replaced across every platform by the Rust workspace documented here.

Wasl is a local-first folder sync engine written in Rust. All sync planning, conflict resolution, chunking, client-side encryption, protocol handling, safety guards, and error wording live in the Rust workspace under `engine/`. Native desktop and mobile UI apps sit outside this repository and interact strictly across process IPC (`wasl-ipc`) or UniFFI (`wasl-ffi`).

## 1. System topology

```text
+------------------------------------------------------------------------------+
|  Clients (outside engine core)                                               |
|  - `wasl` CLI (Apache-2.0, in this repo: `wasl-cli`)                         |
|  - Desktop tray/window app & external scripts (via `wasl-ipc`)               |
|  - Mobile clients (via `wasl-ffi` / `libwasl_mobile` UniFFI bindings)        |
+-------------------+--------------------------------------+-------------------+
                    | Local socket / named pipe            | UniFFI (in-process)
                    | Mutual HMAC + version-range          |
                    | handshake (`wasl-ipc`, Apache-2.0)   |
                    v                                      v
+------------------------------------------------------------------------------+
|  Host & Orchestration (`AGPL-3.0-only`)                                      |
|  `wasld` (daemon: scheduler, notify watcher, OS keychain, IPC server, views) |
|  `wasl-pairs` (single source of truth for pair validation & CRUD rules)      |
|  `wasl-devices` (device identity, group DAG sync, endpoint, discovery)       |
+-------------------+--------------------------------------+-------------------+
                    |                                      |
                    v                                      v
+------------------------------------------------------------------------------+
|  Execution & Safety Boundary (`wasl-engine`)                                 |
|  - `PairLocks`: one async mutex per pair shared by Drive and P2P runs        |
|  - `Apply`: stage -> fsync -> bin copy -> atomic rename -> record baseline   |
|  - `Agent`: changeset hold, BLAKE3 saved copies, confirmation-gated accept   |
+-------------------+--------------------------------------+-------------------+
                    |                                      |
                    v                                      v
+--------------------------------------+  +------------------------------------+
|  Pure Decision Core (`wasl-core`)    |  |  Persistence (`wasl-store`)        |
|  - Three-way Drive `Planner`         |  |  - SQLite (bundled rusqlite)       |
|  - N-way Version Vectors (`nway`)    |  |  - Pre-migration `.pre-vN.bak`     |
|  - Cross-OS `name_key` (NFC+lower)   |  |  - Baselines, chunk index, bin     |
|  - Mass-delete & first-merge guards  |  |  - Append-only `oplog` tables      |
+--------------------------------------+  +------------------------------------+
                    |                                      |
     +--------------+--------------+-----------------------+--------------+
     v                             v                                      v
+-----------------+      +--------------------+                 +------------------+
| Google Drive    |      | P2P Transport      |                 | Files on Demand  |
| `wasl-drive`    |      | `wasl-net`         |                 | `wasl-vfs`       |
| `wasl-crypto`   |      | `wasl-chunk`       |                 | `wasl-vfs-windows`|
| (Vault v1/v2.1) |      | (QUIC + FastCDC +  |                 | `wasl-vfs-fuse`  |
|                 |      |  adaptive zstd)    |                 |                  |
+-----------------+      +--------------------+                 +------------------+
```

## 2. Workspace crates and licence boundary

Every crate in `engine/Cargo.toml` belongs to one of two licence tiers ([OPEN_MODEL.md](OPEN_MODEL.md)), verified on every check by `engine/scripts/check-license-boundaries.py` and `engine/deny.toml`:

| Crate | Licence | Role |
|---|---|---|
| `wasl-proto` | `Apache-2.0` | Shared DTOs, status model, retry policy, catalogue wording & redaction (`words`), op-log DTOs |
| `wasl-ipc` | `Apache-2.0` | Local socket / Windows named-pipe transport, mutual HMAC auth, version-range handshake, user-only path helpers |
| `wasl-cli` | `Apache-2.0` | Thin `wasl` CLI binary; communicates with `wasld` over `wasl-ipc` and delegates offline recovery to `wasld vault decrypt` |
| `wasl-core` | `AGPL-3.0-only` | Pure sync logic (no I/O): three-way Drive `Planner`, N-way `VersionVector` / `nway`, safety `guards`, cross-OS `identity::name_key`, `NameRules`, Google Docs export naming |
| `wasl-ignore` | `AGPL-3.0-only` | `.waslignore` and `.gitignore` matching, presets, and `--explain` diagnostics |
| `wasl-store` | `AGPL-3.0-only` | SQLite schema (14 migrations), atomic pre-migration backup/restore (`backup.rs`), pair baselines, Drive node cache, recycle bin index, device index, append-only op-log |
| `wasl-fs` | `AGPL-3.0-only` | `LocalFs` trait & `StdFs` implementation: scanner, safe atomic installs, `is_same_file` checks, local recycle bin |
| `wasl-drive` | `AGPL-3.0-only` | Google Drive v3 REST client over `rustls` (bundled roots, no redirects, gzip metadata, resumable uploads/downloads, Changes API delta cache) |
| `wasl-crypto` | `AGPL-3.0-only` | Client-side vault encryption: v1 (PBKDF2 + AES-256-GCM) and v2 / v2.1 (Argon2id + HKDF, AES-256-SIV names, keyed FastCDC + Padmé padded AES-256-GCM chunks, rollback state records) |
| `wasl-chunk` | `AGPL-3.0-only` | Two-tier FastCDC (`16/64/256 KiB` up to 16 MiB; `1/4/16 MiB` above 16 MiB), BLAKE3 content addressing, fetch planning, chunk-resumable assembly |
| `wasl-net` | `AGPL-3.0-only` | Device Ed25519 identity, PAKE-free pairing state machine, signed causal group DAG, QUIC/TLS 1.3 pinned-key transport (`quinn`), length-prefixed CBOR protocol, adaptive zstd, mDNS/UDP LAN discovery, STUN, UPnP/NAT-PMP, Drive rendezvous, UDP hole punching |
| `wasl-engine` | `AGPL-3.0-only` | Drive executor, P2P `DeviceSync` & `DeviceService`, single crash-safe `Apply` layer, `PairLocks`, retry runner, agent changeset hold & revert |
| `wasl-pairs` | `AGPL-3.0-only` | Single module owning pair validation (`Problem`), `check`/`create`/`update`/`delete`, preview-hold setting, and vault-link checks ([ADR-0001](adr/0001-pair-rules-in-one-module.md)); offer acceptance and route execution currently reside in `wasld`, `wasl-devices`, and `wasl-ffi` |
| `wasl-devices` | `AGPL-3.0-only` | High-level device lifecycle, group exchange, pairing orchestration, reachability, and shared device status computation |
| `wasl-vfs` | `AGPL-3.0-only` | Portable files-on-demand core: 1 MiB LRU block cache, pins, and sequential read-ahead `Hydrator` |
| `wasl-vfs-windows` | `AGPL-3.0-only` | Windows Cloud Files API (`CfApi`) read-only views; the sole crate permitting `unsafe` (with `undocumented_unsafe_blocks = "deny"`) |
| `wasl-vfs-fuse` | `AGPL-3.0-only` | Linux FUSE 3 (`fuser`) read-only views |
| `wasl-ffi` | `AGPL-3.0-only` | UniFFI bindings (`libwasl_mobile`) exposing engine, pairs, devices, on-demand hydration, and worded status/errors to mobile clients |
| `wasld` | `AGPL-3.0-only` | Background daemon (`wasld`), folder watcher (`notify`), scheduler, OS keychain integration (`keyring`), single-instance lock, IPC server, File Provider host bridge, and offline `wasld vault decrypt` |

**Boundary invariants:**
- `wasl-proto`, `wasl-ipc`, and `wasl-cli` have zero direct or transitive dependencies on any `AGPL-3.0-only` crate or copyleft third-party crate.
- `[workspace.lints.rust]` sets `unsafe_code = "forbid"` everywhere except `wasl-vfs-windows`.
- Every shipping crate root (`19` `src/lib.rs` and `2` `src/main.rs`) enforces `#![cfg_attr(not(test), deny(clippy::unwrap_used))]`, reinforced in CI by `cargo clippy --workspace --lib --bins --examples -- -D warnings -D clippy::unwrap_used`.

## 3. Sync paths

A **pair** has one of three routes: `drive`, `devices` (devices-only, no Google account required), or `drive_and_devices`. Both paths acquire the same per-pair lock (`wasl-engine::pair_lock::PairLocks`) so Drive sync and device pull never mutate the same folder concurrently.

### 3.1 Google Drive path (`wasl-drive`, `wasl-core::planner`, `wasl-engine::executor`)

1. **Delta metadata refresh**: `wasl-drive` maintains a local Drive node cache in SQLite updated via Google Drive's Changes API (gzip-encoded metadata responses, identity encoding for file bytes so range resume and MD5 checks stay exact).
2. **Name normalization and clash detection**: `wasl_core::identity::check_names` folds every path segment through Unicode NFC + lowercase (`name_key`, [ADR-0004](adr/0004-name-identity.md)). Two spellings of the same name on one side are flagged as a **name clash** and excluded from the plan until renamed.
3. **Pure three-way planning**: `wasl_core::Planner::plan` compares `(baseline, local, remote)` for all 7 modes (`TwoWay`, `UploadOnly`, `DownloadOnly`, `UploadMirror`, `DownloadMirror`, `UploadThenDelete`, `DownloadThenDelete`). Hashing is lazy: the planner returns `PlanOutcome::NeedHashes(paths)` only for local files whose `(size, mtime)` drifted from the baseline, then produces a deterministic `SyncPlan` (ordered `mkdirs` shallowest-first → transfers → `deletes` deepest-first).
4. **Encrypted vaults (`wasl-crypto`)**: When a pair targets a vault folder, `RemoteFs` is wrapped transparently. Vault v2.1 derives independent subkeys via Argon2id → HKDF-SHA256, encrypts folder/file names deterministically per parent via AES-256-SIV, cuts file bodies with keyed FastCDC (256 KiB / 1 MiB / 4 MiB), pads chunks into Padmé buckets, seals each chunk with AES-256-GCM bound to `(file_id, chunk_id)`, and writes per-writer MACed state records for rollback detection ([ENCRYPTION.md](ENCRYPTION.md), [ON_DEMAND.md](ON_DEMAND.md)).
5. **Google Docs export**: Native Docs/Sheets/Slides are exported as read-only Office or PDF mirrors using distinct local names whenever a real file claims the plain name.

### 3.2 Device-to-device P2P path (`wasl-core::nway`, `wasl-chunk`, `wasl-net`, `wasl-devices`, `wasl-engine::devices`)

1. **Identity & trust**: Each device generates an Ed25519 key pair stored in the OS keychain / Android Keystore (`DeviceId` = base32 of first 16 bytes of `BLAKE3(pk)`). Group membership is a signed causal DAG (`create`, `add`, `remove` ops in `wasl-net::group`). Members exchange ops over every QUIC link and (when Drive is enabled) via per-device app-data files (`group/<group>-<device>.json`). Concurrent removals fail closed (both leave) with post-signature per-signer rate limiting.
2. **Discovery & NAT traversal**:
   - **LAN**: mDNS (`_wasl._udp`, announcing only a random per-boot token and port so device identity is revealed only after pinned TLS 1.3 handshake) plus UDP broadcast fallback.
   - **Internet (`lan_and_internet`)**: STUN public address discovery, UPnP/NAT-PMP port mapping, and signed peer/offer records in Drive app-data for timed UDP hole punching. Order: LAN → IPv6 → mapped port → hole punch (strictly capped at 4 candidate addresses per attempt). Pairs with reach `lan` never touch STUN or internet endpoints.
3. **Transport**: QUIC (`quinn`) with TLS 1.3 and pinned Ed25519 certificates (no CAs, no session resumption). Both ends negotiate the highest shared protocol version from their advertised `[min, max]` range; out-of-range peers receive a typed `NeedsUpdate` error.
4. **N-way state & chunked delta transfer**: Each device maintains a monotonic `seq` and per-key `VersionVector` (90-day tombstones for deletes). Peers ask for `changes_since(seq)`. Changed files are cut with pinned `fastcdc = 3.2.1` (`16/64/256 KiB` for files ≤ 16 MiB; `1/4/16 MiB` above 16 MiB) addressed by uncompressed BLAKE3. Receivers reuse any locally present chunk in the pair's chunk table and fetch only missing chunks in parallel (8 small / 3 large in flight), with adaptive zstd compression (`level 1` LAN / `3` WAN / `9` metered, skipped on incompressible data, capped at 2 concurrent sender compressions).

## 4. Preview hold, safe apply, op-log, and revert

### Preview hold and safety guards
- Every new pair starts under a **preview hold**: neither the watcher nor the scheduler transfers anything until the user inspects the dry-run `SyncPlan` and starts the first run.
- `wasl_core::guards::evaluate` blocks any plan that deletes more than 20 files or more than 5% of a side (for batches ≥ 5 files), or merges two non-empty folders for the first time without explicit once-approval.

### Single crash-safe apply boundary (`wasl-engine::apply`)
Every local filesystem write from both the Drive executor and device pull passes through `Apply`:
1. Validate mapped local path against disk `NameRules` before downloading.
2. Write incoming bytes to a staged temp file in the target folder and call `sync_all()` (`fsync`).
3. Re-stat the target path (`as_planned`): if the file changed locally after the plan was built, abort the overwrite (`CHANGED_SINCE_PLANNED`) and leave the user's bytes untouched.
4. Copy any replaced file into the local recycle bin and index it in SQLite (30-day retention, capped at 5 local versions per key) **before** replacing the target.
5. Atomically rename the staged file over the target, then commit the updated baseline row.

A deterministic kill-point harness (`wasl-engine/tests/killpoints.rs`) injects crashes at every boundary (`AfterStage`, `AfterFlush`, `AfterBin`, `BeforeRename`, `AfterRename`, `BeforeRecord`, `AfterRowRemoved`, `AfterRemoteTrash`) and verifies zero silent data loss on restart. Database upgrades follow the same principle: `wasl-store/src/backup.rs` checkpoints WAL, writes and flushes `wasl.db.pre-vN.bak`, and atomically restores it if any migration fails.

### Append-only op-log and agent changeset hold
- `wasl-store/src/oplog.rs` maintains two append-only tables (`oplog_changesets`, `oplog_ops`, [ADR-0002](adr/0002-one-op-log.md)) keyed by `(origin_device, id)`, updated in the same SQLite transaction as `agent_changesets`.
- When an AI agent runs under `wasl agent begin` / `wasl agent run` / Claude Code hooks, Wasl snapshots the scope and copies modified files (verified by BLAKE3) before edits. While the changeset is `open` or `staged` (held), both Drive sync and P2P sync skip those keys so unreviewed edits never leave the machine.
- **Accept** (`staged` → `committed`) requires interactive confirmation in the desktop confirmation dialog via a single-use 256-bit stdin/stdout token (with exponential backoff on refusal/timeout) and rejects requests carrying agent environment markers.
- **Revert** appends inverse operations (`create` ↔ `delete`, swapping `prev`/`next`) without rewriting history, moves agent bytes to the recycle bin, restores saved copies with original mtimes, and skips any file the user subsequently edited ("you win" rule).

## 5. Architecture Decision Records

| ADR | Decision |
|---|---|
| [ADR-0001](adr/0001-pair-rules-in-one-module.md) | All pair validation rules, route execution, and offer acceptance live in `wasl-pairs`, below every client |
| [ADR-0002](adr/0002-one-op-log.md) | One append-only op-log (`oplog_changesets`, `oplog_ops`) written atomically with agent working tables |
| [ADR-0003](adr/0003-engine-words-every-error.md) | The engine error/status catalogue (`wasl-proto::words`) owns all user-facing messages and redaction in English and Arabic |
| [ADR-0004](adr/0004-name-identity.md) | Uniform cross-OS name identity (`Unicode NFC + lowercase`) with explicit clash reporting instead of silent overwrites |
