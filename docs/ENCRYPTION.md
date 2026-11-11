# Drive Sync — Encrypted vaults (format v1; format v2 in section 11)

Client-side encryption for folder pairs. Everything under the pair's Drive folder is encrypted on
the phone before upload and decrypted after download; Google only ever stores ciphertext. This
document is the byte-exact specification (a desktop tool, `tools/decrypt_vault.py`, implements it
independently), the threat model, and the how-to for using a vault on more than one device.

## 1. Terms

| Term | Meaning |
|---|---|
| Vault | A passphrase and the key material derived from it. One vault can be used by several folder pairs. |
| Manifest | `vault.json`, the only plaintext file in an encrypted Drive folder (section 3). |
| keyEnc | 256-bit AES key for names and contents. |
| keyName | 256-bit HMAC key for deterministic name nonces and the key check. |

All strings below are UTF-8. `||` is concatenation. Integers are unsigned big-endian.
`base64` is RFC 4648 standard base64 with padding; `base64url` is the URL-safe alphabet
(`-` and `_`) **without** padding.

## 2. Key derivation

```
salt      = 16 random bytes (stored in the manifest)
iterations = 600000 by default (stored in the manifest)
master    = PBKDF2-HMAC-SHA256(password = passphrase UTF-8 bytes, salt, iterations, dkLen = 64)
keyEnc    = master[0..32)
keyName   = master[32..64)
keyCheck  = HMAC-SHA256(key = keyName, message = "drivesync-vault-check-v1")   (32 bytes)
```

`keyCheck` lets a device reject a wrong passphrase before touching any file. It reveals nothing
about `keyEnc` (the two halves of the PBKDF2 output are independent).

The Kotlin implementation runs PBKDF2 on top of `HmacSHA256` directly (RFC 8018), so the
passphrase is always its UTF-8 bytes on every Android version. Test vectors from RFC 7914 §11 are
part of the unit tests.

## 3. Manifest `vault.json`

Stored in the root of the encrypted Drive folder with the literal name `vault.json`, uploaded when
the first encrypted pair uses an empty folder. Never encrypted, never listed as a sync item.

```json
{
  "format": "drivesync-vault",
  "version": 1,
  "kdf": "PBKDF2-HMAC-SHA256",
  "iterations": 600000,
  "salt": "<base64, 16 bytes>",
  "keyCheck": "<base64, 32 bytes>",
  "created": "2026-09-30T12:34:56.789Z",
  "name": "Photos vault"
}
```

Readers must ignore unknown keys and must refuse a different `format`, `version` or `kdf`.
Before an encrypted pair syncs, the app verifies that the folder's manifest has the same salt,
iterations and keyCheck as the pair's vault; a mismatch aborts with "The Drive folder belongs to
a different vault/passphrase". A non-empty folder without a manifest is refused ("Encrypted pairs
need an empty Drive folder or an existing vault").

## 4. Name encryption

File and folder names are encrypted, and the same plaintext name inside the same parent folder
always yields the same ciphertext (so a re-created folder keeps its Drive name and the Drive
duplicate-name check keeps working). The nonce is derived, then embedded, so decrypting a name
needs only the keys and not the parent.

```
nameUtf8    = UTF-8 bytes of the plaintext name
parentToken = the Drive file id of the parent folder ("" when unknown, e.g. some renames)
nonce       = HMAC-SHA256(keyName, "name-nonce" || 0x00 || parentToken || 0x00 || nameUtf8)[0..12)
ct          = AES-256-GCM(key = keyEnc, iv = nonce, plaintext = nameUtf8,
                          aad = "drivesync-name-v1", tag = 128 bits)          // ciphertext || tag
encrypted   = base64url(nonce || ct) + ".dsv"
```

Decryption: strip `.dsv`, base64url-decode (reject padding or foreign characters), split the
first 12 bytes as the nonce, AES-GCM-decrypt the rest with the same AAD, decode strict UTF-8, and
reject empty names, names containing `/`, `.` and `..`. Anything that fails is **not a vault
item** and is ignored by the sync (logged, never deleted). `vault.json` at the root is skipped
explicitly. Google-native documents (Docs/Sheets/Slides) inside an encrypted folder are ignored
entirely.

Encrypted names are 4/3 × (28 + UTF-8 length) characters plus 4, e.g. `hello.txt` becomes a
54-character name. Drive has no practical limit here; local names stay plaintext.

## 5. Content encryption

```
CHUNK      = 1048576 (1 MiB)
fileNonce  = 8 random bytes (fresh for every encryption of every file)
output     = "DSV1" || fileNonce || chunk_0 || chunk_1 || ... || chunk_(n-1)

plaintext is split into n = max(1, ceil(len / CHUNK)) pieces of CHUNK bytes; the last piece
holds the remainder and may be empty (an empty file has exactly one empty chunk).

for chunk index i (uint32, 0-based), lastFlag = 1 if i == n-1 else 0:
  iv_i    = fileNonce || i                                   (12 bytes)
  aad_i   = "drivesync-chunk-v1" || i (4 bytes) || lastFlag (1 byte)
  chunk_i = AES-256-GCM(keyEnc, iv_i, plain_i, aad_i)        (raw ciphertext || 16-byte tag)
```

Chunk boundaries are implied: every chunk but the last is exactly CHUNK + 16 bytes; the last is
whatever remains (at least 16 bytes). Encrypted size = 12 + len + 16 × n.

Decryption rules:
* the 4-byte magic must be `DSV1`, otherwise fail ("unknown format or version");
* every tag is verified with the chunk's own iv/aad — a flipped byte anywhere fails;
* the final chunk of the stream must verify with `lastFlag = 1` — a stream cut at a chunk
  boundary, cut mid-chunk, or extended with extra bytes fails ("modified or truncated");
* a stream with a header but no chunk fails.

Errors surface as `AppError.Integrity` in the app and as `VaultError` in the Python tool.
Nothing is buffered beyond two chunks, so multi-gigabyte files stream.

Nonce budget: the 64-bit random `fileNonce` plus the 32-bit counter give a unique IV per chunk;
the birthday bound across files is about 2^32 encryptions for a 50 % collision chance — far
beyond a personal vault, but a reason not to reuse one vault for billions of files.

## 6. Where the keys live on the phone

Unlocking derives the keys once and stores them in `filesDir/vaults/<vaultId>.key` wrapped with
an Android Keystore AES-256-GCM key (alias `drivesync-vault-wrap`, no user authentication so
background syncs work). File layout: 12-byte IV || AES-GCM ciphertext of `keyEnc || keyName`.
The Keystore key never leaves the device, so a backup or a copy of the app data cannot unlock a
vault. "Lock" deletes the file; the vault metadata (salt, iterations, keyCheck) stays in the app
database and holds no secret.

On a computer, `wasld` keeps the same key material (v1: keyEnc ‖ keyName; v2: the Argon2id
output) in the OS keychain (Windows Credential Manager, macOS Keychain, Secret Service) under the
service `Wasl vaults`, one entry per vault, as `<version>:<hex>`. It is read when a vault pair runs
or a vault view mounts, checked against `keyCheck`, and wiped from memory when dropped. The
`vaults` table of its database holds only the vault's `vault.json` and the Drive folder it came
from. The passphrase travels once from the CLI or the tray app to `wasld` over the local socket
(only the same OS user's clients can connect), the keys are derived off the async threads, and
the passphrase is wiped; it is never stored or logged. "Lock" (`wasl vault lock`) deletes the
keychain entry, stops the vault's running syncs, cuts off its views at once and drops their cached
content except the files someone chose to keep on the computer. New vaults, on the phone and on a
computer, are always format 2.1 (§11.10), and a v1 vault never starts another Drive folder: a v1
pair on an empty folder is refused ("can't start a new Drive folder"); existing v1 folders stay
readable and writable. `wasld` tells whether a vault is unlocked without reading its keys each
time: it remembers what it last wrote or read (the keychain entry is read at most once per start
for that). The daemon's socket wipes each message line once it is parsed, since unlocking and
creating carry the passphrase.

## 7. Threat model

Protects:
* file **contents** and file/folder **names** from Google and from anyone who can read the Drive
  account (shared access, leaked credentials, subpoenas, a stolen laptop with a synced copy);
* against silent corruption or tampering of stored data (every chunk and name is authenticated,
  truncation is detected);
* deterministic names leak only *equality*: an observer can tell that two folders contain an item
  with the same name, but not the name.

Does **not** hide (v1; for v2 see 11.6 and 11.10, which hide modification times and, in 2.1,
exact sizes):
* file sizes (ciphertext is plaintext + 12 + 16 per MiB), modification times, the folder
  structure (depth and number of items per folder), and the vault's name/creation time in the
  manifest;
* which files change and when;
* where an item sits: names are authenticated, but not bound to their folder, and contents are not
  bound to their name. Someone who can write to the Drive folder can move an encrypted item to
  another folder of the same vault, or swap two items' contents, without either failing to
  decrypt. v1 keeps this behaviour so existing vaults stay readable everywhere (the app, Wasl's
  Rust engine and `tools/decrypt_vault.py` all decrypt a name without its parent); vault v2 binds
  every chunk to its file (SRS FR-V2);
* anything from someone who can unlock the phone (the keys are stored for background sync) or who
  learns the passphrase. There is no recovery: a lost passphrase means lost data.

Not in scope: hiding that Drive Sync is used, protecting against a malicious app on the device,
or protecting the local (plaintext) folder on the device.

## 8. Using a vault on another device (Join existing vault)

1. On the first device: Vaults → the vault → **Manifest** → **Copy**. The text is the same
   `vault.json` that sits in the Drive folder, so you can also open that file on Drive.
2. On the second device: Vaults → **Join existing vault** (link icon), paste the text, enter the
   passphrase, Join. The passphrase is checked against `keyCheck`; the vault appears unlocked.
3. Create the folder pair on the second device with the same Drive folder and choose the vault.
   The app verifies the folder's `vault.json` matches before the first sync.

Moving the phone: repeat the steps above on the new phone. Deleting a vault removes only the
device's copy of the metadata and keys; the encrypted files on Drive are untouched and can be
joined again as long as the passphrase is known.

## 9. Desktop decryption: `tools/decrypt_vault.py`

Download the whole encrypted folder from Google Drive (drive.google.com → right click → Download,
then unzip) so that `vault.json` sits next to the `.dsv` items, then:

```
pip install cryptography
python3 tools/decrypt_vault.py <vault_folder> <output_dir>            # prompts for the passphrase
python3 tools/decrypt_vault.py <vault_folder> <output_dir> --passphrase 'your words here'
python3 tools/decrypt_vault.py <vault_folder> <output_dir> --list      # names only, writes nothing
python3 tools/decrypt_vault.py <vault_folder> <output_dir> --overwrite # replace existing outputs
```

The tool reads the manifest, derives the keys, checks the passphrase, decrypts names and contents
recursively, verifies every tag and prints a summary (folders, files, bytes, skipped non-vault
items, failures). Exit codes: 0 ok, 1 usage/manifest problem, 2 wrong passphrase, 3 some files
failed verification. Items whose names do not decrypt (files someone else dropped into the folder)
are listed as skipped, never treated as errors.

## 10. Constants (copy-paste reference)

| Constant | Value |
|---|---|
| Manifest file name | `vault.json` |
| Manifest format / version / kdf | `drivesync-vault` / `1` / `PBKDF2-HMAC-SHA256` |
| PBKDF2 output | 64 bytes → keyEnc[0..32) keyName[32..64) |
| Key check message | `drivesync-vault-check-v1` (HMAC-SHA256 with keyName) |
| Name nonce label | `name-nonce` (HMAC-SHA256 with keyName, 12-byte prefix) |
| Name AAD | `drivesync-name-v1` |
| Name suffix | `.dsv` |
| Content magic | `DSV1` (0x44 0x53 0x56 0x31) |
| File nonce | 8 random bytes |
| Chunk size | 1 MiB (1048576 bytes) plaintext |
| Chunk IV | fileNonce ‖ uint32 index |
| Chunk AAD | `drivesync-chunk-v1` ‖ uint32 index ‖ lastFlag byte |
| Tag | 128 bits |

## 11. Format v2

Vault v2 (design: `docs/ON_DEMAND.md` §1) cuts every file into content-defined chunks, each its
own encrypted Drive object, with a small encrypted manifest per file. An edit uploads only the
chunks that changed, and a reader decrypts any byte range by fetching only the chunks covering it
(SRS FR-V2). v1 (sections 1–10) stays readable and writable unchanged. Implementations:
`wasl-crypto::v2` (Rust), `tools/decrypt_vault.py` (Python, independent), checked against each other
by the known-answer vectors in `engine/crates/wasl-crypto/tests/vectors/v2.json` and by
cross-check tests in both directions. Conventions as in section 1; `hex` is lowercase.

### 11.1 Keys

```
salt     = 16 random bytes                                        (vault.json)
ikm      = Argon2id v1.3 (passphrase UTF-8, salt, m KiB, t, p, no secret, no AD, 32 bytes)
kName    = HKDF-SHA256(salt = none, ikm, info = "wasl-v2-name",  L = 64)   AES-256-SIV key
kMeta    = HKDF-SHA256(salt = none, ikm, info = "wasl-v2-meta",  L = 32)   AES-256-GCM key
kChunk   = HKDF-SHA256(salt = none, ikm, info = "wasl-v2-chunk", L = 32)   AES-256-GCM key
kChunkId = HKDF-SHA256(salt = none, ikm, info = "wasl-v2-cid",   L = 32)   HMAC-SHA256 key
kDir     = HKDF-SHA256(salt = none, ikm, info = "wasl-v2-dir",   L = 32)   HMAC-SHA256 key
keyCheck = HMAC-SHA256(HKDF-SHA256(salt = none, ikm, "wasl-v2-check", 32), "wasl-vault-check-v2")
```

New vaults use m = 65536 KiB (64 MiB), t = 3, p = 1. Readers refuse anything outside
m 19456..262144 KiB (19..256 MiB), t 1..4, p 1..4 (and m × t at most 512 MiB-passes), so an edited `vault.json` cannot make a
device spend unbounded time or memory. A device keeps `ikm` (wrapped like v1's keys), never the
passphrase. All key material is zeroized when dropped.

### 11.2 `vault.json` v2

```json
{
  "format": "wasl-vault",
  "version": 2,
  "kdf": "argon2id",
  "m": 65536,
  "t": 3,
  "p": 1,
  "salt": "<base64, 16 bytes>",
  "keyCheck": "<base64, 32 bytes>",
  "created": "2026-10-21T12:34:56.789Z",
  "name": "Photos vault"
}
```

`m` is in KiB. `version` is 2 for format 2.0 and 3 for format 2.1 (§11.10); everything else in
the file is the same. Readers ignore unknown keys and refuse another `format`, a `version` they do
not know, another `kdf`, costs outside the bounds above, and a salt or keyCheck of the wrong length
or not canonical base64. A reader tells v1 from v2 by `format` (`drivesync-vault` is v1,
`wasl-vault` is v2), and 2.0 from 2.1 by `version`. Two `vault.json` describe the same vault only
when their salt, cost, key check and `version` all match, so a 2.1 file edited back to 2.0 on
Drive is refused like another vault's.

### 11.3 Layout

```
vault.json
<name>.w2/                         a folder
<name>.w2/.wasl-dir                the folder's authenticated dir id (11.5)
<name>.w2/<name>.w2                a file manifest (11.6)
.wasl-chunks/<00..ff>/<chunk>      every chunk object (11.7), in 256 shard folders
.wasl-state.<16 hex>               a writer's state record (11.11; format 2.1)
```

At the root, `vault.json`, `.wasl-chunks` and `.wasl-state.*` are skipped by name; everywhere,
`.wasl-dir` is.
Anything whose name does not decrypt is not a vault item: ignored by the sync, listed as skipped
by recovery, never deleted.

### 11.4 Names and placements

```
dirId      = 16 random bytes per folder; the vault root's is 16 zero bytes
siv        = AES-256-SIV(kName, nameUtf8, AD = [parent dirId])   RFC 5297, one AD component;
                                                                  output V (16) || C
encrypted  = base64url(siv) + ".w2"
placement  = parent dirId (16) || siv                             where an item lives
```

Names are deterministic within their folder (the Drive duplicate-name check keeps working) and
bound to it: an item moved to another folder no longer decrypts. Decryption strips `.w2`, accepts
only canonical unpadded base64url (no padding, no foreign characters, no non-zero trailing bits),
requires more than 16 bytes, verifies, decodes strict UTF-8, and refuses empty names, `.`, `..`,
and names containing `/` or NUL. Recovery also refuses names that are not one plain path component
on the local system (for example containing `\` on Windows).

### 11.5 Folder ids

```
tag        = HMAC-SHA256(kDir, "wasl-v2-dir" || folder placement || dirId)
.wasl-dir  = "W2D1" || dirId (16) || tag (32)                  (52 bytes, inside the folder)
wasl.dir   = base64url(dirId || tag)                           (appProperties, 64 characters)
```

The `wasl.dir` property is what the engine reads; the `.wasl-dir` object repeats it because a Drive
download (zip) drops appProperties, and recovery must still name a folder's children. Both are
bound to the folder's own placement, so a folder id cannot be changed or copied from another
folder (which would re-key the children's names) without failing verification. Renaming or moving
a folder rewrites its `wasl.dir` and `.wasl-dir`, not its children.

### 11.6 File manifests

```
manifest = "W2M1" || nonce (12, random)
           || AES-256-GCM(kMeta, cbor, AAD = "wasl-v2-manifest" || file placement)
cbor     = map(6) {
             "size":    uint                         plaintext bytes
             "mtime":   int                          ms since the Unix epoch
             "blake3":  bytes(32)                    BLAKE3 of the whole plaintext
             "chunks":  [ [bytes(32) chunk_id, uint len], ... ]   in file order
             "file_id": bytes(16)                    random, fixed for the file's life
             "version": uint                         1 for a new file, +1 on every write
           }
```

The CBOR is deterministic (RFC 8949 §4.2.1): definite lengths, shortest heads, keys in exactly the
order above (the bytewise order of their encodings). Readers accept only that encoding and refuse:
a sealed manifest over 64 MiB, more than 1,048,576 chunks, a chunk length outside 1..4194304, chunk
lengths not summing to `size`, version 0, trailing bytes. Binding the AAD to the placement means a
manifest swapped with another file's, or moved to another folder or name, does not open; a rename
or move re-seals the manifest (it is small).

**Rollback.** Someone who can write to the Drive folder can put back an older, genuine manifest
of the same file. A device detects this only against the highest `version` it has seen of that
file (the engine keeps it per file and refuses anything older). A device that never saw the newer
version cannot tell. A rollback of the whole folder to an earlier consistent state is caught in
format 2.1 by the state records of 11.11, within the limits listed there.

**Times on Drive.** A manifest's Drive modification time is when it was uploaded, not the file's
own time, which is only inside the manifest and the listing blob (security review S4-3). Listings
take the time from the blob (or the manifest), so devices compare the real time and nothing is
downloaded again because of it. v1 has no encrypted place for a time, so v1 files keep theirs on
Drive.

**Listing blob.** `appProperties["wasl.m"]` = base64(nonce (12) || AES-256-GCM(kMeta, size (u64) ||
mtime (i64) || blake3 (32), AAD = "wasl-v2-meta" || file placement)): 76 bytes, 104 characters, so
key plus value stay within Drive's 124-byte limit. It lets a listing show sizes and times without
downloading manifests; the manifest stays authoritative.

### 11.7 Chunks

```
cut      = FastCDC (2020, normalization level 1), min 256 KiB, avg 1 MiB, max 4 MiB;
           a file of at most 4 MiB is one chunk; an empty file has none
chunk_id = HMAC-SHA256(kChunkId, file_id || BLAKE3(plaintext))
object   = .wasl-chunks/<hex(chunk_id[0])>/<base64url(chunk_id)>        (43 characters)
content  = "W2C1" || nonce (12, random)
           || AES-256-GCM(kChunk, plaintext, AAD = "wasl-v2-chunk" || file_id || chunk_id)
```

Each chunk is bound to its file and its own keyed content id, not to its index (design decision,
ON_DEMAND.md §1.2: binding to the index would re-upload everything after an insertion). The order
is authenticated by the manifest's list and the whole file by its BLAKE3. A reader verifies the
tag, that the length matches the manifest, and that the plaintext really has this chunk id. A chunk
repeated inside one file is one object; two files never share chunks (the id is keyed per file).

### 11.8 Writing, reading, recovery

Writing: upload the chunks the new list has and the old lacks, then replace the manifest (new
content of the same Drive file, version + 1), then delete chunks only the old list had. A crash
leaves extra chunks, never a manifest pointing at a missing one; a sweep deletes unreferenced
chunks. A 1 MB edit of a 64 MiB file uploads one or two chunks (measured: about 4 MiB).

Reading a range fetches only the chunks covering it: a read at 4 GiB downloads one chunk.

Recovery: `wasl vault decrypt <folder> <output> [--passphrase-stdin] [--overwrite]` decrypts v1,
2.0 and 2.1 folders downloaded from Drive; `python3 tools/decrypt_vault.py <folder> <output>`
decrypts v1 and 2.0 (2.1 support in the Python reference is still to do).
The Python tool needs `cryptography` (AES-SIV, HKDF, AES-GCM); for Argon2id it uses
`cryptography` when its OpenSSL has Argon2, else `argon2-cffi`, else a built-in pure-Python
Argon2id (correct, slow: seconds at the minimum cost, about a minute at the default). With the
optional `blake3` package it also checks chunk ids and whole-file hashes; tags are always checked.

### 11.9 Constants

| Constant | Value |
|---|---|
| vault.json format / version / kdf | `wasl-vault` / `2` (2.0) or `3` (2.1) / `argon2id` |
| Argon2id default | m = 65536 KiB, t = 3, p = 1, version 0x13, 32-byte output |
| Argon2id reader bounds | m 19456..262144 KiB, t 1..4, p 1..4, m × t ≤ 512 MiB-passes |
| HKDF infos | `wasl-v2-name` (64), `wasl-v2-meta`, `wasl-v2-chunk`, `wasl-v2-cid`, `wasl-v2-dir`, `wasl-v2-check`, `wasl-v2-cdc`, `wasl-v2-state` (32) |
| Key check message | `wasl-vault-check-v2` |
| Name suffix / root dirId | `.w2` / 16 zero bytes |
| Folder id | `W2D1`, label `wasl-v2-dir`, object `.wasl-dir`, property `wasl.dir` |
| Manifest magic / AAD label | `W2M1` / `wasl-v2-manifest` |
| Listing blob | property `wasl.m`, AAD label `wasl-v2-meta` |
| Chunk magic / AAD label | `W2C1` / `wasl-v2-chunk` (2.0); `W2C2` / `wasl-v2.1-chunk` (2.1) |
| 2.1 gear table | HKDF-SHA256(kCdc, info `wasl-v2-gear`, 2048 bytes), 256 big-endian u64 |
| 2.1 padding | body = u32 len ‖ plaintext ‖ zeros, padmé(max(4 + len, 4096)) bytes |
| State record | `.wasl-state.<hex writer>`, `W2S1`, 92 bytes, HMAC label `wasl-v2-state`, set label `wasl-v2-state-set` |
| Chunk sizes | min 262144, avg 1048576, max 4194304 bytes |
| Chunk folder | `.wasl-chunks/<00..ff>/` |
| Manifest bounds | 64 MiB sealed, 1048576 chunks |
| Nonces / tags | 12 random bytes / 128 bits |

### 11.10 Format 2.1: keyed chunking and padded chunks

Format 2.0 cuts files with FastCDC's public gear table and stores each chunk at its exact size, so
someone who sees the Drive folder and also has a file can cut it the same way and look for its
sequence of chunk sizes (security review S4-1). Format 2.1 changes only how chunks are cut and
sealed; keys, names, folder ids, manifests and the listing blob are 2.0's.

```
kCdc     = HKDF-SHA256(salt = none, ikm, info = "wasl-v2-cdc", L = 32)
gear     = HKDF-SHA256(salt = none, ikm = kCdc, info = "wasl-v2-gear", L = 2048)
           read as 256 big-endian u64 values; gear_ls[i] = gear[i] << 1 (FastCDC 2020's shifted twin)
cut      = FastCDC 2020 exactly as 11.7 (same sizes, normalization, masks), with gear / gear_ls
           in place of the public tables
body     = u32 BE len || plaintext || zero bytes,   |body| = padmé(max(4 + len, 4096))
content  = "W2C2" || nonce (12, random)
           || AES-256-GCM(kChunk, body, AAD = "wasl-v2.1-chunk" || file_id || chunk_id)
chunk_id = as 11.7 (of the plaintext, not the body)
```

The whole table comes from a PRF, rather than FastCDC's own option of XORing one 64-bit seed into
every entry, which keeps the table's structure. padmé (Nikitin et al., PETS 2019): with
E = floor(log2 L) and S = floor(log2 E) + 1, L is rounded up to a multiple of 2^(E − S); it leaks
O(log log L) bits of a length. Overhead: at most 4 KiB for a chunk under 4 KiB; at most 6.25 %
between 4 and 64 KiB; under 3.2 % from 64 KiB up, which every chunk but a file's last is (256 KiB
minimum). A large file costs about 1.6 % on average plus up to one bucket for its last chunk. The
largest object is 32 + 4,325,376 bytes. Known answers: `engine/crates/wasl-crypto/tests/vectors/v21.json`.

A reader verifies the tag, then requires the length prefix to be at most 4 MiB, the body to be
exactly padmé(max(4 + len, 4096)) bytes and every padding byte to be zero (one accepted form), and
then checks the length and chunk id against the manifest as in 11.7. The magic decides the
framing; the two AAD labels differ, so a 2.0 body cannot be passed off as 2.1 or the reverse.

Compatibility: a 2.1 reader reads 2.0 vaults (both chunk framings open), and a 2.0 vault is written
as 2.0 (`W2C1`, public cuts) so devices that predate 2.1 keep reading it. A 2.1 vault's `vault.json`
says `"version": 3`, which every reader before 2.1 refuses ("vault format version 3 is not
supported here"), so none of them can write unpadded chunks into it. The JSON `version` changes
rather than a new key, because those readers ignore unknown keys. A vault does not change format
in place; it is copied into a new one (11.12).

What 2.1 still shows Google: each chunk's padded size, the number of chunks of a file (about its
size in MiB), the manifest's size (which grows with the number of chunks), the folder tree's shape,
when things change, and which chunks were uploaded together. Chosen-content attacks on keyed CDC
(someone who can make you store files they choose and then watch the sizes) can learn something
about the cut points; padding makes that much harder but is not a proof against it.

### 11.11 State records (rollback of a whole folder)

Manifest versions (11.6) catch an old manifest put back for a file a device has seen. They do not
catch the whole folder put back to an earlier consistent state, nor a deleted file brought back.
Each device that writes to a 2.1 vault keeps one record at the vault root and rewrites it after
every write (upload, rename or move, delete):

```
writer  = 8 bytes: a random 63-bit number (top bit clear), made once per device and pair, kept
name    = ".wasl-state." || hex(writer)
record  = "W2S1" || writer (8) || counter (u64 BE) || time_ms (i64 BE) || digest (32) || tag (32)
kState  = HKDF-SHA256(salt = none, ikm, info = "wasl-v2-state", L = 32)
tag     = HMAC-SHA256(kState, "wasl-v2-state" || writer || counter || time_ms || digest)
digest  = SHA-256("wasl-v2-state-set" || u64 BE n || for each live file, sorted by
                  (placement, blob): u32 BE len || placement || u32 BE len || blob)
```

`placement` is the file's (11.4) and `blob` its `wasl.m` property as listed (empty when missing).
The blob is sealed with a fresh nonce at every write, so it changes with every version and nobody
without the keys can make one: the digest names an exact set of file versions without downloading
any manifest. A writer's counter only grows (the next is one more than the highest it has stored
or listed); only that writer writes that record, so concurrent writers never race on it. Writes
that finish while a record is going up are covered by the next one; a record that cannot be
written is retried at the next write or run and never fails the write itself.

A reader verifies every record (one that does not verify, or whose name is not its writer's, is
ignored like any foreign item) and keeps, per pair, the highest counter seen of each writer and the
highest sum of all counters. It refuses the listing, before anything is planned or applied, when a
writer's counter is lower than seen or the sum is lower than seen (a record was removed or
replaced); the error says the folder is older than a state this device already saw. A listing
whose files do not match the digest of the newest record (by its time) is reported as unpublished.

Guarantees and limits, exactly:

* Detected and refused: the whole folder, or its records, put back to a state older than one this
  device has listed; a writer's record removed or replaced. Kept across restarts for pairs (the
  store's `vault_versions`); views keep it in memory only.
* Reported, not refused: a file brought back, removed or rolled back while the records were kept.
  That cannot be told apart from a writer that has written but not published yet (still running,
  or stopped before it could), or from someone restoring a file from Drive's bin on purpose.
* Not detected: anything on a device that has never listed the vault before (a new device trusts
  its first listing); a rollback to a state newer than the device last saw; 2.0 vaults (no
  records). The records show Google how many writes each device made and when.

### 11.12 Upgrading a vault

A v1 or 2.0 vault moves to 2.1 by copying it into a new vault in a new Drive folder
(`wasld::vaults::Vaults::upgrade`, built on the engine's `copy_vault`). The person types the old
vault's passphrase (checked against it); it becomes the new vault's, with a new salt. Each file is
decrypted from the old vault into the app's private temporary folder, encrypted into the new one
with its modification time, and removed; a rerun after an interruption continues (files the new
vault already lists with the same size and time are skipped). Only into the vault this upgrade
made: the computer records, when it makes the new vault, which vault it is an upgrade of, and a
target folder holding any other vault is refused, even one the same passphrase opens (otherwise the
copy would write into it). A file of the old vault that does not
decrypt is listed and the rest is copied. Nothing in the old folder is changed or deleted.

Pairs and views are not moved automatically: a pair's history belongs to its Drive folder, and
pointing it at another would make its next run see every file as new on both sides. After the copy:

1. Pause the old vault's pairs.
2. For each, make a new pair from a new, empty local folder to the new Drive folder and let it
   download; compare it with the old local folder.
3. Remove the old pair (its local files stay) and, if wanted, move the new local folder in place.
4. Replace each view of the old folder with a view of the new one.
5. Once every device uses the new vault, delete the old Drive folder by hand. Wasl never does.
