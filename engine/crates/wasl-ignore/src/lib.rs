//! Ignore rules: the 1.1.4 include/exclude globs, `.waslignore`/`.gitignore` files and presets.

pub mod legacy;
pub mod presets;
pub mod rules;

pub use presets::Preset;
pub use rules::{FileKind, IgnoreRules, RuleMatch, RuleOrigin, Verdict};
