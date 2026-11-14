# Dialog

A modal confirmation, used only for irreversible actions and large batches.

**Provide:** `icon`, `title` with the number in it, body (`children`), `confirm` and `cancel` labels, `danger` for destructive confirms.

- In daily use only *Delete forever* and *Disconnect Google account* reach a dialog. Everything recoverable uses Undo instead.
- The confirm button repeats the verb from the title (*Delete forever*), never *OK*.
- Body says where the files are removed from and that it can't be undone.

**Compose:** `AlertDialog(icon, title, text, confirmButton = { Button(colors = error) }, dismissButton = { TextButton })`.
