# Callout

Inline guidance placed under the setting that causes it: what will happen, and the undo.

**Provide:** `tone` (`info` · `warning` · `danger` · `success`), optional `title`, body as children, optional `action` (a text `Button` that applies the fix).

- `info` on `secondary-container`; `warning` on `status-approval-container`; `danger` on `status-failed-container`; `success` on `status-synced-container`.
- Put numbers in it (*First sync is about 2.1 GB*). Offer the safe fix as the action.
- Callouts live in forms. Screen-wide states use `StatusBanner`.

**Compose:** `Surface(shape = shapes.medium, color = …)` with `Row { Icon; Column { Text; Text; TextButton } }`.
