Drive Sync keeps folders on an Android phone and folders in Google Drive the same, in both directions. It is a personal tool built with Jetpack Compose and Material 3. The interface has one job beyond syncing: making it obvious that nothing will be lost. Calm when things are fine, specific when they aren't, and never an ad, a nag or a decorative flourish.

## Principles

1. **Preview before change.** Every new pair, and any sync that would delete more than the safety limit, shows its plan first. The filled button on those screens is *Preview first sync* or *Start sync*, never *Save*.
2. **Nothing is lost for 30 days.** Anything sync removes or overwrites goes to the recycle bin or the file's version history. Every reversible action gets an Undo snackbar.
3. **Quiet when well.** Synced, syncing and waiting are plain text with an icon. Colour fills, banners and badges are reserved for the three states that need a person: needs approval, conflict, failed.
4. **Say what will happen, in numbers.** "41 files were deleted on your phone. Drive still has them." Not "Some changes need review."
5. **Status is never colour alone.** Every status carries an icon and a word. Colour is the third cue.

## Content

- Voice: plain, calm, first-person-free. The app doesn't say "I" or "we"; it describes what happens to the person's files. Address the reader as "you".
- Sentence case everywhere, including buttons and titles. No exclamation marks, no emoji, no "Oops".
- The two sides are always **phone** and **Drive**. Never local/remote, device/cloud, source/target.
- Direction is written with an arrow: *Phone → Drive*, *Drive → Phone*, *Two-way*.
- Buttons are verb-first and name the thing: *Review 41 deletions*, *Restore on phone*, *Keep both*. Avoid *OK*, *Yes*, *Continue* when a precise verb exists.
- Errors say what failed, why in plain words, and the one fix: "Can't reach Documents / Scans. Android removed access after an update." + *Grant access again*. Error codes live only in Diagnostics.
- Reassure with facts, not adjectives: "Drive still has every file", not "Don't worry".
- Numbers use digit grouping (1,204), units with a space (2.4 MB), and tabular figures.
- Vocabulary: pair, sync, recycle bin, version, conflict, needs approval, Wi‑Fi (with a non-breaking hyphen).

## Colour

Colour roles follow Material 3. On Android 12+ the app uses dynamic colour from the wallpaper; elsewhere, or when the person turns *Colours from wallpaper* off, it uses the brand fallback themes **Light** and **Dark** (deep green, seed `#1B5E45`). The two *Dynamic sample* themes show the same roles generated from a sample wallpaper so every screen can be checked against a non-green scheme.

- Screen ground is `surface`. Cards and grouped settings sit on `surface-container-low` with a 1dp `outline-variant` border. Dialogs and sheets use `surface-container-high`. Text fields and the storage-bar track use `surface-container-highest`.
- Text is `on-surface`; secondary text, metadata and inactive icons are `on-surface-variant`. Both hold 4.5:1 on every surface role in every theme.
- `primary` is the main action: the filled button, the active switch, progress and the phone side of a pair. One filled button per screen.
- `tertiary` marks the Drive side of a pair: Drive icons, the Drive path row, Drive folders in the picker. It never means a status.
- `secondary-container` is selection: the nav indicator, selected chips, the selected sync-mode card.
- `error` is only for destructive actions: *Delete forever*, *Disconnect*, *Remove pair*, and *Delete on Drive* on Review deletions. The Failed status has its own tokens.
- Status colours (`status-*`, `status-*-container`, `on-status-*-container`) are fixed and never follow dynamic colour. Use `status-x` for an icon or label on any surface, `status-x-container` as a chip or banner ground, `on-status-x-container` for text on that ground.
- Storage bars use `storage-pair` (this pair), `storage-other` (everything else) and `storage-free` (track), with 2dp gaps between segments and a written legend. Below 10% free, `storage-other` becomes `storage-low` and the header says *Low space*.
- The logo uses `logo-ink` on plain surfaces and `brand-mint` on `brand-green`. These never change with dynamic colour.
- Keyboard and D-pad focus is `focus-ring`: a 2dp surface gap, then a solid 3dp `secondary` ring.

## Type

Figtree for everything people read; JetBrains Mono for exact paths, patterns and the log. Both are variable fonts (SIL Open Font License) shipped in `fonts/` and bundled in the APK. The scale is Material 3's, with Figtree weights tuned for calm: titles and labels 600, body 400.

- Screen titles: `title-large` in the small top app bar, `headline-small` in the large one. Onboarding titles: `headline-medium`.
- Pair names on cards and section titles: `title-medium`. Sync-mode names: `title-small`.
- List headlines and setting titles: `body-large`. Supporting text: `body-medium`. Timestamps, helper text and storage captions: `body-small`.
- Buttons, chips and tabs: `label-large`. Status chips and nav labels: `label-medium`. Path-row overlines (PHONE, DRIVE): `label-small` in capitals.
- The dry-run headline number: `display-small`.
- Paths shown in full, exclude patterns: `mono-medium`. Log lines: `mono-small`.
- Every number that can change uses tabular figures (`font-feature-settings: "tnum"`).

## Layout and spacing

- 4dp grid. Spacing tokens are named by their dp value: `space-16` is 16dp.
- Screen side margin `space-16`. Cards: `space-16` padding, `space-12` between cards. Sections: `space-24` apart, with a `title-small` header in `primary`.
- Lists are full-bleed rows with 16dp insets; one-line rows are 56dp, two-line rows 72dp.
- Every tappable thing has a 48dp target (`size-touch-target`), even when it draws smaller.
- Bottom action bars sit on `surface-container`, hold at most two buttons, and may carry one line of `body-small` explaining what the primary action does.
- Designed at 360dp wide; content reflows, nothing scales.

## Shape

Material 3 shape scale, one step softer for cards. `shape-lg` (16dp) for folder-pair cards, sync-mode cards and the FAB; `shape-md` (12dp) for banners and callouts; `shape-sm` (8dp) for chips and path rows; `shape-xl` (28dp) for dialogs and sheet tops; `shape-full` for buttons, switches, progress and storage bars.

## Elevation

Mostly tonal. Cards sit at `elevation-0` on `surface-container-low`; the nav bar and menus at `elevation-2`; the FAB, dialogs and sheets at `elevation-3` with `shadow-3`. Never add shadow to a card to make it stand out; use the surface roles.

## Motion

Calm and short. Use `easing-standard` with `duration-short` for state changes, `duration-medium` for expanding a card or swapping a status, `duration-long` with the emphasized curves for screens and sheets. Progress values tween over `duration-progress`. The syncing icon turns once per `duration-sync-turn` (2s, linear). Nothing bounces. When Android's *Remove animations* is on, all motion stops and the syncing icon stands still; progress and words still carry the state.

## Iconography

Material Symbols Rounded, weight 400, grade 0, optical size 24. Outline by default; filled for the active nav destination, selected states and the status icons `cloud_done`, `front_hand`, `error`. Icons sit at `size-icon` (24dp), or `size-icon-small` (18dp) inside chips and buttons. The phone side is `mobile`, the Drive side `cloud`. Never use Google's Drive logo or any Google product mark for the Drive side. No emoji.

## Components

The live previews are React stand-ins for the Compose components, built from the same tokens (`window.DriveSync`, React 18). Each component's guidelines name its Compose equivalent. Start with **StatusChip**, **FolderPairCard** and **SyncModeOption**; the screens under *Screens* show them in context.
