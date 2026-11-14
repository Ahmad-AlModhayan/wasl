# SettingRow

A settings row with a switch, a value, a link, or a destructive action.

**Provide:** `icon`, `title`, `summary`, `control` (`switch` · `value` · `chevron` · `null`), `on` for switches, `value` text, `danger`, `disabled`.

- The summary states the current effect, especially when a safety default is off (*Off: mobile data allowed*).
- `danger` only for *Disconnect Google account*.
- The whole row toggles the switch.

**Compose:** `ListItem(… , trailingContent = { Switch(checked, onCheckedChange = null) }, modifier = Modifier.toggleable(…))`.
