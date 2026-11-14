# SyncProgress

Live progress for a running sync: counts, bytes, current file, time left and Pause.

**Provide:** `value` (0–1), `done` and `total` file counts, `doneBytes` / `totalBytes` as formatted strings, `direction` (`up` · `down` · `both`), `current` file name, `eta` text; `phase="scanning"` for the indeterminate compare step; `onPause={false}` to hide Pause.

- Always determinate once the plan is known. Indeterminate only while comparing folders, and say so: "Comparing folders… 2,410 files checked".
- The bar tweens over `duration-progress`; never let it run backwards.
- Time left is rounded ("About 6 min left") and hidden when it is under a minute or unknown.

**Compose:** `LinearProgressIndicator(progress = { value }, color = status.syncing.content, trackColor = status.syncing.container)` + `TextButton` with `Icons.Pause`.
