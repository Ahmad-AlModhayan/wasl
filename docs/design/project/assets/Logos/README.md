# Logos

Every file is an SVG upload except `android/`, which holds the same artwork as Android vector drawables ready for `res/`.

- `drive-sync-mark.svg` — the mark on its 24-unit grid, ink `#1B5E45` (brand-green). On dark grounds recolour to `logo-ink` dark (`#94d5b5`).
- `drive-sync-lockup.svg` — mark in `#1B5E45` + outlined Figtree wordmark in `#191c1a` (on-surface, light). For dark grounds use the reversed lockup.
- `drive-sync-lockup-reversed.svg` — mint mark and white wordmark on a brand-green plate.
- `app-icon-512.svg` — Play Store icon: brand-green ground, mint mark, the 72dp visible window at 512px. Export to PNG.
- `ic_launcher_background.svg` — adaptive icon background layer, 108dp, solid brand-green.
- `ic_launcher_foreground.svg` — adaptive icon foreground, 108dp, mark in brand-mint `#bff4d8` inside the 66dp safe zone.
- `ic_launcher_monochrome.svg` — themed-icon layer, 108dp, single ink (Android uses alpha only).
- `ic_stat_drive_sync.svg` — notification small icon, 24dp, heavier 2.8 stroke; single ink, alpha only (Android tints it).
- `android/ic_launcher.xml` — the adaptive-icon definition for `mipmap-anydpi-v26`; `android/ic_launcher_*.xml`, `android/ic_stat_drive_sync.xml` — the vector drawables.
