# Contributing to Wasl

Thanks for helping. Wasl is **open code**: the full source is public under the
[PolyForm Shield License 1.0.0](LICENSE) ("source-available", not open source by the OSI definition). You may read,
run, change and share it, and send pull requests. You may not use it to provide a product that competes with Wasl
or with another Kahf Labs product built on it. Please read the [Code of Conduct](CODE_OF_CONDUCT.md) first, and
report security problems through [SECURITY.md](SECURITY.md), not as public issues.

## Before your first pull request: sign the CLA

Shield has no clause for incoming contributions, so every contributor signs the [CLA](CLA.md) once. A bot comments
on your first pull request with a link; reply with the sentence it asks for. **You keep your copyright**; the CLA
licenses your work to Ahmad AlModhayan, trading as Kahf Labs, so it can ship it in Wasl and relicense it later. If you can't agree to that, open an issue instead of a pull request.

## Where to start

- Browse open issues, especially those labelled `good first issue`. Open an issue before a large change so we agree
  on the shape first. Small fixes can go straight to a pull request.
- [CONTEXT.md](CONTEXT.md) has the words the code uses; [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and
  [docs/adr/](docs/adr) the reasons behind the design.
- A change to behaviour starts with a test that goes red on it, then the fix (red, green, refactor).
- User-visible text is English and Arabic, right to left included. Plain words, no jargon: see
  [docs/ERRORS.md](docs/ERRORS.md) for how errors are worded.

## Build and run the checks

The engine (Rust, `engine/`) and the Android app (Kotlin, `app/`) build from this repository alone: run the check
for the one your change touches. The desktop app (`desktop/`) and the website (`site/`) need Kahf Labs' private
design package (`@kahf/design`), so you can't build them from a public clone; see the note below the checks.

```sh
# engine
cd engine && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace

# android (JDK 17 or newer, Android SDK, cargo-ndk; see docs/SETUP.md)
./gradlew :app:testDebugUnitTest
```

GitHub Actions runs the same checks on every pull request. The desktop and website jobs need a token for the
private package, which fork pull requests don't have, so they are skipped there and Kahf Labs runs them before it
merges a change that touches `desktop/` or `site/`.

Kahf Labs builds of the desktop app and the website: with read access to the private package set up for git
(a read-only token), run `cd desktop && npm ci && npm test && npm run build`,
and `cd site && npm install && npm run build`.

Release builds and signing happen only in our own release
workflow; the repository holds no signing keys and no Google OAuth client. To run sign-in on your own build, create your
own client as described in [docs/SETUP.md](docs/SETUP.md).

## Pull requests

- One change per pull request, with a description of what and why. Link the issue it closes.
- Keep every check green and add a test for new behaviour.
- Wasl never changes a user's files without the rules the user set; don't add a path that does (see
  [docs/PRD.md](docs/PRD.md) and the ADRs).
- Do not commit keys, tokens, real account data or personal paths. Test vectors must be made-up values.
- Do not add a dependency without saying its licence in the pull request. Permissive licences (MIT, Apache-2.0,
  BSD, ISC) are fine; others need a conversation first.
- The names Kahf, Wasl, Qasd and Miqyas are not licensed for use by forks: see [TRADEMARKS.md](TRADEMARKS.md).
