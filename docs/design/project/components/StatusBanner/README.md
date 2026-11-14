# StatusBanner

A screen-level message for the single most urgent state, with what happened, what is safe, and what to do.

**Provide:** `status`, `title` (what happened to the files), `children` (one or two sentences of fact), `actions` (one filled `Button` for the fix, optionally a text button for the safe alternative).

- At most one banner per screen, and only for loud states, plus *Everything is in sync* on the quiet Home.
- Put the reassurance in the body: "Drive still has them." Put the count in the button: *Review 41 deletions*.
- The filled button inside a banner inverts to `on-status-*-container` on `status-*-container`.
- Don't stack banners; the rest go to Problems.

**Compose:** `Surface(color = status.container, contentColor = status.onContainer, shape = shapes.medium)` with a `Column`; `Modifier.semantics { liveRegion = LiveRegionMode.Polite }`.
