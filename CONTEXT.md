# Wasl: domain glossary

The words Wasl's code, docs and agents use, one meaning each. Longer specs live in `docs/*.md`;
decisions that are hard to reverse live in `docs/adr/`.

**Pair**: a folder on this device kept in sync with one place elsewhere (a Drive folder, other
devices, or both). It has a method (two-way, mirror up or down, upload or download then delete),
rules (ignored paths) and a status. _Avoid_: sync job, connection.

**Route**: how a pair reaches its other copies: Drive, Devices, or Drive and Devices. Changing a
pair's route keeps the pair; it doesn't make a new one.

**Devices-only pair**: a pair whose route is Devices alone: it syncs a folder directly between
this person's devices, with no Drive folder and no Google account needed. Any device can make one
or accept an offer as one. _Avoid_: local pair, offline pair.

**Preview hold**: every new pair waits for its first preview. Nothing moves (scheduled, on a folder
change, or "sync now") until the person has seen the first plan and started it from the preview.
Whichever app or device created the pair, the hold is the same.

**Pair rules**: what makes a pair valid on this device: it doesn't overlap another pair (on the
device or on Drive), isn't inside Wasl's own folders, isn't a view, and a vault folder is linked
to its vault. Checked when a pair is created or changed, in one place (ADR-0001). A broken rule
is a typed reason, worded by the error catalogue (ADR-0003); Wasl is English first.

**Same name**: two names whose name keys (Unicode NFC, then lower case) are equal, on every device
and remote (ADR-0004). `Foo.txt` and `foo.txt`, or `أحمد` in NFC and NFD, are one name. A pair holds
one item per name in a folder; a name held twice on one side is a **name clash**, left out of the
sync and reported until one is renamed. _Avoid_: duplicate (that is Drive's word for two items with
exactly the same name).

**Vault**: a Drive folder whose files Wasl encrypts, names included, before they leave the device.
A pair on a vault folder is linked to that vault; a plain pair never writes into one.

**Offer**: a pair another device of yours would serve to this device. It arrives named ("*Laptop*
offers you *Projects*") and nothing syncs until it is accepted here.

**Accepting an offer**: if this device already has a pair on that folder, the pair joins the offer
by adding Devices to its route; otherwise a new pair with route Devices is made. Either way the
share starts narrow: shared with the offering device only, never with all of this device's other
devices. The new or changed pair gets the preview hold.

**Share code**: the hex ID devices use to find the same pair on each other. People see offers, not
codes, on the main path.

**Op-log**: Wasl's one append-only record of every actor's changes, for every reader (other tools
and apps). The agent tables keep the file-level working detail; both are written in one
transaction (ADR-0002).

**Changeset**: one actor's set of changes in one scope. Its states are open, staged, committed,
discarded, reverted and partly reverted. Named by its origin device and id together.

**Actor**: whoever made a change: `agent:<name>`, or `person:<device id>` for the person on a
device. Changes that arrive through Drive or another device's sync have no actor.

**Operation**: one change to one entity in a changeset, with its state before and after and its
outcome. A revert appends an inverse operation; nothing is rewritten.

**Entity URI**: names what an operation changed: `file://<pair or watch id>/<key>` for Wasl files.
Other products' schemes are kept as they come. _Avoid_: path (a URI names the key, not the local
path).

**Watched folder** (watch): a folder outside any pair that agent undo covers (`wasl agent watch`).
Every agent session snapshots it at its start. Its id comes from the same id space as pair ids.
_Avoid_: pair (nothing in a watched folder syncs).

**Snapshot**: what a session found in a watched folder at its start: each file's BLAKE3 and where
its bytes are (a copy-on-write clone, the undo store, or the synced version of a pair file).

**Error catalogue**: the list of every error Wasl can show. Each entry
says what happened (naming the file or pair), what Wasl did about it, and one suggested fix, in
plain words. Every app shows the same entry for the same error. An error not in the catalogue
shows the unknown-error entry. _Avoid_: error message, exception text.

**Suggested fix**: the one action an error's catalogue entry offers. A safe, reversible fix runs
on one tap; a fix that would change files only opens the place to do it.

**Details**: the raw text behind an error, collapsed under the entry and copyable for support. It
never holds tokens or account IDs, shows paths only from the pair's folder down, and is never
sent anywhere by Wasl.
