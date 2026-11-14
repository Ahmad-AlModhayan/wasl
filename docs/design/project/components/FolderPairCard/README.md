# FolderPairCard

The Home unit: one pair's name, direction, state, both folders and the storage it affects.

**Provide:** `name`, `mode` (`two` · `up` · `down`), `status` (+ `progress`, `count`), `phonePath`, `drivePath`, `conditions` text (*Wi‑Fi only*, *Every 6 hours*), `lastSync` text, optional `sync` (props for `SyncProgress`), `storage` (array of `StorageBar` props, `compact` is applied) and `children` for a loud state's action row (`.ds-paircard-actions`).

- Quiet states show the plain chip; loud states the tonal chip plus exactly one action button on the card.
- Show the storage bar for the side the pair fills: Drive for Phone → Drive, phone for Drive → Phone, both for Two-way.
- Paths are human (*DCIM / Camera*), not absolute; the editor shows full paths.
- Tap opens the pair; the overflow menu holds Sync now, Pause, Preview, Edit, Remove pair.

**Compose:** `OutlinedCard(colors = CardDefaults.outlinedCardColors(containerColor = surfaceContainerLow), shape = shapes.large)`.
