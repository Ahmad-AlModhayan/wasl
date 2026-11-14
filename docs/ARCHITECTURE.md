# Drive Sync — Architecture

Personal Android app that keeps device folders and Google Drive folders in sync. No ads, no
analytics, no server: the phone talks to Google Drive directly.

## Stack
Kotlin 2.1 · Jetpack Compose (Material 3, dynamic color) · Hilt · Room · WorkManager · DataStore ·
OkHttp + kotlinx.serialization (hand-written Drive v3 REST client) · Play services Authorization API
for OAuth tokens. minSdk 30, targetSdk 35.

## Packages (`com.almodhayan.drivesync`)
| Package | Role |
|---|---|
| `core` | `Glob` (include/exclude patterns, `PathFilter`), `NameMapper` (Drive names → FAT-safe local names, conflict copy names), `Hashing`, `AppError` (error taxonomy + user hints), `Ids`/`TimeFmt` |
| `data.db` | Room: `AccountEntity`, `FolderPairEntity`, `FileStateEntity` (sync baseline), `SyncRunEntity`/`SyncEventEntity` (history), `ConflictEntity`, `TrashEntity` (recycle bin), `DriveNodeEntity` (Drive metadata cache), `UploadSessionEntity`, `PinEntity` (on-demand pins), `VaultEntity`, `BlockedPlanEntity`. DAOs expose Flows for the UI. |
| `data.prefs` | `SettingsRepository` (DataStore) → `AppSettings` |
| `drive` | `DriveAuth` (Play services AuthorizationClient; silent tokens for background), `DriveApi` (REST: list/changes/download with Range resume/resumable upload with persisted sessions/export/revisions/shared drives/appData), `DriveErrors`, `DriveNodeCache` (one full crawl per account, then Changes API deltas; pairs resolve subtrees from the cache), `DriveRemoteFs` |
| `fs` | `RemoteFs` interface (+`RemoteEntry`), `LocalFs` (scan, atomic commit, recycle bin), `FileWatcher` (inotify best effort) |
| `sync` | `Planner` (pure three-way merge; 7 modes; conflict policies; mass-delete guard; Google Docs export), `Executor` (applies actions, parallel transfers, updates baseline per file, conflict copies, recycle bin), `SyncEngine` (orchestration, run history), `Conditions` (Wi-Fi/SSID/charging/battery/VPN), `SyncStateHolder` (live progress), `RemoteFsFactory` + `RemoteFsWrapper` (extension point for encryption) |
| `scheduler` | `SyncWorker` (WorkManager, dataSync foreground), `SyncScheduler` (periodic + one-shot per pair), `MaintenanceWorker` (retention), `MonitorService` (instant sync: FileObserver + MediaStore + Drive change polling; specialUse FGS), `SyncIntentReceiver` (exported: boot + automation intents, gated by `AutomationGate`: the Allow automation setting plus a per-install secret), `InternalActionReceiver` (not exported: notification buttons and the internal re-arm), `MonitorPolicy` (when the instant-sync watcher runs; `DriveSyncApp` observes it), `SyncTileService`, `SyncNotifications` |
| `ui.theme` | Design system foundation: brand + fixed status colours, Figtree/JetBrains Mono type scale, shapes, spacing, motion, `Sym` (Material Symbols Rounded, generated from the design) |
| `ui.components` | Design system components in Compose (StatusChip/Banner, SyncProgress, StorageBar, PathRow, FolderPairCard, SyncModeOption, Callout, ListItem, ChangeRow/Summary, ConflictCard, VersionItem, HealthCheckRow, LogView, top/bottom bars, dialogs, permission steps) and `Fmt` for numbers and times |
| `ui.status` | `PairStatusResolver`: the six states with the design's precedence |
| `actions` | Business actions the screens call: pause/resume, conflict choices with undo, deletion review (restore or approve), two-sided recycle bin, version history, log export, storage stats |
| `ui` | Compose screens by feature; `Nav.kt` is the route table (bottom navigation: Pairs, Problems, Recycle bin, Settings); `AppNav` wraps navigation |
| `ondemand` | `DocumentsProvider` exposing Drive as browsable files-on-demand with offline pins |
| `crypto` | Client-side encryption vault wrapping `RemoteFs` |
| `backup` | Config export/import (Drive appDataFolder + local file) |
| `share`, `widget` | Share-sheet target, home screen widget |

## Sync algorithm
1. `DriveNodeCache.refresh` applies Changes API deltas (full crawl the first time).
2. `RemoteFs.listSubtree` builds the remote tree for the pair's folder; duplicates and shortcuts are reported, not synced.
3. `LocalFs.scan` walks the device folder honouring the `PathFilter`.
4. Keys are canonical relative paths using Drive names. Local names may differ (`NameMapper`), the mapping lives in `FileStateEntity.localRel`.
5. `Planner.plan` compares baseline/local/remote per key. Content changes are detected by size+mtime then MD5; remote changes by md5/version. Decisions follow the pair's `SyncMode`, `ConflictPolicy` and `propagateDeletions`.
6. The mass-delete guard blocks plans that would delete ≥ N files and ≥ X% of a side; the user approves once from the UI (`approveOnce`).
7. `Executor` runs mkdirs → transfers (parallel, resumable, md5 verified) → deletions (deepest first). Local deletes/overwrites go to the recycle bin; Drive deletes go to Drive trash. Each success updates the baseline immediately.
8. Google-native docs are exported (docx/xlsx/pptx/pdf) as read-only mirrors and never uploaded back.

## Background execution
- Periodic `SyncWorker` (global interval, ≥15 min) + one-shot expedited runs per pair for manual/instant/remote-change triggers.
- `MonitorService` (optional) provides instant sync; it debounces local events and polls Drive changes every few minutes.
- Boot / package-replaced re-arms everything. Android 15's 6h dataSync cap is respected: interrupted runs resume next time because the baseline is committed per file.

## Safety rules (from the design)
- A sync that would delete **more than 20 files or 5% of a folder** stops and asks (the percentage rule applies from 5 files up, so one deletion in a small folder doesn't stop it). Thresholds are in Settings › Safety.
- The **first sync of two non-empty folders** stops and asks unless it was started from the preview.
- Deletions and overwrites on the phone go to the app's recycle bin; deletions on Drive go to Drive's bin and are listed in the app's recycle bin too, for 30 days.
- Conflicts keep both copies; Keep phone / Keep Drive moves the other copy to the bin, with Undo.
