# TopAppBar

The screen header: navigation, title, optional subtitle and up to two actions.

**Provide:** `title`, `subtitle`, `nav` (`back` · `close` · `null`), `logo` (Home only), `actions` (IconButtons or one text Button), `large`, `scrolled`.

- Home shows the mark and *Drive Sync*. Pushed screens show back; editors and pickers show close and a text *Save* when relevant.
- Subtitle names the pair the screen is about.
- `scrolled` switches the ground to `surface-container`.

**Compose:** `TopAppBar` / `LargeTopAppBar` with `TopAppBarDefaults.pinnedScrollBehavior()`.
