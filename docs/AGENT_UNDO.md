# Agent undo: put back what an agent changed, in any folder

| | |
|---|---|
| Feature | Agent undo: snapshot, file journal and restore for any folder an AI agent can reach |
| Host product | Wasl (وصل), repository `wasl` |
| Status | Accepted into Wasl 2 on 2026-11-01 (PRD goal 1 and §5, SRS §3.16) |
| Version | 0.3 (2026-11-01) |
| Builds on | `docs/AGENT_CHANGES.md` (SRS §3.14, FR-AG1–AG5), the safety layer (SRS §3.7), the op-log (`docs/adr/0002-one-op-log.md`, SRS §3.15) |

## 1. What it adds

Wasl already holds an agent's changes in a synced folder until you accept or revert them (`docs/AGENT_CHANGES.md`): one pair folder, one changeset, all or nothing. Agent undo covers the rest.

- **Any folder an agent can reach,** synced or not: a repository that never syncs, `~/Documents`, an ignored `.env`. You list these folders once (the watch list).
- **Every change as it happens.** A snapshot at session start, then a journal of every create, change, rename and delete, with the content before it. Wasl watches the file system, so shell commands (`rm -rf`, `mv`, `sed -i`, a build script) are covered as well as the agent's editing tools.
- **Restore at any grain.** One file, a folder or the whole session, back to session start or to a marker such as `before: npm run clean`, with a dry run first and an undo for the restore itself.
- **Plain limits.** Every session report says what Wasl cannot undo: network calls, sent email, database writes, pushed commits, files outside the watch list.

What stays: in a pair folder the agent's changes are still held from sync until you accept, and Accept still needs your click in the Wasl window (review S3-1). Agent undo adds a timeline beside the hold.

## 2. Why it lives in Wasl

1. **The store exists.** `wasl-chunk` (FastCDC, BLAKE3) and the `chunks` table deduplicate content; the recycle bin makes reflink copies (`reflink-copy`); agent copies are already BLAKE3-checked. Inside Wasl there is one format and one daemon.
2. **The watchers exist.** `wasl-fs` uses ReadDirectoryChangesW, FSEvents and inotify/fanotify, with the periodic scan as the source of truth (FR-C2).
3. **The agent plumbing exists:** sessions, actors (`agent:claude-code`), `wasl agent run`, the hooks, the **Agent changes** screen, `wasld`'s confirmation dialog.
4. **Wasl 2's op-log is the model** (ADR-0002): actor-tagged operations in changesets that are staged, committed or reverted. A file journal is that log for files.
5. **It extends Wasl's first promise,** "no data loss caused by Wasl" (PRD goal 1), to losses caused by agents. Safety is never gated (FR-R6), and local features have no Pro tier.

The need: TechRadar reports a Claude Code session that allegedly deleted 48,000 files in 103 seconds; Gigazine, Docker's blog, a Cursor thread and Adversa's list describe more `rm -rf` cases (press and user reports, not audited incidents). Claude Code's checkpoints do not track files changed by Bash commands; Gemini CLI's checkpointing is off by default; Git covers committed, tracked files only.

## 3. Users and jobs

| User | Job to be done |
|---|---|
| Developer using coding agents | "Let the agent run its commands. If it wipes something, give me back exactly what was there." |
| Power user with a desktop agent | "The agent reorganised my Documents folder. Put back the three files it misplaced and leave the rest." |
| A personal assistant agent | "Mark a restore point before a risky file step and roll back if it fails." |

## 4. Scope

| Priority | Feature |
|---|---|
| P0 | Watch list (pair folders or not); session-start snapshot (copy-on-write clone where supported, else the chunk store); journal with content captured after each change; "not captured" list |
| P0 | Markers; restore of a file, folder or session to session start or a marker; dry run; undo of a restore; session report; caps |
| P0 | Timeline and Restore on the desktop **Agent changes** screen; new `wasl agent` commands; sessions recorded as op-log changesets (ADR-0002) |
| P1 | Risky restores and large deletions ask through an external approvals inbox when one is configured, else the Wasl window; delete guard; MCP tools `checkpoint`, `list_checkpoints`, `diff_since` through `kahf-mcp` |
| P2 | Restore from a peer device's copy (today a limit of device-only pairs); Android |

## 5. Requirements

| ID | Requirement | Pri | Acceptance |
|---|---|---|---|
| UND-1 | `wasl agent watch <dir>` and `unwatch` manage the watch list; `.waslignore` applies; default ignores (`node_modules/`, `.venv/`, `target/`, `build/`) are listed and removable; file-system roots and Wasl's data folder are refused | P0 | `wasl agent watch /` and `C:\` fail; ignore fixture tree |
| UND-2 | A session covers its folder and every watched folder. At begin: APFS clones, Btrfs snapshots or reflinks where supported, else BLAKE3 content IDs in the chunk store, read incrementally; synced pair files reuse the synced version (FR-AG1) | P0 | Snapshot hashes equal folder hashes per file system in CI; unchanged 10 GB re-snapshot reads < 1% (macOS and Linux; on Windows every file is read again until the NTFS file index and change time are read, #194) |
| UND-3 | After each create, change or rename, capture the new content and journal the previous and new content IDs; deletes journal the previous ID | P0 | A file changed five times one second apart restores to all six versions |
| UND-4 | On watcher overflow, error or daemon restart, rescan against the last known state; list each lost earlier version as not captured, with the reason | P0 | Forced overflow during a 50,000-file delete: every file restorable or listed |
| UND-5 | Markers: the Claude Code `PreToolUse` hook (matcher now includes `Bash`) adds one per tool call with the command; `wasl agent mark "<label>"`; restores add their own | P0 | Scripted session: one marker per call, in order in `wasl agent log` |
| UND-6 | `wasl agent restore <path> --to <marker>` (or `begin`, or a time) and `--session <id>` show a dry run (create, overwrite, delete, sizes) and apply on confirmation; bytes, mode bits and modification time equal the captured version; symlinks restored as links | P0 | Property test: random operations, then session restore equals the snapshot |
| UND-7 | A restore first captures what it will touch and adds a `restore` marker; `wasl agent undo-restore` reverses it; replaced versions go to the recycle bin. You win: a file changed after the session is skipped unless you include it | P0 | Restore then undo-restore returns the pre-restore bytes; conflict fixture |
| UND-8 | In a pair folder the session is also a held changeset: FR-AG1–AG5 apply unchanged; restoring part of it marks the changeset `partly_reverted` | P0 | Existing FR-AG tests pass unchanged |
| UND-9 | Each journal entry is an op-log operation (ADR-0002), written through `wasl-store`'s `oplog` module: in the session's changeset, with its actor (`agent:<name>`), on `file://<pair id or watch id>/<key>` (the path relative to the pair or watch root, each segment percent-encoded), with the state before and after (content ID: BLAKE3); a rename is a `delete` plus a `create`. A held session is `staged`; accepted, or closed outside a pair, `committed`; restored, `reverted` or `partly_reverted`, with inverse operations appended | P0 | A restore appends rows; row count never drops; the FR-OL tests pass |
| UND-10 | Session report: counts by operation, the not-captured list and the fixed notice of what cannot be undone, in the app, CLI and `--json` | P0 | Snapshot tests in Arabic and English |
| UND-11 | Caps 20 GB per store and 2 GB per file (larger files journaled, not captured); closed sessions kept 30 days, oldest pruned first; held and pinned never pruned. Store outside every watched folder, owner-only, never synced; no network use | P0 | Clock-advance test; over-cap file listed; permission test; packet capture |
| UND-12 | Targets: session start < 2 s for 10 GB and 100,000 files with copy-on-write, < 30 s through the store after the first snapshot; capture < 1 s p95 | P0 | Benchmark per OS |
| UND-13 | A restore that overwrites files changed after the session, crosses the mass-delete thresholds (20 files or 5%), or is asked for by an agent sends an approval request (`source_app: "wasl"`) to the external approvals inbox when configured, else opens the Wasl window confirmation | P1 | Decision matrix against a local inbox; `denied` and `expired` restore nothing |
| UND-14 | Delete guard: the `Bash` hook resolves deletes and moves (`rm -rf`, `find -delete`, `git clean -fdx`) outside the watch list or into Wasl's data folder, asks, and answers `deny` unless approved; pattern-based, and documented so | P1 | Pattern suite |

## 6. How it fits Wasl's architecture

```
 agent --(wasl agent run | Claude Code hooks)--> wasl CLI / desktop app --IPC--> wasld
   wasld: snapshot (reflink-copy | wasl-chunk) <-- watchers (wasl-fs)
          journal + markers (wasl-store v12) --> op-log (wasl-store oplog, v11)
          restore --> recycle bin; risky restore --> approvals inbox | Wasl window
```

- **`wasl-fs`** gains capture-after-change; **`wasl-chunk`** holds snapshots and captured versions, pinned until pruned.
- **`wasl-store`** schema v12 (after the op-log's v11): `agent_watches`, `agent_journal(changeset_id, seq, op, rel, old_rel, prev_blake3, next_blake3, origin, captured, reason)`, `agent_markers`, `agent_restores(…, decided_by, inbox_request_id)`; `agent_changesets.pair_id` becomes optional, with `watch_id`.
- **Op-log.** The journal writes each operation through `wasl-store`'s `oplog` module (ADR-0002, schema v11), in the same transaction as its `agent_journal` row; a restore appends the inverse operations there. The `agent_*` tables keep the working detail (where a captured version is, why one wasn't captured), as they do for pair folders. `wasl-crdt` comes only later: the `oplog` module's pure part moves into it once a second user needs it (#53), with its own ADR.
- **`wasld`** owns sessions and reuses the `wasl-desktop --confirm-agent` dialog when no inbox is configured. **CLI:** `wasl agent watch`, `unwatch`, `log`, `mark`, `restore`, `undo-restore` beside today's commands.
- **Native UI clients:** the Agent changes screen consumes `wasld` IPC and `wasl` CLI dry-run/oplog output across the IPC boundary.
- **Approvals inbox:** requests go to an inbox on the user's own hub. Android is unchanged.

## 7. Risks

| Risk | Mitigation |
|---|---|
| A file changes again before Wasl copies it | The guarantee is the session-start state plus every captured version; intermediate versions are documented as not guaranteed |
| The agent deletes the store (`rm -rf ~`) | Store outside every watched folder; the P1 guard protects its path; option to place it outside the home folder |
| People believe it undoes everything | The cannot-undo notice at session start, in every report and on the screen |
| The store holds copies of secrets (`.env`) or fills the disk | Owner-only, never synced; caps, default ignores, pruning; encryption at rest is open |

## 8. Open questions

1. When agent undo enters Wasl's PRD (as a goal under "no data loss") and its milestones; it needs the desktop daemon from M3. Answered 2026-11-01: it is in Wasl 2. PRD goal 1 covers losses caused by agents, PRD §5 has its P0 and P1 rows, and SRS §3.16 makes UND-1–UND-14 requirements. Work starts in cycle 1 (2026-11-01 → 2026-11-14) with #56 and #57, on the M3 desktop daemon.
2. Retention: 30 days (matching the recycle bin) or a shorter 14; is a 20 GB cap enough for large monorepos?
3. Encrypt the store at rest, with the key in the OS keychain?
4. Move `agent_changesets` into `wasl_oplog` once `wasl-crdt` exists, or keep both with a view? Answered in `docs/adr/0002-one-op-log.md`: both stay, linked by changeset id; the op-log (schema v11) is the record, `agent_*` the working detail.
5. Do Codex, Cursor and Gemini CLI offer hooks like Claude Code's? Until confirmed they run under `wasl agent run` with session-level markers.

## 9. Sources

- Wasl docs: `docs/` (AGENT_CHANGES, PRD, SRS).
- [TechRadar: Claude Code allegedly deleted 48,000 files in 103 seconds](https://techradar.com/ai-platforms-assistants/do-not-let-programs-run-commands-on-your-machine-claude-code-allegedly-deleted-48-000-files-in-103-seconds-and-its-a-terrifying-warning-about-ai-agents)
- [Docker blog: Coding agent horror stories, the rm -rf incident](https://www.docker.com/blog/coding-agent-horror-stories-the-rm-rf-incident/)
- [Gigazine: Claude Code CLI deleted a Mac home directory](https://gigazine.net/gsc_news/en/20251216-claude-code-cli-mac-deleted)
- [Cursor forum: rm -rf report](https://forum.cursor.com/t/fix-this-rm-rf-that-folder/107227)
- [Adversa AI: AI coding agent incidents](https://adversa.ai/blog/ai-coding-agent-incidents/)
- [Claude Code docs: Checkpointing, Bash command changes not tracked (read 2026-10-23)](https://code.claude.com/docs/en/checkpointing)
- [Claude Code docs: Hooks reference (read 2026-10-23)](https://code.claude.com/docs/en/hooks)
- [Gemini CLI docs: Checkpointing (read 2026-10-23)](https://geminicli.com/docs/cli/checkpointing/)
