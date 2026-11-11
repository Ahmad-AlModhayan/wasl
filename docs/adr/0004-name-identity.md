# ADR-0004: One rule for when two names are the same name

Status: Accepted (2026-11-03)

## Context

The engine compared names as exact strings, but the places a pair lives do not agree on what one
name is:

- Windows (NTFS), macOS (APFS) and Android's shared storage fold case: `Foo.txt` and `foo.txt` are
  one file there. APFS also folds Unicode normalisation: `أحمد` written with U+0623 (NFC) or with
  U+0627 U+0654 (NFD, as macOS and HFS+ often hand names over) is one file.
- Drive and Linux fold neither, so both spellings can sit side by side.
- The Drive listing already folded case when it looked for duplicates, but nothing else did, and
  the duplicate it dropped read as "deleted on Drive".

Each mismatch lost or duplicated files, with every run reporting Success: a case-only rename on
Drive deleted the file on the device and then trashed it on Drive (#138); a case variant added on
Drive was binned and trashed (#140); a case-only rename on the PC became `Meeting (1).txt` on every
side once the phone (casefolded storage) synced (#137); NFC and NFD spellings of one name became
two files (#143).

## Decision

**Two names are the same name when their name keys are equal**: the name in Unicode NFC, then in
lower case (`wasl_core::identity::name_key`). The rule is the same on every device and every
remote, whatever the local file system does, because one pair's files reach all of them. A pair
holds at most one item per name key in a folder.

`wasl_core::identity::check_names` applies it to every plan's input, after the scan and listing:

1. **The same file spelled two ways is one key, under Drive's spelling.** When it meets for the
   first time (NFD here, NFC on Drive), the device keeps its own spelling as the key's local name.
   When Drive renamed a synced name (by a rename, or by another device's upload under the new
   name), the baseline row moves to Drive's spelling. In a run (not a preview) the engine first
   renames the device's file in place to Drive's spelling when it is unchanged here, and only onto
   itself (the folder must say both spellings are one file); otherwise the device keeps its name
   and the change syncs as usual. Items new on the device inside such a folder take the folder's
   key, so they upload into Drive's folder. A folder the renames emptied (a device that keeps both
   spellings apart) is removed.
2. **A name held twice on one side is a clash**: two spellings in one Drive folder, or on a
   case-sensitive device. Every key with that name, and everything under it, stays out of the
   plan on both sides and is named in the run's notes until one of them is renamed. Nothing is
   merged, binned or trashed (SRS FR-C7). Two items with exactly the same name in one Drive folder
   keep the older rule: the newest syncs, the others are reported.
3. **A rename made on the device stays a rename**: the old name synced and gone here, the new name
   only here. When Drive's copy is as synced, or gone, it syncs as an upload of the new name and
   the removal of the old one, like any rename made on the device; other devices then follow it by
   rule 1. When Drive changed it meanwhile, the device's file stays the old key under its new local
   name, so the usual conflict rules apply (the rename waits for the next one).

Every Drive and vault listing finds duplicates with the same `name_key`. `StdFs::rename` accepts a
case-only rename only when both spellings are one file (`LocalFs::is_same_file`: file index and
volume on Windows, device and inode on Unix). A folder that cannot tell (Android's folder picker,
for now) never renames in place; the device keeps its spelling under Drive's key.

## Consequences

- Case and accents are folded even on Linux, where they would not need to be, so a Linux device
  cannot sync `Notes.txt` and `notes.txt` from one folder; it reports them instead. That is the
  price of a pair meaning the same thing on the phone and the PC.
- A device renamed only by case or accents re-uploads the file (no Drive rename yet), like other
  renames made on a device.
- Lower casing is Unicode's default lower case, not the exact tables of NTFS or APFS. The rare
  names those tables fold differently (some Turkish, Greek and Georgian letters) may still meet as
  two names on one device.
- Device-to-device sync applies the rule in every pull (`wasl-engine::devices`, #173): a file
  the peer renamed to another spelling of the same name in one folder is renamed in place here;
  a name held twice (on this device, in what the peer sent, or one spelling on each side that
  is not such a rename) is a clash, and every key with that name, and everything under it, stays
  out of the pull and is named in the run's message until one is renamed. A folder renamed that
  way still waits (#190).
- Reversing this means a migration: baselines written under Drive's spelling with a different
  local name would need to be re-read.
