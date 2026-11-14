-- Wasl schema v4: vault v2 rollback detection (docs/ENCRYPTION.md §11.6). The highest manifest
-- version this device has seen of each vault v2 file; an older manifest is refused.
CREATE TABLE vault_versions (
    pair_id     TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    file_id     TEXT NOT NULL,
    version     INTEGER NOT NULL,
    PRIMARY KEY (pair_id, file_id)
);
