# ADR-0003: The engine's error catalogue words every error

Date: 2026-11-03. Status: Accepted (2026-11-03). Supersedes the wording rule in ADR-0001.

## Context

ADR-0001 put the pair rules in one module and had a broken rule come back as a typed reason,
never as a sentence; each app worded it from its own string tables. Other errors went the same
way or worse: Android mapped coarse kinds to titles, the desktop had its own kind table and
otherwise showed the engine's raw message, and the CLI printed the message. The same failure read
differently on each device, and some showed raw exception text (#154) or a bare code (#52).

The grilling for #155 decided that every error says what happened, what Wasl did, and one
suggested fix, with the same words on Android, the desktop and the CLI (`docs/ERRORS.md`).

## Decision

The engine's **error catalogue** owns every sentence Wasl shows for an error, pair-rule reasons
included, in English and Arabic. Each entry has a stable id, a kind, its sentences with slots for
the pair and file names, one suggested fix as a typed action, and the redacted Details. An error
crossing to an app carries its entry; apps render it and map the fix to a button, and keep no
error wording of their own.

ADR-0001's typed reasons stay: the pairs module still returns a typed reason, and the catalogue
turns it into an entry. Only who words it changes.

A new language means adding it to the catalogue, in the engine, not to each app's tables.

## Rejected alternative

**Each app keeps its own string tables, generated from one shared source.** The engine would hold
only entry ids, slots and fix actions, and a build step would generate each app's English and
Arabic tables from one file so they can't drift. Rejected: it keeps a generation step in every
app's build, lets an app ship stale tables between releases, and still leaves the CLI and any new
app to wire it up; the engine owning the sentences gives the same words everywhere with no step.

## Consequences

- One error reads the same on every device; a new app gets every sentence by depending on the
  engine.
- A table test in the engine fails if any entry lacks English, Arabic or a fix.
- Apps drop their own error tables (Android's status titles, the desktop's kind table, its
  pair-rule strings).
- Copy edits to error text are engine changes and ship with an engine release.
