# Test parity with Drive Sync 1.1.4

SRS FR-E5: the Kotlin planner, glob, name-mapping and vault tests pass against the Rust engine before the Kotlin engine is retired. M1 covers everything except vaults (M5). Each Kotlin test is one Rust test with the same name in snake_case, in the same order.

## Ported tests

| Kotlin class (`app/src/test/...`) | Kotlin | Rust | Passing | Rust file |
|---|---:|---:|---:|---|
| `sync/PlannerTest` | 109 | 109 | 109 | `crates/wasl-core/tests/planner.rs` |
| `core/GlobTest` (`GlobTest` + `PathFilterTest`) | 36 | 36 | 36 | `crates/wasl-ignore/tests/legacy_glob.rs` |
| `core/NameMapperTest` | 19 | 19 | 19 | `crates/wasl-core/tests/name_mapper.rs` |
| `drive/DriveApiTest` | 55 | 55 | 55 | `crates/wasl-drive/tests/drive_api.rs` |
| `drive/DriveErrorsTest` | 16 | 16 | 16 | `crates/wasl-drive/tests/drive_errors.rs` |
| `drive/DriveNodeCacheTest` | 23 | 23 | 23 | `crates/wasl-drive/tests/drive_node_cache.rs` |
| `drive/BandwidthLimiterTest` | 3 | 3 | 3 | `crates/wasl-drive/tests/bandwidth_limiter.rs` |
| **Total** | **261** | **261** | **261** | |

Not ported in M1: the vault tests (`crypto/*`, M5), `DriveAuthErrorsTest` (Play services status codes; the Rust client takes a token provider and has no OAuth flow), and the Android-only classes (`scheduler/*`, `ondemand/*`, `ui/*`, `backup/*`, `fs/LocalFs*`), which belong to the milestones that move those parts.

How the ports differ in form, not behaviour:
- **Lazy hashing.** 1.1.4 hashed through a callback during planning. The Rust planner returns the files it needs hashed and is called again. The tests assert the same hashed files.
- **Drive tests.** `MockWebServer` became `wiremock`, and coroutine virtual time became paused tokio time.
- **`listSubtree`.** The two `DriveRemoteFs.listSubtree` cases run against `DriveNodeCache::list_subtree`.
- **Session store keys.** Resumable-upload keys are now namespaced by account (`account|key`), so the key strings asserted in those tests changed. The behaviour is the same.

## Tests added beyond 1.1.4

| Area | Tests | File |
|---|---:|---|
| Planner properties (2,000 cases each): deterministic; no deletion without a baseline entry outside mirror methods; one-way methods and two-way without deletions never delete; one-way methods only write to their target; mirror methods converge in one run; two-way converges; one-way methods are idempotent | 7 | `wasl-core/tests/properties.rs` |
| Safety-guard boundaries at the SRS defaults (20/21 files, exactly 5%, 5-file floor, wipe, first merge, approval) | 10 | `wasl-core/tests/guards.rs` |
| Planner timing: 100,000 keys in about 0.19 s in a release build (NFR-3 budget: 2 s) | 1 | `wasl-core/tests/performance.rs` |
| `/` in Drive names; conflict-name parts | 2 | `wasl-core/tests/name_mapper.rs` |
| Ignore engine against `git check-ignore`: same verdict and same deciding `file:line` for every path | 4 | `wasl-ignore/tests/git_parity.rs` |
| Rule files, presets, pair patterns, explain, case, unusual folder names | 12 | `wasl-ignore/tests/rules.rs` |
| Ignored items produce no action in any method; adding a rule deletes nothing | 2 | `wasl-ignore/tests/planning.rs` |
| SQLite schema, migrations, baseline round trip (with property test), cascade, transactions | 11 | `wasl-store/tests/store.rs` |
| Drive: error hints, call timeout, MD5-verified download, `Send` futures, session-URI trust, node-cache extras, limiter | 14 | `wasl-drive/tests/*`, unit tests |

Each property and the git comparison were checked against a deliberately broken implementation: each catches the bug.

## Behaviour kept from 1.1.4 that looks questionable

These are marked `// parity:` in the code and left as they are, so M2 can switch Android to the Rust engine without changing what users see. Each one deserves a decision after M1.

| Where | Behaviour |
|---|---|
| `wasl-core/src/planner.rs` (size limit) | `max_file_size` skips only files never synced; a Drive file above the limit is still downloaded when a local copy exists. |
| `wasl-core/src/planner.rs` (folders) | A baseline file that became a local folder while Drive deleted it gets a folder deletion; a baseline folder that became a file on one side and vanished on the other is left alone. |
| `wasl-core/src/planner.rs` (ordering) | Keys sort by UTF-16 order in 1.1.4 and byte order here; they differ only for characters above U+FFFF against U+E000–U+FFFF, which reorders independent actions only. |
| `wasl-ignore/src/legacy.rs` | `{01..03}` drops leading zeros (matches `1`, not `01`). |
| `wasl-drive/src/client.rs` | A 401 token refresh uses up one of the retry attempts. |
| `wasl-drive/src/node_cache.rs` | `path_of` drops the last ancestor when the 256-step cycle guard trips (corrupt data only). |

## Intentional changes from 1.1.4

- **`/` in Drive names.** The name is mapped to `／` locally. 1.1.4 did not map it, so a Drive name containing `/` could point outside the pair.
- **Conflict copy names.** The device and timestamp parts are mapped like file names.
- **Drive upload sessions.**
  - A session URI must be on the configured upload host before the bearer token is sent to it.
  - Sessions are stored per account.
  - An unparseable or negative `Retry-After` counts as 0.
- **MD5-verified download.** `download_verified` is new.
- **Drive change order (#144).** A delta applies each item's last change across pages. 1.1.4 applied all upserts before all removals, so an item removed on one page and restored on a later one stayed deleted in the cache.
- **Google file next to a same-named file (#139).** When a Google file's export name (`Report.docx`) is already another item's local name, on Drive, on the device or in the baseline, the export is named `Report (Google Doc).docx` (`Google Sheet`, `Google Slides`, `Google Drawing`, `Apps Script`, `Jamboard` likewise), then `Report (Google Doc 2).docx` if that is taken too, and keeps that name. A never-synced device file keeps its name and uploads. An export moving off its old name bins the old file only while it still has the size and time the export left. 1.1.4 wrote the export over the real file, which was then uploaded over it on Drive.
- **Folder deleted on Drive with new files here (#142).** In two-way sync the folder is made again on Drive when the device has new or changed files under it; its synced files are deleted one by one. 1.1.4 planned a folder deletion the executor could not do, and the new files never uploaded.

## Decisions

1. **FR-F7 vs mirror methods (decided 2026-10-19).** Mirror methods keep removing extras on their target side; SRS FR-F7 now exempts them. The deletions go to the bin and the mass-delete guard still applies. The property test exempts exactly those methods.
2. **FR-F3 wording (decided 2026-10-19).** The SRS now matches the code: the 5% rule applies once at least 5 files are deleted.

## Open questions

1. **Guard messages.** Messages still say "phone" and "Drive Sync", as in 1.1.4. The summary also carries `blocked_kind`, `blocked_side`, `blocked_count` and `blocked_percent`, so in M2 the apps should build the localized sentence from those fields.
2. **Case-sensitive ignore patterns.** `.waslignore` patterns are case-sensitive, like git, so every OS reaches the same verdict. The 1.1.4 pair patterns stay case-insensitive.
