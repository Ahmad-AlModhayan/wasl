# Device sync (M4): design

This is how Wasl devices sync with each other directly, with or without Google Drive in the
middle (SRS §3.3 FR-C1–C8, §3.6 FR-N1–N12, FR-P2–P3). Wasl's own design; other tools were read for
research only. Each section names the crate that implements it.

## 1. Words

- **Device**: one install of Wasl (a phone, a computer). Identified by its Ed25519 public key.
- **Group**: the devices of one person, listed in a signed `group.json` in the Drive app-data
  folder of one Google account (FR-N3, FR-D5).
- **Pair** (as today) with a **route**: `drive` (today's behaviour), `drive_and_devices`, or
  `devices`; and a **reach**: `lan` or `lan_and_internet` (FR-P3).
- **Participant**: a device, or Drive, holding a copy of a pair's folder.

## 2. Identity and pairing (`wasl-net::identity`, `wasl-net::pairing`)

- First run creates an Ed25519 key pair (FR-N1). The private key lives in the OS keystore
  (Android Keystore-wrapped AES key encrypting the seed; Windows Credential Manager, macOS
  Keychain, Secret Service). **Device ID** = first 16 bytes of BLAKE3(public key), written as
  base32 in groups (`WASL-ABCD-EFGH-…`). Short ID (version vectors) = its first 8 bytes as u64.
- **Pairing** (FR-N2). The existing device shows a code: QR or 8 characters from a 32-symbol
  alphabet without look-alikes. It encodes `(device ID, a 20-byte one-time secret, LAN addresses)`.
  The new device connects (LAN, or a rendezvous offer through Drive when both use the same Google
  account) and both run a PAKE-free proof:
  1. Each side sends its public key and a random nonce over a TLS connection whose certificate is
     that key (§6); neither is trusted yet.
  2. Each side proves it knows the secret: `HMAC(secret, "wasl-pair" ‖ own pk ‖ peer pk ‖ nonces)`.
     A wrong code fails here, never adds anything (acceptance test).
  3. Both show **four confirmation words** from a 2048-word list, derived from
     `BLAKE3("wasl-words" ‖ sorted(pkA, pkB) ‖ secret)`. The person checks the words match on both
     screens and confirms on both. Only then does each add the other to `group.json`.
  The secret is single-use and expires after 10 minutes.

  Where it runs (M4): on the inviter's own device endpoint. While a code is open any key may
  complete a TLS handshake, but a gate answers a non-member nothing except pairing rounds, and
  trust is asked again before every request, so a stranger is cut off when the code closes. The
  joiner pins the inviter's device ID from the code, and the key in the joiner's Hello must be
  the key it proved to TLS. The joiner drives the exchange in rounds (its messages and name; the
  inviter's messages back); once both people confirmed, the inviter adds it and returns every
  group operation, which the joiner adopts with the inviter as the trusted signer.

## 3. The group file (`wasl-net::group`)

**As built (2026-10):** members exchange operations directly: on every link, after the Hello, each run and every minute, a device sends its heads (`Request::Group`) and receives the operations it lacks, parents first; `GroupChanged` asks a member to do it now. Trust updates at once, so a removed device is cut off by every member that has heard of the removal.

**Group files (S2-1, `wasl-devices::board`, `wasl-net::rendezvous`):** for devices that are never online together, each device also writes every operation it holds to its own file in the app-data folder, `group/<group>-<device id>.json` (`<group>`: the first 32 hex digits of the genesis ID), a JSON array of signed operations, parents first, at most 1 MiB (the newest that fit). One file per writer because Drive has no compare-and-swap: a shared file would lose news. A device reads its members' files at start and every minute, by exact name only (never a removed device's or a stranger's), and takes them in through the same checks as operations sent over a link; it rewrites its own file within seconds of any change, and at every read also reads its own file back and writes it again if it differs (whoever holds the account's token, a removed device included, can overwrite or delete it). Only where Drive is already in use for device sync: some enabled pair has route `drive_and_devices`, or route `devices` with reach `lan_and_internet`, and a signed-in account (the first without a sign-in problem; devices on different accounts fall back to peer to peer). A person who keeps every device pair to the LAN sends nothing about the group (device names, keys) to Google. Groups without an account stay peer to peer.

The group is a set of signed operations in the app-data folder, `group/<id>.json`: `create` (the
first device), `add` and `remove`, each `{change, parents, signed_by, signature}`. `parents` are
the operations the signer had seen, so the operations form a graph, and every device holding the
same operations computes the same trusted set whatever order they arrived in. The signature is
Ed25519 over the canonical JSON (keys sorted, no whitespace) without `signature`; an operation's
ID is the BLAKE3 of its canonical JSON. Rules (FR-N3):
- An operation counts only if its signer is a member in its past (added there, not removed).
  Tampered, outsider-signed or second-`create` operations are refused.
- A removal is permanent; a removed device re-pairs with a new key.
- An addition by a device concurrent with that device's removal, or after it, does not count, so
  a device racing its own removal, or forging old parents later, can never stay or bring anyone
  in; devices it added before its removal stay.
- A removal concurrent with its signer's own removal still counts: two devices removing each
  other both leave (fail closed). A removed device can force a device that existed before its
  removal to re-pair, which is no more than it could do as a member.
- Operations that do not count are not stored, and each signer has a bounded share, so a removed
  device cannot fill the group.

**Removals by a device being removed (S2-2, decided 2026-10).** The rule above stays: a removal
concurrent with its signer's own removal counts. Voiding it would lose honest removals (a laptop
offline for a week removes an old tablet while the phone removes the laptop; the tablet would stay
trusted), and every exception tried (only the remover may be removed back; only the signer's
first such removal; only devices it added) depends on which removals count, so a removed device
can set removals against each other until the evaluation stops settling and its own removal is
the one left out. The bound is on reach instead: a device asks only members for operations and
reads only members' group files, so a removed device's forgeries reach only devices that have not
yet heard of its removal, a window the group files close within a minute for devices online on
the same account; anything it forges there costs at most a re-pair and never lets it stay or bring
anyone in. Each signer's new operations are also taken in at most 64 a minute
(`SignerThrottle`), whichever way they arrive; operations over the share come again at the next
exchange. An operation is charged only once its signature checks against the key its signer has
in the group (`GroupOps::screen`): one already kept or waiting for its parents costs nothing, a
forged one is dropped and charged to no one (so whoever can write a group file cannot use up a
member's share in its name), and one from a signer not known yet is charged to a single shared
bucket.

Without Drive (route `devices` only, no account) the group is
kept locally on each device and changed only by pairing.

## 4. The sync model: version vectors (`wasl-core::vv`, `wasl-core::nway`)

Today's planner compares two sides with one baseline. Device sync has N participants, so each
file version carries a **version vector** (FR-C4): a map from short device ID to a counter.

- **Local index** per pair on each device: for each key, `{hash (BLAKE3), size, mtime, vv,
  deleted, seq}`. `seq` is a per-device sequence number that grows with each change, so peers ask
  "what changed since seq N".
- **A local change** (scan finds size/mtime/hash differs from the index, FR-C1) increments this
  device's counter in the key's vv. A deletion keeps a tombstone (`deleted: true`) with its vv
  for 90 days, so a delete propagates instead of being undone by a peer that still has the file.
- **Comparing a key** between local `L` and remote `R` (FR-C5):
  - `R.vv` dominates (≥ everywhere, > somewhere): take R (download, delete or rename).
  - `L.vv` dominates or equal: nothing to do (the peer pulls from us).
  - Concurrent: if `L.hash == R.hash` (and both deleted or both present), **merge** vvs
    (element-wise max): no conflict copy. Otherwise a **conflict**: the pair's policy picks the
    winner deterministically on every device (newer mtime, then higher device ID), the loser is
    kept as a conflict copy, and the merged vv plus one tick of the resolving device dominates
    both, so every device resolves it the same way exactly once.
- **Renames and moves**: a key that disappears and a key that appears in the same scan with the
  same hash are one move; on a peer, a move whose content hash is already present locally is
  applied as a local rename (no transfer). On Drive the file id is stable across renames, so a
  rename on Drive is recognised by id (this fixes M3's "download the new name, bin the old one").
- **Safety** stays where it is: guards (FR-S*) run on the plan, every overwrite and delete goes
  through the bin, mass-delete and first-merge holds apply.
- The comparison is pure (no I/O) and property-tested: any interleaving of edits on 3 devices and
  Drive, delivered in any order, converges to one state; no change is lost except as a conflict
  copy; a delete is never undone by a stale peer.

**Drive as a participant** (route `drive_and_devices`): each file Wasl uploads carries its vv in
`appProperties["wasl.vv"]` (compact: per entry a varint short-ID index into the group's device
list plus a varint counter, base64url; when longer than 124 bytes, `wasl.vv` holds `h:` + a
hash naming an entry in the app-data `index/` file) and `wasl.b3` (BLAKE3). A Drive file whose
MD5 no longer matches its `wasl.b3`-tagged version was changed outside Wasl: it becomes a new
version authored by the pseudo-participant `drive` (short ID 0). A file a device receives that
Drive already has with the same `wasl.b3` or MD5 is marked synced without a transfer (FR-C6).

Route `drive` keeps today's two-sided planner unchanged.

**What ships first** (M4): on route `drive_and_devices` each device runs both syncs, the Drive
planner and the device pull, one after the other. Drive and the devices then meet in the folder:
a file received from a device is uploaded like any local edit, and a file downloaded from Drive
becomes a new version that peers merge without a conflict when the content matches (same BLAKE3).
This converges with the safety of each path, at the cost of some duplicate transfers. Drive as a
participant with `wasl.vv` (above) replaces it later without changing the device protocol.

**Renames on Drive** are followed on every route with Drive: a file whose Drive id is in the
baseline under another path, unchanged on the device since the last sync, with its new place free,
is renamed locally and the baseline moves with it, so nothing is downloaded or binned. Android's
folder picker renames only within one folder; a move there falls back to download and bin.

## 5. Chunked transfer (`wasl-chunk`)

Files are cut with FastCDC in two tiers (COMPRESSION.md §3.1): up to 16 MiB with min 16 KiB,
average 64 KiB, max 256 KiB (a file of at most 256 KiB is one chunk); above 16 MiB with min 1 MiB,
average 4 MiB, max 16 MiB (FR-N9). Each chunk is named by its BLAKE3. A file version's **chunk
list** travels with its index entry. The receiver builds the new file from chunks it already has (in
any version of any file of the pair, found through a per-pair chunk table) and asks the peer only
for the rest, several at a time (8 small or 3 large chunks in flight); each chunk is verified by
BLAKE3 on arrival; a transfer resumes at chunk granularity. A device that cuts with other sizes (an
older Wasl) still syncs correctly, with less reuse; a pair whose lists predate the two tiers is cut
again once, locally, without a new version.

How small a delta is, honestly: content-defined chunking bounds it with high probability, not for
every input. An edit leaves every chunk before it unchanged and the chunking re-aligns soon after
it; one chunk changes unless a cut point falls among the few positions the edit touched. Measured
over 5,000 random edits (wasl-chunk tests): an overwrite changed one chunk 91% of the time and
three or fewer 99.7%; a small insert changed one chunk 99.6%; no edit cost more than 2 × the
largest chunk. Content crafted against the rolling hash (or long runs of zeros) can do worse, so
the FR-N9 acceptance is tested as a distribution, not a guarantee.
The result is written to a temporary file in the target folder and renamed into place after the
whole-file BLAKE3 matches (FR-N10).

## 6. Transport (`wasl-net::quic`)

QUIC (`quinn`) with TLS 1.3. Each device's certificate is a self-signed X.509 wrapping its Ed25519
key; the verifier accepts exactly the keys in the group (pinned, no CAs; FR-N4), and during
pairing any key (step 2 then decides). One connection per peer; streams:

- `hello`: device ID, version, pairs offered (pair IDs shared with this peer, by name hash).
- `index`: "changes since seq N" for a pair; answered in pages.
- `chunk`: request chunk hashes, receive bytes (bounded window, cancellable). The request carries
  the zstd level the asker accepts (by path: 1 on the LAN, 3 over the internet, 9 on a metered
  network); the sender, within its own budget (never above level 9, level 1 on battery, at most two
  compressions at once), sends a chunk zstd-compressed when that saves at least 5 %, else raw
  (COMPRESSION.md §2). Requests without the field (older
  devices) get raw bytes.
- `notify`: "pair P changed, seq is now N", so peers pull promptly.

Messages are length-prefixed CBOR. Limits on every length; unknown message types close the stream.

## 7. Finding each other (`wasl-net::discover`, `wasl-net::rendezvous`)

- **LAN** (FR-N5): mDNS `_wasl._udp` announcing only a random per-boot token and port; the device
  ID is revealed only after the TLS handshake succeeds with a pinned key. Also a UDP broadcast
  probe on the same port as a fallback for networks that block multicast.
- **Internet** (reach `lan_and_internet`, FR-N6–N7): each device publishes a signed
  `peers/<id>.json` (LAN addresses, IPv6 addresses, the public address from STUN, a UPnP/NAT-PMP
  mapped port if obtained, expiry ≤ 10 min) in the app-data folder. To connect, a device writes a
  signed `offers/<from>-<to>.json` with a start time; the other device, polling while active,
  answers; both send UDP at the agreed time (hole punching). Order: same LAN → IPv6 → mapped port
  → hole punching. STUN servers are a configurable list of public servers. A member chooses where
  others send packets only within limits (S2-3): one punch or direct attempt goes to at most 4
  addresses (one of each internet kind first, then the best LAN ones), and an answerer runs one
  punch per offerer at a time, at most 4 a minute.
- **No path** (FR-N8): route `drive_and_devices` continues through Drive; route `devices` waits
  ("Can't reach <device> from this network") and accepts a typed address (Tailscale/WireGuard IP or
  a self-hosted relay).
- Reach `lan` never contacts STUN, Drive rendezvous or any internet address (FR-N11).

## 8. Phases

1. **Core model**: version vectors, the N-way comparison with renames, its property tests; the
   index table and tombstones in the store. (`wasl-core`, `wasl-store`)
2. **Chunking**: FastCDC, chunk tables, delta reconstruction and its size test. (`wasl-chunk`)
3. **Identity, pairing, group file**: keys in keystores, codes and words, signed group with
   rollback and tamper tests. (`wasl-net`)
4. **Transport and LAN**: QUIC with pinned keys, the protocol, mDNS; two daemons on one machine
   syncing a pair over loopback; Drive + devices on route `drive_and_devices`. (`wasl-net`,
   `wasl-engine`, `wasld`)
5. **Internet reach**: STUN, port mapping, rendezvous through Drive, hole punching; a NAT lab in
   Linux network namespaces in CI. (`wasl-net`)
6. **Clients**: `wasl device add|list|remove`, pairing screens in the tray app and on Android
   (QR), device reachability in Diagnostics (FR-B7), Android participation rules (FR-N12).
