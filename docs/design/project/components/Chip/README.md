# Chip

Filter chips for narrowing lists, with counts.

**Provide:** `selected`, optional `icon`, label, optional `trailing` count.

- Used in the dry run (All / To Drive / To phone / To bin / Conflicts), the recycle bin (All / Phone / Drive) and the log (All / Errors / pair).
- One row, horizontally scrollable; never wrap to a second line.

**Compose:** `FilterChip(selected, onClick, label, leadingIcon = if (selected) Check else null)` in a `LazyRow`.
