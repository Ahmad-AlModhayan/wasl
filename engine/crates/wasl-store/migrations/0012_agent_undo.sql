-- Wasl schema v12: agent undo (docs/AGENT_UNDO.md §6). Folders an agent may change outside any
-- pair, what each session found in them at its start, and what it changed there. The op-log (v11)
-- keeps the record; these tables keep the working detail (ADR-0002).

-- The watch list (UND-1). `root` is the folder's full path. `ignores` holds the default ignores
-- still applied, one pattern per line: removing one from the list stops it applying. `.waslignore`
-- files in the folder apply as in a pair. Unwatching sets `removed_at`: sessions that worked
-- there keep their watch, and watching the folder again adds a new row with a new id. Ids come
-- from the same id space as pair ids (ADR-0002, `file://<pair id or watch id>/<key>`).
CREATE TABLE agent_watches (
    id          TEXT PRIMARY KEY NOT NULL,
    root        TEXT NOT NULL,
    ignores     TEXT NOT NULL DEFAULT '',
    added_at    INTEGER NOT NULL,
    removed_at  INTEGER
);
CREATE UNIQUE INDEX idx_agent_watches_root ON agent_watches(root) WHERE removed_at IS NULL;

-- A session works in a pair or in a watched folder: exactly one of `pair_id` and `watch_id`. SQLite
-- can't drop NOT NULL in place, so the table is rebuilt (the store turns foreign keys off for this
-- migration and checks them before it commits).
CREATE TABLE agent_changesets_v12 (
    id          TEXT PRIMARY KEY NOT NULL,
    pair_id     TEXT REFERENCES pairs(id) ON DELETE CASCADE,
    watch_id    TEXT REFERENCES agent_watches(id),
    actor       TEXT NOT NULL,
    label       TEXT,
    scope       TEXT NOT NULL,
    session     TEXT,
    status      TEXT NOT NULL CHECK (status IN ('open', 'held', 'accepted', 'reverted', 'partly_reverted')),
    started_at  INTEGER NOT NULL,
    ended_at    INTEGER,
    decided_at  INTEGER,
    decided_by  TEXT,
    CHECK ((pair_id IS NULL) <> (watch_id IS NULL))
);
INSERT INTO agent_changesets_v12
    (id, pair_id, watch_id, actor, label, scope, session, status, started_at, ended_at, decided_at, decided_by)
    SELECT id, pair_id, NULL, actor, label, scope, session, status, started_at, ended_at, decided_at, decided_by
    FROM agent_changesets;
DROP TABLE agent_changesets;
ALTER TABLE agent_changesets_v12 RENAME TO agent_changesets;
CREATE INDEX idx_agent_changesets_pair ON agent_changesets(pair_id, status);
CREATE INDEX idx_agent_changesets_watch ON agent_changesets(watch_id, status);

-- What a session found in each watched folder at its start (UND-2). A session covers its own folder
-- and every watched folder, so a pair session has rows here too. `origin` says where the bytes are:
-- `cloned` (a copy-on-write clone at `location`; `blake3` is filled once it is hashed), `stored`
-- (the undo store's object at `location`, named by `blake3`) or `synced` (the synced version of a
-- pair file, `blake3` from the device index; no `location`). Times are Unix ms. `ctime` (status
-- change time, ns; creation time on Windows) and `file_id` (inode; NULL on Windows for now) tell a
-- replaced file from an unchanged one when size and mtime match: the next snapshot reuses a row only
-- when all four do.
CREATE TABLE agent_snapshots (
    changeset_id TEXT NOT NULL REFERENCES agent_changesets(id) ON DELETE CASCADE,
    watch_id     TEXT NOT NULL REFERENCES agent_watches(id),
    rel          TEXT NOT NULL,
    size         INTEGER NOT NULL,
    mtime        INTEGER NOT NULL,
    ctime        INTEGER,
    file_id      INTEGER,
    blake3       TEXT,
    origin       TEXT NOT NULL CHECK (origin IN ('cloned', 'stored', 'synced')),
    location     TEXT,
    PRIMARY KEY (changeset_id, watch_id, rel)
);
CREATE INDEX idx_agent_snapshots_watch ON agent_snapshots(watch_id);

-- The watched folders a session snapshotted at its start, an empty one too; its journal covers
-- these. A folder that failed is in agent_snapshot_failures instead.
CREATE TABLE agent_session_watches (
    changeset_id TEXT NOT NULL REFERENCES agent_changesets(id) ON DELETE CASCADE,
    watch_id     TEXT NOT NULL REFERENCES agent_watches(id),
    PRIMARY KEY (changeset_id, watch_id)
);
CREATE INDEX idx_agent_session_watches_watch ON agent_session_watches(watch_id);

-- A watched folder a session couldn't snapshot at its start, and why; the session report lists it.
CREATE TABLE agent_snapshot_failures (
    changeset_id TEXT NOT NULL REFERENCES agent_changesets(id) ON DELETE CASCADE,
    watch_id     TEXT NOT NULL REFERENCES agent_watches(id),
    reason       TEXT NOT NULL,
    at           INTEGER NOT NULL,
    PRIMARY KEY (changeset_id, watch_id)
);

-- What a session missed in a watched folder while Wasl couldn't watch or rescan it (UND-4): `rel`
-- '' for the whole folder, else a file it last knew there. The session report lists each.
CREATE TABLE agent_watch_gaps (
    changeset_id TEXT NOT NULL REFERENCES agent_changesets(id) ON DELETE CASCADE,
    watch_id     TEXT NOT NULL REFERENCES agent_watches(id),
    rel          TEXT NOT NULL,
    reason       TEXT NOT NULL,
    at           INTEGER NOT NULL,
    PRIMARY KEY (changeset_id, watch_id, rel)
);

-- Each journaled file's size, mtime, ctime and file id when last captured, so a rescan skips the
-- files it would only read again (the snapshot's reuse key).
CREATE TABLE agent_journal_stats (
    changeset_id TEXT NOT NULL REFERENCES agent_changesets(id) ON DELETE CASCADE,
    watch_id     TEXT NOT NULL,
    rel          TEXT NOT NULL,
    size         INTEGER NOT NULL,
    mtime        INTEGER NOT NULL,
    ctime        INTEGER,
    file_id      INTEGER,
    PRIMARY KEY (changeset_id, watch_id, rel)
);

-- Every change a session made (UND-3, UND-4), numbered 1, 2, 3 … per changeset. `root` is the pair
-- id or watch id `rel` is relative to. A rename is a `delete` plus a `create` (`old_rel` names the
-- path it came from). `captured` 0 is a version Wasl could not keep; `reason` says why. Each row is
-- written in the same transaction as its op-log operation (UND-9).
CREATE TABLE agent_journal (
    changeset_id TEXT NOT NULL REFERENCES agent_changesets(id) ON DELETE CASCADE,
    seq          INTEGER NOT NULL CHECK (seq >= 1),
    root         TEXT NOT NULL,
    op           TEXT NOT NULL CHECK (op IN ('create', 'update', 'delete')),
    rel          TEXT NOT NULL,
    old_rel      TEXT,
    prev_blake3  TEXT,
    next_blake3  TEXT,
    origin       TEXT CHECK (origin IN ('cloned', 'stored', 'synced')),
    location     TEXT,
    captured     INTEGER NOT NULL,
    reason       TEXT,
    at           INTEGER NOT NULL,
    PRIMARY KEY (changeset_id, seq)
);

-- Points in a session to restore to (UND-5): its begin, each tool call, `wasl agent mark` and each
-- restore. `journal_seq` is the last journal entry before the marker (0: none yet).
CREATE TABLE agent_markers (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    changeset_id TEXT NOT NULL REFERENCES agent_changesets(id) ON DELETE CASCADE,
    journal_seq  INTEGER NOT NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('begin', 'tool', 'mark', 'restore')),
    label        TEXT,
    at           INTEGER NOT NULL
);
CREATE INDEX idx_agent_markers_changeset ON agent_markers(changeset_id, journal_seq);

-- Restores (UND-6, UND-7, UND-13). `target` is a marker id, `begin` or a time; `marker_id` is the
-- `restore` marker the restore added. `decided_by` is who approved it (`cli`, `desktop`, `raqeeb`),
-- `inbox_request_id` Raqeeb's request when one was sent.
CREATE TABLE agent_restores (
    id               TEXT PRIMARY KEY NOT NULL,
    changeset_id     TEXT NOT NULL REFERENCES agent_changesets(id) ON DELETE CASCADE,
    target           TEXT NOT NULL,
    marker_id        INTEGER REFERENCES agent_markers(id),
    status           TEXT NOT NULL CHECK (status IN ('planned', 'applied', 'undone', 'denied', 'expired', 'failed')),
    files            INTEGER NOT NULL DEFAULT 0,
    created_at       INTEGER NOT NULL,
    applied_at       INTEGER,
    undone_at        INTEGER,
    decided_by       TEXT,
    inbox_request_id TEXT
);
CREATE INDEX idx_agent_restores_changeset ON agent_restores(changeset_id);
