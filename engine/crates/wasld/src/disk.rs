//! Free space on the drive holding a folder, for the desktop's sidebar (#91). The system calls
//! go through safe wrappers: `nix`'s statvfs, and `wasl-vfs-windows` (the one crate where Win32
//! calls are allowed) on Windows.

use std::path::Path;

/// Free bytes this user may write on the drive holding `folder`, or on the drive of its nearest
/// existing parent while the folder isn't there yet. None when it can't be told: no folder on
/// this computer by a full path, or the system refused to say.
pub fn free_bytes(folder: &str) -> Option<u64> {
    let path = Path::new(folder);
    if !path.is_absolute() {
        return None;
    }
    let existing = path.ancestors().find(|p| p.exists())?;
    available(existing)
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // the field types differ by OS
fn available(path: &Path) -> Option<u64> {
    let st = nix::sys::statvfs::statvfs(path).ok()?;
    (st.blocks_available() as u64).checked_mul(st.fragment_size() as u64)
}

#[cfg(windows)]
fn available(path: &Path) -> Option<u64> {
    wasl_vfs_windows::volume::free_bytes(path).ok()
}

#[cfg(not(any(unix, windows)))]
fn available(_path: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_on_this_computer_has_free_space() {
        let dir = tempfile::tempdir().unwrap();
        assert!(free_bytes(dir.path().to_str().unwrap()).is_some_and(|b| b > 0));
    }

    #[test]
    fn a_folder_not_made_yet_reads_its_parents_drive() {
        let dir = tempfile::tempdir().unwrap();
        let later = dir.path().join("not-yet").join("deeper");
        assert!(free_bytes(later.to_str().unwrap()).is_some_and(|b| b > 0));
    }

    #[test]
    fn a_picked_folder_uri_has_no_answer() {
        assert_eq!(
            None,
            free_bytes("content://com.android.externalstorage/tree/primary%3ASync")
        );
        assert_eq!(None, free_bytes(""));
    }
}
