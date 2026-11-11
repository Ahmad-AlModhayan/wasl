# Icon

Material Symbols Rounded at weight 400, optical size 24: the only icon set in the app.

**Provide:** `name` (a Material Symbols name), optional `fill`, `size` (24, or 18 inside chips and buttons), and `title` when the icon carries meaning on its own.

- Outline by default. Filled for the active nav destination, selected states and the status icons `cloud_done`, `front_hand`, `error`.
- Phone side `mobile`, Drive side `cloud`, folders `folder` (`folder_shared` when shared). Directions `arrow_upward` (to Drive) and `arrow_downward` (to phone).
- Icon colour comes from the text it sits with, or from a role: `on-surface-variant` for neutral, `primary` for the phone side, `tertiary` for the Drive side, `status-*` for states.
- Never Google product logos, never emoji.

**Compose:** import the needed symbols as vector drawables; `Icon(ImageVector.vectorResource(R.drawable.ic_cloud_done), contentDescription)`.
