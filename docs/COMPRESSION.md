# Compression and chunking: design note

Status: steps 1–3 built 2026-10-26 (§4); steps 4–5 proposed. Owner direction: **Zstandard (zstd) as
the compression layer on top of content-defined chunking (CDC)** in the data pipeline, to save time,
data and bandwidth. This note says where it pays, where it cannot apply, and in what order to do it.

**Measured** on a copy of the KahfLabs workspace (6,540 files; 590 MB synced), two devices over QUIC:

| | Before | Now |
|---|---|---|
| First sync of the whole folder to a new device | 589.6 MB on the wire | 307.5 MB (internet level 3), 316.2 MB (LAN level 1), 299.6 MB (metered level 9) |
| 6 small edits in text files of 256 KiB–16 MiB | 7.97 MB (whole files) | 0.64 MB (8 %) |
| A Drive listing of the folder (8,010 items, 9 pages) | 3.03 MB of JSON | 0.70 MB gzip-encoded (23 %; estimate in Drive's format) |

## 1. Where Wasl stands

| Path | Chunking today | Compression today |
|---|---|---|
| Device ↔ device (QUIC, DEVICE_SYNC.md §5) | FastCDC 1/4/16 MiB (min/avg/max) for files over 16 MiB; smaller files are one chunk. Chunks named by BLAKE3; the receiver fetches only chunks it lacks, one at a time | None |
| Vault v2 on Drive (ENCRYPTION.md §11.7) | FastCDC 256 KiB/1 MiB/4 MiB; only changed chunks are uploaded | None |
| Plain Drive pairs | None: Drive stores whole files and has no delta upload | None |
| Drive metadata (list, changes) | n/a | None: requests do not ask for gzip |

(The table above is the state before this work.) CDC already did the "send only what changed" half.
Zstd adds the "send fewer bytes for what did change" half.

## 2. Decisions proposed

1. **Zstd per chunk on the device wire.** A chunk is compressed when it is sent and decompressed on
   arrival. Chunk ids stay the BLAKE3 of the *uncompressed* bytes, so deduplication, resume and
   verification do not depend on the compression level, and two peers with different settings still
   agree on every chunk. A capability bit in `hello` turns it on per connection (older peers keep
   working). Decompression is capped at the chunk's declared length (no decompression bombs).
2. **Only when it helps.** Media and archives are already compressed (JPEG, HEIC, PNG, WebP, MP4,
   MP3, ZIP, APK, 7z, gz, and Office files, which are ZIPs). A chunk is sent compressed only if a
   level-1 attempt on its first 64 KiB saves at least 10%; known compressed types skip the attempt.
   Typical gain: source code, Markdown, JSON and logs shrink 3–5×; photos and video do not shrink.
3. **Level by link and power**, not fixed: LAN on mains power → level 1 (fast enough not to slow a
   gigabit link); internet or a hole-punched path → level 3; metered or cellular → level 9; a phone
   on battery stays at level 1. The asker names the level; the sender caps it at 9 whatever is asked
   (higher levels cost many times the CPU for a few percent) and compresses at most two chunks at
   once for its peers, sending raw when both slots are busy, so no peer can tie up its CPU.
4. **Zstd inside vault v2 chunks** (compress, then encrypt): smaller uploads and less Drive storage.
   This needs a format revision: a new chunk magic (`W2C2`) with a codec byte. `W2C1` chunks stay
   readable forever. Trade-off to accept knowingly: the size of an encrypted chunk then reveals how
   compressible it was (text vs. media). Contents and names stay secret, and Wasl's threat model has
   no attacker who can mix their own data into your files, which is what CRIME-style attacks need.
   Proposed: on by default for new vault writes, with a per-vault switch to turn it off.
5. **Plain Drive pairs stay uncompressed.** Files on Drive must stay ordinary files that Drive's
   web, preview, search and other apps can open. Compressing them would break that, so this route
   gets no zstd.
6. **Crate:** `zstd` (the reference C library via `zstd-sys`). It builds with the C toolchain Wasl
   already needs for SQLite, including the Android NDK. LZ4 is faster but compresses less; zstd at
   level 1 is close to LZ4's speed with a better ratio, so zstd is the right single choice.

## 3. Better ideas on top (my recommendations)

1. **Smaller chunks for medium files on the device route.** Before this, a one-line edit to a
   10 MiB file resent the whole 10 MiB, because files up to 16 MiB were one chunk. A second CDC tier
   for files from 256 KiB to 16 MiB (16 KiB / 64 KiB / 256 KiB) makes such an edit cost about one
   small chunk. Most daily edits (documents, code, notes) are in this size range, so **this is the
   biggest single bandwidth win for edits** (measured: 92 % less). The cutting parameters stay
   pinned (`fastcdc = 3.2.1`), but no protocol bump is needed: a list names its chunks, so a device
   that cuts differently still syncs correctly, only with less reuse.
2. **Gzip on Drive metadata.** Ask for `Accept-Encoding: gzip` on Drive list and changes calls
   (reqwest's `gzip` feature). Large accounts list hundreds of thousands of items as JSON, which
   compresses about 4× (ids and checksums are random and do not shrink). Cheap, safe, and it helps
   every Drive pair, including plain ones.
3. **Zstd dictionaries for many small files.** Source trees are thousands of small files that look
   alike, and zstd compresses those poorly one at a time. A dictionary trained per pair from a
   sample of its files, shared with peers once, typically doubles the ratio on files under 16 KiB.
   Worth it after 1 and 2.
4. **One chunk table per device, not per pair.** A file copied or moved between two pairs on the
   same device then costs no transfer. Small change, since chunk ids are already content hashes.
5. **Prefer the nearest copy.** When a file is both on Drive and on a device on the same network,
   fetch its chunks from the device (LAN speed, no Drive quota). FR-C6 does this for whole files
   already; extend it to chunks.

## 4. Order and status

| Step | What | Status |
|---|---|---|
| 1 | Gzip on Drive metadata | **Built.** `wasl-drive` asks for gzip on every call (user agent says "gzip", as Google requires); file content asks for `identity`, so resume offsets, sizes and MD5 are unchanged |
| 2 | Zstd per chunk on the device wire, adaptive, skip what does not shrink | **Built.** `wasl-net::compress`; the level comes from the path (`wasl-devices::compression_level`); Android reports metered and battery before each round (`setBudget`). Desktop does not yet detect a metered network or battery, so a computer counts as on mains power and unmetered |
| 3 | Second CDC tier for files up to 16 MiB, plus parallel chunk fetching | **Built.** `ChunkParams::MEDIUM` (16/64/256 KiB) via `ChunkParams::for_size`; existing pairs are cut again once on the next scan (no new versions); 8 small or 3 large chunks in flight. No protocol bump was needed: lists name their chunks, so older devices stay correct, only with less reuse |
| 4 | Vault chunk format `W2C2` with zstd | Proposed |
| 5 | Per-pair dictionaries, device-wide chunk table, nearest-copy fetch | Proposed |

How it was checked: unit tests for packing, unpacking and hostile frames (a decompression bomb, a
lying length); protocol tests that a device from before compression reads requests and is never sent
a compressed chunk; end-to-end QUIC tests (text compressed, noise raw, a sender with no budget sends
raw, an 8 MiB document edit fetches at most two small chunks, an old pair is cut again once without
a new version); a Drive API test that metadata arrives gzip-encoded and file bytes do not; and the
measurement above (`measure_a_real_tree` in `wasl-engine/tests/devices.rs`, run with
`WASL_MEASURE_TREE=<folder>`).

Each remaining step keeps old peers and old vaults working and ships with tests in `wasl-chunk`,
`wasl-net` and `wasl-crypto`. The recovery tools (`wasl vault decrypt`, `tools/decrypt_vault.py`)
learn the new vault format in the same change as step 4.
