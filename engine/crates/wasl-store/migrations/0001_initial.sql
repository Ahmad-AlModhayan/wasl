-- Wasl schema v1 (SRS §6). One schema on every platform.

CREATE TABLE devices (
    device_id  TEXT PRIMARY KEY NOT NULL,
    name       TEXT NOT NULL,
    os         TEXT NOT NULL,
    public_key BLOB NOT NULL,
    added_at   INTEGER NOT NULL,
    last_seen  INTEGER,
    last_path  TEXT
);

CREATE TABLE accounts (
    id                TEXT PRIMARY KEY NOT NULL,
    email             TEXT NOT NULL,
    change_page_token TEXT
);

CREATE TABLE pairs (
    id              TEXT PRIMARY KEY NOT NULL,
    name            TEXT NOT NULL,
    account_id      TEXT REFERENCES accounts(id),
    local_root      TEXT NOT NULL,
    drive_folder_id TEXT,
    route           TEXT NOT NULL CHECK (route IN ('drive', 'drive_and_devices', 'devices')),
    reach           TEXT NOT NULL CHECK (reach IN ('lan', 'lan_and_internet')),
    method          TEXT NOT NULL,
    conflict_policy TEXT NOT NULL,
    conditions_json TEXT NOT NULL DEFAULT '{}',
    size_limit      INTEGER NOT NULL DEFAULT 0,
    docs_format     TEXT NOT NULL DEFAULT 'OFFICE',
    vault_id        TEXT,
    gitignore       INTEGER NOT NULL DEFAULT 0,
    presets_json    TEXT NOT NULL DEFAULT '[]',
    paused          INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE pair_devices (
    pair_id   TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES devices(device_id) ON DELETE CASCADE,
    PRIMARY KEY (pair_id, device_id)
);

-- Baseline: the last synced state of each key, one row per side ('local', 'drive', later device ids).
CREATE TABLE file_state (
    pair_id        TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    key            TEXT NOT NULL,
    side           TEXT NOT NULL,
    is_dir         INTEGER NOT NULL,
    size           INTEGER,
    mtime_ns       INTEGER,
    blake3         TEXT,
    md5            TEXT,
    version_vector TEXT,
    drive_id       TEXT,
    drive_version  INTEGER,
    local_name     TEXT,
    export_mime    TEXT,
    PRIMARY KEY (pair_id, key, side)
) WITHOUT ROWID;

CREATE TABLE chunks (
    blake3   TEXT PRIMARY KEY NOT NULL,
    size     INTEGER NOT NULL,
    refcount INTEGER NOT NULL
);

CREATE TABLE file_chunks (
    pair_id TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    key     TEXT NOT NULL,
    idx     INTEGER NOT NULL,
    offset  INTEGER NOT NULL,
    blake3  TEXT NOT NULL,
    PRIMARY KEY (pair_id, key, idx)
);

CREATE TABLE ignore_hits (
    pair_id     TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    source_file TEXT NOT NULL,
    line        INTEGER NOT NULL,
    bytes       INTEGER NOT NULL,
    files       INTEGER NOT NULL,
    PRIMARY KEY (pair_id, path)
);

CREATE TABLE runs (
    id         TEXT PRIMARY KEY NOT NULL,
    pair_id    TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    trigger    TEXT NOT NULL,
    route_used TEXT,
    status     TEXT NOT NULL,
    stats_json TEXT NOT NULL DEFAULT '{}',
    started_at INTEGER NOT NULL,
    ended_at   INTEGER
);
CREATE INDEX runs_by_pair ON runs (pair_id, started_at);

CREATE TABLE run_events (
    run_id  TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    seq     INTEGER NOT NULL,
    level   TEXT NOT NULL,
    key     TEXT,
    message TEXT NOT NULL,
    PRIMARY KEY (run_id, seq)
);

CREATE TABLE conflicts (
    pair_id       TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    key           TEXT NOT NULL,
    versions_json TEXT NOT NULL,
    status        TEXT NOT NULL,
    PRIMARY KEY (pair_id, key)
);

CREATE TABLE trash (
    id         TEXT PRIMARY KEY NOT NULL,
    pair_id    TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    side       TEXT NOT NULL,
    key        TEXT NOT NULL,
    reason     TEXT NOT NULL,
    location   TEXT NOT NULL,
    version_no INTEGER NOT NULL DEFAULT 0,
    deleted_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX trash_by_expiry ON trash (expires_at);
CREATE INDEX trash_by_pair_key ON trash (pair_id, key);

CREATE TABLE drive_nodes (
    account_id          TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    id                  TEXT NOT NULL,
    parent_id           TEXT,
    name                TEXT NOT NULL,
    mime                TEXT NOT NULL,
    md5                 TEXT,
    version             INTEGER,
    app_properties_json TEXT,
    PRIMARY KEY (account_id, id)
);
CREATE INDEX drive_nodes_by_parent ON drive_nodes (account_id, parent_id);

CREATE TABLE upload_sessions (
    pair_id TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    key     TEXT NOT NULL,
    uri     TEXT NOT NULL,
    offset  INTEGER NOT NULL,
    PRIMARY KEY (pair_id, key)
);

CREATE TABLE pins (
    doc_id    TEXT PRIMARY KEY NOT NULL,
    pinned_at INTEGER NOT NULL
);

CREATE TABLE vaults (
    id              TEXT PRIMARY KEY NOT NULL,
    name            TEXT NOT NULL,
    format_version  INTEGER NOT NULL,
    kdf_params_json TEXT NOT NULL,
    key_check       BLOB NOT NULL
);

CREATE TABLE blocked_plans (
    pair_id      TEXT PRIMARY KEY NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    reason       TEXT NOT NULL,
    summary_json TEXT NOT NULL
);
