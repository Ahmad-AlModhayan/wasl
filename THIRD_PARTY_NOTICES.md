# Third-party notices

Open-source libraries Wasl's code depends on and that need a notice here. Each entry gives the
licence, where the source is, and which crate of this repository uses it. The full dependency list
with exact versions is `engine/Cargo.lock`; the fonts and other assets are in `NOTICE.md`.

## regex

- What for: finding secrets and paths in an error's cause so they can be hidden before it is shown
  (`wasl-proto`, `words::redact`).
- Licence: MIT OR Apache-2.0 (used here under either; both permit this use without a source
  obligation, and both ask that the licence text and copyright notice go with a copy of the library).
- Source: https://github.com/rust-lang/regex (the `regex` crate, version 1.x).
- Pulls in `regex-automata` and `regex-syntax` (MIT OR Apache-2.0, the same repository), and
  `aho-corasick` and `memchr` (Unlicense OR MIT, https://github.com/BurntSushi/aho-corasick and
  https://github.com/BurntSushi/memchr).
