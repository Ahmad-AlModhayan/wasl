# Snackbar

A short confirmation with Undo after any reversible action.

**Provide:** message text and `action` label (default *Undo*).

- Message names what happened and where: "Restored 12 files to Camera".
- Undo snackbars stay for `duration-snackbar-undo` (10s). One at a time; a new one replaces the old.
- Never use a snackbar for errors that need action; those go to Problems.

**Compose:** `SnackbarHostState.showSnackbar(message, actionLabel = "Undo", duration = SnackbarDuration.Long)`.
