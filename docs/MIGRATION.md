# Moving from Drive Sync 1.1 to Wasl (SRS FR-K2)

Wasl has a new application ID (`com.kahflabs.wasl`), so Android gives it no access to Drive Sync's
database or recycle bin. Drive Sync 1.1.5 therefore writes everything Wasl needs into one file the
user chooses, and Wasl reads it back. After the export Drive Sync pauses every pair, so the two apps
never sync the same folders at once; Drive Sync stays installed as the fallback until the user
removes it.

## The file

A zip named `drive-sync-to-wasl-<yyyyMMdd-HHmm>.zip` holding:

| Entry | Content |
|---|---|
| `migration.json` | Everything but file contents (below) |
| `bin/<trash id>` | The phone-side recycle bin files, one per phone-side bin entry that still exists |

`migration.json` is UTF-8 JSON. Unknown fields are ignored by the reader; missing ones take the
defaults of the 1.1.4 entities.

```jsonc
{
  "format": "drive-sync-to-wasl",
  "version": 1,
  "exportedAt": 1760000000000,          // ms since the epoch
  "appVersion": "1.1.5",
  "config": { /* the ConfigSnapshot of Settings › Backup (version 1): settings, accounts,
                 pairs with their ids, vaults (salt, iterations, key check: no keys), pins */ },
  "pairs": [
    {
      "pairId": "…",
      "lastStatus": "SUCCESS", "lastRunAt": 0, "lastSuccessAt": 0,   // 1.1.4 pair status
      "baseline":  [ /* FileStateEntity rows: key, localRel, isDir, localSize, localMtime,
                       localHash, remoteId, remoteMd5, remoteSize, remoteMtime, remoteVersion,
                       exportMime */ ],
      "runs":      [ /* SyncRunEntity rows of the last 90 days */ ],
      "events":    [ /* SyncEventEntity rows of those runs */ ],
      "conflicts": [ /* ConflictEntity rows still open */ ]
    }
  ],
  "bin": [ /* TrashEntity rows; "file": "bin/<id>" when the phone-side file is in the zip */ ]
}
```

Enum values are the 1.1.4 names (`TWO_WAY`, `KEEP_BOTH`, `SUCCESS`, `DELETED`, `INFO`…). Paths in the
baseline are pair-relative, as in 1.1.4's `file_state`; bin rows keep 1.1.4's absolute
`originalPath`, and Wasl derives the key from `syncKey` (or from the path relative to the pair).

## What Wasl does with it

1. Imports `config` with Settings › Backup's merge: pairs keep their ids, vaults arrive locked
   (the passphrase unlocks them), accounts are listed for sign-in.
2. For each pair, writes the baseline, the history and the open conflicts into the engine, so the
   first run is an ordinary incremental run, not a first merge. Each pair is held for one preview,
   so the user sees the first plan before anything moves.
3. Moves the bin files into Wasl's own bin with their dates, so restore and 30-day expiry work as
   before. Drive-side bin rows point at Drive's own bin and need no file.
4. Importing the same file twice changes nothing the second time.

Secrets are never in the file: no OAuth tokens (the user signs in to Wasl), no vault keys.
