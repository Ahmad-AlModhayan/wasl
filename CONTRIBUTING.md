# Contributing to Wasl

Thanks for helping. This public Wasl repository uses a **decoupled open-core** model:

- The sync engine crates, daemon, storage, cryptography, network/device sync, VFS, FFI (`wasl-ffi`), and relay/server components (`engine/`) are OSI-approved open source under [GNU AGPL v3.0](LICENSE) (`AGPL-3.0-only`), with optional commercial dual-licensing from Kahf Labs.
- The wire protocol (`wasl-proto`), local IPC client (`wasl-ipc`), and thin CLI (`wasl-cli`) are licensed under [Apache License 2.0](LICENSE-APACHE) (`Apache-2.0`) with zero AGPL dependencies.
- Official native UI apps (Android and desktop UI/shell integrations) connect across the FFI/IPC boundary and live outside this public repository under separate commercial terms.

Please read the [Code of Conduct](CODE_OF_CONDUCT.md) first, and report security problems through [SECURITY.md](SECURITY.md), not as public issues.

## Before your first pull request: sign the CLA

Every external contributor signs the [CLA](CLA.md) once before merge. A bot comments on your first pull request with a link; reply with the sentence it asks for. **You keep your copyright**; the CLA licenses your work to Ahmad AlModhayan, trading as Kahf Labs, so Kahf Labs can ship official native clients across FFI/IPC and offer commercial dual-licences alongside the public AGPL-3.0-only / Apache-2.0 codebase. If you can't agree to that, open an issue instead of a pull request.

## Where to start

- Browse open issues, especially those labelled `good first issue`. Open an issue before a large change so we agree
  on the shape first. Small fixes can go straight to a pull request.
- Keep core crates focused on indexing, hashing, chunking, planning, storage, crypto, and transport — prefer exposing local IPC event hooks over adding UI- or workflow-specific bloat to `wasl-core`.
- [CONTEXT.md](CONTEXT.md) has the words the code uses; [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md),
  [docs/OPEN_MODEL.md](docs/OPEN_MODEL.md), and [docs/adr/](docs/adr) explain the reasons behind the design.
- A change to behaviour starts with a test that goes red on it, then the fix (red, green, refactor).
- User-visible text is English and Arabic, right to left included. Plain words, no jargon: see
  [docs/ERRORS.md](docs/ERRORS.md) for how errors are worded.

## Build and run the checks

The engine, daemon, FFI, protocol, and CLI (`engine/`) build from this repository alone. The website (`site/`) needs Kahf Labs' private design package (`@kahf/design`), so it can't be built from a public clone.

```sh
cd engine
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p wasl-ffi --all-features
```

CI also verifies the licence boundary so Apache-2.0 crates (`wasl-proto`, `wasl-ipc`, `wasl-cli`) never take a normal, build, or dev dependency on any `AGPL-3.0-only` workspace crate.

GitHub Actions runs the engine checks on every pull request. The website job needs a token for `@kahf/design`, which fork pull requests don't have, so it is skipped there and Kahf Labs runs it before merging a change that touches `site/`.

## Pull requests

- One change per pull request, with a description of what and why. Link the issue it closes.
- Keep every check green (including the Apache-2.0 / AGPL-3.0 boundary check) and add a test for new behaviour.
- Wasl never changes a user's files without the rules the user set; don't add a path that does (see
  [docs/PRD.md](docs/PRD.md) and the ADRs).
- Do not commit keys, tokens, real account data or personal paths. Test vectors must be made-up values.
- Do not add a third-party dependency without saying its licence in the pull request. Permissive licences (MIT, Apache-2.0,
  BSD, ISC) are fine; strong copyleft third-party dependencies (GPL, AGPL, LGPL) are not allowed inside engine or protocol crates (see `engine/deny.toml`).
- The names Kahf, Wasl, Qasd and Miqyas are not licensed for use by forks: see [TRADEMARKS.md](TRADEMARKS.md).
