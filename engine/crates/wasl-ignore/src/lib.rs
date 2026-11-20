//! Ignore rules: the 1.1.4 include/exclude globs, `.waslignore`/`.gitignore` files and presets.

#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod legacy;
pub mod presets;
pub mod rules;

pub use presets::Preset;
pub use rules::{FileKind, IgnoreRules, RuleMatch, RuleOrigin, Verdict};
