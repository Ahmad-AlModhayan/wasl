# Wasl (وصل) · Software Requirements Specification

| | |
|---|---|
| Version | 1.0 (2026-10-19) |
| Status | Specifies Wasl 2 (Super Sync). Android 1.1.4 (Kotlin engine) is the behavioural baseline |
| PRD | `docs/PRD.md` |

## 1. Introduction

### 1.1 Purpose
Specifies the shared Rust engine `libwasl`, its two transports (Google Drive and direct device sync), ignore rules, safety layer, files on demand, encryption vaults, and the Android and desktop applications built on it.

### 1.2 Scope
In scope: Android 11+, Windows 10/11, macOS 13+, Linux; Google Drive; direct sync between a user's own devices on the local network and across the internet. Out of scope: other clouds, iOS, any Kahf-operated server, syncing dependency or build environments.

### 1.3 Baseline rule
Every behaviour the Android 1.1.4 app has today (described in `docs/ARCHITECTURE.md`, `docs/USER_GUIDE.md`, `docs/AUTOMATION.md`) is a requirement unless this document changes it. Its JVM tests are ported to the Rust engine before the Kotlin engine is retired.

### 1.4 Definitions
| Term | Meaning |
|---|---|
| Device | One installation of Wasl, identified by an Ed25519 key pair generated on first run |
| Device group | The set of devices the user has paired with each other; trust is per group, per Google account |
| Pair | A local folder synced with a Drive folder and/or the same pair on other devices, with a method, route, reach, conditions and ignore rules |
| Route | Where a pair syncs: `drive`, `drive_and_devices`, `devices` |
| Reach | Where device sync may connect: `lan` (local network only) or `lan_and_internet` |
| Method | One of the seven sync modes |
| Baseline | The last synced state of each file in a pair, per side, committed per file |
| Key | A file's canonical path relative to the pair root |
| Version vector | Map `device_id → counter` recording which edits a file version includes |
| Chunk | A content-defined block of a large file, identified by its BLAKE3 hash |
| Rendezvous | The exchange of connection details between two devices through files in the user's Drive app-data folder |
| Vault | A passphrase and keys derived from it; encrypts a pair's Drive copy |

## 2. Overall description

### 2.1 Product perspective
```
 Android app (Kotlin/Compose UI)        Desktop: wasld daemon + tray app + `wasl` CLI
 WorkManager, SAF/All files, provider   Autostart, Cloud Files API / File Provider / FUSE
              \                                   /
               +------ UniFFI ------+---- IPC ---+
                                    |
                          libwasl (Rust core)
  scanner + watcher -> ignore engine -> planner (pure) -> safety guards -> executor
          |                                   |
     SQLite baseline + version vectors        |
                                    +---------+----------+
                                    |                    |
                           Drive transport        Device transport
                        (REST v3, Changes API)   (QUIC; LAN discovery;
                                                  rendezvous via Drive app data)
```
No Kahf server. Network peers: Google APIs, the user's own devices, and optionally public STUN servers (configurable, used only to learn a device's public address).

### 2.2 User classes
Device owner (all features); automation tools (Android intents with secret; desktop CLI under the same OS user); third-party apps reading files on demand through the OS.

### 2.3 Operating environment
- Android 11 (API 30) to current; target the latest required API level.
- Windows 10 1809+ and 11 (x64, ARM64); macOS 13+ (Apple silicon, Intel); Linux x86_64 and aarch64, glibc 2.31+, FUSE 3 for files on demand.

### 2.4 Constraints
- Restricted OAuth scope `https://www.googleapis.com/auth/drive`. Android: Play services Authorization API. Desktop: OAuth installed-app flow with loopback redirect and PKCE; tokens in the OS keychain (Windows Credential Manager, macOS Keychain, Secret Service).
- Google Drive stores whole files: no partial-file updates through the API.
- Drive `appProperties`: at most 30 per app per file, each key plus value at most 124 bytes.
- Android background limits: WorkManager 15-minute floor; data-sync foreground service capped at 6 h per 24 h; network restricted in Doze.
- No ads, analytics, crash SDKs or telemetry; no Kahf-operated server.

## 3. Functional requirements

Priority: **M** must, **S** should, **C** could. Milestones refer to PRD §8.

### 3.1 Engine (`libwasl`)
| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-E1 | One Rust workspace builds the engine for `aarch64-linux-android`, `x86_64-linux-android`, `x86_64/aarch64-pc-windows-msvc`, `aarch64/x86_64-apple-darwin`, `x86_64/aarch64-unknown-linux-gnu` | M | CI builds all targets on every pull request |
| FR-E2 | The planner is a pure function: (baseline, local tree, remote trees, pair config) → plan. No I/O, no clock, no randomness | M | Property tests with `proptest` |
| FR-E3 | Platform code contains no sync decisions; it supplies file access, watchers, scheduling, credentials and UI | M | Code review rule; engine API is the only path to a plan |
| FR-E4 | The engine exposes a typed API through UniFFI (Kotlin) and a local IPC protocol (desktop daemon) with the same operations: pairs CRUD, preview, sync, pause, status stream, problems, history, recycle bin, versions, devices | M | Contract tests run against both bindings |
| FR-E5 | The ported Kotlin planner, glob, name-mapping and vault tests pass against the Rust engine | M | Test suite parity report before M2 ships |

### 3.2 Pairs, methods and conditions
| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-P1 | Create, edit, pause, resume and remove pairs; a pair binds a local folder, optionally a Drive folder (My Drive or shared drive), and optionally the same pair on other devices in the group | M | |
| FR-P2 | Seven methods: two-way, upload only, upload then delete, upload mirror, download only, download then delete, download mirror. On device routes, "upload" means from this device, "download" means to this device | M | Planner tests per method and route |
| FR-P3 | Route per pair: `drive`, `drive_and_devices`, `devices`. Reach per pair: `lan`, `lan_and_internet` | M | |
| FR-P4 | Conditions per pair: Wi-Fi only, Wi-Fi name allow/deny, metered, roaming, VPN, charging, battery floor; manual **Sync now** overrides them, automation does not | M | |
| FR-P5 | Conflict policy per pair: keep both (default), newer wins, this device wins, other side wins, skip and report | M | |
| FR-P6 | Size limit per pair (default 2 GB on Android, none on desktop); selective sync of subfolders | M | |
| FR-P7 | Schedule: instant (watchers plus remote polling) with a periodic fallback; global default 60 min on Android | M | |

### 3.3 Change detection and planning
| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-C1 | Local scan in parallel; content change detected by size and modification time, confirmed by BLAKE3 | M | 100,000-file scan within NFR-3 |
| FR-C2 | Watchers: `ReadDirectoryChangesW` (Windows), FSEvents (macOS), inotify/fanotify (Linux), FileObserver and MediaStore (Android); events are hints, a periodic scan is the source of truth | M | Missed-event test still syncs on the next scan |
| FR-C3 | Drive changes through the Changes API with a stored page token; one metadata cache per account; polling, no push webhooks | M | |
| FR-C4 | Every file version carries a version vector. On Drive, the vector is stored in the file's `appProperties` (`wasl.vv`, compact encoding); when it does not fit in 124 bytes, `wasl.vv` holds a hash that points to an entry in the app-data index file | M | Round-trip tests at the size limit |
| FR-C5 | Three-way and N-way comparison: a key is in conflict only when neither version vector dominates the other and the content hashes differ; identical content with concurrent vectors merges without a conflict copy | M | Convergence property tests with 3 devices and Drive |
| FR-C6 | A file received from a device and already present on Drive with the same BLAKE3 (stored in `appProperties`) or MD5 is marked synced without a transfer | M | Route `drive_and_devices` test: no duplicate download |
| FR-C7 | Name mapping: characters a platform rejects (`:*?"<>|` on Windows, Android FAT rules) map reversibly; case-only collisions between case-sensitive and case-insensitive systems are reported, never silently merged. Names are compared case- and normalisation-insensitively (NFC, then lower case) on every device (ADR-0004) | M | Cross-OS round-trip tests |
| FR-C8 | Drive duplicates (same name in one folder) and shortcuts are reported, not synced | M | |
| FR-C9 | Google Docs, Sheets and Slides export as read-only Office or PDF mirrors, re-exported on change, never uploaded | M | |

### 3.4 Ignore rules
| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-G1 | `.waslignore` files at any depth, with gitignore syntax and semantics (negation, anchoring, directory-only rules, nearest file wins) | M | Fixture trees compared with `git check-ignore` output |
| FR-G2 | Option per pair to honour `.gitignore` files | M | |
| FR-G3 | Presets per pair: Node, Python, Rust, Android/Gradle, Flutter, Docker, editors (`.idea`, `.vscode` caches), OS junk (`.DS_Store`, `Thumbs.db`, `desktop.ini`) | M | |
| FR-G4 | Ignored items are never uploaded, downloaded, transferred between devices or deleted on any side | M | |
| FR-G5 | Adding a rule never deletes copies that already exist elsewhere; removal from remote sides only through an explicit "Remove ignored from other sides" action with preview | M | |
| FR-G6 | Ignored-items view: folders and files with counts, sizes and the rule (file and line) that matched; same data from `wasl ignore --explain <path>` | S | |
| FR-G7 | `.waslignore` files sync like normal files so every device applies the same rules | M | |
| FR-G8 | The include/exclude glob patterns of 1.1.4 keep working and are shown as pair-level rules | M | Migration test |

### 3.5 Drive transport
| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-D1 | Drive REST v3 client in Rust (HTTPS, rustls): list, changes, get, resumable upload with persisted sessions, ranged download, export, revisions, shared drives, app-data folder | M | Recorded-response tests |
| FR-D2 | On normal pairs Drive holds real, whole files with their real names, so they stay usable in Drive's own apps; Wasl metadata lives only in `appProperties` and the app-data folder | M | A file synced by Wasl opens normally in Drive on the web |
| FR-D3 | Uploads and downloads verify MD5 (Drive) and BLAKE3 (Wasl) | M | |
| FR-D4 | 403/429 and quota errors back off with truncated exponential delay and show the time of the next attempt | M | Fault-injection tests |
| FR-D5 | Multiple Google accounts; each pair belongs to one account; a device group belongs to one account | S | |

### 3.6 Device transport
| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-N1 | Each device has an Ed25519 identity key created on first run and kept in the OS keystore (Android Keystore-wrapped, OS keychain on desktop) | M | |
| FR-N2 | Pairing: the existing device shows a QR code (or 8-character code) holding its public key, its device ID and a one-time secret; the new device connects, both show the same four confirmation words derived from both keys; on confirmation each adds the other to the group list | M | Wrong code or mismatched words never add a device |
| FR-N3 | The group list (device IDs, names, public keys) is stored in the Drive app-data folder, signed by the device that last changed it; devices accept only lists signed by a key already in their trusted set | M | Tampered list in Drive is rejected |
| FR-N4 | Transport: QUIC (quinn) with TLS 1.3; each side authenticates the other's Ed25519 key pinned from the group list; no certificate authorities | M | Connection from a key outside the group is refused |
| FR-N5 | LAN discovery by mDNS/DNS-SD (`_wasl._udp`), announcing only a random per-boot token; the device ID is revealed only after a successful authenticated handshake | M | Packet capture shows no device name or ID in announcements |
| FR-N6 | Rendezvous for reach `lan_and_internet`: each device publishes a signed, short-lived candidate file in the Drive app-data folder (`peers/<device_id>.json`: LAN addresses, IPv6 addresses, public address learned through STUN, UPnP/NAT-PMP mapped port if obtained, expiry). To connect, a device writes a signed `offers/<from>-<to>.json` with a start time; the other device, polling its offers while active, replies; both start simultaneous UDP sends at the agreed time to open the path (hole punching) | M | Connection succeeds between two home routers with endpoint-independent NAT |
| FR-N7 | Candidate order: same LAN → IPv6 direct → mapped port → hole punching. STUN servers are a configurable list of public servers; no Kahf STUN | M | |
| FR-N8 | When no direct path exists: route `drive_and_devices` continues through Drive with no user action; route `devices` waits, shows "Can't reach <device> from this network", and accepts a user-supplied address (Tailscale/WireGuard IP or a self-hosted relay URL) | M | |
| FR-N9 | Chunked transfer between devices: files above 16 MB are split by FastCDC (1 MB min, 4 MB average, 16 MB max); the receiver requests only chunks it does not have (from any version of any file in the pair); chunks verified by BLAKE3; resume at chunk granularity | M | 1 MB change in a 10 GB file moves < 32 MB |
| FR-N10 | Files are written to a temporary file in the target folder and atomically renamed after verification | M | Kill mid-transfer leaves no partial target file |
| FR-N11 | Reach `lan` never attempts rendezvous, STUN or any internet connection for that pair | M | Packet capture test |
| FR-N12 | Android joins device sync while the app is open, while charging on Wi-Fi (if enabled), and during a user-started foreground session; outside those, Android pairs with route `devices` show "Waiting for <device>" | M | |

### 3.7 Safety
| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-F1 | Preview (dry run) shows every upload, download, device transfer and deletion with sizes, per side | M | Preview equals the executed plan for the same inputs |
| FR-F2 | First sync of a pair whose sides are all non-empty stops and asks unless started from the preview | M | |
| FR-F3 | Mass-delete guard: a plan deleting more than 20 files, or at least 5 files that are more than 5% of a side, stops on every device until approved once; a plan that would empty a side that was synced before always stops; thresholds in settings | M | Boundary tests |
| FR-F4 | Conflicts keep both copies: the losing local copy becomes "name (conflicted copy from <device>, <date>).ext"; Keep this / Keep other moves the other copy to the bin, with Undo | M | |
| FR-F5 | Local deletions and overwrites move to the Wasl recycle bin on that device (reflink copy where the file system supports it, else move); Drive deletions go to Drive's bin and are listed in Wasl's bin; retention 30 days; up to 5 overwritten versions kept per file locally | M | Byte-identical restore from each side |
| FR-F6 | Versions view for any file: local recycle-bin versions and Drive revisions, with restore and keep-forever | M | |
| FR-F7 | A device never deletes on another device or on Drive what it did not have in its own baseline. Exception: the mirror methods (upload mirror, download mirror) remove extras on their target side by definition; those deletions go to the bin and the mass-delete guard (FR-F3) still applies | M | Property test |

### 3.8 Background execution and status
| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-B1 | Android: periodic WorkManager sync, expedited one-shot runs, optional instant-sync foreground service, re-arm after boot and update | M | |
| FR-B2 | Desktop: `wasld` starts at login (Windows Task Scheduler or service, macOS LaunchAgent, systemd user unit) and runs without a window | M | |
| FR-B3 | Every run commits the baseline per file or chunk; interrupted runs resume without repeating finished work | M | Kill tests on every platform |
| FR-B4 | A run with no progress for 15 minutes is abandoned and retried; a change made during a run triggers one more run | M | |
| FR-B5 | Status always visible: Android persistent notification; desktop tray icon with states syncing, up to date, paused, waiting, needs you; both show per-pair detail | M | |
| FR-B6 | Problems list with a plain-words fix per problem; run history with per-file events, kept 90 days; log export | M | |
| FR-B7 | Diagnostics: permissions, battery and background restrictions (Android, per maker), watcher health, Drive connectivity, device reachability per peer and the path used (LAN, direct, hole punched, none) | M | |
| FR-B8 | Pair status on every platform (desktop app, Android; `wasl status` with FR-A3), worked out once by the engine. Each Home row shows one line, e.g. "Up to date · MacBook online · PC offline, seen 2 h ago"; the pair screen shows the full panel. (1) The pair: Up to date, Syncing (N files), N waiting, Paused, or Needs you (the FR-B5 states); each file and folder in the pair's file list: Synced, Waiting, Conflict, or Held (kept back by an agent session, FR-AG2). (2) This device, each case with its own words and fix: "No network", "Can't reach Drive", "Sign in again". (3) One line per other end the route uses: Drive for `drive` and `drive_and_devices`; each other device for `devices` and `drive_and_devices`. A device line reads "Online, via LAN / direct / hole punched"; "Offline, last seen <time>"; "Can't reach <device> from this network" when the device is announced but no path opens (FR-N8), followed on `drive_and_devices` by "Syncing through Drive"; or "Waiting for <device>" with FR-N12's reason for an Android device outside its sync windows. (4) Per device, whether this device's changes reached it: "All your changes reached <device>" or "N changes waiting for <device>". A receipt makes it: after applying changes from a link, the receiving device sends back, over that link, the newest version it holds of each changed key; the sender counts a change as reached once a receipt covers its version. Devices that never meet on a link show "Not confirmed" instead | M (CLI: S, as FR-A3) | With a fake network, Drive and peer, each state in (1)–(4) appears on the Home row, the pair screen and `wasl status`; a change made on A shows "1 change waiting for B" while B is off and "All your changes reached B" after B's receipt arrives |

### 3.9 Files on demand
| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-O1 | Android: DocumentsProvider exposing Drive and vaults, block streaming, offline pins (exists in 1.1.4) | S | |
| FR-O2 | Windows: Cloud Files API placeholders (the OneDrive mechanism) for on-demand pairs; hydrate on open | S | Open a 20 GB file and seek within 2 s on a 100 Mbit link |
| FR-O3 | macOS: File Provider extension | S | |
| FR-O4 | Linux: FUSE 3 mount | S | |
| FR-O5 | Pins keep files offline; unpinned content in an LRU cache (default 2 GB mobile, 20 GB desktop) | S | Eviction never removes pinned items |

### 3.10 Encryption vaults
| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-V1 | Vault v1 (`docs/ENCRYPTION.md`) remains readable and writable | M | v1 test vectors pass in Rust |
| FR-V2 | Vault v2: Argon2id key derivation (parameters stored in `vault.json`); names encrypted with AES-256-SIV; contents split into FastCDC chunks, each encrypted with AES-256-GCM bound to the file ID and the chunk's own keyed content ID, their order authenticated by the file's encrypted manifest (design decision 2026-10-21: binding to the chunk index would make an insert re-upload the rest of the file; docs/ON_DEMAND.md §1.2); chunks stored on Drive as separate objects under the vault folder, so edits upload only changed chunks and files on demand can seek | S | Seek at 4 GB decrypts only the target chunk; 1 MB change re-uploads < 32 MB |
| FR-V3 | An open recovery tool (Rust CLI plus the Python reference) decrypts v1 and v2 vaults offline | S | Cross-check in CI |
| FR-V4 | The passphrase is never stored remotely or in backups; losing it loses the data, and the UI says so | M | |

### 3.11 Automation and entry points
| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-A1 | Android intents SYNC_ALL, SYNC_PAIR, PAUSE_ALL, RESUME_ALL, CANCEL, RESCHEDULE, only with **Allow automation** on and the per-install secret | S | |
| FR-A2 | Android tile, widget, shortcuts, share target | S | |
| FR-A3 | Desktop CLI: `wasl status`, `wasl sync [pair]`, `wasl pause|resume`, `wasl pair add|list|remove`, `wasl device add|list|remove`, `wasl ignore --explain`, `wasl pin`; talks to `wasld` over a socket restricted to the current OS user | S | |

### 3.12 Backup, migration and accounts
| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-K1 | Pairs and settings back up to Drive app data daily and to a file; restore on a new device; secrets and private keys never included | S | |
| FR-K2 | Android 1.1.4 → Wasl: import pairs, baselines, recycle bin, history and vaults from the Drive Sync database on first run of the new app | M | Upgrade test from real 1.1.4 data |

### 3.13 Identity, localisation and release
| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-R1 | Name Wasl / وصل everywhere; app ID `com.kahflabs.wasl`; store listing name "Wasl Sync" / «وصل للمزامنة» | M | |
| FR-R2 | English and Arabic, complete RTL mirroring, Readex Pro for both scripts on Android (design decision 2026-10-22); the desktop sets in the Kahf family's IBM Plex Sans and Plex Sans Arabic (desktop redesign, 2026-11-01, #60) | M | Screenshot tests in both languages |
| FR-R3 | Android storage modes: Storage Access Framework (default) and All files access (optional), per pair | M | Test suite passes in both modes |
| FR-R4 | Privacy policy and terms on the Wasl site, linked in the apps | M | |
| FR-R5 | Signed installers built with the Tauri bundler: MSIX (Windows), notarised DMG (macOS), AppImage and .deb (Linux); in-app update check against the Wasl site | M | |
| FR-R6 | Free and Pro feature gating: Free allows 3 pairs, LAN device sync and all safety features; Pro unlocks unlimited pairs, internet device sync, vaults, multiple accounts and desktop files on demand. Safety features are never gated | M | Gating tests per feature |
| FR-R7 | Licence without a Kahf server: a licence is a signed token the app verifies offline with the embedded public key. A verified licence is recorded in the signed `group.json`, so every device in the user's group gets Pro | M | Forged or edited licence rejected; new device in a Pro group is Pro |

### 3.14 Agent changes (docs/AGENT_CHANGES.md)
| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-AG1 | An agent session (begin … end) in a folder of a pair records the scope's file list at begin and copies every file there not yet synced; `wasl agent save` copies a file before an agent writes it (first copy per session wins) | M | Earlier version restorable for synced, unsynced and pre-saved files |
| FR-AG2 | While a session is open and after it ends until decided, sync leaves every changed key of the scope alone on every route: no upload, download or deletion; Drive and other devices keep the earlier versions. Files Wasl itself wrote during the session are not the agent's | M | Held keys never reach Drive or peers; a download during a session is not attributed |
| FR-AG3 | Accept releases the keys and syncs at once; revert restores every entry (deepest first) with its old modification time, moves the agent's versions to the recycle bin and skips files changed after the session | M | Revert leaves the folder byte-identical and the next sync with nothing to do |
| FR-AG4 | Sessions in overlapping folders of one pair are refused; a session id resumes its open session | M | |
| FR-AG5 | `wasl agent run`, `begin`, `save`, `end`, `list`, `show --diff`, `accept`, `revert`; Claude Code hooks (`wasl agent hooks claude-code`) that never block the agent and never write to its stdout | M | Hook exits 0 with wasld stopped or bad input |

### 3.15 Op-log (docs/adr/0002-one-op-log.md)
One record of every actor's changes, for every reader. Out of scope: syncing changesets between devices (#53) and deciding for another device (#55).

| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| FR-OL1 | Schema v11 adds `oplog_changesets` and `oplog_ops` (§6); `wasl-store`'s `oplog` module is their only writer. It records only changes made under an actor (FR-OL2); changes through Drive or from another device's sync have no actor and are not recorded | M | A v10 database migrates to v11 with its data unchanged; a sync that changes files writes no op-log rows |
| FR-OL2 | An actor is `agent:` followed by `[a-z0-9._-]{1,64}` (exactly what `actor_id` accepts) or `person:<device id>`; anything else is refused. The device id is this device's `DeviceId` (`WASL-XXXX-…`) from `DeviceKey::id()` in `wasl-net` (key seed in the OS keychain, `device-key`), passed in by the engine; `origin_device` uses the same form | M | Valid and invalid actors, including names with `.` and `_`, each refusal a typed error |
| FR-OL3 | An entity URI is `file://<pair id or watch id>/<key>` with each segment percent-encoded; a folder scope ends in `/`. Other schemes are stored and returned byte for byte, never parsed | M | Keys with spaces, `#`, `%` and Arabic names round-trip; an `app://` URI comes back unchanged |
| FR-OL4 | Changeset moves: new → `open`; `open` → `staged`, `committed`, or deleted when it has no operations; `staged` → `committed` or `discarded`; `staged` or `committed` → `reverted` or `partly_reverted`; `partly_reverted` → `reverted`, or `partly_reverted` again when a later revert still leaves files skipped (only outcomes change). Every other move is refused with a typed error. Each accept, discard or revert sets `decided_by` (the client) and `decided_actor` (FR-OL2: the agent that asked, else `person:<device id>`). Operations are appended only while `open`, plus a revert's inverse operations; `seq` runs 1, 2, 3 … without gaps | M | Property test: random stage, append, commit and revert sequences only move as allowed; one test per refused move; a second partial revert keeps `partly_reverted` and changes only outcomes; an agent's revert records that agent as `decided_actor` |
| FR-OL5 | Append-only: rows are never deleted or rewritten; only `status` and `outcome` move, forward only (empty → any; `applied` → `skipped_changed`, `failed`, `reverted`; `skipped_changed` or `failed` → `reverted`). While the changeset is `staged`, `committed` or `partly_reverted`, `skipped_changed` and `failed` may be decided again (to `skipped_changed`, `failed` or `reverted`), since a resumed or later revert runs them again; once it is `reverted`, every outcome is final. A revert appends one inverse operation per operation it takes back (`reverts` = its `seq`, `prev` and `next` swapped) and marks the original `reverted` in the same transaction. The one-time v10 copy (FR-OL7) is the only write that skips these checks | M | Row count never drops in the property test; reverting N operations appends N rows; a revert cut short after a `failed` entry resumes and decides it again |
| FR-OL6 | A `committed`, `discarded`, `reverted` or `partly_reverted` changeset is pruned with its operations 90 days after it was decided; `open` and `staged` are never pruned. The record holds no file contents; contents follow their own windows (recycle bin 30 days, agent undo store 30 days, UND-11) | M | Clock-advance test |
| FR-OL7 | Every change to `agent_changesets.status`, and every entry outcome write (`set_entry_outcome`), writes the op-log in the same transaction: `open` → `open`, `held` → `staged`, `accepted` → `committed`, `reverted` and `partly_reverted` alike; one operation per entry on `file://<pair id>/<key>` (the entry's `key`, not its local `rel`) with its before and after (size, modification time, BLAKE3 where known). Changesets from before v11 are copied once, as an import: each with its final status and outcomes as they are, skipping the move and outcome checks, marked with `imported_at` | M | FR-AG tests pass unchanged; an accepted session is a committed changeset with one operation per entry; full and partial reverts; a crash mid-revert leaves both records at the same entry; copy from a v10 database with held, accepted, reverted and partly reverted changesets keeps their status and outcomes |
| FR-OL8 | Read-only access: `wasld` IPC and `wasl oplog list [--actor] [--status] [--since] [--pair] [--json]`, `wasl oplog show ID [--json]`; JSON as in ADR-0002 with a `version` field. No accept, revert or discard through it | M | CLI snapshot tests (text and JSON); IPC round trip; a test fails when a JSON field is removed or renamed |

### 3.16 Agent undo (docs/AGENT_UNDO.md)
Snapshot, file journal and restore for any folder an agent can reach, synced or not. UND-1–UND-14 in `docs/AGENT_UNDO.md` §5 are requirements of this SRS, with their priority (P0 as M, P1 as S) and acceptance tests. Agent undo's tables are schema v12 (§6); its journal writes each operation through `wasl-store`'s `oplog` module (§3.15, UND-9). In a pair folder FR-AG1–AG5 apply unchanged (UND-8).

## 4. Non-functional requirements

| ID | Category | Requirement |
|---|---|---|
| NFR-1 | Data safety | Zero unrecoverable deletions or overwrites; atomic replacement; every transfer verified by hash |
| NFR-2 | Reliability | Survives process death, reboot, sleep, network changes and the Android 6-hour cap without repeating or skipping work |
| NFR-3 | Performance | Scan 100,000 unchanged files: < 2 s desktop SSD, < 6 s mid-range phone. Plan 100,000 keys: < 2 s desktop, < 5 s phone. LAN throughput ≥ 80% of link speed on gigabit. Engine idle memory < 40 MB desktop |
| NFR-4 | Battery | Android idle cost < 1% battery per day with instant sync on |
| NFR-5 | Privacy | No data to anyone except Google APIs, the user's own devices and configured STUN servers (which see only an IP address). No telemetry. Play Data safety: no data collected or shared |
| NFR-6 | Security | Device keys in OS keystores; pinned-key QUIC; signed rendezvous and group files; automation secret on Android; CLI socket limited to the OS user; dependencies audited (`cargo audit`, `cargo deny`) in CI; vault crypto per spec with test vectors |
| NFR-7 | Accessibility | Screen-reader labels (TalkBack, Narrator, VoiceOver, Orca), keyboard navigation on desktop, targets ≥ 48 dp / 24 px, text scaling to 200%, WCAG 2.2 AA contrast |
| NFR-8 | Localisation | English and Arabic in every surface, including notifications, tray menus, CLI messages and errors |
| NFR-9 | Design | Kahf family design system with Wasl's accent on every platform |
| NFR-10 | Maintainability | Engine crates have unit and property tests on all planning, ignore, chunking and crypto code; coverage of changed lines reported on every pull request |

## 5. External interfaces

- Google Drive API v3; Google OAuth (Android Authorization API; desktop installed-app flow with PKCE).
- QUIC between devices; mDNS/DNS-SD on the LAN; STUN (RFC 8489) to public servers; UPnP-IGD and NAT-PMP/PCP for optional port mapping.
- OS: Android WorkManager, foreground services, SAF, DocumentsProvider, MediaStore; Windows Cloud Files API, Credential Manager, Task Scheduler; macOS File Provider, Keychain, LaunchAgents, FSEvents; Linux FUSE 3, Secret Service, systemd, inotify/fanotify.
- Recovery tool for vaults (open source).

## 6. Data model (SQLite through `rusqlite`, one schema on every platform)

- `devices(device_id, name, os, public_key, added_at, last_seen, last_path)`
- `accounts(id, email, change_page_token)`
- `pairs(id, name, account_id, local_root, drive_folder_id?, route, reach, method, conflict_policy, conditions_json, size_limit, docs_format, vault_id?, gitignore, presets_json, paused)`
- `pair_devices(pair_id, device_id)`
- `file_state(pair_id, key, side, is_dir, size, mtime_ns, blake3, md5?, version_vector, drive_id?, drive_version?, local_name?)` (baseline per side)
- `chunks(blake3, size, refcount)` and `file_chunks(pair_id, key, idx, offset, blake3)`
- `ignore_hits(pair_id, path, source_file, line, bytes, files)` (cache for the ignored-items view)
- `runs(id, pair_id, trigger, route_used, status, stats_json, started_at, ended_at)` and `run_events(run_id, level, key, message)`
- `conflicts(pair_id, key, versions_json, status)`
- `trash(id, pair_id, side, key, reason, location, version_no, deleted_at, expires_at)`
- `drive_nodes(account_id, id, parent_id, name, mime, md5, version, app_properties_json)`
- `upload_sessions(pair_id, key, uri, offset)`; `pins(doc_id, ...)`; `vaults(id, name, format_version, kdf_params_json, key_check)`; `blocked_plans(pair_id, reason, summary_json)`
- `oplog_changesets(origin_device, id, actor, label?, scope, status, opened_at, closed_at?, decided_at?, decided_by?, decided_actor?, imported_at?)` (v11; primary key (`origin_device`, `id`), since ids are unique only per device; status `open`, `staged`, `committed`, `discarded`, `reverted`, `partly_reverted`; `id` equals the `agent_changesets` id it records; `decided_by` the client, `decided_actor` an actor (FR-OL2); `imported_at` only on the v10 copy)
- `oplog_ops(origin_device, changeset_id, seq, entity_uri, op, prev?, next?, reverts?, outcome?, at)` (v11; primary key (`origin_device`, `changeset_id`, `seq`), referencing `oplog_changesets(origin_device, id)`; `prev` and `next` JSON; outcome `applied`, `discarded`, `skipped_changed`, `failed`, `reverted`)
- `agent_watches`, `agent_journal`, `agent_markers`, `agent_restores` (v12, agent undo; columns in `docs/AGENT_UNDO.md` §6); `agent_changesets.pair_id` becomes optional, with `watch_id`

Drive app-data folder: `group.json` (signed device list and licence), `peers/<device_id>.json` (signed candidates, expiry ≤ 10 min), `offers/<from>-<to>.json` (signed, deleted after use), `index/` (overflow version vectors), `backup/` (config backups).

## 7. Architecture and stack

Rust workspace:
- `wasl-core`: planner, version vectors, conflict rules, safety guards, run state machine
- `wasl-store`: SQLite schema and migrations (`rusqlite`)
- `wasl-fs`: scanner (`jwalk`), watchers (`notify` plus Android hooks), atomic writes, recycle bin (`reflink-copy`), name mapping
- `wasl-ignore`: gitignore-compatible matcher (`ignore` crate), presets, explain
- `wasl-drive`: Drive client (`reqwest` + `rustls`), Changes API, resumable transfers
- `wasl-net`: QUIC (`quinn`), mDNS (`mdns-sd`), STUN, port mapping, Drive rendezvous, pairing
- `wasl-chunk`: FastCDC (`fastcdc`), BLAKE3 (`blake3`)
- `wasl-crypto`: v1 and v2 vaults (`aes-gcm`, `aes-siv`, `argon2`, `pbkdf2`), device keys (`ed25519-dalek`)
- `wasl-vfs`: files-on-demand trait with Cloud Files, File Provider and FUSE (`fuser`) back ends
- `wasl-ffi`: UniFFI bindings for Kotlin
- `wasld`, `wasl` (CLI): desktop daemon and command line

Android keeps its Kotlin/Compose UI, WorkManager, SAF, DocumentsProvider, notification, widget, tile and intents, calling the engine through `wasl-ffi`. The desktop app is built with Tauri 2: its Rust side links `libwasl` directly and talks to `wasld`; its web UI uses the Kahf design system. The macOS File Provider extension is a Swift target.

## 8. Verification

| Area | Method |
|---|---|
| Planner | Property tests: no deletion without a baseline entry; mirror methods converge in one run; N devices plus Drive converge to one state under random edit orders |
| Version vectors | Model-based tests against a reference implementation; `appProperties` size-limit tests |
| Ignore rules | Fixture trees compared with `git check-ignore` |
| Device transport | NAT lab (network namespaces with full-cone, restricted, symmetric NAT and CGNAT) for every candidate type; pairing and key-pinning tests; packet capture for `lan` reach |
| Chunking | Delta size tests on edited large files; corrupted chunk rejection |
| Faults | Process kill, disk full, network drop and 403/429 bursts mid-run on every platform |
| Safety | Guard boundary tests; recycle-bin round trips on each side |
| Vaults | v1 and v2 test vectors; recovery tool cross-check |
| Migration | Real 1.1.4 databases imported and synced without changes |
| UI | Screenshot tests in English and Arabic; screen-reader passes per platform |

## 9. Traceability

| PRD goal | Requirements |
|---|---|
| G1 No data loss | FR-F1–F7, FR-B3, FR-N10, NFR-1, NFR-2 |
| G2 Never stop silently | FR-B4–B8, FR-D4, FR-N8, FR-N12 |
| G3 Routes and reach | FR-P3, FR-N6–N8, FR-N11 |
| G4 Zero running cost | FR-N5–N8 |
| G5 One engine | FR-E1–E5 |
| G6 Developer folders | FR-G1–G8 |
| G7 Files on demand | FR-O1–O5, FR-V2 |
| G8 Release | FR-R1–R5, FR-K2 |
| Agent changes (`docs/AGENT_CHANGES.md`) | FR-AG1–AG5 |
| Op-log (`docs/adr/0002-one-op-log.md`) | FR-OL1–OL8 |
| Agent undo (`docs/AGENT_UNDO.md`, G1 for losses caused by agents) | UND-1–UND-14 |
