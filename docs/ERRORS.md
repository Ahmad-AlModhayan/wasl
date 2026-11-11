# Every error says what happened and suggests a fix (#155)

Spec for issue #155. Decisions: the design discussion in the issue comments (2026-11-03). Words follow
`CONTEXT.md`. Related: #102 (error kinds in the daemon API), #52 ("Sign in again
(internal_failure)" with no cause), #154 (a raw Android file error fails a whole pair), #150 and
#147 (review findings on wording), #136 (what logs may hold).

## Problem Statement

When something goes wrong, Wasl often shows the raw text of whatever failed underneath: a Java
exception, a Rust error, an HTTP body. On a real phone a pair failed with "Failed to determine if
primary:LifeOS/…/combined_statements.xlsx is child of primary:LifeOS: java.io.FileNotFoundException
…". It doesn't say what happened, what Wasl did about it, or what to do, and it reads like a
fault in Wasl. Each app also words errors on its own: Android maps a coarse kind to a title, the
desktop maps the same kinds through its own tables and otherwise shows the engine's message, and
the CLI prints the message. The same failure reads differently on each device, and some failures
("internal_failure") carry no cause at all. This breaks PRD goal 2: never stop silently, every
problem has a fix in plain words.

## Solution

One **error catalogue** in the engine. Every error Wasl shows (pair status, run history,
notifications, Problems, the desktop window, the CLI) comes from a catalogue entry, which says
three things:

1. **What happened**, naming the file or pair.
2. **What Wasl did** ("Skipped it; the rest of LifeOS synced").
3. **One suggested fix**, as a button where possible.

"Details" is collapsed by default. It holds the raw error text with a Copy button, never tokens or
account IDs, with paths shortened to the pair's folder. Safe, reversible fixes run on one tap;
fixes that change files only open the place to do it. An error the catalogue doesn't know reads
"Something went wrong syncing *pair*. Wasl will try again at the next sync.", with Retry and
Details, and is logged so it can be added to the catalogue.

## User Stories

What the message says

1. As a person whose sync failed, I want the message to say what happened in plain words, naming the file or pair, so that I know what it's about.
2. As that person, I want it to say what Wasl did ("Skipped it; the rest of LifeOS synced"), so that I know whether the rest of my files are safe.
3. As that person, I want one suggested fix, so that I'm not left guessing.
4. As that person, I want the fix as a button where possible, so that I can act in one tap.
5. As a person reading an error, I want no Java, Rust or HTTP text in the message itself, so that it never reads like a crash.
6. As an Arabic-language user, I want the same message in Arabic, so that errors are as clear in my language.
7. As a person using the phone, the desktop and the CLI, I want the same words for the same error on each, so that I recognise it anywhere.

One-tap fixes

8. As a person whose network dropped, I want Retry to run on one tap, so that I can try again at once.
9. As a person whose Google session expired, I want "Sign in again" on one tap, so that the account is back without hunting through Settings.
10. As a person whose folder access Android took away, I want "Grant access again" on one tap, so that the pair resumes.
11. As a person whose folder is missing or moved, I want "Open folder" (or the folder's place) on one tap, so that I can see what changed.
12. As a person whose phone or disk is full, I want "Open storage settings" on one tap, so that I can free up space myself.
13. As a person whose file name Drive or the system refused, I want the message to name the file and open the place to rename it, never to rename it for me, so that nothing changes my files without me.
14. As a person asked to delete or remove files to free space, I want Wasl to open the place, not delete anything, so that a tap never loses data.

Details and support

15. As a person who wants to know more, I want a collapsed "Details" holding the raw error, so that the main message stays plain.
16. As a person asking for help, I want Copy on Details, so that I can paste it to support.
17. As that person, I want Copy to add the Wasl version and platform, so that support needn't ask.
18. As that person, I want Details never to hold tokens or account IDs, so that copying it is safe.
19. As that person, I want paths in Details shortened to the pair's folder (`LifeOS/…/file`), so that my folder layout outside the pair stays private.
20. As that person, I want nothing sent anywhere when I copy, so that sharing stays my choice.

Unknown errors

21. As a person hitting an error Wasl doesn't know, I want "Something went wrong syncing *pair*. Wasl will try again at the next sync.", so that I know it isn't stuck.
22. As that person, I want Retry and Details on it, so that I can try now or get the raw text.
23. As the developer, I want each unknown error logged with its kind and where it came from, so that it can be added to the catalogue.

Where errors show

24. As a person looking at a failed pair, I want its status to use the catalogue entry, so that the pair screen explains itself.
25. As a person reading run history, I want each failed run worded from the catalogue, so that old failures read as clearly as new ones.
26. As a person who gets a notification, I want it to use the entry's "what happened" line and its fix as the action, so that I can fix it from the notification.
27. As a person on the Problems list, I want every problem to show its fix, so that the list is a to-do list.
28. As a CLI user, I want the entry's three lines printed, with Details behind a flag, so that scripts and people both get plain words.
29. As a CLI user, I want the exit code to keep following the error kind, so that scripts keep working.
30. As a person whose account shows a sign-in problem, I want it to say why ("Google ended the session", "Wasl couldn't read the saved sign-in"), never a bare code like "internal_failure", so that I know whether signing in again will help.

## Implementation Decisions

- **The catalogue lives in the engine**, beside the existing error kinds, and is the one place an error is worded. Each entry has: a stable id, a kind (the existing kinds, extended where a fix differs, for example a full local disk versus a full Drive, a missing local folder versus a missing file, #102), its "what happened" and "what Wasl did" sentences in English and Arabic with slots for the pair and file names, and one suggested fix.
- **A suggested fix is a typed action**, not text: Retry, Sign in again, Grant access again, Open folder, Open storage settings, and "open the place to" rename or remove (which only navigates). Each app maps an action to its own button and handler. Only the first five run on one tap.
- **An error crossing to an app carries its entry**: the entry id, the filled sentences, the fix action, and the Details text. The phone binding's error and the daemon API's error gain these as additive fields read with defaults, so older apps still show the message they show today. Stored run outcomes keep the entry id so run history words old failures from the catalogue.
- **Details redaction** happens once in the engine, when Details is built: tokens and account IDs are never put in; paths are cut to the pair's folder name followed by the path inside it. Copy (in each app) adds the Wasl version and platform; nothing is uploaded.
- **Unknown errors** get the fallback entry (Retry, Details) and one log line with the error's kind and source, without the redacted-out parts.
- **Apps stop wording errors themselves.** Android's kind-to-title mapping, the desktop's kind table and the CLI's raw message print give way to the entry's sentences. Pair-rule reasons (ADR-0001's typed reasons) are catalogue entries too (ADR-0003).
- **Language.** The catalogue holds English and Arabic, the two languages the apps ship (ADR-0003). A new language is added to the catalogue, not to each app.

## Testing Decisions

- Good tests check the words and fix a person gets for a given failure, through the highest seam, not how the catalogue is stored.
- **Seam 1, the catalogue** (main seam): for each kind, a failure gives an entry with both sentences filled in both languages and a fix; an unknown error gives the fallback; Details never contains a token, an account ID or a path outside the pair's folder (a test feeds it errors that do). Every entry has English and Arabic text and a fix (a table test, so a new entry can't ship half-written).
- **Seam 2, the app boundary**: the phone binding's contract tests and the daemon tests check an engine error arrives with its entry id, sentences, fix and Details, and an older client reading it still gets a message. Prior art: the FFI `contract` tests and the daemon tests.
- **Seam 3, each app's rendering**: the desktop's format tests, Android's status-text tests and the CLI output tests check the three lines, the button for the fix, and collapsed Details.
- The #154 failure is the first worked example: a file that vanished mid-run gives "*file* was gone before Wasl could sync it. Skipped it; the rest of *pair* synced." with Retry.

## Out of Scope

- Sending error reports anywhere (crash reporting, telemetry). Copy is the only way out.
- Fixing the causes of individual errors (#154's one-file-fails-the-pair, #52's sign-in cause); those stay their own bug issues. This spec gives them their words.
- Languages beyond English and Arabic.
- Rewording non-error status text (progress, "waiting for Wi-Fi").

## Further Notes

- **Decided (ADR-0003, 2026-11-03):** the catalogue owns every sentence, pair-rule reasons included, in English and Arabic. It supersedes ADR-0001's rule that each app words reasons from its own tables; the pairs module still returns typed reasons.
- Seams and the ticket breakdown were confirmed on 2026-11-03.
