# LogView

The sync log: timestamped, monospace, levels as words.

**Provide:** `lines`: `[time, level, message]` with levels `OK` · `INFO` · `WARN` · `ERROR`.

- Set in `mono-small` on `surface-container-highest`. Levels are coloured and written.
- Messages truncate with an ellipsis; tap a line to see it in full.
- Exports hide file names by default.

**Compose:** `LazyColumn` of `Row`s with `MonoSmall`; `SelectionContainer` for copying.
