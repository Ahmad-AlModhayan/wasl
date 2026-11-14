-- Wasl schema v3: device sync (docs/DEVICE_SYNC.md §4–5).

-- The random ID a pair is known by between devices; the same on every device that syncs it.
CREATE TABLE pair_shares (
    pair_id     TEXT PRIMARY KEY NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    share_id    TEXT NOT NULL UNIQUE
);

-- This device's index of a pair: one version per key, tombstones included.
CREATE TABLE device_index_meta (
    pair_id     TEXT PRIMARY KEY NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    me          INTEGER NOT NULL,
    seq         INTEGER NOT NULL
);
CREATE TABLE device_index (
    pair_id     TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    key         TEXT NOT NULL,
    is_dir      INTEGER NOT NULL,
    deleted     INTEGER NOT NULL,
    hash        TEXT,
    size        INTEGER NOT NULL,
    mtime       INTEGER NOT NULL,
    vv          TEXT NOT NULL,
    author      INTEGER NOT NULL,
    seq         INTEGER NOT NULL,
    local_size  INTEGER NOT NULL,
    local_mtime INTEGER NOT NULL,
    PRIMARY KEY (pair_id, key)
);
CREATE INDEX device_index_by_seq ON device_index (pair_id, seq);

-- How far this device has read each peer's index of a pair.
CREATE TABLE peer_progress (
    pair_id     TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    peer        TEXT NOT NULL,
    since       INTEGER NOT NULL,
    PRIMARY KEY (pair_id, peer)
);

-- Chunk lists of local files (wire form) and where each chunk sits, for answering peers and
-- for building received files from chunks already here. v1 shipped two placeholder chunk tables
-- that nothing ever wrote; these replace them.
DROP TABLE file_chunks;
DROP TABLE chunks;
CREATE TABLE chunk_lists (
    pair_id     TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    key         TEXT NOT NULL,
    blake3      BLOB NOT NULL,
    list        BLOB NOT NULL,
    PRIMARY KEY (pair_id, key)
);
CREATE TABLE chunks (
    pair_id     TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    hash        BLOB NOT NULL,
    key         TEXT NOT NULL,
    offset      INTEGER NOT NULL,
    len         INTEGER NOT NULL
);
CREATE INDEX chunks_by_hash ON chunks (pair_id, hash);
CREATE INDEX chunks_by_key ON chunks (pair_id, key);
