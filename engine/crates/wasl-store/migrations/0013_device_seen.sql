-- When this device last had a connection to each member of its group, Unix milliseconds by this
-- device's clock (issue #32). The status "last seen 5 minutes ago" reads it; a member never reached
-- from here has no row. The store does not know the group: wasl-devices writes the rows.
CREATE TABLE device_seen (
    device_id    TEXT PRIMARY KEY NOT NULL,
    last_seen_at INTEGER NOT NULL
);
