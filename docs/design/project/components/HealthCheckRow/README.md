# HealthCheckRow

One diagnostic check, its finding in plain words, and the fix.

**Provide:** `state` (`ok` · `warn` · `fail` · `wait`), `title`, `detail`, optional `action` label.

- States reuse the status colours with `check_circle`, `warning`, `error`, `schedule`, and an accessible word (*OK*, *Check*, *Problem*, *Waiting*).
- Only failing or warning rows get an action, and only one.

**Compose:** `ListItem(leadingContent = Icon, trailingContent = FilledTonalButton)`.
