# Status system

Six states describe every pair, file and notification. Each has one icon, one word, one colour family, and one rule for when it may be loud. Use exactly these; don't invent a seventh.

| State | Icon (Material Symbols) | Label | Tokens | Means | What the person can do |
|---|---|---|---|---|---|
| Synced | `cloud_done` (filled) | Synced | `status-synced` | Phone and Drive folders match as of the time shown. | Nothing. |
| Syncing | `sync`, turning | Syncing 42% | `status-syncing` | Work in progress. Always shown with a value, or "Comparing folders…" while scanning. | Pause. |
| Waiting | `wifi` (or `bolt` for charging, `schedule` for a scheduled run, `pause` when paused) | Waiting for Wi‑Fi | `status-waiting` | Held back on purpose by a condition the person set. Nothing is wrong. | Sync now on mobile data, or wait. |
| Needs approval | `front_hand` (filled) | Needs approval | `status-approval` | Drive Sync stopped before a risky change: more deletions than the limit, or a first merge into a non-empty folder. | Review, then approve or restore. |
| Conflict | `call_split` | Conflict / 2 conflicts | `status-conflict` | The same file changed on both sides since the last sync. Both copies are kept. | Keep both, keep phone, keep Drive. |
| Failed | `error` (filled) | Failed | `status-failed` | Couldn't finish: access lost, Drive full, a file Drive rejects, repeated network errors. | The one fix named in the message. |

## Loud and quiet

- **Quiet states** (synced, syncing, waiting) use the *plain* chip: icon and label in `status-x`, no ground. They never badge, never banner, never make a sound.
- **Loud states** (needs approval, conflict, failed) use the *tonal* chip on `status-x-container` with text in `on-status-x-container`, put an action on the pair card, count toward the Problems badge, and post a notification.
- **Icon-only** is allowed in dense rows (the compact home list, file rows) and always carries a content description with the full label.

## Precedence

When a pair or the app has several states at once, show the most urgent:

**Needs approval → Failed → Conflict → Syncing → Waiting → Synced**

Only the top state of the whole app gets a `StatusBanner` on Home. Everything else lives on its card and in Problems.

## Where each state appears

- **Pair card**: the chip under the title, plus `SyncProgress` while syncing and one action button for loud states.
- **Home**: one banner for the most urgent loud state; the nav badge counts approvals, conflicts and failures.
- **Problems**: every loud item, grouped *Needs you* and *Retrying on its own*.
- **Notifications**: syncing is a silent, ongoing notification with Pause and a progress bar. Loud states post a default-priority notification whose actions match the in-app ones. Synced and waiting post nothing.
- **Diagnostics**: health checks reuse the same colours with `check_circle`, `warning`, `error` and `schedule`.

## Colour and contrast

Status colours are fixed custom colours. They don't follow dynamic colour, so *Needs approval* looks the same on every phone and every wallpaper. Containers use 60% of the hue's chroma so loud states stay calm.

| Theme | synced | syncing | waiting | approval | conflict | failed |
|---|---|---|---|---|---|---|
| Light: icon/label | `#2a6b37` | `#185eaf` | `#57605b` | `#7f5600` | `#7f4a8b` | `#ba1a1a` |
| Light: container | `#c8ecc6` | `#d6e3ff` | `#e0e3df` | `#ffdeae` | `#fdd6ff` | `#ffdad6` |
| Dark: icon/label | `#93d698` | `#a9c7ff` | `#bfc9c2` | `#ffba3e` | `#f0b0fa` | `#ffb4ab` |
| Dark: container | `#244227` | `#213c63` | `#383c39` | `#513600` | `#4d3252` | `#701e18` |

- `status-x` holds 4.5:1 on `surface` and every `surface-container-*` role in all four themes; `on-status-x-container` holds 4.5:1 on its container; `status-x` holds 3:1 on its container for icons.
- Synced (green) and failed (red) share a lightness, so they must never be told apart by colour: their icons differ in shape (cloud vs. circle) and the word is always present.

## Words

- Labels are fixed: *Synced*, *Syncing 42%*, *Waiting for Wi‑Fi*, *Needs approval*, *Conflict*, *Failed*. Use the counted forms *2 conflicts*, *3 failed* when grouping.
- Banner and notification titles say what happened to the files: "Waiting for your OK", "Can't reach the Documents folder", "2 conflicts in Work documents".
- Never say "error" in a title. Never blame the person.
