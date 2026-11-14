-- Wasl schema v2: what the engine needs to run pairs, keep history and cache Drive, with every
-- field Drive Sync 1.1.4 stores so its data imports without loss (SRS FR-K2).

-- Pair options and status (1.1.4 FolderPairEntity).
ALTER TABLE pairs ADD COLUMN remote_path TEXT NOT NULL DEFAULT '';
ALTER TABLE pairs ADD COLUMN drive_id TEXT;
ALTER TABLE pairs ADD COLUMN storage_mode TEXT NOT NULL DEFAULT 'files' CHECK (storage_mode IN ('files', 'saf'));
ALTER TABLE pairs ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
ALTER TABLE pairs ADD COLUMN include_patterns TEXT NOT NULL DEFAULT '';
ALTER TABLE pairs ADD COLUMN exclude_patterns TEXT NOT NULL DEFAULT '';
ALTER TABLE pairs ADD COLUMN excluded_subfolders TEXT NOT NULL DEFAULT '';
ALTER TABLE pairs ADD COLUMN sync_hidden INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pairs ADD COLUMN propagate_deletions INTEGER NOT NULL DEFAULT 1;
ALTER TABLE pairs ADD COLUMN mass_delete_guard INTEGER NOT NULL DEFAULT 1;
ALTER TABLE pairs ADD COLUMN docs_export INTEGER NOT NULL DEFAULT 1;
ALTER TABLE pairs ADD COLUMN preserve_mtime INTEGER NOT NULL DEFAULT 1;
ALTER TABLE pairs ADD COLUMN interval_minutes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pairs ADD COLUMN instant_sync INTEGER NOT NULL DEFAULT 1;
ALTER TABLE pairs ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pairs ADD COLUMN updated_at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pairs ADD COLUMN last_run_at INTEGER;
ALTER TABLE pairs ADD COLUMN last_success_at INTEGER;
ALTER TABLE pairs ADD COLUMN last_status TEXT;
ALTER TABLE pairs ADD COLUMN last_error TEXT;
ALTER TABLE pairs ADD COLUMN last_error_kind TEXT;
ALTER TABLE pairs ADD COLUMN consecutive_failures INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pairs ADD COLUMN pending_approval INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pairs ADD COLUMN approve_once INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pairs ADD COLUMN hold_for_preview INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pairs ADD COLUMN pause_reason TEXT;

-- Drive cache bookkeeping per account.
ALTER TABLE accounts ADD COLUMN root_folder_id TEXT;
ALTER TABLE accounts ADD COLUMN last_crawl_at INTEGER;
ALTER TABLE accounts ADD COLUMN last_delta_at INTEGER;
ALTER TABLE accounts ADD COLUMN auth_error TEXT;

-- Run history (1.1.4 SyncRunEntity / SyncEventEntity).
ALTER TABLE runs ADD COLUMN message TEXT;
ALTER TABLE run_events ADD COLUMN at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE run_events ADD COLUMN action TEXT NOT NULL DEFAULT 'INFO';
ALTER TABLE run_events ADD COLUMN detail TEXT;
ALTER TABLE run_events ADD COLUMN bytes INTEGER NOT NULL DEFAULT 0;
CREATE INDEX run_events_by_key ON run_events (key);

-- Recycle bin (1.1.4 TrashEntity): both sides, readable after a pair is removed.
ALTER TABLE trash ADD COLUMN size INTEGER NOT NULL DEFAULT 0;
ALTER TABLE trash ADD COLUMN account_id TEXT;
ALTER TABLE trash ADD COLUMN remote_file_id TEXT;
ALTER TABLE trash ADD COLUMN display_name TEXT;
ALTER TABLE trash ADD COLUMN pair_name TEXT;

ALTER TABLE blocked_plans ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;

-- v1 shipped these empty and keyed differently; nothing has written to them yet.
DROP TABLE conflicts;
CREATE TABLE conflicts (
    id              TEXT PRIMARY KEY NOT NULL,
    pair_id         TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    key             TEXT NOT NULL,
    detected_at     INTEGER NOT NULL,
    policy          TEXT NOT NULL,
    local_copy_rel  TEXT,
    remote_file_id  TEXT,
    message         TEXT NOT NULL,
    resolved        INTEGER NOT NULL DEFAULT 0,
    resolved_at     INTEGER
);
CREATE INDEX conflicts_by_pair ON conflicts (pair_id, resolved);

DROP TABLE upload_sessions;
CREATE TABLE upload_sessions (
    session_key TEXT PRIMARY KEY NOT NULL,
    uri         TEXT NOT NULL,
    updated_at  INTEGER NOT NULL
);

DROP TABLE drive_nodes;
CREATE TABLE drive_nodes (
    account_id           TEXT NOT NULL,
    id                   TEXT NOT NULL,
    name                 TEXT NOT NULL,
    mime_type            TEXT NOT NULL,
    parent_id            TEXT,
    drive_id             TEXT,
    size                 INTEGER,
    md5                  TEXT,
    modified_time        INTEGER NOT NULL,
    version              INTEGER,
    trashed              INTEGER NOT NULL,
    is_folder            INTEGER NOT NULL,
    is_shortcut          INTEGER NOT NULL,
    shortcut_target_id   TEXT,
    shortcut_target_mime TEXT,
    shared               INTEGER NOT NULL,
    permission_count     INTEGER,
    app_properties_json  TEXT,
    PRIMARY KEY (account_id, id)
);
CREATE INDEX drive_nodes_by_parent ON drive_nodes (account_id, parent_id);

-- Engine-wide settings (device name, guard thresholds, concurrency…).
CREATE TABLE settings (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
