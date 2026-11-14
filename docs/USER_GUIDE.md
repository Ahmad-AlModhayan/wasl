# Drive Sync — user guide

## First run
1. Install the APK (see SETUP.md), open the app, follow the onboarding: All files access, notifications, battery optimization exemption, optional location (only for Wi-Fi name rules), then **Add Google account**.
2. The app indexes your Drive once. Large accounts take a minute; you can create pairs meanwhile.

## Folder pairs
A pair links one device folder with one Drive folder and a **method**:
| Method | Use it for |
|---|---|
| Two-way | Working folders you edit on both sides |
| Upload only | Camera/photo backup, recordings, exports |
| Upload then delete | Moving files off the phone |
| Upload mirror | Drive must always equal the phone folder |
| Download only | Reference material you want offline |
| Download then delete | Inbox-style hand-off from Drive to phone |
| Download mirror | Phone folder must always equal Drive |

Each pair has its own conditions (Wi-Fi, metered, roaming, VPN, charging, battery floor, Wi-Fi allow/deny lists), include/exclude patterns, subfolder exclusions (Selective sync), size limit, conflict policy, deletion propagation, Google Docs export format, optional encryption vault, interval override and instant sync.

**Google Docs, Sheets and Slides** are exported to Office files (or PDF): the Doc `Report` becomes `Report.docx`. They are copies Wasl never uploads back. When a real file already has that name, on Drive or on the device, the real file keeps it and the export is named after its kind: `Report (Google Doc).docx`, `Budget (Google Sheet).xlsx`, `Deck (Google Slides).pptx`. If that name is taken too, Wasl adds a number: `Report (Google Doc 2).docx`.

**Preview (dry run)** shows exactly what a run would do before it does it. Use it after creating a pair and after big reorganisations.

## Safety features
- **Conflict copies**: when both sides changed, the device copy is renamed "name (conflicted copy from <device>, <date>).ext" and both are kept. Conflicts are listed under Problems until you mark them resolved.
- **Recycle bin**: anything the sync deletes or overwrites on the device goes to the app's recycle bin first (Home → menu → Recycle bin) and is purged after the retention period. Deletions on Drive go to Drive's own trash.
- **Mass-delete guard**: a run that would delete a large share of a folder is blocked until you approve it once from the pair screen or the Problems screen.
- **Versions**: any file in a pair or in the Drive browser has a Versions screen (Drive revisions) with restore and keep-forever.

## AI agents (computer)
When an AI agent (Claude Code, Gemini CLI or any command-line agent) changes files in a synced folder, Wasl keeps those changes **on this computer only**, labelled with the agent's name, until you decide. Drive and your other devices keep the earlier versions meanwhile. The tray shows "Needs you" and the app's **Agent changes** screen lists each changeset with its files and a line-by-line view of what changed.

- **Accept** sends the agent's work everywhere at once.
- **Revert** puts every file back exactly as it was before the session. The agent's versions go to the recycle bin, so even a revert can be undone. A file you changed yourself after the agent finished is left alone and reported.
- **Connect Claude Code** from the Agent changes screen (Copy hooks, then paste into `~/.claude/settings.json`): each request you give Claude becomes one changeset, and every file Claude edits is copied first.
- **Other agents**: start them through Wasl in the folder they should work in, e.g. `wasl agent run --as gemini -- gemini`.
- From a terminal: `wasl agent list`, `wasl agent show <id> --diff`, `wasl agent accept <id>`, `wasl agent revert <id>`. Accepting asks you first and is refused inside an agent's own shell.
- An agent that crashed leaves its session "Working": use **End session** (or `wasl agent end <id>`) and decide as usual.
- The op-log, every actor's changesets, read-only: `wasl oplog list [--actor agent:claude-code] [--status staged] [--since 7d] [--pair Work] [--limit 100]` and `wasl oplog show <id>`. Add `--json` for the format other tools read (ADR-0002).

Details: AGENT_CHANGES.md.

## Instant sync
Settings → Instant sync keeps a small foreground watcher running. Local changes are picked up within seconds; Drive-side changes within the remote poll interval. Turn it off to rely on the periodic schedule only.

## Status notification
While any pair is turned on, one silent notification says what Drive Sync is doing: which pair is syncing and how far along, or that it is waiting for changes and when the next check is, or that something is paused or needs you. It stays: swiped away or cleared, it comes straight back.

A run that stops making progress for 15 minutes (a connection that hangs, a call that never answers) is abandoned and tried again, so a pair never sits on "Syncing" for hours. A run Android stops part way, or one that failed, is picked up by the next check; everything synced before the stop stays synced. A change made while its pair is syncing gets one more run right after.

## Pair status
Every pair says, on its Home row and on its own screen, what it is doing: up to date, syncing, waiting, paused, or needs you. This device shows "No network", "Can't reach Drive" or "Sign in again" when it can't sync, each with what to do. Below that is one line for each other end the pair uses: Drive, if the pair syncs through Drive, and each other device, if it syncs with devices. A device line says whether it is online and how it is connected, or when it was last seen, or why Wasl is waiting for it. It also says whether your changes have reached that device, or how many are still waiting for it. Inside a pair, each file and folder is marked Synced, Waiting, Conflict or Held (kept back while an AI agent works). `wasl status` prints the same lines.

## Drive in the Files app
Home → menu → Browse Drive, or open the system Files app: Drive Sync appears as a location. Files stream on open without downloading; pin files or folders for offline use.

## Encryption vaults
Home → menu → Vaults. Create a vault with a passphrase, then create a pair with "Encrypt with vault" and an **empty** Drive folder. Names and contents are encrypted on the phone before upload; see ENCRYPTION.md to decrypt on a computer. Losing the passphrase loses the data.

## Backup and restore
Home → menu → Backup & restore backs up pair definitions and settings to Drive app data (daily when enabled) or to a file. On a new phone: install, add the account, then Restore.

## If something does not sync
Open Diagnostics: it checks permissions, battery optimization, the background jobs, vendor autostart restrictions and Drive connectivity, and offers a fix for each. Problems lists errors with the fix in plain words.
