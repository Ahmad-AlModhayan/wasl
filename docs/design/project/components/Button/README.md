# Button

Buttons, icon buttons and the FAB.

**Provide:** `variant` (`filled` · `tonal` · `outlined` · `text` · `danger` · `danger-text`), `size` (`sm` 32dp, `md` 40dp, `lg` 56dp), optional `icon`, label as children. `IconButton` takes `icon`, `label` (required, it is the content description), `variant`, `badge`. `Fab` takes `icon` and an optional label to extend it.

- One filled button per screen, for what the screen is for (*Preview first sync*, *Start sync*, *Use this folder*).
- Tonal for secondary positive actions (*Fix*, *Keep both*), outlined for alternatives, text for low-emphasis and inside banners.
- `danger` only for *Delete forever* and *Disconnect*.
- Labels are verb-first and specific; include the count when there is one.
- Focus shows `focus-ring`; pressed and hover use the state-layer opacities.

**Compose:** `Button`, `FilledTonalButton`, `OutlinedButton`, `TextButton`, `IconButton` / `FilledTonalIconButton`, `ExtendedFloatingActionButton`.
