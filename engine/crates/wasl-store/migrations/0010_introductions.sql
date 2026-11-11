-- Wasl schema v10: introductions (docs/INTRODUCTIONS.md, 2026-10). Who may sync each shared pair,
-- what this device knows of each member, and the folders other devices offer this one.

-- A share made before v10 (or without a device list) is served to every member known here.
ALTER TABLE pair_shares ADD COLUMN all_devices INTEGER NOT NULL DEFAULT 1;

-- The devices a pair is shared with by name: the pair is offered to each, and served to each
-- whether or not it is known here (§3.2–3.3). Device IDs as `WASL-XXXX-…`.
CREATE TABLE pair_share_devices (
    pair_id     TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,
    device_id   TEXT NOT NULL,
    PRIMARY KEY (pair_id, device_id)
);

-- This device's note on each group member (§3.1): 1 when known here, 0 while new. The store does
-- not know the group: wasl-devices fills it when it loads the group (every member present then is
-- known, so an upgrade changes nothing) and notes each member that arrives later.
CREATE TABLE device_notes (
    device_id   TEXT PRIMARY KEY NOT NULL,
    known       INTEGER NOT NULL
);

-- Folders other devices offer this one (§3.3), one row per (share, sender): the sender is the
-- device the transport authenticated, so a member offering another device's share ID cannot take
-- that device's offer over. `share` is the share ID (hex), `first_seen` Unix ms by this device's
-- clock.
CREATE TABLE received_offers (
    share       TEXT NOT NULL,
    from_device TEXT NOT NULL,
    name        TEXT NOT NULL,
    files       INTEGER NOT NULL,
    bytes       INTEGER NOT NULL,
    first_seen  INTEGER NOT NULL,
    state       TEXT NOT NULL CHECK (state IN ('new', 'declined', 'accepted')),
    PRIMARY KEY (share, from_device)
);
CREATE INDEX received_offers_by_device ON received_offers (from_device);
