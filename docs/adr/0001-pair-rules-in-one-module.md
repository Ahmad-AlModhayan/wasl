# ADR-0001: Pair rules live in one module, below every app

Date: 2026-10-31. Status: accepted; its wording rule superseded by ADR-0003 (2026-11-03).

## Context

Wasl has several apps in front of the same engine: the desktop service (`wasld`, used by the
desktop window and the CLI), the phone (`wasl-ffi` plus Kotlin), and soon the Mac File Provider.
The rules for pairs were written in each of them:

- The desktop service checked a new pair (overlap, Wasl's own folders, views, vault link) and set
  the preview hold. The phone's `create_pair` only wrote to the store, and Kotlin re-checked
  overlaps and set the hold itself.
- Sharing a pair was a near line-for-line copy in `wasld/src/daemon.rs` and `wasl-ffi/src/devices.rs`.
- Accepting an offer was fixed in both and then diverged: the phone joined an existing pair, the
  desktop always made a new one (and its overlap check refused the phone's case).
- The pair's remote (plain-folder guard, vault v1 or v2) and the vault passphrase minimum were
  each defined twice; running a pair across its route and merging the outcomes were written in
  the desktop service and again in Kotlin.

Nothing tested that the copies agreed.

## Decision

A new crate (working name `wasl-pairs`), above `wasl-engine` and `wasl-devices`, owns the
pair rules and operations: create, update, delete, share and unshare, accept, decline and keep
offers, build a pair's remote, run a pair across its route and merge the outcomes, and the vault
rules a pair depends on (linking, passphrase minimum). `wasld` and `wasl-ffi` call it and only
translate between it and their transport (IPC, UniFFI) and language.

Apps keep no rules of their own. Where an editor needs to warn before saving, it calls the
module's check, which returns the same reasons the save would.

Accepting an offer behaves the same everywhere: join an existing pair on that folder by adding
Devices to its route, otherwise make a new Devices pair; the share starts narrow (CONTEXT.md).

A broken rule comes back as a typed reason (overlaps a named pair, inside Wasl's folders, a view,
a vault folder without its vault, …), never as a sentence. ~~Each app words it from its own string
tables: Wasl is English first for a global audience, and a new language means adding tables in
the apps, not touching the module.~~

> **Superseded (2026-11-03) by ADR-0003:** the struck sentence no longer holds. The typed reason
> stays; the engine's error catalogue words it, in English and Arabic, for every app.

Pairs made before the rules were shared may break them (the phone didn't check overlaps or always
set the preview hold). They are checked once on upgrade, and each one that breaks a rule shows a
problem with its fix ("*Photos* overlaps *Camera*: edit one"). Nothing is changed silently.

Tests: one suite at the module's interface (the real store on a temp folder, the in-memory Drive).
Each case where the apps disagree today (overlap on the phone, accepting into an existing pair,
the preview hold from every app) gets a test that fails before the move and passes after.

The move happens in two changes, after the Introductions PR (#28) is merged so its offer code
moves too: first create, update, share and the pair's remote; then route runs, outcome merging
and offers.

## Consequences

- A pair rule is fixed once and applies on every device; a new app gets every rule by depending
  on the module.
- The rules are tested once, at the module's interface, with in-memory adapters; the desktop
  service and the phone keep only thin tests of their translation.
- `wasld/src/vaults.rs` loses the vault-linking and passphrase rules to the module.
- Rejected: putting the rules in `wasl-engine` (it can't depend on `wasl-devices`, which owns
  sharing and routes), in `wasl-devices` (it would grow from device sync into everything about
  pairs), or in `wasld` (the phone would depend on the desktop service's IPC and system code).
