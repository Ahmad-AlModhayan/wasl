# Introductions: bringing a new device into your folders

Design, 2026-10. Wasl's own design; Syncthing's introducer was read for research only. It builds on
the device group and pairing of [DEVICE_SYNC.md](DEVICE_SYNC.md) §2–§3.

## 1. The problem

**Syncthing's introducer.** You mark a remote device as an introducer. From then on:

- your device adds every device the introducer shares a folder with you;
- it removes them again when the introducer does;
- folders those devices offer still have to be accepted, unless "auto accept" is on.

It saves typing device IDs, but it has known weak spots:

- **The flag is invisible.** Devices appear on a machine nobody touched, and the person there can't tell where they came from.
- **Trust is all or nothing and passes on.** An introducer can bring in any device, and that device is trusted at once, for every folder it is offered.
- **Removals cascade by surprise.** Removing a device on the introducer removes it everywhere, and re-adding it there brings it back.
- **The words are technical.** "Introducer", "auto accept" and "device ID" mean nothing to most people.

**Wasl today (0.3.1) is less safe than the introducer, not more.** All devices of a person form one signed group. Any member's `add` is trusted by every member (DEVICE_SYNC.md §3), so every device is effectively an introducer, and silently. Then:

1. **A new device is trusted at once.** No other device tells its person.
2. **It can read every shared folder.** "Every member of the group may sync every shared pair" (`DeviceService::pair`). A device that was paired through a lost phone can read every folder the laptop shares, without anyone on the laptop agreeing.
3. **Sharing a folder means carrying a code by hand.** You copy a share code on one device, then create a pair on the other and paste the code there. No device ever offers a folder.
4. **A device can be in only one group.** Joining replaces the joiner's group, so two people's devices can't be merged into one (unchanged here, see §7).

## 2. Goals

- **No surprises.** Every device says which devices joined, who brought them in and when. Until someone on this device agrees, no folder on this device is open to a device it didn't add itself.
- **Clear consent, on each side.** Folders are *offered* to a named device and *accepted* there. The person accepting picks where the folder goes and sees the usual first-sync preview. Nothing syncs before that.
- **Revocable, one step each:**
  - stop sharing a folder with one device;
  - withdraw an offer that hasn't been accepted;
  - decline an offer;
  - remove a device from all of them (the existing group removal).
- **Understandable words.** No "introducer", "device ID" or "share code" in the main path. The text reads, for example, "Your phone added *Work laptop*", "*Laptop* offers you *Projects*".
- **Compatible.** Devices on 0.3.1 keep syncing with upgraded ones, and folders already shared keep working.

## 3. The design

### 3.1 Who brought this device (arrivals)

Every member already carries `added_by` and `added_at` in its signed group operation. Each device keeps a local note per member, `device_notes(device_id, known)`:

- **Already known (`known = 1`):**
  - this device itself;
  - a device this device added (its person checked the four words here);
  - the device this one joined through;
  - every member present when this design first runs, so upgrading changes nothing for devices that already sync.
- **New (`known = 0`):** any other member that arrives through the group. The device's list shows it under "New" with "Added by *Phone* · 29 Oct". Pairs and Problems show one notice: "*Phone* added *Work laptop* to your devices. If it isn't yours, remove it."
  - **Keep** marks it known, on this device only. Each device's person decides for that device.
  - **Remove** signs the group removal, which reaches every member as today.

**What "new" changes.** A new device is a full group member for the transport and the group (removals keep working). It just gets no folder from this device yet:

- it is left out of `shared()`;
- every pair request from it is refused (`NotShared`);
- this holds even for folders shared with "all my devices", until it is kept on this device or offered a folder here.

This is what closes problem 2. A device added through a lost phone reaches nothing on the laptop until the laptop's person says so.

### 3.2 Folders are shared with chosen devices

A shared pair now has a scope, stored with its share (`pair_shares.all_devices`, `pair_share_devices(pair_id, device_id)`):

- **Chosen devices (new shares).** The pair is served only to the devices on its list.
- **All my devices (shares made before this design).** The pair is served to every member that is known on this device (§3.1). The UI shows this as "All my devices" and offers to switch to chosen devices.

The two combine: a pair served to all known devices may also list devices, which are then offered it and served it even while new here. In the store a share starts as "all my devices" (`all_devices` defaults to 1), so a share made by code, by `wasl pair share` without `--with`, or by a client from before this design keeps today's meaning; choosing devices is a second step (`ShareWith`), which the screens take at once when they share with chosen devices. As built (2026-10).

`DeviceService` checks the asking device on every request:
- `shared`, `changes_since`, `chunk_list`, `chunk` and `notify`: the peer must be allowed for that pair, besides the existing reach and route checks;
- the transport's members-only gate is unchanged.

**One rule, both ways** (security review 2026-10). "Allowed" is one check (`Store::share_allows`): the device is on the pair's list, or the pair is shared with all devices and the device is known here. It decides whom a pair is served to and also whom this device syncs it with: a run pulls only from, and tells of changes only, the connected devices it allows, and a `notify` from any other device is ignored. So a device new here, or off a pair's list, neither reads the pair here nor feeds files into it.

**A received share starts narrow.** A pair made by accepting an offer (§3.3) is shared with the offering device only (`all_devices = 0`, the sender listed), set in the same transaction as its share ID. Accepting a folder never makes it reach this device's other devices; only the person widening it here does.

**Removing a device from a pair's list** stops it at its next request; there is no data to call back. That device's own copy of the pair stays on it and shows "*Laptop* stopped sharing *Projects* with this device".

### 3.3 Offers instead of codes

**Offering.** Sharing a pair with a device adds that device to the pair's list. That list *is* the offer: an offer is a pair this device would serve to that peer.

**On the wire.**
- A new request, `Request::Offers`, answered with `Response::Offers(Vec<Offer>)`.
- An offer is `{share, name, files, bytes}`. The name is at most 128 bytes, the counts come from the pair's index, and there are at most 64 offers.
- Only members ask and only members answer, and an offer reveals nothing the sender isn't already willing to serve to that device.

**Receiving.**
- A device asks each connected peer for its offers:
  - when the link comes up;
  - on every group exchange (each minute);
  - when the peer says something changed (`GroupChanged` is reused, so no new request is needed). Changing a pair's list sends it to every connected member.
- New offers are kept in `received_offers(share, from_device, name, files, bytes, first_seen, state)`, one row per share *and sender*, whose state is `new`, `declined` or `accepted`.
  - `from_device` is the peer the transport authenticated on the link the offer came over, never a value in the offer.
  - `first_seen` is this device's clock when the offer first arrived.
  - Two devices offering the same share make two offers, shown apart with their senders; accepting or declining names the sender. A member offering another device's share ID (security review 2026-10) adds its own offer beside the real sender's and can never replace it.
- An offer the sender no longer makes is dropped, unless it was accepted. (Not built yet: an accepted pair saying the sharing stopped; its pulls are refused with `NotShared`.)

**What the person sees:**
- "*Laptop* offers you *Projects* (1,240 files, 2.1 GB)".
- **Accept** opens the normal "add a pair" flow, pre-filled: route Devices, the share already set, the name proposed. The person picks the local folder and sees the first-sync preview. The pair is created only when they confirm.
  - As built (2026-10): the daemon's `AcceptOffer` makes the pair (route Devices, reach LAN like any new device pair, the folder created if missing), sets its share and marks the offer accepted. The pair is held like every new pair: a device-only pair held for its preview runs only when started from the preview (`Trigger::Preview`); scheduled, notified and "sync now" runs skip it. There is no Drive plan to show for it, so `wasl pair preview <pair> --start` says so and starts it.
  - Either way the new pair is shared with the offering device only (§3.2, "A received share starts narrow"), never with "all my devices".
  - On the phone the app makes the pair as it does for any pair (as built in 0.4: its pairs always have a Drive folder; [DEVICES_ONLY.md](DEVICES_ONLY.md) (#152) replaces this: the phone accepts as a Devices-only pair, with no Drive folder or account needed) and then calls `accept_offer(pair, share, from)` in place of `share_pair`. In one transaction that sets the share with that narrow scope, adds Devices to the pair's route (as built in 0.4: Drive becomes Drive and devices; with #152 a phone with no account makes a Devices-only pair instead) and marks the offer accepted. A pair already shared under another share is refused, never re-keyed.
  - Held for its preview on every device: the device side of a run skips a held pair until the person starts it from the preview (`approve_once`), so nothing comes from the offering device before then.
  - A device isn't offered a folder a pair of its own already syncs: after accepting, the receiver lists the sender on its pair, which would otherwise read on the sender as an offer of its own folder.
  - Accepting will behave the same on every device (join a pair already on that folder, else make a Devices pair): ADR-0001, ticket #41.
- **Decline** hides it on this device. The sender sees "Not accepted yet" and can withdraw it.
- Accepting a share that some local pair already uses is refused. This is the existing rule for share codes.

Share codes stay available under "More" for devices that can't ask for offers (0.3.1).

### 3.4 Introducing a new device to your folders (the introducer, done Wasl's way)

**At pairing.** When pairing succeeds on the device that showed the code, its last screen lists the folders it shares:

> **Work laptop is now one of your devices.**
> Offer it folders? ☐ Projects ☐ Photos ☐ Invoices
> [Offer selected] [Not now]

- Each ticked folder is offered (§3.3).
- The new device sees the offers at once, since it is connected, and accepts each one or not.
- The device that showed the code is the introducer, for this device only, and only for what its person ticked.
- Nothing is offered by default. No device keeps an "introducer" flag that could act later without a person.

**Arrivals are visible everywhere.**
- The other devices see the new device as new (§3.1), with who introduced it.
- If they share folders with "all my devices", it gets them only once their person keeps it.
- Their person can also offer it folders of their own.

**Compared with Syncthing's introducer:**

| | Syncthing introducer | Wasl introductions |
|---|---|---|
| Who can bring in a device | Devices marked introducer, forever | The person pairing, once, after checking four words |
| Other devices learn of it | Silently added | Shown as new, with who added it and when |
| Folder access of the new device | Whatever it is offered, auto-accepted if set | Only folders offered to it by name, accepted there; on other devices, nothing until kept |
| Undo | Remove on the introducer, may come back | Stop sharing, withdraw, decline, or remove (signed, permanent, reaches all) |
| Words | Introducer, auto accept, device ID | "Added by", "offers you", "Keep / Remove" |

## 4. Security notes

- **Offers are data, not commands.**
  - An offer never creates a pair, picks a folder or writes a file.
  - Its name is shown as text, cut to 128 bytes, and stripped of control and bidi characters, as for device names (`clean_name`).
  - Counts are informational.
- **Every request is checked again.**
  - A request for a pair from a device that is no longer allowed is refused.
  - So is a request from one still "new" here, from a stranger, or from beyond the LAN for a LAN pair.
  - The check uses the peer ID the transport authenticated, never anything in the request.
- **A removed device loses everything at once.** It leaves the transport's trusted set as today, and every list and note naming it is ignored.
- **A rogue member can forge little.**
  - It can offer folders, and those are just as visible and still need acceptance.
  - It can make itself "added by" someone, but only within what the group rules already allow (DEVICE_SYNC.md §3 S2-2). The note shows the signer of its `add`, which the group checked.
- **Limits.** At most 64 offers are kept per peer and 256 in all, cut in two passes (security review 2026-10):
  1. each device is cut to 64 on its own: its declined offers go first, then its oldest new ones;
  2. the total is counted again and cut to 256, one offer at a time from the device holding the most, so a device that offers a lot cannot push out other devices' offers.

  Accepted offers are never cut. "Oldest" is by this device's clock.
- **Names.** Device names and offer names alike lose control and bidi characters (`clean_name`, now also on device names).

## 5. Compatibility

- **An older device (0.3.1) asked for offers** fails to decode `Request::Offers` and answers with a protocol error; the link keeps serving. The asker keeps the offers it already had from that device (none, from a device that never offered). A protocol error is never read as "offers nothing": that dropped the offers kept from the device, so a declined offer came back as new (review of #28).
- **An older device sharing with an upgraded one** keeps working:
  - its shares are "all my devices";
  - the upgraded device serves them to members it knows, and every member present at upgrade is known.
- **An upgraded device sharing with an older one:**
  - the older one asks `Shared` and gets the pairs it is allowed;
  - it can't see offers, so its person uses a share code (still available under "More").
- **Store migration 0010** (the next free number; 0006 was taken by the time this was built) adds `pair_shares.all_devices` (default 1), `pair_share_devices`, `device_notes` and `received_offers`.
  - The store does not know the group, so `device_notes` is filled where the group is loaded (`wasl-devices`, `Devices::open`): when there are no notes at all, every member present is noted as known; afterwards each member without a note is noted as new. This device is always known.
  - Unsharing a pair drops its device list with its share; sharing it again starts from "all my devices".

## 6. Surfaces

**Engine.**
- `wasl-store`: the tables above.
- `wasl-net`: `Request::Offers`, `Response::Offers`, `Offer`, `PeerService::offers`, `PeerLink::offers`.
- `wasl-engine` `DeviceService`: per-peer checks and offers.
- `wasl-devices`: fetching offers, notes on arrival, `offers()`, `decline_offer`, `share_with`, `keep_device`.

**Daemon and CLI.**
- IPC: `Offers`, `DeclineOffer{share, from}`, `AcceptOffer{share, from, local_root, name}`, `ShareWith{id, all, devices}`, `Sharing{id}`, `KeepDevice{id}`; `DeviceInfo` gains `added_by` and `known` (`added_at` was there), both read with defaults from an older daemon. Offers name their sender because one share can be offered by several devices (§3.3).
- CLI: `wasl device offers`, `wasl device accept`, `wasl device decline`, `wasl device keep`, `wasl pair share --with <device>`; `wasl device list` shows new devices and who added them.
- Phone binding (`WaslDevices`): `offers`, `decline_offer`, `accept_offer`, `keep_device`, `share_with`, `sharing`; `info` carries `added_by` and `known`.

**Desktop** (`desktop/src/screens/introductions.ts`).
- **Devices:**
  - A new device shows "New", who added it, and Keep beside Remove.
  - "Offered to this computer" lists offers with Accept and Decline. Accept asks for the folder, then opens the new pair's preview.
- **A pair's sharing row:** "Shared with …" and a device chooser (All my devices, or chosen ones).
- **Pairing done, on the device that showed the code:** "Offer folders to *name*", none ticked.

**Android** (`ui/devices/Introductions.kt`).
- **Devices:** the same New note with Keep, and "Offered to this phone".
- **The pair screen's Devices section:** "Devices it syncs with" and the same chooser.
- **Pairing done:** "Offer folders to *name*".
- **Accept on the phone** opens the usual new-pair editor, with the offer named at the top. The phone has no devices-only pairs yet (E2E 2026-10, G4), so the person also picks a Drive folder; [DEVICES_ONLY.md](DEVICES_ONLY.md) (#152) removes that step. On save, `accept_offer` shares the pair with the offering device only, and the pair syncs with Drive and devices (as built in 0.4; with #152 it can be a Devices-only pair).

**Not built yet:** the "*Laptop* stopped sharing *Projects*" line (§3.2). Today the pair's device runs just fail with "not shared".

## 7. Not in this design

- **Offers through Drive group files**, for devices that are never online together. Offers are only fetched over a link, so both devices must meet once. Group files could carry them later, signed by the sender.
- **Limiting which devices may add others** (group-level roles). That needs a new signed operation every member understands, and it is the next step if wanted. Until then, the arrivals gate (§3.1) bounds what a device added by surprise can reach.
- **Merging two existing groups** (two people's devices). Unchanged: joining replaces the joiner's group.
