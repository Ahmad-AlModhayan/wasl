# Device sync without a Google account (#152)

Spec for issue #152. Decisions: two design rounds in the issue comments (2026-11-03).
Words follow `CONTEXT.md`; the pair rules and offer handling follow ADR-0001 (one pairs module
below every app).

## Problem Statement

A person who doesn't use Google, or doesn't want their files in Drive, can't use Wasl on a phone
today. Pairing two devices works without an account, but the moment they accept a folder another
device offers, the phone asks them to "Add a Google account to choose a Drive folder": every pair
on the phone must have a Drive folder. Onboarding makes sign-in step 1 and calls it "required",
and the Home empty state says "Add a Google account". The desktop already makes Devices-only
pairs, so the same person gets two different products depending on the device in their hand.

## Solution

On every platform, Google is needed only for Drive. A person can set Wasl up with "my devices
only", pair their devices, accept offers as **Devices-only pairs** and make Devices-only pairs
of their own, with no account. When there is no account, Drive is shown greyed out with "Sign in
to use Drive" so the person knows it exists; it is never hidden and never required. Without an
account, devices sync on the same network (LAN) in 0.5, and the pair screen says so. Signing in
later changes no pair on its own: each pair offers "Also sync with Drive". Device-only sync counts
the same as Drive pairs; an account changes nothing about what is free.

## User Stories

Onboarding

1. As a new person, I want the first screen to offer two equal choices, "Sync with Google Drive" and "Sync between my devices only", so that I'm not pushed into Google.
2. As a person who picks "my devices only", I want the next screen to be "Pair your first device", with a code to scan or type, so that I can start at once.
3. As a person on "Pair your first device", I want a line saying the other device needs Wasl too, so that I know what to install there.
4. As a person who isn't ready to pair, I want a "Later" choice that leads to an empty Home whose call to action is "Pair a device", so that I'm never stuck.
5. As a person who picked "my devices only", I want no screen to call a Google sign-in "required", so that the app agrees with my choice.
6. As a person who picked Drive, I want today's onboarding (sign in, choose a first folder) unchanged, so that nothing gets worse for Drive users.
7. As a person who started with devices only, I want to add a Google account later in Settings, so that the first choice isn't final.

Home and empty states

8. As a person with no account and no pairs, I want Home to say "Pair a device", not "Add a Google account", so that the call to action matches how I use Wasl.
9. As a person with no account and devices paired but no pairs, I want Home to point me to the offers waiting or to "Add a pair", so that I see the next step.

Making a pair

10. As a person adding a pair on any device, I want "Add a pair" to ask where the folder syncs: My devices, Google Drive, or both, so that I choose the route up front.
11. As a person with no account, I want Google Drive in that choice greyed out with "Sign in to use Drive", so that I know it exists and how to get it.
12. As a person with no account, I want My devices chosen for me, so that I don't have to pick the only option.
13. As a person making a Devices-only pair, I want to pick the local folder and the devices it syncs with, and see the first-sync preview, so that it works like any other pair.
14. As a person making a Devices-only pair on a phone, I want the same pair rules as on the desktop (no overlap, not inside Wasl's own folders), so that the phone refuses what the desktop refuses.
15. As a person on a Devices-only pair screen with no account, I want it to say "Syncs when both devices are on the same Wi-Fi", so that I know when to expect it to run.

Accepting offers

16. As a person with no account, I want Accept on an offer to make a Devices-only pair without asking for a Drive folder, so that I can receive folders from my computer.
17. As a person accepting an offer, I want it to work the same on the phone as on the desktop (join a pair already on that folder, else make a Devices pair), so that the result doesn't depend on the device.
18. As a person accepting an offer of a pair that syncs with Drive and devices, on a phone with no account, I want it accepted as a Devices-only pair that syncs with the offering device directly, so that I don't need the account the other device uses.
19. As that person, I want the pair to say "Syncs with your devices; Drive copy is kept by *PC*", so that I know where the Drive copy is and who keeps it.
20. As the person on the offering PC, I want my pair to keep Drive and add the phone as a device, so that nothing changes on my side.
21. As a person accepting any offer, I want the new pair held for its first preview, so that nothing arrives before I've seen the plan.
22. As a person accepting any offer, I want the share to start narrow (the offering device only), so that it doesn't spread to my other devices by itself.

Devices and group news

23. As a person with no account, I want pairing, removing and renaming devices to work, so that the group is fully mine without Google.
24. As a person with no account, I want news of a device joining or being removed to reach my other devices when they meet, so that every device ends up with the same group.
25. As a person looking at Devices, I want each device to show "Last seen", so that I can tell when group news might not have reached it yet.
26. As a person with no account, I want nothing about my group (device names, keys) sent to Google, so that device-only really means device-only.

Signing in later

27. As a person who signs in later, I want none of my pairs to change on their own, so that signing in is safe.
28. As that person, I want each Devices-only pair to offer "Also sync with Drive", so that I can add Drive to the pairs I choose.
29. As a person who picks "Also sync with Drive", I want to choose the Drive folder and see the first-sync preview as usual, so that adding Drive is never a blind merge.
30. As a person who signs out, I want my Devices-only pairs to keep syncing, so that losing the account doesn't stop my devices.

Limits

31. As a person with no account, I want the same unlimited pairs as a person with Drive pairs, so that an account changes nothing about what is free.

Every platform

33. As a desktop user with no account, I want the first-run screen to offer the same two choices as the phone, so that the product reads the same everywhere.
34. As a CLI user with no account, I want `wasl pair add` and `wasl device accept` to make Devices-only pairs without asking for an account, so that a headless machine works too.

## Implementation Decisions

- **One pairs module decides.** Making a Devices-only pair, accepting an offer, and adding Drive to a pair later all go through the pairs module (ADR-0001), so the phone, the desktop service and the CLI get the same behaviour and the same typed reasons. The phone stops making a Drive pair first and then adding Devices to it; it calls the module's accept, which joins an existing pair on that folder or makes a Devices pair. This builds on #41 (offers into the pairs module).
- **A pair needs a Drive folder only when its route includes Drive.** The module's rule already says so; the phone's own pair record and run path must accept a pair with no account and no Drive folder, and the phone's run must sync such a pair with devices only.
- **Mixed offers.** An offer names a share, not a route. A receiver with no account makes a Devices pair on that share; the sender's pair keeps its route (Drive and Devices) and lists the receiver as a device. The receiver shows who keeps the Drive copy from what the sender says about its own pair; the offer gains an optional field saying the sender's pair also syncs with Drive (additive, read with a default from older senders).
- **Reach without an account is LAN.** A new Devices pair made with no account has reach LAN, like every new device pair today. This spec adds no internet reach without an account.
- **The group without Drive** is kept on each device and changed by pairing and by operations exchanged when devices meet; group files in Drive are written only where an account is in use for device sync (`docs/DEVICE_SYNC.md` §3, unchanged). Each device's entry shows when this device last saw it.
- **"Add a pair" destination chooser.** One question, three answers: My devices, Google Drive, both. With no account, Drive and both are disabled with "Sign in to use Drive", and My devices is selected. The answer sets the pair's route.
- **Signing in later** adds an account and nothing else. "Also sync with Drive" on a pair is a route change (Devices → Drive and Devices) plus a Drive folder, followed by the usual first-sync preview hold.
- **Onboarding** gets a first choice (Drive / my devices only). The devices-only branch skips sign-in and leads to "Pair your first device" or, on "Later", an empty Home with "Pair a device". The Drive branch is today's flow.
- **Limits:** there are no local pair-count limits (no Wasl Pro tier; see `docs/OPEN_MODEL.md`).

## Testing Decisions

- Good tests here check what a person can do (make, accept, change a pair; what a screen offers) through the highest seam, not how a screen or store is built.
- **Seam 1, the pairs module** (main seam): accepting an offer with no account makes a Devices pair with the preview hold and a narrow share; accepting a Drive-and-devices offer with no account makes a Devices pair; "Also sync with Drive" changes the route and holds the pair; signing in changes no pair. Prior art: the pairs module's own tests and the offer tests in the FFI and daemon test suites (`devices` tests).
- **Seam 2, each app's screen model**: the Android pair editor and onboarding view models, and the desktop first-run and add-pair models, given no account, offer My devices selected, Drive disabled with its reason, and the right empty-state call to action. Prior art: the desktop `*Model.test.ts` files and the Android view-model tests.
- **End to end**, once: two devices on one network, neither signed in, pair, offer, accept on the phone, first preview, files arrive. Recorded in the PR as for the 0.4.3 introducer test.

## Out of Scope

- Internet reach without an account.
- Offers carried through Drive group files for devices that never meet (`docs/INTRODUCTIONS.md` §7).
- Moving the group off Drive for people who do have an account.

## Further Notes

- `docs/INTRODUCTIONS.md` §3.3 and §6 describe the phone's pairs as always having a Drive folder; that is today's behaviour and this spec replaces it.
- Seams (pairs module first, then each app's screen model), the ticket breakdown and the optional offer field were confirmed on 2026-11-03.
