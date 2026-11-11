# Files on demand and vault v2 (M5): design

Two features that meet in one place: a file can sit on Drive (or in a vault) and still look like a
normal file on the computer, read only when something opens it, and an encrypted vault can be
read the same way, a piece at a time (SRS §3.9 FR-O1–O5, §3.10 FR-V1–V4). Wasl's own design;
other tools were read for research only. Each section names the crate that implements it.

## 1. Vault v2 (`wasl-crypto::v2`, `wasl-engine::vault_v2_remote`)

Vault v1 (`docs/ENCRYPTION.md`) encrypts each file as one stream. It stays readable and writable
(FR-V1). v2 exists for two reasons: an edit should upload only what changed, and a reader should
be able to decrypt any part of a large file without the rest.

### 1.1 Keys

```
salt     = 16 random bytes                                  (vault.json)
params   = Argon2id m = 64 MiB, t = 3, p = 1                (vault.json; readers accept
                                                             m 19..256 MiB, t 1..4, p 1..4,
                                                             m × t at most 512 MiB)
ikm      = Argon2id(passphrase UTF-8, salt, params, 32 bytes)
kName    = HKDF-SHA256(ikm, info = "wasl-v2-name",  64 bytes)   AES-256-SIV key
kMeta    = HKDF-SHA256(ikm, info = "wasl-v2-meta",  32 bytes)   AES-256-GCM key
kChunk   = HKDF-SHA256(ikm, info = "wasl-v2-chunk", 32 bytes)   AES-256-GCM key
kChunkId = HKDF-SHA256(ikm, info = "wasl-v2-cid",   32 bytes)   HMAC key
keyCheck = HMAC-SHA256(HKDF-SHA256(ikm, "wasl-v2-check", 32), "wasl-vault-check-v2")
```

`vault.json`: `{format: "wasl-vault", version: 2, kdf: "argon2id", m, t, p, salt, keyCheck,
created, name}`; `version` is 3 for format 2.1 (§1.6), which adds two sub-keys:
`kCdc = HKDF(ikm, "wasl-v2-cdc")` and `kState = HKDF(ikm, "wasl-v2-state")`. The passphrase is never stored anywhere but in the person's head (FR-V4); the
derived keys are kept like v1's (Android Keystore wrap; OS keychain on a computer).

### 1.2 Layout in the Drive folder

```
vault.json
<encrypted folder>/<encrypted folder>/<encrypted file manifest>   the tree, names encrypted
.wasl-chunks/<00..ff>/<chunk object>                               every chunk, 256 shards
```

- **Names** (folders and file manifests): `base64url(AES-256-SIV(kName, name, ad = [parent dir
  id]))` + `.w2`. Deterministic in its parent, so the Drive name check keeps working; the parent
  dir id is a random 16-byte id kept in the parent folder's `appProperties["wasl.dir"]` (the root's
  is all zeros), so moving a folder does not change its children's names.
- **File manifest**: one small Drive file per vault file. Content:
  `"W2M1" || nonce(12) || AES-256-GCM(kMeta, CBOR{file_id(16, random, fixed for the file's life),
  size, mtime, blake3, chunks: [(chunk_id(32), len)]}, aad = "wasl-v2-manifest")`.
  Its `appProperties["wasl.m"]` holds `nonce || AES-GCM(kMeta, size(8) || mtime(8) || blake3(32))`
  (≤ 124 bytes base64), so a listing knows sizes and times without downloading manifests.
- **Chunks**: the plaintext is cut with FastCDC (min 256 KiB, avg 1 MiB, max 4 MiB: smaller than
  device sync's, because Drive objects are the unit of upload). For each chunk:
  `chunk_id = HMAC-SHA256(kChunkId, file_id || BLAKE3(plaintext))`, stored as
  `.wasl-chunks/<chunk_id[0] hex>/<base64url(chunk_id)>`, content
  `"W2C1" || nonce(12, random) || AES-256-GCM(kChunk, plaintext, aad = "wasl-v2-chunk" ||
  file_id || chunk_id)`.

**Deviation from FR-V2's wording, on purpose**: the SRS says chunks are bound to "file ID and
chunk index". Binding to the index would change every chunk after an insertion, so a 1 MB insert
early in a large file would re-upload the rest of it and fail the same requirement's "< 32 MB"
acceptance. v2 binds each chunk to the file ID and its own content id instead; the order is
authenticated by the manifest's chunk list (and the whole file by its BLAKE3). Swapping chunks
between files fails (file_id is in the AAD and the id), reordering fails (manifest), and two
files with the same content do not share or reveal shared chunks (the id is keyed per file).

### 1.3 Writing and editing

Upload the chunks the new chunk list has that the old one lacks, then replace the manifest
(atomically, as new content of the same Drive file), then delete chunks only the old list had.
A crash leaves extra chunks, never a manifest pointing at a missing one; a sweep deletes chunks no
manifest references (by listing `.wasl-chunks/` and the manifests' lists, run with the bin purge).
A 1 MB edit uploads the changed chunks (one or two of avg 1 MiB, max 4 MiB each) plus a manifest:
well under FR-V2's 32 MB.

### 1.4 Reading a range

`read(offset, len)` finds the chunks covering the range in the manifest's list, fetches only
those objects, checks each tag, and returns the bytes. Seeking to 4 GB downloads and decrypts one
chunk (FR-V2's acceptance).

### 1.5 Recovery (FR-V3)

`wasl vault decrypt <folder>` (Rust) and `tools/decrypt_vault.py` (Python, the independent
reference) decrypt v1 and v2 vaults from a downloaded Drive folder. A cross-check test encrypts
with Rust and decrypts with Python, and the reverse for names and manifests.

### 1.6 Format 2.1, Drive times and rollback (security review S4)

Byte-exact rules: `docs/ENCRYPTION.md` §11.10–11.12.

- **Keyed chunking and padding (S4-1).** 2.0 cuts with FastCDC's public gear table and stores
  exact chunk sizes, so a known file can be recognised by its sizes. New vaults are 2.1: the gear
  table is derived from the vault's keys and every chunk is padded (padmé buckets, at least 4 KiB;
  under 3.2 % extra for chunks of 64 KiB and more) before sealing (`W2C2`). Edits still re-upload
  only the chunks they touch. 2.0 vaults stay 2.0 (readable by older devices); older readers refuse
  a 2.1 vault (`version` 3) instead of writing into it.
- **Neutral Drive times (S4-3).** A manifest's Drive modification time is its upload time; the
  file's own time is only in the manifest and `wasl.m`, which listings read, so nothing is fetched
  again because of it.
- **Rollback (S4-4).** Every 2.1 writer keeps an authenticated record (`.wasl-state.<writer>`:
  counter, time, digest of the live files' placements and `wasl.m` blobs) and rewrites it after
  each write. A device refuses a listing whose records went back or lost one (the folder is older
  than what it saw; kept per pair across restarts), before anything is applied, and reports a
  listing that does not match the newest record (a file brought back, removed or rolled back with
  the records kept, or a writer that has not published yet: the two look alike). A device that
  never listed the vault before cannot tell, and 2.0 vaults have no records.
- **Upgrade (S4-2).** v1 never starts a new folder. A v1 or 2.0 vault is copied, restartably, into
  a new 2.1 vault in a new folder; the old folder is never changed, and pairs and views are moved
  by hand (ENCRYPTION.md §11.12 lists the steps).

## 2. Files on demand (`wasl-vfs`)

A pair can be **on demand** (Drive, vault v1/v2 or both): its files appear in the local folder at
full size with their times, but take no space until read. Reading fetches the needed ranges
(Drive range requests; vault v2 chunks), keeps them in a cache, and a pinned file stays complete
on the device (FR-O5).

### 2.1 Core (`wasl-vfs`, portable)

- **Placeholders** come from the pair's listing (the engine's remote listing and baseline).
- **Block cache**: content in 1 MiB blocks (vault v2: its chunks) on disk under the data folder,
  an LRU over bytes, default 20 GB on a computer and 2 GB on the phone; pinned files' blocks are
  never evicted (FR-O5's acceptance). A fully cached file can be **hydrated** (made a normal local
  file); pinning hydrates.
- **Reads** go through one `Hydrator`: given a file and a byte range it returns the bytes, from the
  cache or the remote, with read-ahead for sequential readers. Seeking within a 20 GB file on a
  100 Mbit link costs one block (FR-O2's acceptance: under 2 s).
- **Writes**: a file written locally becomes an ordinary local file and syncs as today.

### 2.2 Platforms

| | Mechanism | Crate / target | Notes |
|---|---|---|---|
| Windows | Cloud Files API (placeholders, hydrate on open), the OneDrive mechanism | `wasl-vfs-windows` | Calls Win32 through the `windows` crate: the only crate where `unsafe` is allowed, kept to thin wrappers with the safety reasoning written at each call |
| Linux | FUSE 3 mount (`fuser`) | `wasl-vfs::fuse` | The pair's folder becomes a mount while wasld runs |
| macOS | File Provider extension (`NSFileProviderReplicatedExtension`), one domain per view | `desktop/macos-provider` (Swift) talking to wasld; `wasld::provider` | Builds and is tested on a Mac; running it needs an Apple signing team (§2.6) |
| Android | DocumentsProvider (exists since 1.1.4) | app | Reads through the engine's `Hydrator` so vault v2 and the cache are shared |

### 2.3 Views, not pairs (design decision 2026-10-22)

Files on demand ships first as **read-only views**: a view shows one Drive folder (plain, vault v1
or vault v2) at a local path, fetches a file when something opens it and keeps pinned files on the
device, but nothing written there is uploaded. Editing stays the job of ordinary synced pairs.
Views are their own records (`views` table), so the planner, the baseline and the safety layer of
pairs are untouched. A view refreshes its tree from the Drive listing on a schedule and on Drive
changes. On Windows its placeholders are read-only files; on Linux the mount is read-only; a local
change Windows still lets through (a placeholder made writable by hand) is never uploaded: on
the next refresh its content is set aside under `<data>/views/<id>/changed-here/` and the file
gets Drive's version back, and the view reports it (security review S3-2). A file created in the
view that Drive doesn't have is kept and reported. Two-way on-demand pairs (OneDrive-like) come in a later step.

A view of a vault's folder is linked to the vault when it mounts (the vault is recorded on the
computer, locked, if it is new there). While the vault is unlocked the view lists through it
(decrypted names, plaintext sizes) and reads a v1 file a 1 MiB chunk at a time or a v2 file by
its chunks. While it is locked the view shows nothing and says so; locking cuts off its reads at
once (even a fetch already asked for) and drops its cached blocks, keeping only pinned files,
which the person chose to keep on the device.

### 2.4 Safety

On-demand pairs keep every rule of the safety layer: a placeholder is never "deleted locally"
because it is not hydrated; an evicted block is not a change; a remote delete of a pinned file
goes to the bin like any other.

A plain pair never runs on a folder that holds a vault: a `vault.json` at the root, or in a
subfolder the pair doesn't exclude, stops it before planning, and one that can't be read counts
as a vault (`wasld::vaults::PlainGuard`).

### 2.5 Windows: whole-file fetches on open (finding, 2026-10-22)

On the development PC, opening a view's file for reading makes Windows ask Wasl for the whole
file, in the opening program's name, before the program reads anything; opening it for
attributes only asks for nothing. It happened for every program tried (Python, PowerShell 7,
Rust and .NET programs, whatever the open flags) except Windows PowerShell, which gets exactly the
4 KiB blocks it reads. No sync root setting changes it (partial or progressive hydration, any
modifier) and neither does the offline attribute. An on-access scan in the opener's context
(Defender real-time protection is on there) fits; confirming it needs a Defender exclusion for a
test folder. `OnDemandRoot::recent_fetches` names the program behind every fetch, and the ignored
`who_fetches` test in `wasl-vfs-windows` repeats the experiment.

### 2.6 macOS: File Provider (2026-10-28)

On macOS a view is a **File Provider domain** of Wasl.app's extension. The system keeps the
domain's folder (`~/Library/CloudStorage/Wasl-<view name>`, listed in Finder's sidebar under
Locations), creates dataless placeholders as Finder looks into folders, and asks the extension
for items and content when something opens a file. The extension asks wasld; it never talks to
Drive and holds no keys, so vaults and the cache stay wasld's.

```
Finder / any app ──▶ fileproviderd ──▶ WaslFileProvider.appex (sandboxed)
                                          │  Unix socket + token in the app group's container
                                          ▼  ~/Library/Group Containers/<team>.com.kahflabs.wasl/
wasld ── ProviderHost: the views' trees, Hydrator ──▶ fetch/<random>  (content for the system)
  │
  └── runs wasl-fileprovider (in Wasl.app): add / remove / signal / evict domains
```

- **Why two processes talk this way.** An app extension is always sandboxed: it can reach the
  app group's container but not wasld's data folder or socket. wasld, which runs from Wasl.app,
  opens a second socket there (`wasld.sock`, with its own `ipc.token`, both this user's only) with
  the usual handshake (`wasl-ipc` `auth`). That socket answers four requests only
  (`ProviderItem`, `ProviderList`, `ProviderChanges`, `ProviderFetch`; anything else is
  `Permission`), so whatever can reach the container can read the views' files and nothing else.
  The other direction, wasld telling the system something changed, goes through
  `wasl-fileprovider`, a helper next to wasld in `Wasl.app/Contents/MacOS`: only code inside the
  app holding the extension may manage its domains, and wasld (Rust, no `unsafe`) does not call
  Objective-C itself.
- **Items.** An item's identifier is its path in the view (the root container is `""`), its
  content version is the file's version (`FileRef::version`), and its metadata version changes
  with name, size, time and pin. Every item is read-only (no writing, renaming, moving or
  deleting capability). Paths rather than Drive ids: folders come from paths (`Tree`), and a file
  renamed on Drive then shows as removed and added, which a read-only view can afford.
- **Changes.** wasld keeps the tree before the last refresh. The working set's change
  enumeration from the previous anchor gets exactly what was updated and deleted; from any other
  anchor (older, another run of wasld, or after a pin) it gets `syncAnchorExpired` and the system
  lists again. After each refresh that changed something, and after a pin, wasld runs
  `wasl-fileprovider signal`.
- **Content.** `fetchContents` asks for the whole file; `fetchPartialContents`
  (`NSFileProviderPartialContentFetching`, macOS 12.3+) for a range, aligned as the system asks.
  wasld reads through the view's `Hydrator` (cache first, then Drive or the vault, with
  read-ahead) in 8 MiB pieces into a new file under `fetch/` in the container, a range at its own
  offset with a hole before it, as the system requires; the system clones and unlinks it. Files
  the system never took (a crash in between) are removed after an hour, at wasld's start, and
  when the view's vault is locked or the view removed, so no decrypted content stays there. So
  opening a large file reads the blocks the reader needs, as on Linux.
- **Pins.** `wasl view pin` fetches the file into the cache (pinned blocks are never evicted) and
  the item's content policy becomes `downloadEagerlyAndKeepDownloaded`: the system downloads it
  from the cache and keeps it. Unpinning returns it to the default (fetched on open, evictable).
  A path in the view's folder or in the domain's folder both work.
- **The view's folder.** The folder given to `wasl view add` becomes a symbolic link to the
  domain's folder (it must be new or empty, as everywhere). Removing the view removes the domain
  with every local copy, and the link.
- **Lock and stop.** When wasld stops, or the view is turned off, the domain stays and the
  extension answers "not authenticated" (the system shows the domain as unavailable) until the
  view shows again. Locking a vault evicts every file of its view that is not pinned
  (`wasl-fileprovider evict`); a view of a locked vault left from an earlier run has its whole
  domain removed (wasld does not know what it held), and pinned files come back from the cache
  when the vault is unlocked. Keeping files when removing a view is not available yet (as on
  Linux).
- **Not uploaded, ever.** What the system reports anyway (a file written into the folder from
  Terminal) is answered with `excludedFromSync`: it stays on this Mac and is never uploaded.

#### Building and packaging

`desktop/macos-provider/build.sh` builds the extension and the helper with the Swift compiler of
the Command Line Tools (no Xcode project); `npm run package` runs it (through
`scripts/stage-engine.mjs`) and `tauri.bundle.conf.json` copies
`WaslFileProvider.appex` into `Wasl.app/Contents/PlugIns` and `wasl-fileprovider` into
`Contents/MacOS`. wasld finds the helper next to itself (or at `WASL_FILEPROVIDER_CTL`); without
it, or when the helper can't find the app group, views are not available, as before.

#### Signing (required to run it)

The system loads a File Provider extension only from an app that is signed by an Apple team and
carries the team's app group. Tried on the development Mac (macOS 26, no signing identity) with an
ad-hoc signed app: LaunchServices registers the app as launch-disabled, PlugInKit never offers its
extension, and adding a domain fails with `NSFileProviderErrorDomain -2001` (provider not found).
What is needed:

1. An Apple Developer team (membership): its **team id** and a signing identity in the
   keychain: *Apple Development* to try it on registered Macs, *Developer ID Application* to
   ship (`security find-identity -v -p codesigning` lists them).
2. The **app group** `<team id>.com.kahflabs.wasl` (macOS form, team id prefix: no
   provisioning profile needed with Developer ID). `build.sh` writes it into the extension's
   `Info.plist` (`WaslAppGroup`, `NSExtensionFileProviderDocumentGroup`) and both entitlement
   files.
3. Build and sign, then notarize as usual:

   ```
   WASL_TEAM_ID=ABCDE12345 WASL_SIGN_IDENTITY="Developer ID Application: …" npm run package
   WASL_TEAM_ID=ABCDE12345 WASL_SIGN_IDENTITY="Developer ID Application: …" \
     macos-provider/sign.sh src-tauri/target/release/bundle/macos/Wasl.app
   ```

   `sign.sh` signs inside out: the extension (sandbox and app group), then `wasld`, `wasl`,
   `wasl-fileprovider` and the app (app group; wasld needs it to use the container without a
   prompt). The DMG is made again from the signed app.

#### Status

Done and tested: the provider protocol and `ProviderHost` (items, listings, change sets, whole
and partial content at its offset, pins; `wasld/tests/provider.rs`, including the real socket and
handshake), the macOS back end with a stand-in helper (domain added, folder linked, signals on
change, pins through either folder, removal), and the Swift extension and helper building and
packaged in `Wasl.app`. Not yet seen running, for want of signing: the extension in Finder, the
helper's domain calls, and how often the system asks for partial content. The first signed build
should check: the view under Locations, its folders listed, a file opened (hydrated from the
cache or Drive), a pin downloading and keeping a file, a vault lock evicting, and that
`wasl-fileprovider` (a tool in `Contents/MacOS`, not the app's main executable) is accepted as the
app by `NSFileProviderManager`; if it is not, the tray app has to manage the domains instead.

## 3. Phases

1. **Vault v2 format**: keys, names, manifests, chunks; test vectors; the Python reference and the
   recovery CLI; cross-check tests. (`wasl-crypto`, `wasl-cli`, `tools/`)
2. **Vault v2 remote**: the engine syncs v2 pairs (chunked upload, manifest swap, chunk sweep),
   v1 unchanged; measured edit cost. (`wasl-engine`)
3. **Hydrator and cache**: ranges from Drive, v1 and v2; LRU with pins; read-ahead. (`wasl-vfs`)
4. **Linux FUSE** mount, testable in CI. (`wasl-vfs`)
5. **Windows Cloud Files** placeholders and hydration. (`wasl-vfs-windows`)
6. **Clients**: on-demand pairs and pins in the CLI, tray app and Android; Android's
   DocumentsProvider on the hydrator. **macOS File Provider**: built and packaged; running it
   needs an Apple signing team (§2.6).
