# SyncModeOption

A radio card for one sync direction, with what it copies and what it never touches.

**Provide:** `mode` (`two` · `up` · `down`), `selected`, `description` (one or two short sentences), `recommended`.

- Always show all three, in this order: Two-way (recommended), Phone → Drive, Drive → Phone.
- Descriptions name the side that is never changed for one-way modes.
- Selected: 2dp `primary` border on `secondary-container`. The diagram repeats the direction with icons.

**Compose:** `Card(Modifier.selectable(selected, role = Role.RadioButton))` with `RadioButton(onClick = null)` inside a `Column(Modifier.selectableGroup())`.
