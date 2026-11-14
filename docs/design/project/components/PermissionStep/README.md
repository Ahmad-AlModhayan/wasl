# PermissionStep

One permission in onboarding: what it is, why in one line, and its state.

**Provide:** `icon`, `title`, `why`, `state` (`needed` · `next` · `granted`), `optional`, `cta` label.

- `next` is the step to do now: 2dp `primary` border and a filled button. Others stay outlined until their turn.
- `granted` shows *Allowed* with the synced colour; the row stays so people see the full list.
- Optional permissions are labelled and never block Continue.

**Compose:** `OutlinedCard` with `Row { Icon; Column; Button }`; re-check grants in `LifecycleResumeEffect`.
