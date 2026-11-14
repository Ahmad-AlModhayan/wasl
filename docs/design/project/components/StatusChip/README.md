# StatusChip

The single visual for a sync state: icon plus word, in one of the six status colours.

**Provide:** `status` (`synced` · `syncing` · `waiting` · `approval` · `conflict` · `failed`), optional `progress` (0–1, adds the percentage to *Syncing*), `count` (pluralises conflicts and failures), `label` to override the word, `variant` and `size` (`md` 28dp, `sm` 24dp).

- `tonal` (container ground) for loud states: approval, conflict, failed. `plain` (no ground) for quiet ones: synced, syncing, waiting. `icon` only in dense rows, always with its label as the content description.
- Labels are fixed; see *Status system*. Don't shorten them to fit, widen the row instead.
- The syncing icon turns slowly (`duration-sync-turn`) and stops when animations are off.
- Never show a status as a coloured dot alone.

**Compose:** a `Row(Modifier.semantics(mergeDescendants = true) {})` with `Icon` and `Text(style = labelMedium)` inside `Surface(color = status.container, shape = shapes.small)`.
