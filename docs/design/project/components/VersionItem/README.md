# VersionItem

One entry in a file's version timeline.

**Provide:** `when`, `source` (`phone` · `drive` · `conflict` · `restored`), `size`, `note`, `current`, `first`, `last` (rail ends).

- Newest first. The current version has a filled `primary` dot and the *Current* tag and no actions.
- Every other version offers *Restore* and *Save as copy*. Restore never deletes: the replaced version becomes a new entry.

**Compose:** custom `Row` with a `Canvas` rail; `TextButton`s with icons.
