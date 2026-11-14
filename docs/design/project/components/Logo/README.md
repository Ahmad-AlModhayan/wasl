# Logo

The Drive Sync mark in its five forms: mark, lockup, adaptive app icon, themed (monochrome) icon and notification glyph.

**Provide:** `variant` (`mark` · `lockup` · `app-icon` · `themed-icon` · `notification`), `size` in px, and for icons a launcher `mask` (`circle` · `squircle` · `rounded` · `full`). `color` overrides the ink only for one-off placements.

- Use `mark` in the top app bar at 28dp, `lockup` where the app isn't already named, `notification` below 20dp.
- The mark is `logo-ink` on surfaces and `brand-mint` on `brand-green`. It never follows dynamic colour; only Android's themed icon tints it.
- Don't redraw, rotate, outline, add effects to, or split the colours of the two arrows. Don't place it on photos.

**Compose:** ship `ic_launcher_*` and `ic_stat_drive_sync` from `assets/Logos/android/`; in-app, `Icon(painterResource(R.drawable.ic_mark), null, tint = LogoInk)`. See *Logo and app icon*.
