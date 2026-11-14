# PathRow

One side of a pair: which side, which folder, and how much is in it.

**Provide:** `side` (`phone` · `drive`), `path` (human form, *Documents / Work*; empty shows *Choose a folder*), `meta` (files · size), `onPick` to make it a picker entry, `error` text, `dense` for cards.

- Phone rows use `primary-container`, Drive rows `tertiary-container` behind the side icon, so the two sides are told apart by icon, overline and colour.
- Errors sit inside the row in `error`, with the one-line cause.

**Compose:** `ListItem(overlineContent, headlineContent, supportingContent, leadingContent, trailingContent)` on `surfaceContainerLowest`.
