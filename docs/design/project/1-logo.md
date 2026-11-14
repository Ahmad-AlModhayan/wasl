# Logo and app icon

## The mark

One loop, two halves. The left end is squared like the corner of a phone; the right end is a cloud's outline. Two arrows run around it in opposite directions: the top one leaves the phone for the cloud, the bottom one comes back. Each arrow is a single-barbed harpoon, so the pair reads like ⇌: two-way, in balance. It shares nothing with Google's Drive triangle or the generic circular sync icon.

- Drawn on a 24-unit grid with round caps and joins: stroke 2.5 for the mark, 2.8 for the notification glyph. The geometry is in `assets/Logos/drive-sync-mark.svg`.
- Use the mark alone wherever the app is already named (top app bar, notification, launcher). Use the lockup (`drive-sync-lockup.svg`) where it isn't (about screen, store listing, documents).
- The wordmark is Figtree at weight 640, tracked −1.2%, outlined in the SVG. Don't retype it.

## Colour

- On light surfaces: `logo-ink` (brand-green `#1B5E45`). On dark surfaces: `logo-ink` resolves to a light mint.
- Reversed: `brand-mint` on `brand-green` (6.3:1). This is the app icon.
- The mark never follows dynamic colour, except as the Android themed icon, where the system tints it.
- Never place the mark on a busy image, add a gradient or shadow, outline it, or recolour the two halves differently.

## Adaptive icon (Android 8+)

| Layer | File | Content |
|---|---|---|
| Background | `ic_launcher_background.svg` / `.xml` | Solid `brand-green`, full 108dp |
| Foreground | `ic_launcher_foreground.svg` / `.xml` | Mark in `brand-mint`, scaled ×2.15 and centred: about 45dp wide, inside the 66dp safe circle |
| Monochrome | `ic_launcher_monochrome.svg` / `.xml` | Same geometry, single ink; Android 13+ tints it for themed icons |

- `android/ic_launcher.xml` wires the three layers (`mipmap-anydpi-v26`).
- The foreground stays inside the 66dp safe zone under every mask: circle, squircle, rounded square and square all keep the whole loop visible.
- Play Store: `app-icon-512.svg`, the 72dp visible window rendered at 512px. Export to 32-bit PNG; Play applies its own mask and shadow.

## Notification icon

- `ic_stat_drive_sync.svg` / `.xml`: 24dp, white, alpha only. Android ignores the colour; only the shape shows.
- Heavier stroke (2.8) so the loop and both barbs survive at 18–24dp in the status bar.
- In the notification itself the large icon area shows the same glyph on `primary-container`.

## Minimum sizes and clear space

- Mark: 16dp minimum on screen. Below 20dp use the notification geometry.
- Lockup: 24dp tall minimum.
- Clear space around the mark: one quarter of its height on every side; around the lockup, the height of the wordmark's cap.
