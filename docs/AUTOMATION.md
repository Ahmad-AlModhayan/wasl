# Automation

## Broadcast intents (Tasker, MacroDroid, adb)
Any app on the phone can send a broadcast, so automation is **off by default**. Turn it on in
**Settings → Automation → Allow automation**. Turning it on creates a secret; every command must
carry it as the string extra `secret`, or Drive Sync ignores it. **New secret** in the same dialog
replaces it, and anything still using the old one stops working. The secret is never included in
config backups.

```bash
S=<your secret>
# sync every enabled, unpaused pair now, within each pair's Wi-Fi, VPN, roaming and battery rules
adb shell am broadcast -a com.almodhayan.drivesync.SYNC_ALL --es secret $S -n com.almodhayan.drivesync/.scheduler.SyncIntentReceiver
# sync one pair (id from the pair detail screen); a paused pair stays paused
adb shell am broadcast -a com.almodhayan.drivesync.SYNC_PAIR --es pairId <id> --es secret $S -n com.almodhayan.drivesync/.scheduler.SyncIntentReceiver
# pause / resume everything
adb shell am broadcast -a com.almodhayan.drivesync.PAUSE_ALL --es secret $S -n com.almodhayan.drivesync/.scheduler.SyncIntentReceiver
adb shell am broadcast -a com.almodhayan.drivesync.RESUME_ALL --es secret $S -n com.almodhayan.drivesync/.scheduler.SyncIntentReceiver
# cancel running syncs / re-arm the background schedule
adb shell am broadcast -a com.almodhayan.drivesync.CANCEL --es secret $S -n com.almodhayan.drivesync/.scheduler.SyncIntentReceiver
adb shell am broadcast -a com.almodhayan.drivesync.RESCHEDULE --es secret $S -n com.almodhayan.drivesync/.scheduler.SyncIntentReceiver
```
In Tasker use *System → Send Intent* with Action = the string above, Extra = `secret:<your secret>`
(and `pairId:<id>` for SYNC_PAIR), Package = `com.almodhayan.drivesync`,
Class = `com.almodhayan.drivesync.scheduler.SyncIntentReceiver`, Target = Broadcast Receiver.

Automation syncs follow each pair's conditions, unlike **Sync now** in the app, which is you
deciding on the spot. Notification buttons (Pause, Restore on phone) go to a separate receiver
that other apps cannot reach.

## Other triggers
- **Quick Settings tile** "Drive Sync": tap to sync everything, tap again to cancel.
- **Home screen widget**: status + Sync now button.
- **App shortcuts** (long-press the icon): Sync now, Pause all, Resume all.
- **Share sheet**: share any file to "Add to Drive Sync" to drop it into a pair's folder and sync.
- **Instant sync**: Settings → Instant sync keeps a lightweight watcher running so local changes and Drive changes are picked up within seconds/minutes instead of the periodic interval.
