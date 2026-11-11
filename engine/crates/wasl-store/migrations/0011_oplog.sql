-- Wasl schema v11: the op-log (docs/adr/0002-one-op-log.md). One record of every actor's changes,
-- which other products read. `wasl-store`'s `oplog` module is the only writer and checks every move.

-- One actor's set of changes in one scope. Ids are unique only on the device that made them, so a
-- changeset is named by (origin_device, id). Times are Unix ms.
CREATE TABLE oplog_changesets (
    origin_device TEXT NOT NULL,
    id            TEXT NOT NULL,
    actor         TEXT NOT NULL,
    label         TEXT,
    scope         TEXT NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('open', 'staged', 'committed', 'discarded', 'reverted', 'partly_reverted')),
    opened_at     INTEGER NOT NULL,
    closed_at     INTEGER,
    decided_at    INTEGER,
    -- The client that decided ("cli", "desktop", "unknown"), and who, as an actor.
    decided_by    TEXT,
    decided_actor TEXT,
    -- Set only on a changeset written by the one-time v10 copy.
    imported_at   INTEGER,
    PRIMARY KEY (origin_device, id)
);
CREATE INDEX idx_oplog_changesets_decided ON oplog_changesets(status, decided_at);

-- One change to one entity, numbered 1, 2, 3 … per changeset in the order written. A revert appends
-- inverse operations (`reverts` = the seq taken back). The cascade is used only by pruning.
CREATE TABLE oplog_ops (
    origin_device TEXT NOT NULL,
    changeset_id  TEXT NOT NULL,
    seq           INTEGER NOT NULL CHECK (seq >= 1),
    entity_uri    TEXT NOT NULL,
    op            TEXT NOT NULL,
    prev          TEXT,
    next          TEXT,
    reverts       INTEGER,
    outcome       TEXT CHECK (outcome IN ('applied', 'discarded', 'skipped_changed', 'failed', 'reverted')),
    at            INTEGER NOT NULL,
    PRIMARY KEY (origin_device, changeset_id, seq),
    FOREIGN KEY (origin_device, changeset_id) REFERENCES oplog_changesets(origin_device, id) ON DELETE CASCADE
);

-- Append-only: only status, the decision and outcomes move once a row is written.
CREATE TRIGGER oplog_changesets_fixed BEFORE UPDATE OF origin_device, id, actor, label, scope, opened_at, imported_at
ON oplog_changesets
BEGIN
    SELECT RAISE(ABORT, 'op-log changesets are append-only');
END;
CREATE TRIGGER oplog_ops_fixed BEFORE UPDATE OF origin_device, changeset_id, seq, entity_uri, op, prev, next, reverts, at
ON oplog_ops
BEGIN
    SELECT RAISE(ABORT, 'op-log operations are append-only');
END;
