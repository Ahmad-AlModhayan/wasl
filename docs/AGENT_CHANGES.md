# Agent changes: who changed your files, held until you say yes

Status: design, 2026-10-25. Builds on the safety layer (SRS §3.7). Comes from Kahf's agent time-machine idea (an
actor-tagged op-log of agent changes), adapted from a database op-log to Wasl's synced folders.

## 1. Why

AI agents (Claude Code, Gemini CLI, personal assistants and whatever comes next) now edit, create and delete files on their own.
A synced folder makes that worse: Wasl sends an agent's mistake to Drive and to every device within seconds. Git
helps only for code inside a repository, and only when the agent commits cleanly. Nothing tells you "these 14
files were changed by Claude Code during the 3 PM task; keep them or undo them all?"

Wasl already asks before a risky sync (first sync, mass delete, conflicts) and keeps what it replaces for 30 days.
This adds the other half: **an agent's changes stay on this computer, labelled with the agent's name and grouped
into one changeset, until you accept them. Revert puts every file back in one step.**

## 2. Words

| Word | Meaning |
|---|---|
| Actor | Who made a change: `agent:<name>` (`agent:claude-code`, `agent:gemini`, `agent:my-assistant`). Everything else is the person on this device. |
| Session | The time an agent works in a folder: from *begin* to *end*. With Claude Code, one session is one request (one prompt and the work it starts). |
| Scope | The folder the agent works in: the pair folder or a subfolder of it. Changes outside the scope are not the agent's. |
| Changeset | What one session changed inside its scope: files added, changed and deleted, each with its earlier version. |
| Held | A changeset waiting for you. Its files are not synced anywhere: Drive and your other devices keep the earlier versions. |

## 3. Behaviour

1. **Begin.** Wasl records the scope's file list (path, size, modification time; ignored items excluded) and copies
   every file there whose current content is not yet synced, so each earlier version can be put back. A file already
   synced needs no copy: Drive or the other devices hold it, and a held changeset keeps it that way.
2. **Before a write (optional, exact).** An agent that can tell Wasl a file is about to change (Claude Code's
   `PreToolUse` hook) makes Wasl copy it first. That copy is the earlier version, even for a file changed since the
   last sync.
3. **While the session is open,** sync leaves alone every key in the scope that differs from the begin list: no
   upload, download or deletion of it on any side. The rest of the pair syncs as usual.
4. **End.** Wasl compares the scope with the begin list. Each added, changed or deleted file becomes an entry of the
   changeset, along with where its earlier version is (a saved copy, the synced version, or none for a new file). A
   file that Wasl itself downloaded during the session is not the agent's and is left out. A session that changed
   nothing leaves no changeset.
5. **Held.** The changeset's keys stay out of sync until you decide. `wasl agent list` and the Problems list show it
   as "needs you".
6. **Accept**, once you confirm it in the Wasl window that opens (§5), releases the keys. The next sync sends the agent's work everywhere, and the changeset stays in history.
7. **Revert** goes through the entries, deepest paths first. The agent's version goes to the recycle bin (so a revert
   can be undone too), and the earlier version comes back with its old modification time, so sync sees nothing to
   do. A new file goes to the bin, a deleted file comes back, and a new empty folder is removed.
8. **You win.** A file you changed after the session ended is skipped by revert and reported, never overwritten.
   (During an open session Wasl cannot tell your edits from the agent's inside its scope. Work outside the scope,
   or end the session first.)
9. A held changeset never expires. Its saved copies are deleted once it is accepted or reverted.

## 4. How agents connect

All of these go through `wasld`. A hook never blocks the agent: if `wasld` is not running, the hook prints a warning
and exits 0.

| Agent | How |
|---|---|
| Any CLI agent | `wasl agent run --as gemini -- gemini`: begins in the current folder, runs the agent, ends when it exits. |
| Claude Code | Hooks from `wasl agent hooks claude-code`: `UserPromptSubmit` begins, `PreToolUse` (Edit, Write, MultiEdit, NotebookEdit) saves the file, `Stop` ends. One changeset per request. |
| Assistants, scripts | `wasl agent begin --as my-assistant --dir <folder>` … `wasl agent end`. |

Later: an MCP server so agents can ask Wasl what is held and propose changes without touching the folder, plus
review on the phone (the changeset summary in the Drive app folder, approved from any device in the group).

## 5. Limits (v1)

- **Only you accept, in a Wasl window.** Since the security review of 2026-10 (S3-1), asking to accept never
  accepts by itself. `wasld` opens a confirmation of its own: it starts the Wasl app's dialog
  (`wasl-desktop --confirm-agent`, the app's binary next to `wasld`), which shows the agent, the number of items,
  the folder, the agent's label, which client asked and a 6-digit code. Only a click on **Accept** there accepts.
  `wasl agent accept` and the app's **Accept** button both print or show the code and wait; the CLI tells you to
  confirm in the Wasl window. Without the app installed next to `wasld`, accepting is refused (development builds
  name it with `WASL_DESKTOP_APP` in `wasld`'s environment). Revert stays open to agents, since it only puts
  earlier versions back.

  How the dialog's answer is trusted: `wasld` mints a token for that one changeset (256 random bits, single use,
  valid for 2 minutes) and gives it to the dialog through the child process's stdin; the dialog writes it back on
  its stdout only when you click Accept. The token is never written to disk, a command line, an environment
  variable or the IPC socket. Every program running as you can read the IPC token and speak to `wasld`, so it can
  ask for an accept, but it cannot answer one: that takes your click. One confirmation is open at a time (asking
  again for the same changeset gets the same code; another changeset is refused until the first is answered), so
  a flood of requests cannot bury you in dialogs. Check the code: a dialog you did not ask for is an agent asking.

  After you decline a confirmation, or let it time out, that changeset gets no new dialog for a while: 1 minute,
  then twice as long after each further refusal, up to 30 minutes; accepting resets it (since the review of phases
  S2–S5). Asking during the wait opens nothing: `wasl agent accept` and the app say "try again in N minutes"
  (`AcceptConfirmation.retry_in_secs`), so an agent asking in a loop cannot keep a dialog on screen until it is
  clicked by mistake or out of weariness. The token is never printed: the dialog's and the daemon's debug output
  leave it out.

  Still in place: `wasl agent accept` refuses inside an agent (`WASL_AGENT_SESSION`, set by `agent run`, or Claude
  Code's `CLAUDECODE`, or `GEMINI_CLI`), the CLI sends the marker it found and `wasld` refuses any accept that
  carries one, before any dialog. Each accept records which client asked (`cli`, `desktop`, `unknown`).

  A pair with an open or held changeset can't be removed, or pointed at another folder, Drive folder or vault,
  until the changeset is accepted or reverted (#134). Its rows go with the pair, so otherwise an agent could
  remove the pair, make it again on the same folders and sync its files with no hold.

  What this is, plainly: a deterrent against an agent silently accepting its own work, not a security boundary.
  What it does not stop: a program running as you can, with deliberate effort, click the dialog's
  button through the operating system's accessibility or input APIs, read the token from the app's or `wasld`'s
  memory, replace the app's binary (per-user installs are writable by you), edit Wasl's database directly, or stop
  `wasld` and start its own copy. None of that is possible through Wasl's CLI or protocol alone, and none of it can
  be ruled out by any check inside your own session: the bar is that an agent must attack Wasl, not merely call it.
  A dialog that appears behind other windows (Windows limits which programs may take the focus) waits in the
  taskbar until it is answered or the 2 minutes pass.
- **Saved copies are checked.** Each earlier version copied to the agent folder is hashed (BLAKE3) as it is saved.
  Revert and diff hash it again: a copy changed after it was saved is never put back or shown; that file is
  reported as failed ("the earlier copy was changed after it was saved") and the agent's version stays.

- Only one open session per scope. Two agents may work at the same time in folders that do not overlap.
- Changesets live on the computer the agent ran on. Accepting and reverting happen there: the desktop app's
  **Agent changes** screen (Accept, Revert, Show changes) or `wasl agent`. `wasl problems` lists held ones.
- An agent that runs shell commands may change files without a hook seeing them. The session diff still catches
  them, but their earlier versions come only from the begin copy or the synced version.
- Device-to-device pairs (route *devices only*) hold the same way: a held key is reported unchanged to peers, and
  peer changes to it wait. Fetching an earlier version back from a peer is not built yet: on these pairs revert
  relies on the copies (made before a write, or at begin for files not yet synced, up to 512 MB per session and
  100 MB per file) and reports a file it cannot put back.

## 6. Data (schema v8, v9)

- `agent_changesets(id, pair_id, actor, label, scope, session, status, started_at, ended_at, decided_at)`, where
  status is `open`, `held`, `accepted`, `reverted` or `partly_reverted`.
- `agent_manifest(changeset_id, rel, is_dir, size, mtime)`: the begin list.
- `agent_saved(changeset_id, rel, location, size, mtime, blake3)`: copies taken at begin or before a write, with
  the BLAKE3 of each (v9; empty for copies saved before).
- v9 adds `agent_changesets.decided_by`: which client accepted.
- `agent_entries(changeset_id, rel, key, kind, is_dir, before_size, before_mtime, after_size, after_mtime, source,
  location, outcome)`, where kind is `added`, `changed` or `deleted`, source is `saved`, `synced` or `none`, and
  outcome is empty, `reverted`, `skipped_changed` or `failed`.

Copies live under `<data dir>/agent/<changeset id>/`.

## 7. Commands

```
wasl agent run --as NAME [--dir PATH] [--label TEXT] -- COMMAND [ARGS…]
wasl agent begin --as NAME [--dir PATH] [--label TEXT] [--session ID]
wasl agent save PATH [--session ID]
wasl agent end [--session ID | --dir PATH]
wasl agent list [--all] [--json]
wasl agent show ID [--diff]
wasl agent accept ID                  # opens the confirmation in the Wasl app and waits
wasl agent revert ID
wasl agent hooks claude-code          # prints the settings.json block
wasl agent hook claude-code EVENT     # called by Claude Code; reads its JSON on stdin
```
