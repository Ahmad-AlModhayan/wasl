# Android UX audit, 2026-10-29

This audit walks every Android screen reachable on the test phone and looks for what a person would find unclear or invisible:
- controls that are hard to find;
- states with no explanation;
- text that is low-contrast or cut off;
- missing feedback;
- jargon;
- dead ends;
- anything that needs a trick.

**Setup:**
- **Phone:** Galaxy A54 (SM-A546E), Android 15, font scale 1.08, dark theme.
- **Wasl:** 0.3.1, freshly installed, with no account.
- **Method:** driven over adb, reading the UI tree and taking screenshots.

**Scope:**
- **Covered:** first run, Pairs, Problems, Bin, Settings and every Settings sub-screen, Devices and pairing, Accounts, Vaults, Backup, Diagnostics and Automation.
- **Not covered yet:** the screens that need a signed-in Google account (pair editor, Drive picker, preview, sync in progress, conflicts, history, revisions). They wait for the owner to sign in to the test account on the phone; a Google sign-in is the owner's to do.
- **Black screenshots:** pairing screens block screenshots by design (S1). Their findings come from the UI tree.

**Severity:**
- **High:** a person can't read or do something.
- **Medium:** it misleads, or needs guessing.
- **Low:** polish.

Fixes are on branch `ux/android-audit`. "Fixed" means the fix was checked on the phone with a release build (same key as 0.3.1).

## Findings

| # | Sev | Where | What a person meets | Status |
|---|---|---|---|---|
| F1 | High | First run › What Wasl needs ([before](ux-audit-2026-10/02-what-wasl-needs.png), [after](ux-audit-2026-10/02-what-wasl-needs-fixed.png)) | The "Optional" label beside *Files on this phone* is squeezed to one letter per line, and on *Notifications* it wraps as "Option / al". The button beside each text squeezed the explanation into a third of the card. | **Fixed:** title and label flow, and the button sits under the text (`PermissionStep`) |
| F14 | High | Devices › Join a device | Same layout: the camera card's explanation runs about 13 lines in a narrow column beside "Allow camera". | **Fixed** with F1 (same component) |
| F15 | High | Devices › Join a device, wrong code | The message reads "detail=That is not a Wasl pairing code." Every engine error shown through `AppError.wrap` started with the generated "detail=". | **Fixed:** the engine's sentence alone (`AppError.wrap`, unit test) |
| F13 | High | Devices › Add a device | The screen says "scan this code, or type the code below", but the code is about 75 characters (`WASL:06P6…`). Nobody types that on a phone. The 8-character short code exists in `wasl-net` (`ShortCode`) but isn't wired to the apps. | Open: wire the short code through `wasl-devices` and both apps |
| F18 | High | Settings › Vaults › Join a vault | You must paste the vault's manifest JSON, copied from "Show manifest" on the other phone. That is jargon and a dead end for most people. | Open (known): pick the vault folder in Drive instead |
| F2 | Medium | First run ([welcome](ux-audit-2026-10/01-welcome.png)), About, Settings › Privacy | "Your files go between your phone and your Drive. Nowhere else." and "Files go only to your Drive." are untrue once device sync is used: files go straight to the person's computer. | **Fixed:** "to your Drive and your own devices" (EN, AR) |
| F4 | Medium | First run › Set up later ([shot](ux-audit-2026-10/03-set-up-later-prompt.png)) | Choosing "Set up later" immediately raises Android's notification question. | **Fixed:** "Set up later" leaves quietly; Pairs asks once a pair exists (as before) |
| F6 | Medium | Settings ([shot](ux-audit-2026-10/06-settings-devices-row.png)) | Devices (direct phone-to-computer sync, pairing) is one row in the middle of Settings; nothing on Pairs or the bottom bar leads there. | Open (navigation, for the redesign) |
| F9 | Medium | Devices ([before](ux-audit-2026-10/09-devices.png), [after](ux-audit-2026-10/09-devices-fixed.png)) | "Add a device" and "Join a device" sit side by side with no hint which goes on which device. | **Fixed:** one line under the buttons says which to press where |
| F10 | Medium | Devices › When this phone syncs | A switch called "Sync with my devices now" reads "Off: only while Wasl is open". It's unclear what turning it on does, or for how long. | Open (wording and behaviour, for the redesign) |
| F11 | Medium | Devices › This phone | The device ID is cut off after its fifth group, with no wrap and no ellipsis. | **Fixed:** it wraps; long device names end in "…" |
| F12 | Medium | Settings › Phone name | The setting says it is "used in the names of conflict copies", but the engine always used the model code (`Build.MODEL`, "SM-A546E") whatever it said. The default was also the model code, not the name Android shows ("Galaxy A54" or the person's own). | **Fixed:** the engine takes the setting (from the next start), and new installs default to Android's device name. Installs that already saved a name keep it. |
| F17 | Medium | Settings › Diagnostics ([shot](ux-audit-2026-10/14-diagnostics.png)) | It reads "All files access is off, so pairs can't read their phone folders" with "Grant access again", even with no pairs or only picker pairs. First run called this permission optional, and "again" is odd for something never granted. | **Fixed:** this fails only when a pair on a path needs it; otherwise it reads "Off. Not needed for folders you pick with Android's picker" |
| F3 | Low | First run › What Wasl needs | Continue is greyed out with no line saying why (sign in first). The highlighted Google card is the only clue. | Open |
| F5 | Low | Pairs, no account ([shot](ux-audit-2026-10/05-sync-all-no-account.png)) | "Sync all now" shows with nothing to sync, and tapping it does nothing (it was disabled but looked the same). | **Fixed:** shown only once a pair exists |
| F7 | Low | Settings › Appearance ([shot](ux-audit-2026-10/07-settings-appearance.png)) | "Off: Modhayan Labs navy" is a company name, not a colour anyone recognises. | **Fixed:** "Off: Wasl's navy blue" |
| F8 | Low | Settings › About ([shot](ux-audit-2026-10/08-settings-about.png)) | Sync history sits under "About" at the very end of Settings. | Open (navigation) |
| F16 | Low | Problems, no account ([shot](ux-audit-2026-10/10-problems-no-account.png)) | It says "Nothing needs you" while nothing can sync until an account is added. | Open: needs a non-failure item style in Problems |

**Also seen, fine as is:**
- the empty Bin, Accounts, Vaults and Backup screens explain themselves ([accounts](ux-audit-2026-10/11-accounts-empty.png), [vaults](ux-audit-2026-10/12-vaults-empty.png), [backup](ux-audit-2026-10/13-backup.png));
- Automation is off with a one-line reason.

## Checks

- `./gradlew :app:testDebugUnitTest`: 410 tests, 0 failures.
- `:app:assembleRelease` was installed over 0.3.1 on the test phone, and the fixes were checked there:
  - F1, F14: layout from the UI tree and a screenshot;
  - F15: the message now reads "That is not a Wasl pairing code.";
  - F2, F7: Settings texts;
  - F4: no prompt;
  - F5: no button without pairs;
  - F9, F11: Devices;
  - F12: the default name after a reset;
  - F17: Diagnostics.
- Not checked on the phone: F12's engine side, that conflict copies carry the new name. That needs a conflict, which needs the signed-in screens above.
