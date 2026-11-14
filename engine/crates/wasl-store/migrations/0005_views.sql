-- Wasl schema v5: on-demand views (docs/ON_DEMAND.md §2.3): a Drive folder (plain or a vault)
-- shown read-only at a local path, fetched when opened. Views are not pairs: nothing here is
-- synced or uploaded.
CREATE TABLE views (
    id              TEXT PRIMARY KEY NOT NULL,
    name            TEXT NOT NULL,
    account_id      TEXT NOT NULL,
    drive_folder_id TEXT NOT NULL,
    drive_id        TEXT,
    remote_path     TEXT NOT NULL DEFAULT '',
    local_path      TEXT NOT NULL UNIQUE,
    vault_id        TEXT,
    cache_limit     INTEGER NOT NULL,
    enabled         INTEGER NOT NULL DEFAULT 1,
    created_at      INTEGER NOT NULL,
    last_refresh_at INTEGER,
    last_error      TEXT
);
