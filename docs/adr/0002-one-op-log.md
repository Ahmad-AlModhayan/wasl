# ADR-0002: One op-log for every actor's changes

Date: 2026-11-01. Status: Accepted (2026-11-01).

## Context

Wasl records agent changes (`docs/AGENT_CHANGES.md`, SRS §3.14) in `agent_changesets`,
`agent_manifest`, `agent_saved` and `agent_entries` (schema v8, v9). Those tables are the working
detail of a file session: the begin list, where each saved copy is and its BLAKE3, the hold that
sync reads, the error text of a failed revert. Nothing outside Wasl can read them, and they only
know files in a pair.

Three plans assume a shared record on top:

- Agent undo (`docs/AGENT_UNDO.md`, UND-9) journals each change as an operation with `actor_id`,
  `changeset_id`, `entity_uri` and previous and next content, staged, committed or reverted. Its
  open question 4 asks whether `agent_changesets` moves into that op-log.
- Another Kahf Labs app, written in Dart, keeps the same shape (`agent_changesets` and `agent_ops`, the
  "you win" rule, Discard of a waiting proposal and Revert of an accepted one).
- Other tools and apps need to read who changed what, without reading Wasl's file tables.

The SRS doesn't specify the op-log yet.

## Decision

### Where it lives

Two tables in `wasl-store`, schema v11 (`0011_oplog.sql`): `oplog_changesets` and `oplog_ops`.
The module `wasl-store/src/oplog.rs` owns the rules and is the only writer: it checks every move
in the same transaction as the write and refuses a move this ADR doesn't allow with a typed error.
Its pure part (the types, the allowed moves, actor and entity URI parsing) imports no SQL and no
other Wasl crate, so it can later move into the `wasl-crdt` package (planned AGPL-3.0 plus a
commercial licence) without a rewrite. That move gets its own ADR when the package has a second
user (syncing changesets, #53).

### The model

A **changeset** is one actor's set of changes in one scope. An **operation** is one change to one
entity inside it.

`oplog_changesets`, primary key (`origin_device`, `id`)

| Column | Meaning |
|---|---|
| `origin_device` TEXT | The device the changes were made on (SRS §1.4: Device), as its device id (Actors below). Always this device until #53 |
| `id` TEXT | Same id as the `agent_changesets` row it records, when there is one. Ids are `<ms>-<counter>` (`new_id`), unique only on the device that made them, so a changeset is named by (`origin_device`, `id`) everywhere |
| `actor` TEXT | Who made the changes (Actors below) |
| `label` TEXT? | The actor's own words for the work |
| `scope` TEXT | Entity URI of the folder (or other container) the changeset covers |
| `status` TEXT | `open`, `staged`, `committed`, `discarded`, `reverted`, `partly_reverted` |
| `opened_at`, `closed_at?`, `decided_at?` INTEGER | Milliseconds since the Unix epoch, as in `agent_changesets` |
| `decided_by` TEXT? | Which client decided (`cli`, `desktop`, `unknown`), as `agent_changesets.decided_by` |
| `decided_actor` TEXT? | Who decided, as an actor (Actors below): the agent when an agent asked (agents may revert), else `person:<device id>`. Set with `decided_by`, so an agent's revert never reads as the person's |
| `imported_at` INTEGER? | Set only on a changeset written by the one-time v10 copy (below), milliseconds |

`oplog_ops`, primary key (`origin_device`, `changeset_id`, `seq`)

| Column | Meaning |
|---|---|
| `origin_device`, `changeset_id` TEXT | Together reference `oplog_changesets(origin_device, id)`, `ON DELETE CASCADE` (used only by pruning) |
| `seq` INTEGER | 1, 2, 3 … per changeset, no gaps, in the order written |
| `entity_uri` TEXT | What changed (Entity URIs below) |
| `op` TEXT | For `file://`: `create`, `update`, `delete`. Other schemes name their own |
| `prev`, `next` TEXT? | JSON of the state before and after; empty for a created or deleted entity's missing side |
| `reverts` INTEGER? | On an inverse operation: the `seq` it takes back |
| `outcome` TEXT? | Empty until decided; then as in Outcomes below |
| `at` INTEGER | When it was written, milliseconds |

A file's state is `{"kind": "file", "size": n, "mtime_ms": n, "blake3": "<hex>"}`, `blake3` only
where Wasl knows it; a folder's is `{"kind": "dir"}`. A rename is a `delete` plus a `create` (the
agent entries have no rename either); agent undo may add a `move` with its own inverse later.

There is no foreign key to `pairs`: the record outlives a deleted pair until it is pruned.

### Actors

- `agent:<name>`, `<name>` 1 to 64 lowercase ASCII letters, digits, `.`, `_` and `-` (`[a-z0-9._-]{1,64}`,
  exactly what `actor_id` in `wasl-engine` accepts): `agent:claude-code`, `agent:gemini`,
  `agent:planner`. This is today's `agent_changesets.actor`.
- `person:<device id>`: the person using that device. Wasl doesn't record a person's own edits yet;
  the form is fixed now so products that do write the same thing.

A device id is the `DeviceId` as Wasl writes it everywhere (`WASL-XXXX-…-XXXX`): `DeviceKey::id()` in
`wasl-net/src/identity.rs`, the first 16 bytes of BLAKE3 of the device's Ed25519 public key. This
device's key seed lives in the OS keychain (entry `device-key`, `KeychainKeyStore` in
`wasld/src/tokens.rs`; on Android the app's key store through `wasl-ffi`), and `Devices::id()` in
`wasl-devices` returns the id. `wasl-store` doesn't hold it, so the engine passes it to the `oplog`
module for `origin_device` and `person:<device id>`.

Anything else is refused.

The op-log records only changes made under an actor: agent sessions now, a person's own changes
later. Changes that arrive through Drive or from another device's sync have no actor and are not
recorded in it.

### Entity URIs

- `file://<pair id or watch id>/<key>`: the key (SRS §1.4: the file's canonical path relative to the
  pair root, as named on Drive; not the local path `rel`, which can differ) with each path segment percent-encoded
  (RFC 3986), so spaces, `#`, `?`, `%` and non-ASCII names round-trip. The authority is a Wasl id,
  not a host: this is not an OS file URL. Watch ids (agent undo) are drawn from the same id space as
  pair ids. A folder scope ends in `/`; the pair root is `file://<pair id>/`.
- Any other scheme (`app://…`, for example a block of another app) is stored and returned byte for byte.
  Wasl checks only that it is an absolute URI and never parses it further.

### States and moves

| From | To | When |
|---|---|---|
| (new) | `open` | A session begins |
| `open` | `staged` | The session ends with changes to hold (a pair folder: Wasl's `held`) |
| `open` | `committed` | The session ends with nothing to hold (agent undo outside a pair: UND-9 "closed") |
| `open` | (deleted) | The session ends with no operations: an empty changeset leaves no record |
| `staged` | `committed` | Accepted. In Wasl only in the Wasl window (AGENT_CHANGES §5, review S3-1) |
| `staged` | `discarded` | the Dart app's Discard: nothing took effect, so nothing is appended |
| `staged`, `committed` | `reverted` or `partly_reverted` | A revert ends: `reverted` when every operation that took effect is taken back, else `partly_reverted` |
| `partly_reverted` | `reverted` | A later revert takes back the rest (UND-7: you include a skipped file) |
| `partly_reverted` | `partly_reverted` | A later revert that still leaves files skipped: only outcomes change |

Every other move is refused. Moves never go back: nothing returns to `open` or `staged`, and
`discarded` and `reverted` are final. `partly_reverted` → `partly_reverted` is the only move to the
same state: the status stays, and the revert appends its inverse operations and changes only outcomes.
Operations are appended only while the changeset is `open`, plus the inverse operations a revert
appends. Who may ask for a move (accept only from the Wasl window, revert open to agents) is the
engine's and `wasld`'s rule; the store checks only that the move is allowed.

Wasl and the Dart app map onto it like this:

| Wasl (`agent_changesets.status`) | Dart app | Op-log |
|---|---|---|
| `open` | (proposals arrive whole) | `open` |
| `held` | waiting proposal | `staged` |
| `accepted` | accepted | `committed` |
| `reverted` | reverted | `reverted` |
| `partly_reverted` | reverted, some `kept:changed` | `partly_reverted` |
| (none: a held revert puts files back) | Discard | `discarded` |

### Outcomes and append-only

| Outcome | Meaning |
|---|---|
| `applied` | The operation stands: set on commit, and on every inverse operation as it is appended |
| `discarded` | It never took effect: Discard, or left unticked in a partial accept (Dart app), or its `skipped:changed` at accept |
| `skipped_changed` | A revert left it alone because the entity changed after the changeset (you win; the Dart app's `kept:changed`) |
| `failed` | A revert could not take it back; the reason stays in the working detail (`agent_entries.outcome`) |
| `reverted` | An inverse operation was appended for it |

An outcome moves only forward: empty to any value; `applied` to `skipped_changed`, `failed` or
`reverted`; `skipped_changed` or `failed` to `reverted`. `discarded` and `reverted` are final.
One exception: a resumed or later revert runs `skipped_changed` and `failed` operations again, so
those two may be decided again (to `skipped_changed`, `failed` or `reverted`) while the changeset's
status hasn't moved past that decision (`staged`, `committed` or `partly_reverted`). Once the
changeset is `reverted`, every outcome is final.

Rows are never deleted or rewritten while the changeset is kept. `entity_uri`, `op`, `prev`,
`next`, `reverts` and `at` never change once written; only `status` and `outcome` move, and only
as above (the one-time v10 copy below is the only write that skips these checks). A revert appends, for each operation it takes back, an inverse operation in the same
changeset (`reverts` = that `seq`, `prev` and `next` swapped, `create` ↔ `delete`) and sets the
original's outcome to `reverted`, both in one transaction, so a revert cut short carries on where it
stopped.

Pruning is the only other delete. A changeset that is `committed`, `discarded`, `reverted` or
`partly_reverted` goes 90 days after `decided_at`, with all its operations, oldest first: the same
window as `agent_changesets` today (`AGENT_HISTORY_MS`), so the record and its detail go together.
`open` and `staged` changesets are never pruned. The record keeps 90 days, longer than the
contents, because it holds no file contents: contents follow their own windows (the recycle bin 30 days, the
agent undo store 30 days, UND-11). A revert after those windows reports the operations whose
contents are gone as `failed`.

### The agent tables stay, as the working detail (AGENT_UNDO open question 4)

`agent_changesets` and its tables stay as they are. The op-log becomes the record every reader
uses, linked by changeset id. `wasl-store` writes both in one transaction: every path that changes
`agent_changesets.status`, and every write of an entry's outcome (`set_entry_outcome`), also writes
the op-log in the same transaction (#47), mapped as above, with one operation per `agent_entries`
row on `file://<pair id>/<key>`. So a revert cut short leaves both records at the same point. A
changeset is opened in the op-log when its session begins, so readers see an agent at work.

Changesets made before v11 are copied once, by the engine at start: each `agent_changesets` id not
yet in the op-log, with its entries (its open session's operations follow at end, as for a new
one). The copy is an import: it writes each changeset with its final status and outcomes as they
are (a v10 `reverted` or `partly_reverted` changeset can't be reached through the allowed moves),
skips the move and outcome checks, and sets `imported_at`. It is the only write that skips them.

### Out of scope

Syncing changesets between devices (#53) and deciding on one device for another (#55). The op-log
read API (#48: `wasl oplog list/show --json`, IPC) is read-only; accepting stays in the Wasl window.

### Read format (#48)

`wasl oplog list --json` and `show --json` print one object: `{"version": 1, "changesets": [...]}`.
Each changeset carries the columns above by name (times as RFC 3339 UTC strings, `opened_at` and so
on) and `origin_device_name`: the origin device's name from the device list, or this device's own,
`null` when unknown. `list` adds `op_count`, `show` adds `operations`, each with the operation
columns by name: `origin_device`, `changeset_id`, `seq`, `entity_uri` as `entity`, `op`, `prev` and
`next` as JSON objects, `reverts`, `outcome`, `at`. Adding a field keeps version 1; removing or
renaming one raises it.

`list` filters by `--actor` (exact), `--status`, `--since` (opened at or after: a time, a date at
local midnight, or `30m`, `24h`, `7d` ago; a count below zero is refused) and `--pair` (the scope is
in that pair; a name no pair has now is matched as a pair id, since a deleted pair's changesets stay
until pruned), newest first. `show` names a changeset as `<origin_device>:<id>`, or by `id` alone
for this device; `list` prints a bare `id` only for this device's changesets. The IPC requests
`oplog_list` and `oplog_show` answer the same object. `list` gives the newest 100 unless
`--limit N` asks for another count (the IPC request carries `limit`, capped at 1000), and `show`
gives every operation: a changeset whose answer would pass the 32 MiB IPC line limit is refused
with an error that says so, not paged.

## Consequences

- Other tools and apps read one record of who changed what, in any product, without knowing
  Wasl's file tables. Agent undo writes its operations through the `oplog` module (UND-9); its own
  tables (schema v12) keep the working detail, as `agent_*` do for pair folders.
- Sync's hold, revert and their tests don't change: they keep reading `agent_*` (UND-8). The cost is
  a status kept in two tables, held together by one transaction and #47's test that every status
  path writes both.
- `docs/AGENT_UNDO.md` §6 placed its tables in schema v10 and the op-log in `wasl-crdt`.
  v10 is now Introductions, so its tables are schema v12, and it writes through this op-log;
  `wasl-crdt` comes only later (Where it lives, above).
- Rejected: moving `agent_changesets` into the op-log (sync's hold and revert would be rewritten, the
  FR-AG tests put at risk, and file-only columns such as saved-copy paths pushed into a generic
  table); a SQL view over `agent_*` (no room for other products' entities or for undo's journal, and
  a view can't be append-only); a new crate now (one user, and an interface split before a second
  user is a guess; the Dart app can't link it); the rules in `wasl-engine` (the store must refuse
  a bad move itself, so no caller can write one, and #46 tests it there).
