# Wasl (وصل) · Product Requirements

| | |
|---|---|
| Product | Wasl, one sync engine for your phone and computers: through Google Drive, directly between your devices, or both |
| Owner | Kahf Labs |
| Status | Android 1.1.4 built and in daily use (as Drive Sync). This document specifies Wasl 2: the Super Sync release |
| Version | 1.0 (2026-10-19) |
| Related | SRS: `docs/SRS.md`. Current Android architecture: `docs/ARCHITECTURE.md`. Vault v1 format: `docs/ENCRYPTION.md`. Site: wasl.kahflabs.com |

## 1. Summary

Wasl keeps folders the same across an Android phone, Windows, macOS and Linux computers, and Google Drive. Each folder pair chooses its route: through Drive, directly between the user's own devices, or both at once. Direct sync works inside the home network or across the internet, with no Kahf server in the path: devices find each other through the user's own Drive and connect directly.

Wasl is the sync that asks before it acts. A first sync, a run that would delete a large share of a folder, and a file changed in two places all stop for a decision. Nothing Wasl deletes or overwrites is gone for 30 days. One Rust engine (`libwasl`) runs on every platform, so the rules are identical everywhere. No ads, no analytics, no Kahf server touching user files.

**Direction.** Beyond Wasl 2, Wasl aims to be the next generation of sync and of safe cloud storage for data, not only folder sync across Android, desktop and Google Drive. Hosted storage is outside Wasl 2 (§4 non-goals) and gets its own spec before it enters this PRD.

## 2. Problem

- **Android has no folder sync with Drive.** Drive offers offline pins and, since February 2026, a Downloads backup that makes static copies. Third-party apps sync but overwrite without versions, ignore conflicts, carry ads and stop silently when Android kills them.
- **Cloud-only sync is slow between devices on the same desk.** A 20 GB folder moved from PC to laptop goes up to Drive and back down, even on the same Wi-Fi.
- **Peer-to-peer tools have no cloud.** Syncthing needs two devices online at once, has no off-site copy and its official Android app was discontinued in 2024.
- **Drive has no ignore rules.** Project folders upload `node_modules`, `.venv`, build output and caches, which stall Drive for desktop, hit API rate limits and are useless on another OS anyway.
- **Encryption breaks usability.** Wrappers make browsing and streaming encrypted files across phone and desktop painful.

## 3. Users and jobs

| User | Job to be done |
|---|---|
| Multi-device worker | "My Projects and Documents folders are the same on my PC, laptop and phone, and backed up on Drive, without me thinking about it." |
| Developer | "Sync my source code between machines; never sync `node_modules`, `.venv` or build folders." |
| Large-file owner | "Move installers, media and VM images between my own devices at network speed, and open big Drive files on my laptop without downloading them first." |
| Phone-first user | "Photos and recordings go to my PC over Wi-Fi at home and to Drive when I'm out." |
| Privacy-minded user | "Some folders sync only between my devices, never to any cloud; others go to Drive encrypted, where Google can't read names or contents." |

## 4. Goals and non-goals

**Goals**
1. **No data loss caused by Wasl.** Every deletion or overwrite has a 30-day undo path on every side. The same promise covers losses caused by agents: agent undo puts back what an AI agent changed in any folder you watch, synced or not (`docs/AGENT_UNDO.md`).
2. **Never stop silently.** Every device always shows what Wasl is doing, waiting for, or needs, and every problem has a fix in plain words.
3. **Three routes per pair.** Drive only, Drive + devices, devices only; device sync reaches the local network only or the internet too.
4. **Zero running cost for device sync.** Discovery and connection setup use the local network and the user's own Drive; no Kahf relay or signalling server.
5. **One engine everywhere.** A single Rust core for Android, Windows, macOS and Linux; platform apps only provide UI and OS integration.
6. **Clean developer folders.** `.waslignore`, optional `.gitignore` support and presets keep dependency and build folders out of every route.
7. **Files on demand everywhere.** Drive and vault folders browsable without downloading, through each OS's native mechanism.
8. **Ship on Google Play and as desktop installers**, with Google's restricted-scope verification passed, in English first for a global audience, with Arabic (full RTL) as the first translation.

**Non-goals**
- Syncing dependency or build environments between machines (they are OS- and machine-specific; Wasl ignores them).
- Storing user files on Kahf servers, or running a Kahf relay, in Wasl 2. Hosted storage is a possible later direction (§1).
- Clouds other than Google Drive in Wasl 2 (OneDrive, Dropbox, WebDAV are candidates for later; the engine keeps a transport interface for them).
- Live collaborative editing or merging the contents of conflicting files; Wasl keeps both copies.
- iOS in Wasl 2 (the Rust core keeps it possible later).

## 5. Scope

| Priority | Area | Feature |
|---|---|---|
| P0 | Engine | Rust core `libwasl` on Android (via UniFFI/JNI), Windows, macOS, Linux; all planning, hashing, ignore matching, crypto and transports in Rust |
| P0 | Pairs | Seven methods (two-way, upload only, upload then delete, upload mirror, download only, download then delete, download mirror); conditions (Wi-Fi, Wi-Fi names, metered, roaming, VPN, charging, battery floor); size limit; selective sync |
| P0 | Routes | Per pair: Drive only, Drive + devices, devices only. Device reach per pair: local network only, or local network and internet |
| P0 | Device sync | Pair devices by QR code or short code; mutual authentication by device keys; discovery on the local network; internet connection set up through the user's own Drive with direct NAT traversal; encrypted QUIC transport |
| P0 | Delta transfer | Content-defined chunking for large files between devices, so only changed blocks move |
| P0 | Safety | Preview (dry run), first-sync stop, mass-delete guard, conflict copies, two-sided recycle bin (30 days), Drive revisions with restore and keep-forever |
| P0 | Op-log | One record of every actor's changes: changesets tagged with who made them (an agent, or the person on a device), staged, committed, discarded or reverted, each operation with its state before and after. Append-only: a revert appends inverse operations. Agent changes write through it; other Kahf apps read it through the read-only `wasl oplog` CLI and IPC (ADR-0002, SRS §3.15) |
| P0 | Agent undo | Watch list for any folder, synced or not; a snapshot at session start and a journal of every change with the content before it, plus a "not captured" list; markers; restore of a file, folder or session to session start or a marker, with a dry run and an undo of the restore; a session report that says what can't be undone; caps; Timeline and Restore on the **Agent changes** screen and new `wasl agent` commands; sessions written to the op-log (`docs/AGENT_UNDO.md`, SRS §3.16) |
| P0 | Ignore rules | `.waslignore` at any depth, optional `.gitignore`, presets (Node, Python, Rust, Android/Gradle, Flutter, Docker, editors, OS junk), ignored-items view with sizes |
| P0 | Status | Android persistent notification; desktop tray icon and status window; Problems list with fixes; Diagnostics; run history |
| P0 | Release | Wasl / وصل identity, app ID `com.kahflabs.wasl`, English first, Arabic as the first translation (full RTL), Storage Access Framework and All files access on Android, restricted `drive` scope verification |
| P1 | Files on demand | Android DocumentsProvider (exists), Windows Cloud Files API, macOS File Provider, Linux FUSE; offline pins; LRU cache |
| P1 | Vaults | Client-side encryption (names and contents); v2 format with chunked random access; v1 vaults keep working; open recovery tool |
| P1 | Large files on Drive | Resumable whole-file upload and ranged download; streaming through files on demand |
| P1 | Automation | Desktop CLI (`wasl status`, `wasl sync`, `wasl pair`, `wasl ignore`); Android intents behind a secret, tile, widget, shortcuts, share target |
| P1 | Backup | Pairs and settings backed up to Drive app data; restore on a new device |
| P1 | Agent undo | Risky restores and large deletions ask through an external approvals inbox when one is configured, else the Wasl window; delete guard for deletes outside the watch list; MCP tools `checkpoint`, `list_checkpoints`, `diff_since` through `kahf-mcp` (`docs/AGENT_UNDO.md`) |
| P2 | Later | More clouds; photo rules; bandwidth windows; iOS |

## 6. Key flows

1. **First device.** Install on the PC → sign in with Google → Wasl indexes the Drive → create a pair `Projects/` with route *Drive + devices* → **Preview** shows every action → start.
2. **Add a device.** On the laptop, choose *Add this device* → scan the QR code shown on the PC (or type the short code) → both devices show the same confirmation words → the laptop joins; existing pairs offer to sync to it.
3. **Same Wi-Fi.** The PC saves a 6 GB video export → the laptop on the same network receives it directly at LAN speed → Drive receives it in parallel (route *Drive + devices*) → the laptop recognises the Drive copy as identical and downloads nothing.
4. **Away from home.** The laptop at a café edits a file → the PC at home is online → the two devices exchange connection details through the user's Drive app folder and connect directly → the change syncs device to device. If a direct connection is impossible on that network, a *Drive + devices* pair still syncs through Drive; a *devices only* pair waits and says so.
5. **Developer folder.** The user pairs a project and picks the Node preset → the ignored-items view shows `node_modules/` (1.2 GB) and `dist/` excluded → only source files sync on every route.
6. **Mass delete.** A script deletes a project sub-tree on the laptop → the next run would delete more than 20 files → every device shows "Needs you" → the user restores from the recycle bin or approves once.
7. **Private folder.** A pair with route *devices only* and reach *local network only* syncs between PC and phone at home and never touches the internet.

## 7. Success metrics (targets for the first 6 months after release)

| Metric | Target |
|---|---|
| Confirmed data-loss reports caused by Wasl | 0 |
| Runs that end in success or a stop that asks the user | ≥ 99.5% |
| Device-to-device connections that succeed directly over the internet (no fallback needed) | ≥ 85% |
| LAN transfer speed between two desktops on gigabit Ethernet | ≥ 80% of link speed |
| Bytes moved when 1 MB changes inside a 10 GB file, device to device | < 32 MB |
| Play rating / crash-free sessions | ≥ 4.6 / ≥ 99.5% |

These are targets, not results.

## 8. Release plan

| Milestone | Content |
|---|---|
| M1 Core | Rust workspace: planner, baseline store, ignore engine, Drive transport, safety guards; ported tests from the Kotlin planner pass in Rust |
| M2 Android on the core | Android app switches its engine to `libwasl` behind the existing UI; Wasl rename, Arabic, SAF mode; migration from 1.1.4 data |
| M3 Desktop | Windows, macOS and Linux daemon, tray app and CLI on the same core; Drive pairs |
| M4 Device sync | Device pairing, LAN discovery, Drive-based connection setup, QUIC transport, chunked delta transfer, routes and reach per pair |
| M5 On demand and vaults | Cloud Files API, File Provider, FUSE; vault v2 |
| M6 Release | OAuth verification, Play listing, signed desktop installers, site switches to downloads |

Each milestone ends with a working build used daily on the maintainers' devices before the next starts.

## 9. Risks

| Risk | Mitigation |
|---|---|
| Restricted `drive` scope verification is slow; unverified apps are capped at 100 users | Start early; server-less design avoids the CASA assessment; privacy policy and terms on the site; separate OAuth clients for Android and desktop verified together |
| Direct connection fails on strict networks (symmetric NAT, carrier-grade NAT) | *Drive + devices* pairs fall back to Drive automatically; *devices only* pairs wait, explain why, and accept a user-supplied path (Tailscale/WireGuard address or self-hosted relay) |
| Android limits background networking (Doze, standby buckets) | The phone joins device sync while Wasl is open, while charging on Wi-Fi, or during a user-started foreground session; Drive covers the rest |
| False conflicts with three or more devices and two routes | Version vectors plus content hashes in every baseline; vectors stored with each file on Drive so any device can compare states |
| Rewriting a working engine introduces regressions | Port the Kotlin planner's tests first; Android keeps the old engine until the Rust one passes them and a month of daily use |
| Play rejects the All files access declaration | SAF mode is P0 |
| Google rejects verification when app name, domain and privacy policy don't match | Buy kahflabs.com now (decision 7); privacy policy at wasl.kahflabs.com/privacy; domain verified in Google Search Console |
| Phone makers kill background work | One-time guided setup and Diagnostics per maker |
| Name confusion with existing "wasl" apps and brands (e.g. a Play developer named wasl; Dubai's wasl group) | Store name "Wasl Sync" / «وصل للمزامنة» (decision 8); trademark search before launch |

## 10. Decisions

1. App ID `com.kahflabs.wasl` on Android; matching bundle identifiers on desktop.
2. Rust is the single engine; platform code holds no sync logic.
3. Dependency and build folders are ignored, never packed or transferred.
4. No Kahf server: device discovery and connection setup run over the local network and the user's own Drive.
5. English first, for a global audience (2026-10-31); Arabic, with full RTL, is the first translation and ships from the first release. Messages cross module seams as typed reasons each app translates, so more languages only add string tables.
6. **Free tier plus Pro, no subscription.**
   - Free: Drive sync with up to 3 pairs, device sync on the local network, and the whole safety layer. Safety is never gated.
   - Pro: unlimited pairs, device sync over the internet, vaults, multiple accounts, files on demand on desktop. One licence covers every platform and every device in the user's group.
   - Every Pro feature runs on the user's own devices, Drive and public STUN servers; nothing in Pro may touch a Kahf server, relay or paid API.
7. **Desktop toolkit: Tauri 2.** Rust like the engine, so `libwasl` links directly; small installers and tray support; the web UI reuses the Kahf design system and its Arabic/RTL work. The macOS File Provider extension is written in Swift. Compose Multiplatform was the alternative (reuses Android screens) but runs on the JVM with a much larger memory footprint.
8. **Store name "Wasl Sync" / «وصل للمزامنة»;** "Wasl" stays the short name in the app. Before launch, search SAIP (Saudi Arabia), WIPO Global Brand Database, EUIPO and USPTO in classes 9 and 42.
9. **Domain: buy kahflabs.com now.** Privacy policy and terms at wasl.kahflabs.com; domain verified in Google Search Console before the OAuth verification request.
