//! The local file access the engine needs, and the path check every implementation applies.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// One child of a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub mtime_ms: i64,
}

/// A path the Drive transport can read a file from. Some back ends (Android SAF) must keep a handle
/// open while it is used; `guard` holds it until the value is dropped.
pub struct ReadablePath {
    pub path: PathBuf,
    pub guard: Option<Box<dyn Send>>,
}

impl ReadablePath {
    pub fn plain(path: PathBuf) -> ReadablePath {
        ReadablePath { path, guard: None }
    }
}

/// Access to one pair's local folder. Paths are `/`-separated and relative to the pair root
/// (`""` is the root). Implementations must reject any path [`check_rel`] rejects.
pub trait LocalFs: Send + Sync {
    /// The children of folder `dir`.
    fn list(&self, dir: &str) -> io::Result<Vec<FsEntry>>;

    /// The entry at `rel`, or `None` when nothing is there.
    fn stat(&self, rel: &str) -> io::Result<Option<FsEntry>>;

    fn open_read(&self, rel: &str) -> io::Result<Box<dyn Read + Send>>;

    /// A path the Drive transport can read `rel` from while the returned value lives.
    fn readable_path(&self, rel: &str) -> io::Result<ReadablePath>;

    /// Where a download for `rel` is written before [`LocalFs::install`] puts it in place. On a
    /// plain file system this sits next to the target, so installing is an atomic rename.
    fn staging_path(&self, rel: &str) -> io::Result<PathBuf>;

    /// Puts the staged file at `rel`. The caller has already moved any previous file at `rel` to
    /// the bin. Sets the modification time when given. Returns the installed entry.
    fn install(&self, staged: &Path, rel: &str, mtime_ms: Option<i64>) -> io::Result<FsEntry>;

    /// Creates folder `rel` and any missing parents.
    fn mkdir(&self, rel: &str) -> io::Result<()>;

    /// Deletes the file at `rel` outright. Only for "upload then delete", where Drive holds the
    /// verified copy; every other removal goes through [`LocalFs::move_to_bin`].
    fn remove_file(&self, rel: &str) -> io::Result<()>;

    /// Removes folder `rel` if it is empty (ignoring leftover part files). Returns whether it did.
    fn remove_dir_if_empty(&self, rel: &str) -> io::Result<bool>;

    /// Renames `from` to `to` (used for conflict copies; both in the same folder).
    fn rename(&self, from: &str, to: &str) -> io::Result<()>;
    /// True when `a` and `b` name one existing item, as `Foo.txt` and `foo.txt` do on a folder
    /// that folds case. A folder that cannot tell says false.
    fn is_same_file(&self, _a: &str, _b: &str) -> io::Result<bool> {
        Ok(false)
    }

    /// Moves the file at `rel` to the recycle bin. Returns an opaque location for restoring it.
    fn move_to_bin(&self, rel: &str) -> io::Result<String>;

    /// Puts a binned file back at `rel` (which must not exist).
    fn restore_from_bin(&self, location: &str, rel: &str) -> io::Result<()>;

    /// Deletes a binned file for good (after the retention period).
    fn purge_from_bin(&self, location: &str) -> io::Result<()>;
}

/// Checks a pair-relative path: `/`-separated, no empty, `.` or `..` components, no backslash,
/// no NUL, not absolute. Every [`LocalFs`] method applies it, so a hostile Drive name or a corrupt
/// baseline row can never reach outside the pair folder.
pub fn check_rel(rel: &str) -> io::Result<&str> {
    check_rel_on(rel, cfg!(windows))
}

/// [`check_rel`] with Windows' extra rules switched on or off (for tests on any OS). On Windows a
/// component must not be a reserved device name (`CON`, `NUL`, `COM1`…, with any extension),
/// contain `:` (alternate data streams) or end in `.` or a space (Windows drops them, so two names
/// would point at one file).
pub fn check_rel_on(rel: &str, windows: bool) -> io::Result<&str> {
    let bad = |why: &str| {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsafe path {rel:?}: {why}"),
        ))
    };
    if rel.is_empty() {
        return Ok(rel);
    }
    if rel.contains('\0') {
        return bad("NUL");
    }
    if rel.contains('\\') {
        return bad("backslash");
    }
    if rel.starts_with('/') {
        return bad("absolute");
    }
    // A drive letter or a UNC/verbatim prefix would make Path::join replace the root on Windows.
    if rel.len() >= 2 && rel.as_bytes()[1] == b':' {
        return bad("drive prefix");
    }
    for part in rel.split('/') {
        match part {
            "" => return bad("empty component"),
            "." | ".." => return bad("dot component"),
            _ => {}
        }
        if windows {
            if part.contains(':') {
                return bad("colon");
            }
            if part.ends_with('.') || part.ends_with(' ') {
                return bad("trailing dot or space");
            }
            if is_windows_device_name(part) {
                return bad("reserved device name");
            }
            if looks_like_short_name(part) {
                return bad("looks like an 8.3 short name, which can stand for another file");
            }
        }
    }
    Ok(rel)
}

/// `CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9`, `LPT0`-`LPT9` (and the superscript-digit forms), with
/// or without an extension, in any case.
/// `REPORT~1.DOC`: a stem of at most 8 characters with `~` and digits at its end, and an extension
/// of at most 3. On Windows such a name may refer to a different file's short name.
fn looks_like_short_name(part: &str) -> bool {
    let (stem, ext) = part.rsplit_once('.').unwrap_or((part, ""));
    let Some((head, digits)) = stem.rsplit_once('~') else {
        return false;
    };
    !head.is_empty()
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && stem.chars().count() <= 8
        && ext.chars().count() <= 3
}

fn is_windows_device_name(part: &str) -> bool {
    let stem = part
        .split('.')
        .next()
        .unwrap_or(part)
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => {
            // Split after three characters, not three bytes: names are often not ASCII.
            let cut = stem.char_indices().nth(3).map_or(stem.len(), |(i, _)| i);
            let (prefix, digit) = stem.split_at(cut);
            (prefix == "COM" || prefix == "LPT")
                && matches!(
                    digit,
                    "0" | "1"
                        | "2"
                        | "3"
                        | "4"
                        | "5"
                        | "6"
                        | "7"
                        | "8"
                        | "9"
                        | "\u{b9}"
                        | "\u{b2}"
                        | "\u{b3}"
                )
        }
    }
}

/// Names the engine creates for itself and never syncs.
pub fn is_internal_name(name: &str) -> bool {
    // Ignoring case as Unicode maps it: on a case-insensitive disk `x.WASL-PART`, or `x.waſl-part`
    // with a long s (upper case `S`), is the same file as Wasl's own `x.wasl-part`.
    let same = |a: char, b: char| {
        a == b || a.to_uppercase().eq(b.to_uppercase()) || a.to_lowercase().eq(b.to_lowercase())
    };
    let ends = |suffix: &str| {
        let mut tail = name.chars().rev();
        suffix
            .chars()
            .rev()
            .all(|s| tail.next().is_some_and(|c| same(c, s)))
    };
    ends(crate::PART_SUFFIX) || wasl_ignore::legacy::INTERNAL_SUFFIXES.iter().any(|s| ends(s))
}

#[cfg(test)]
mod internal_name_tests {
    use super::is_internal_name;

    #[test]
    fn internal_names_match_in_any_case() {
        for n in [
            "a.wasl-part",
            "a.WASL-PART",
            "b.DriveSync-Part",
            "c.drivesync-tmp",
            "d.wa\u{17F}l-part",
            "e.\u{212A}.wasl-par\u{54}",
        ] {
            assert!(is_internal_name(n), "{n}");
        }
        for n in ["a.txt", "wasl-part", "é.wasl-par", ""] {
            assert!(!is_internal_name(n), "{n}");
        }
    }
}
