# StorageBar

How full one side is, and how much of it this pair uses.

**Provide:** `side` (`phone` · `drive`), `total`, `pair`, `other` in GB, optional `label`; `compact` drops the legend for cards.

- Three segments: `storage-pair`, `storage-other`, `storage-free`, split by 2dp gaps so they don't depend on colour. The header always states free of total in words.
- Below 10% free the bar switches `other` to `storage-low` and the header adds *Low space* with a warning icon.
- Exposes a single accessible label: "Drive: this pair 3.4 GB, other 6.1 GB, free 5.5 GB of 15 GB".

**Compose:** a `Canvas` drawing rounded segments with `CornerRadius`, inside a `Column` with the header and legend `Row`s.
