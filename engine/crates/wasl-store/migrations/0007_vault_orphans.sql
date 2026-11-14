-- Wasl schema v7: when each vault v2 chunk object stopped being referenced by a live manifest
-- (docs/ENCRYPTION.md §11). The chunk sweep deletes a chunk only after it has been unreferenced
-- for the whole grace period, so a file deleted today stays restorable from the bin for that long
-- however old its chunks are.
CREATE TABLE vault_orphans (
    pair_id     TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    chunk       TEXT NOT NULL,
    since_ms    INTEGER NOT NULL,
    PRIMARY KEY (pair_id, chunk)
);
