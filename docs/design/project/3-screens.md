# Screens and flows

Every screen below has a live page under *Screens*, drawn with the components at 360 × 780dp in the current theme.

## Navigation

A bottom `NavigationBar` with four destinations: **Pairs** (home), **Problems** (badge = approvals + conflicts + failures), **Recycle bin**, **Settings**. Everything else is pushed on top with a back arrow: pair editor, pickers, preview, conflict, versions, diagnostics. Editors and pickers open full-screen with a close (×) instead of back.

```
Onboarding → Pairs ─┬─ Pair card → Pair editor ─┬─ Phone folder picker
                    │                           ├─ Drive folder picker
                    │                           └─ Preview first sync → Running
                    ├─ Banner → Review deletions
                    └─ FAB → Pair editor (new)
Problems ─┬─ Review deletions
          ├─ Conflict → Version history
          └─ Failed → fix (system settings or picker)
Recycle bin → Version history
Settings → Diagnostics
```

## Sync rules the UI promises

| Mode | Copies | Deletions (default) | Never changes |
|---|---|---|---|
| Two-way (recommended) | New and edited files both ways | Synced, via the recycle bin | n/a |
| Phone → Drive | Phone to Drive | Not synced; *Mirror deletions* is opt-in with a red warning | The phone |
| Drive → Phone | Drive to phone | Not synced; *Mirror deletions* is opt-in with a red warning | Drive |

Safety defaults: deletions and overwrites go to the recycle bin for 30 days; a sync that would delete more than 20 files or 5% of a folder stops and asks; Wi‑Fi only is on; files over 2 GB and hidden files are skipped; two pairs may not overlap.

## 1 · Permission onboarding

Welcome with the three promises (preview first, nothing lost for 30 days, no ads or tracking) → a checklist of `PermissionStep`s: Google Drive and Files are required, Notifications and Background are optional and labelled so. *Continue* is disabled until both required steps show *Allowed*. Choosing file access is its own screen: **All files access** (in-app picker, recommended) or **One folder at a time** (Android's picker, with its limits stated up front). Android/data and Android/obb are unavailable either way, and the app says so before anyone tries.

## 2 · Home dashboard

`TopAppBar` with the mark, *Sync all now* and the account. One `StatusBanner` for the most urgent loud state, then a `FolderPairCard` per pair: name, mode and conditions, status chip, `SyncProgress` while running, the two `PathRow`s with the direction arrow, and a compact `StorageBar` for the side the pair fills (two bars for two-way). The extended FAB *New pair* collapses to an icon on scroll. When everything is synced, Home becomes a summary: one line, the phone and Drive storage bars with all pairs, and a compact list with icon-only status.

## 3 · Pair editor

Name → Folders (two `PathRow`s that open the pickers) → Sync mode (three `SyncModeOption` cards) → Deletions → When to sync → Skip. Warnings are `Callout`s placed under the setting that causes them and always include the undo: info when the Drive folder isn't empty, warning for an overlap with another pair or a large first sync on mobile data, danger when deletions are mirrored in a one-way mode. The bottom bar's filled button is *Preview first sync*; an overlap disables it.

## 4 · Folder pickers

Both pickers use `ListItem` rows: tap to open a folder, then the bottom bar chooses the folder you are in and states its size. Phone folders are `primary`, Drive folders `tertiary`. Folders already in a pair show which pair. Protected folders stay visible, dimmed, with the reason. The phone picker switches *Internal storage / SD card*; the Drive picker switches *My Drive / Shared with me*, and shared folders say how many people will see changes. Storage roots can never be paired.

## 5 · Dry-run preview

`ChangeSummary` leads with what will not be touched, then four tiles: to Drive, to phone, to recycle bin, conflicts. Filter `Chip`s and `ChangeRow`s below. *Start sync* turns the same screen into a live log grouped *Done / Waiting for you*, with Pause in the bottom bar and in the notification. When the delete limit trips, *Review deletions* shows the count, the share of the folder and every file, with two recoverable answers: *Restore on phone* and *Delete on Drive*.

## 6 · Problems and conflicts

One list grouped by who acts: *Needs you* (approvals, conflicts, failures, each with its single action) and *Retrying on its own* (never badged). A conflict opens a compare screen: both copies side by side with time, size and where they were edited, and three choices with *Keep both* selected by default. Resolving shows a snackbar with Undo. Empty state: "Nothing needs you".

## 7 · Recycle bin

Everything sync removed or replaced, from either side, for 30 days. Each row shows the side it came from (`mobile` / `cloud`), the pair, and days left (amber at 3 or fewer). Restore per row or by selection; *Delete forever* is the only confirmation `Dialog` in daily use. Files from the phone are kept in app-private storage; files from Drive are in Drive's own bin.

## 8 · File version history

A header card for the file, then a `VersionItem` timeline, newest first, merging Drive revisions (30 days or 100 versions) with the last 5 versions this phone overwrote. *Restore* makes an old version current on both sides and keeps the replaced one; *Save as copy* adds it beside the file.

## 9 · Diagnostics

`HealthCheckRow`s for the six reasons a sync stops: folder access, Google account, battery optimisation, background work, network, Drive storage, each failing row with one *Fix*. Then the last 24 hours, a filterable `LogView`, and *Export log* with file names hidden by default.

## 10 · Settings

Account card with Drive storage; Sync defaults (pause all, Wi‑Fi only, check interval); Safety (large-delete approval, conflict default, bin retention, versions kept); Appearance (*Colours from wallpaper*, theme); About (diagnostics, privacy, licences). *Disconnect Google account* is the only red row; it confirms and stops syncing without deleting files.
