# PathBreadcrumb

Where you are inside a folder picker; every crumb goes back to that level.

**Provide:** `root` label, `rootIcon` (`mobile` or `cloud`), `parts` (folder names, last is current).

- The current crumb sits on `surface-container-high`. Long paths scroll horizontally, anchored to the end.

**Compose:** `LazyRow` of `TextButton`s separated by chevron icons; `animateScrollToItem(last)`.
