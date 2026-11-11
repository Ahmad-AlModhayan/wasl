//! What an on-demand folder holds that Wasl left alone (docs/ON_DEMAND.md §2.3): never uploaded,
//! never deleted, only reported. The kind lets a client say why in its own words and language.

/// A path in an on-demand folder that Wasl did not touch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    /// Relative to the folder, `/`-separated.
    pub path: String,
    pub kind: KeptKind,
    /// The same in English, with any OS error (for logs and the CLI).
    pub detail: String,
}

/// Why a path was left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeptKind {
    /// Made here; not in the Drive listing.
    OnlyHere,
    /// A file changed here (Drive has the earlier version).
    ChangedHere,
    /// A regular file where Drive has a different file of the same name.
    Differs,
    /// The OS can't show it as a placeholder (its name or id).
    Unsupported,
    /// Creating, updating or removing its placeholder failed; tried again on the next refresh.
    Failed,
}

impl Kept {
    pub fn new(path: impl Into<String>, kind: KeptKind, detail: impl Into<String>) -> Kept {
        Kept {
            path: path.into(),
            kind,
            detail: detail.into(),
        }
    }
}
