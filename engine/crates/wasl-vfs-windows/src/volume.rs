//! What the volume under a path can do. Here rather than in `wasl-ipc` because this is the one
//! crate where Win32 calls (`unsafe`) are allowed: `wasl-ipc` asks whether a folder's volume
//! keeps access lists, so it can refuse to run with a data folder it could not protect
//! (security review S3-3).

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetVolumeInformationW, GetVolumePathNameW};
use windows::core::PCWSTR;

/// `FILE_PERSISTENT_ACLS` (System.SystemServices, not worth a feature of its own).
const FILE_PERSISTENT_ACLS: u32 = 0x0000_0008;

/// Whether the volume holding `path` keeps access lists (NTFS, ReFS, most SMB shares: true;
/// FAT, exFAT: false). `path` need not exist; its volume must.
pub fn persists_acls(path: &Path) -> io::Result<bool> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // A volume root such as `C:\` or `\\server\share\`, or a mounted folder's path.
    let mut root = vec![0u16; wide.len().max(260) + 1];
    // SAFETY: `wide` is NUL-terminated and lives for the call; `root` is a writable buffer whose
    // length the binding passes, and the call writes a NUL-terminated path within it.
    unsafe { GetVolumePathNameW(PCWSTR(wide.as_ptr()), &mut root) }.map_err(io::Error::from)?;
    let mut flags = 0u32;
    // SAFETY: `root` holds the NUL-terminated root written above and lives for the call; `flags`
    // is a live u32 the call writes; every other buffer is absent.
    unsafe { GetVolumeInformationW(PCWSTR(root.as_ptr()), None, None, None, Some(&mut flags), None) }
        .map_err(io::Error::from)?;
    Ok(flags & FILE_PERSISTENT_ACLS != 0)
}

/// Free bytes this user may write on the volume holding `path` (quotas counted), for the
/// desktop's free space (wasl#91). `path` must exist.
pub fn free_bytes(path: &Path) -> io::Result<u64> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is NUL-terminated and lives for the call; `free` is a live u64 the call
    // writes; the two other counts are absent.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&raw mut free), None, None) }
        .map_err(io::Error::from)?;
    Ok(free)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_temp_folders_volume_keeps_access_lists() {
        // Windows' temp folder is on the system drive, NTFS (ReFS on some servers).
        let dir = tempfile::tempdir().unwrap();
        assert!(persists_acls(dir.path()).unwrap());
        assert!(persists_acls(&dir.path().join("not-yet")).unwrap());
    }

    #[test]
    fn the_temp_folders_volume_has_free_space() {
        let dir = tempfile::tempdir().unwrap();
        assert!(free_bytes(dir.path()).unwrap() > 0);
    }
}
