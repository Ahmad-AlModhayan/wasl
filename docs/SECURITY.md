# Security

Status: milestone in progress, started 2026-10-26. This is the security model of Wasl on every platform, what has been
reviewed, what was fixed, and what is still to do before Wasl can call itself "state of the art". The findings come from
a code review in five areas (Drive and sign-in, device sync, vault cryptography, the computer's local surface, the
Android app) plus dependency audits; each finding was checked against the code before it was fixed.

## 1. What Wasl protects, and from whom

| Asset | Where it lives |
|---|---|
| Your files | Synced folders on each device; Google Drive; device-to-device links |
| Vault contents and names | Encrypted on the device before upload; keys on each device that unlocked the vault |
| Google access | Refresh tokens: OS keychain (computer), Google Play services (Android) |
| Device identity | Ed25519 key per device: OS keychain / Android Keystore-wrapped |
| Wasl's own state | Database, recycle bin, agent copies, staging: the app's private data folder |

| Adversary | What they can do | What Wasl promises |
|---|---|---|
| Someone on the network path (café Wi-Fi, ISP) | See and alter traffic | Nothing readable or alterable: TLS 1.3 to Google (rustls, built-in roots, no way to disable checks); device links are TLS 1.3 with pinned Ed25519 keys |
| A stranger on the same LAN | Talk to the device's port, announce fake services | Cannot sync, pair or learn the device's identity without the pairing code and the four words |
| Google, or anyone with access to the Google account | Read and change Drive | Plain pairs: can read them (Google's encryption at rest only). **Vaults**: cannot read names or contents; tampering is detected; see §4 for what stays visible |
| A device removed from your group | Has its old key | Cut off by every device it connects to (S1-1), and by members it never meets once they read the group files in Drive (S2-1; peer to peer only for LAN-only groups) |
| Another app on the phone / another user on the computer | Call exported components, read shared folders | Cannot read Wasl's private data, tokens or keys, or make it sync |
| Malware running as you on the computer | Everything you can do | Out of scope for any sync app; Wasl limits what it leaves lying around (no plaintext tokens, vault content dropped on lock) |
| An AI agent you run in a synced folder | Change files | Held until you accept in a Wasl dialog the agent cannot answer through Wasl's own channels (AGENT_CHANGES.md §5, S3-1) |

## 2. Already at the bar (verified in code)

- Transport to Google: rustls TLS 1.3/1.2 with bundled roots (a locally installed interception CA is not trusted), no
  redirects on Drive calls, upload session URIs receive the token only on the exact upload host.
- Sign-in: PKCE S256, random `state`, loopback on 127.0.0.1 random port, 5-minute window; refresh tokens only in the OS
  keychain; on Android Google Play services holds them and Wasl never sees a refresh token.
- Device links: TLS 1.3 only, Ed25519 only, the handshake signature is verified against the pinned key, no session
  resumption, trust re-checked on every request; pairing secret 160 bits, single use, 10 minutes, HMAC proofs with
  constant-time checks, four confirmation words.
- Vault v2: Argon2id (64 MiB, t=3) → HKDF subkeys, AES-256-GCM chunks bound to file id and keyed chunk id, AES-SIV names
  bound to their folder, authenticated manifests, per-device rollback protection, zeroized keys.
- Local: the daemon's IPC uses a mutual HMAC challenge over a per-start token, a first-instance named pipe that refuses
  remote clients (Unix: 0600 socket in a 0700 folder); synced paths reject `..`, absolute paths, Windows device names,
  alternate data streams, symlinks and junctions.
- Android: no cleartext traffic, every PendingIntent immutable, the automation receiver gated by a 128-bit secret, the
  Files-app provider read-only for vaults and protected by `MANAGE_DOCUMENTS`, keys wrapped by non-exportable Keystore keys.
- Desktop app: strict CSP, no remote content, minimal Tauri capabilities.

## 3. Findings and status

Severity: critical (data loss or exposure without user action), high, medium, low, info.

### Fixed in milestone phase S1 (2026-10-26)

Each fix has a test that fails without it (engine: `cargo test --workspace`, 941+ tests; Android: JVM unit tests), and
the whole change was reviewed independently before release; that review found one bypass (S1-3), fixed before release.

| ID | Sev. | Finding | Fix |
|---|---|---|---|
| S1-1 | high | Group changes did not travel: a device removed on one computer was still trusted by the others, and a device paired through one was unknown to the rest | Members exchange signed group operations on every link (at connect, each run, every minute, and at once on a change); trust updates immediately and the removed device's links close. Test: three devices, one removed, one added |
| S1-2 | high | A crafted backup file could create phone pairs that start syncing at once, on any folder (even the app's private data), with the mass-delete guard off | Imported pairs wait for their preview; private folders refused, also when reached through /proc, a mount prefix or a link (fixed after 0.2.3 shipped, from an automated review); deletion safeguards not taken from files; the dialog lists every pair |
| S1-3 | medium | The share target opened `file://` and Wasl's own URIs ("Dirty Stream"), copying private files into a synced folder | Only other apps' `content://` URIs, including the `user@authority` form Android resolves to Wasl itself |
| S1-4 | medium | A pair set to "LAN only" could sync over internet links | Refused on both ends: the puller skips internet links, the server refuses non-LAN addresses |
| S1-5 | medium | mDNS: dialled any announced address (even public ones), slowly and one at a time, and showed this device's certificate to strangers while a pairing code was open | LAN addresses only (8 per device), 3 s parallel dials, members-only trust for discovery |
| S1-6 | medium | A malicious paired device could page its index forever (memory) | 200,000 entries / 128 MiB per pull, resumed next run |
| S1-7 | medium | Oversized objects in a vault folder could fill disk or memory on every device | Refused by listed size and cut off while downloading |
| S1-8 | medium | Locking a vault on Windows left the files opened through its view on disk as plaintext | Unpinned files dehydrated before the view disconnects |
| S1-9 | medium | Removing an account on the computer did not revoke Google's grant | Revoked at Google first (best effort), then forgotten |
| S1-10 | medium | Dependencies with advisories: `rustls-webpki` and `webpki-roots` (distrusted intermediates), `tokio`, `time`, `quick-xml` | Updated; `cargo audit` and `npm audit` show no vulnerabilities |
| S1-11 | low | Token requests had no timeout (a refresh could hang a sync); stored errors carried Drive URLs with file names and upload session URIs; response bodies uncapped | Timeouts and no redirects; URLs stripped from errors; 16 MiB cap; downloads stop at their expected size; tokens only over https (or to this machine) |
| S1-12 | low | Argon2id reader accepted 1 GiB / t=10 from an edited `vault.json`; Android v1 reader had no iteration bound; passphrase minimum only in the UI | m at most 256 MiB, t at most 4, p at most 4; v1 100k to 10M iterations; minimum enforced in the vault manager |
| S1-13 | low | A pair could include Wasl's own data folder (token, database) | Refused on the computer; on the phone an import refuses it (the folder picker can't reach it) |
| S1-14 | low | Agent saved copies were not integrity-checked; only the CLI refused an agent's own "accept" | BLAKE3 recorded and verified before revert or diff; the daemon also refuses an accept that says it comes from an agent, and records who accepted |
| S1-15 | low | Portable installs on Windows: data folder not restricted to the user; the IPC accepted 32 MiB lines and any number of connections before authentication; a squatted pipe name read as "already running" | Owner-only ACL (icacls); 4 KiB and 16 unauthenticated connections; squatting reported as such |
| S1-16 | low | Rendezvous: replayed offers answered after a restart; junk offers could hide real ones | Offers from before the start refused; only members' offers counted |
| S1-17 | low | Android: no FLAG_SECURE on passphrase, manifest, pairing and automation screens; secrets copied without the sensitive flag; device-to-device transfer not excluded; file names in logcat; pair names on the lock screen | All addressed |
| S1-18 | low | Pairing: the joiner could change the name shown to the inviter after the first round | Name frozen at the first round; the joiner's device ID is available beside the words |
| S1-19 | info | Hydration trusted a placeholder's Drive id; chunk lengths uncapped; static pages without security headers | Checked against the view's tree; 16 MiB cap; CSP, frame-ancestors, nosniff, HSTS |

### Fixed in milestone phase S2 (2026-10-26)

| ID | Sev. | Finding | Fix |
|---|---|---|---|
| S2-1 | medium | Group changes travel only between devices that connect to each other; two devices never online together don't learn of a removal | Each device writes its group operations to its own file in the Drive app-data folder (`group/<group>-<device>.json`, signed operations, at most 1 MiB) and reads its members' files at start and every minute, by exact name, through the same checks as a link. Only where a device pair already uses Drive or the internet (route "Drive and devices", or reach "LAN and internet"); LAN-only and account-less groups stay peer to peer (DEVICE_SYNC §3). Test: A removes C while B is away; B reads the folder and cuts C off |
| S2-2 | low | A removed device can still sign removals of devices older than itself (by design: concurrent removals both count) | Rule kept, by decision (DEVICE_SYNC §3): voiding such removals would lose honest offline ones, and every count-dependent exception lets a removed device play removals against each other. Bounded instead by reach (only members are asked or read, so only devices that have not heard of the removal can be reached, a window S2-1 closes within a minute) and by a per-signer throttle (64 new operations a minute, charged only after the signature checks; see the review below) |
| S2-3 | low | A malicious member can make others send hole-punching packets to addresses it chooses | At most 4 addresses per punch or direct attempt (one per internet kind, then LAN); one punch per offerer at a time, at most 4 a minute |

### Review of phases S2–S5 (2026-10-26)

An independent review of the S2–S5 work found six problems, each checked against the code and fixed
before release with a test that fails without the fix (the Android one has a JVM test only).

| Area | Finding | Fix |
|---|---|---|
| Group throttle (S2-2) | A signer's per-minute share was charged before the operation's signature was checked, so anyone who can write a group file (a removed device that still has the Google account) could send 64 junk operations "signed by" a member each minute and starve its real ones, including the attacker's own removal; re-sent operations already waiting for their parents were charged again | Each operation is screened first (`GroupOps::screen`): already kept or waiting costs nothing, a malformed one or one whose signature fails against the signer's known key is dropped and charged to no one, a verified one is charged to its signer, and one from a signer not known yet shares a single bucket of its own. Test: a flood of forged operations in a member's name does not stop that member's removal |
| Group files (S2-1) | A device rewrote its own group file only when its group changed, so a removed device with the account's token could overwrite or delete the remover's file and keep the news from devices that rely on Drive | Every read round also reads back this device's own file and writes it again when it differs from what it would write. Test: an overwritten and a deleted file are both restored within a round |
| Agent accept (S3-1) | After a decline or a timeout an agent could ask again at once, in a loop, keeping a dialog on screen | Per-changeset wait after a decline, a timeout or an answer without the token: 1 minute, doubling, at most 30, reset by an accept; the CLI and the app say "try again in N minutes" in both languages. The token is left out of every `Debug` output. AGENT_CHANGES.md §5 now says plainly this is a deterrent against silent self-acceptance, not a security boundary |
| Windows views (S3-2) | Bytes written between the changed copy being set aside and the dehydrate were lost | The copy and the dehydrate go through one handle that denies other writers: a file still open for writing is reported and restored at a later refresh, once closed. Test on real Cloud Files |
| Android keys (S4-5) | Get-or-generate of a wrap key was not serialized: two first uses at once could have one thread's StrongBox key deleted by the other's fallback, leaving what it wrapped unreadable | One process-wide lock per alias around get-or-generate; the fallback never discards an alias that holds a usable key by then |
| Vault upgrade (S4-2) | A rerun resumed into any 2.1 vault in the target folder that the passphrase opened, copying into it | The computer records which vault an upgrade's new vault comes from when it makes it; a resume goes only into that vault, any other is refused. Test |

Still open from this review: a deliberate restore of a whole 2.1 vault folder to an earlier state is
refused permanently by the state check (S4-4), with no "accept the current state" action yet; and
`.wasl-state.*` records accumulate in a vault folder from remotes that write without a store.

### Still to do (milestone phases S2 to S5)

| ID | Sev. | Finding | Plan |
|---|---|---|---|
| S2-4 | medium | Android: removing an account does not revoke Google's grant | Done on Android: removing an account calls `AuthorizationClient.revokeAccess` first (best effort, 15 s, play-services-auth 21.4.0) and says whether access was withdrawn or must be removed in the Google Account by hand |
| S3-1 | medium | A process that hides its agent markers can still accept an agent's changes | Approval confirmed in the desktop app (a code or native dialog the agent can't see or answer). **Done (2026-10-26)**: `AgentAccept` never accepts by itself; wasld starts the app's native dialog (`wasl-desktop --confirm-agent`, next to wasld), passes it a token minted for that changeset (256-bit, single use, 2 minutes) through the child's stdin and reads the answer from its stdout, so the token never touches disk, a command line, the environment or the IPC; one dialog at a time; the client gets only a 6-digit code to match against the dialog. Revert stays open. Residual: a program running as you can still click the dialog through accessibility or input APIs, read the app's or wasld's memory, replace the app's binary, edit the database, or run its own wasld; deliberate tampering of that kind is out of reach of any check in your own session (AGENT_CHANGES.md §5) |
| S3-2 | low | Windows views are read-only by attribute only; views of a locked vault that were not showing keep content from an earlier run | Detect and revert local edits on refresh; dehydrate on the next start. **Done (2026-10-26)**: a view's placeholder that is no longer in sync gets Drive's version back at the next refresh (marked in sync and dehydrated), after its changed content is copied to `<data>/views/<id>/changed-here/`; the view reports it (`restored`), and an attribute cleared without a write is set again. At each start, and when a vault is locked, every view of a locked vault that is not showing (turned off, or not running at lock time) is connected with a source that refuses every read and dehydrated except pinned files, before anything is shown. Tests on real Cloud Files |
| S3-3 | low | The Windows data-folder ACL fails open (warning only) if `icacls` fails on NTFS | Fail closed where the volume supports ACLs. **Done (2026-10-26)**: when `icacls` fails, `GetVolumeInformationW` (in wasl-vfs-windows, the crate allowed Win32 calls) decides: a volume with persistent ACLs (NTFS, ReFS, shares), or one that cannot be identified, refuses the folder (wasld does not start, the token is not written); FAT and exFAT keep the warning |
| S4-1 | medium | Vault v2 chunk sizes can fingerprint known files (public chunking parameters) | Vault format 2.1: keyed chunking and size padding. **Done (engine, 2026-10-26)**: new vaults are 2.1 (`version` 3); gear table from an HKDF sub-key, chunks padded to padmé buckets (at least 4 KiB, under 3.2 % for chunks of 64 KiB and more); 2.0 vaults stay 2.0; older readers refuse 2.1; known-answer vectors (ENCRYPTION.md §11.10). Python reference: 2.1 still to do |
| S4-2 | medium | Vault v1 is still written; its 64-bit nonces and PBKDF2 are weaker | New vaults v2 only; migrate v1 vaults. **Done in the engine and daemon**: no client creates v1; a v1 vault never starts a new folder; `Vaults::upgrade` copies a v1 or 2.0 vault into a new 2.1 vault (restartable, old folder untouched, pairs moved by hand: ENCRYPTION.md §11.12). `wasl vault upgrade` runs it over the daemon's socket (tested on real Drive, 2026-10-26; a 2.1 vault joined on the phone reads and writes). Still to do: upgrading from the phone |
| S4-3 | info | Vault file modification times are visible to Google | Upload with neutral times; real time only inside the encrypted manifest. **Done for v2**: manifests carry the upload time on Drive, listings read `wasl.m`. v1 has no encrypted time and keeps it |
| S4-4 | low | Rollback of a whole vault folder or of deleted files is not detected | Authenticated folder listings or a signed epoch. **Done for 2.1 vaults**: per-writer MACed state records (counter, digest of the live files); a folder put back to an older state, or a removed record, is refused before anything is applied; a file brought back with the records kept is reported. Limits in ENCRYPTION.md §11.11 |
| S4-5 | low | Device and vault keys usable without the person present; passphrases not wiped from every buffer | StrongBox where available; optional auto-lock; zeroize the IPC buffer and FFI copies. Vault part done: the IPC line buffer is wiped after parsing (and when it grows), the daemon no longer reads a vault's keys to show whether it is unlocked; the one key copy the phone binding returns has to cross to the app. Android: new device and vault wrap keys in StrongBox, TEE fallback; existing keys untouched (moving one would lose what it wraps). Not `setUnlockedDeviceRequired`: background sync opens the keys while the phone is locked. No auto-lock on Android: an unlocked vault is its wrapped key file, read by every background sync run, so an idle timer would either lock under a running sync or never fire; lock by hand |
| S5-1 | n/a | Release integrity: Windows Authenticode signing, Apple Developer ID and notarization, R8 on Android | Before public release. Android: R8 and resource shrinking on for release; the app's classes, the UniFFI bindings and JNA kept whole (reached by name at run time); narrow the app rule after a minified build has run on a device |
| S5-2 | n/a | Independent assurance: fuzzing of every parser (CBOR, manifests, Drive JSON, IPC), `cargo-deny`/`cargo-audit` on every build, an external penetration test, Google's CASA assessment (required for the full Drive scope in a public app) | Before public release. Done (2026-10-26): robustness suites (`tests/robustness.rs` in wasl-net, wasl-crypto, wasl-chunk, wasl-drive, wasl-ipc) feed every parser that reads outside data arbitrary and mutated-valid input (no panic, bounded time and output, nothing tampered accepted); they found F1 (low: share codes and probe tokens accept `+` in hex) and F2 (low: `to_rfc3339` panics for a file time beyond year 9999, reachable from a local file's mtime on upload), both fixed (hex digits only; times clamped to years 1 to 9999), their tests no longer ignored. `engine/fuzz` holds cargo-fuzz targets for peer frames, `vault.json`, v2 manifests and the IPC handshake: run them in the Mac session (nightly, Linux/macOS). `tools/security-check.sh` (cargo audit, cargo deny with `engine/deny.toml` for both workspaces, npm audit) is the pre-release gate; it passes, with five unmaintained (not vulnerable) crates allowed with reasons. Still to do: penetration test, CASA |
| S5-3 | n/a | macOS: Keychain use, File Provider, notarization reviewed on the Mac | Mac session |

### Automated review of the S2-S5 commit (2026-10-26)

| Finding | Fix |
|---|---|
| The accept dialog showed the requesting program's own claim of who it is as a fact | Shown as what the program says, marked as not verifiable |
| Agent text in the dialog was stripped of control characters but not of bidi overrides, zero-width marks or line separators | Those are dropped too (test) |
| Files someone edited in a vault view, set aside under `changed-here/`, stay unencrypted after the vault locks; a view whose cache could not be opened was not cleared | Never deleted silently: the locked view says where they are and to delete them; the folder is cleared through an empty cache |

### Found on real hardware (E2E 0.3.0, 2026-10-26)

| Sev. | Finding | Fix |
|---|---|---|
| critical | Locking a vault left a view file edited by hand (read-only attribute cleared) as plaintext in the view's folder, and the clear-out at the next start skipped it too: both freed only placeholders in sync | Freeing a read-only view sets such a file's content aside under `changed-here/` first, then frees the placeholder; the locked view says where the copy is. Tests on real Cloud Files (`wasl-vfs-windows`, `wasld`) |
| high | `wasl vault upgrade` asked for a new passphrase while wasld checks the vault's own, so no upgrade could run | Asks once for the vault's passphrase; upgraded on real Drive and read on the phone, byte-identical |

### Pre-release review for 0.4.3 / 0.5 (2026-11-02, #134, #135, #136)

| ID | Sev. | Finding | Status |
|---|---|---|---|
| M1 | medium | An agent holding the IPC token could release its held changes with no confirmation: remove its pair (the changeset rows cascade with it), make it again on the same folders, approve the preview, sync | Fixed (#134): the store refuses to delete a pair with an open or held changeset, in the delete statement itself, and `Engine::update_pair` refuses to re-point one under the lock it saves with. The person accepts (app dialog) or reverts first. Daemon test drives the whole bypass |
| M2 | medium | `zstd` 0.13.3 / `zstd-safe` 7.3.0 carried RUSTSEC-2026-0391, -0398, -0403, so `tools/security-check.sh` failed | Fixed (#135): zstd 0.13.4 / zstd-safe 7.4.0 with the `legacy` feature off (peers never send pre-1.0 frames); the yanked `async-executor` 1.14.0 moved to 1.14.1. The gate passes again |
| L1 | low | Desktop and Android ask for the full `drive` scope, so a stolen refresh token reaches the whole Drive | Accepted risk: syncing arbitrary existing folders needs it. CASA (S5-2) covers it before a public release; `drive.file` with the Picker stays an option for people who only sync folders Wasl made |
| L2 | low | `SignedIn` derived `Debug` with both tokens | Fixed: a manual `Debug` prints `…` for them (test) |
| L3 | low | The Windows pipe keeps the default access list and a computable name: another local user can squat the name before wasld starts or hold the 16 unauthenticated slots (denial of service only; they can't authenticate) | Open: an owner-only DACL needs unsafe Win32 calls, which the workspace allows only in `wasl-vfs-windows` (owner decision); #174 |
| L4 | low | Android logged plain Drive file names and the engine's detail text to logcat on a failed read | Fixed: the line names the file by the tail of its Drive id and the error by its class (test) |
| L5 | low | The desktop `is_folder` command looked at any path the webview sent, network paths included (an NTLM leak if the webview were ever compromised) | Fixed: only absolute local paths are looked at; UNC, `\\?\` and device paths are refused (test) |
| L6 | low | v1 vaults still take writes with 64-bit file nonces | Tracked as S4-2: the phone can't upgrade a v1 vault yet, so making v1 read-only now would stop phone-only users writing; the collision bound is negligible at normal volumes |
| I1 | info | No dependency vulnerability check for the Android (Gradle) side | Open: needs a choice of tool and CI minutes; #175 |

## 4. What Google can always see

Even in a vault: the folder tree's shape and item counts, name lengths, file and chunk sizes, when things change, and
the vault's KDF parameters. Plain pairs: everything. With reach "LAN and internet", the device's LAN and public addresses
in the app-data folder. Wasl does not hide that you use Wasl.

## 5. Reporting a vulnerability

Report privately as described in the root [SECURITY.md](../SECURITY.md) (GitHub private vulnerability reporting, or hello@kahflabs.com). Do not open a public issue.
