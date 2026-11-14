-- Wasl schema v8: agent changes (docs/AGENT_CHANGES.md). A session of an agent in a scope of a pair
-- becomes a changeset; its files stay out of sync until it is accepted or reverted.
CREATE TABLE agent_changesets (
    id          TEXT PRIMARY KEY NOT NULL,
    pair_id     TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    actor       TEXT NOT NULL,
    label       TEXT,
    scope       TEXT NOT NULL,
    session     TEXT,
    status      TEXT NOT NULL CHECK (status IN ('open', 'held', 'accepted', 'reverted', 'partly_reverted')),
    started_at  INTEGER NOT NULL,
    ended_at    INTEGER,
    decided_at  INTEGER
);
CREATE INDEX idx_agent_changesets_pair ON agent_changesets(pair_id, status);

-- The scope's files when the session began.
CREATE TABLE agent_manifest (
    changeset_id TEXT NOT NULL REFERENCES agent_changesets(id) ON DELETE CASCADE,
    rel          TEXT NOT NULL,
    is_dir       INTEGER NOT NULL,
    size         INTEGER NOT NULL,
    mtime        INTEGER NOT NULL,
    -- Synced at begin (Drive baseline or device index): if it is gone at the end and no longer
    -- synced, Wasl removed it for another device, not the agent.
    synced       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (changeset_id, rel)
);

-- Earlier versions copied at begin (not yet synced) or just before an agent wrote the file.
CREATE TABLE agent_saved (
    changeset_id TEXT NOT NULL REFERENCES agent_changesets(id) ON DELETE CASCADE,
    rel          TEXT NOT NULL,
    location     TEXT NOT NULL,
    size         INTEGER NOT NULL,
    mtime        INTEGER NOT NULL,
    PRIMARY KEY (changeset_id, rel)
);

-- What the session changed, decided at its end.
CREATE TABLE agent_entries (
    changeset_id TEXT NOT NULL REFERENCES agent_changesets(id) ON DELETE CASCADE,
    rel          TEXT NOT NULL,
    key          TEXT NOT NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('added', 'changed', 'deleted')),
    is_dir       INTEGER NOT NULL,
    before_size  INTEGER,
    before_mtime INTEGER,
    after_size   INTEGER,
    after_mtime  INTEGER,
    source       TEXT NOT NULL CHECK (source IN ('saved', 'synced', 'none')),
    location     TEXT,
    outcome      TEXT,
    PRIMARY KEY (changeset_id, rel)
);
