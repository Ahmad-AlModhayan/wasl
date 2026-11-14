# ListItem

The one row used by pickers, the recycle bin and simple lists.

**Provide:** `icon` (+ `iconFill`), or `leading` for a custom slot, `thumb` for a tinted square thumbnail, `headline`, `supporting`, `mono`, `meta` (right-aligned small text), `trailing` (a button), `chevron`, `selected`, `disabled` with `disabledReason`, `tone` (`folder` colours the icon `primary`, `drive` `tertiary`).

- Disabled rows stay visible: the content dims to `disabled-content`, the reason stays fully readable.
- `meta` names relationships (*Camera backup*, *2 paired*, *30 days left*).

**Compose:** `ListItem` with `Modifier.alpha` on the content of disabled rows and a `supportingContent` reason.
