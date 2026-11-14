# ChangeSummary

The dry-run headline: what stays untouched, then what would move.

**Provide:** `unchanged` count and `items`: `[{ action, count, size?, label? }]` for upload, download, bin and conflict.

- Always lead with the unchanged number; it's the reassurance.
- Zero tiles stay, dimmed: "0 to recycle bin" is information.

**Compose:** `Text(style = displaySmall)` + a 2-column `LazyVerticalGrid` of outlined tiles.
