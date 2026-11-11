//! [`LocalFs`] over Android's Storage Access Framework. Metadata calls go to Kotlin through
//! [`SafBridge`]; file data never crosses the language boundary: Kotlin opens a file descriptor and
//! Rust reads it through `/proc/self/fd/N`, so large files stream at disk speed without copies and
//! without `unsafe`.
//!
//! Opening `/proc/self/fd/N` is a new open of the file the descriptor points at
//! (`/mnt/user/0/emulated/0/...` for shared storage), checked against the app's own storage
//! permissions, not against the folder grant. Without All files access Android refuses it for every
//! file the app didn't create (`Permission denied`, #127), so such a file is copied out through the
//! grant instead ([`SafBridge::copy_out`]).

use crate::WaslError;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wasl_fs::{FsEntry, LocalFs, ReadablePath, check_rel};

/// One child of a SAF folder.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SafEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime_ms: i64,
}

/// What Kotlin implements with `DocumentsContract`, relative to a pair's tree URI. Paths are
/// pair-relative, `/`-separated and already checked by Rust.
#[uniffi::export(with_foreign)]
pub trait SafBridge: Send + Sync {
    fn list(&self, tree: String, dir: String) -> Result<Vec<SafEntry>, WaslError>;
    fn stat(&self, tree: String, rel: String) -> Result<Option<SafEntry>, WaslError>;
    /// Opens the document read-only and returns a file descriptor Rust may read until it calls
    /// [`SafBridge::close_fd`].
    fn open_read_fd(&self, tree: String, rel: String) -> Result<i32, WaslError>;
    fn close_fd(&self, fd: i32);
    /// Copies the document at `rel` to `dest` (a plain path in the app's cache) through the folder
    /// grant: the way to read a file the app may not open by its path.
    fn copy_out(&self, tree: String, rel: String, dest: String) -> Result<(), WaslError>;
    /// Copies the finished download at `staged` (a plain path in the app's cache) into the tree at
    /// `rel` (creating folders), sets its time when given, deletes `staged`, returns the entry.
    fn install(
        &self,
        tree: String,
        staged: String,
        rel: String,
        mtime_ms: Option<i64>,
    ) -> Result<SafEntry, WaslError>;
    fn mkdir(&self, tree: String, rel: String) -> Result<(), WaslError>;
    fn remove_file(&self, tree: String, rel: String) -> Result<(), WaslError>;
    fn remove_dir_if_empty(&self, tree: String, rel: String) -> Result<bool, WaslError>;
    fn rename(&self, tree: String, from: String, to: String) -> Result<(), WaslError>;
    /// Moves the document into Wasl's bin folder and returns an opaque location.
    fn move_to_bin(&self, tree: String, rel: String) -> Result<String, WaslError>;
    fn restore_from_bin(&self, tree: String, location: String, rel: String) -> Result<(), WaslError>;
    fn purge_from_bin(&self, tree: String, location: String) -> Result<(), WaslError>;
}

pub(crate) struct SafFs {
    pub bridge: Arc<dyn SafBridge>,
    pub tree: String,
    /// The app's cache folder for this pair; downloads land here before `install`.
    pub staging: PathBuf,
    /// Opens `/proc/self/fd/N`; [`File::open`] except in tests.
    pub reopen: fn(&Path) -> io::Result<File>,
    /// Free bytes this app may write where `staging` is, if known; [`free_bytes`] except in tests.
    pub free_space: fn(&Path) -> Option<u64>,
}

/// Room left on the phone after a copy through the folder grant: the copy never fills the disk.
pub(crate) const COPY_RESERVE: u64 = 256 * 1024 * 1024;

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // the field types differ by OS
fn free_bytes(path: &Path) -> Option<u64> {
    let st = nix::sys::statvfs::statvfs(path).ok()?;
    (st.blocks_available() as u64).checked_mul(st.fragment_size() as u64)
}

#[cfg(not(unix))]
fn free_bytes(_path: &Path) -> Option<u64> {
    None
}

fn megabytes(bytes: u64) -> String {
    format!("{} MB", bytes.div_ceil(1024 * 1024))
}

impl SafFs {
    pub fn new(bridge: Arc<dyn SafBridge>, tree: String, staging: PathBuf) -> SafFs {
        sweep_stale_copies(&staging);
        SafFs {
            bridge,
            tree,
            staging,
            reopen: |p| File::open(p),
            free_space: free_bytes,
        }
    }
}

/// Copies made for reading, numbered so two reads of one file never share a copy.
static COPIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Start of a copy's name; the process id follows, so a copy left behind is known as stale.
const COPY_PREFIX: &str = "read-";

/// Deletes the copies an earlier process left in `staging` (it died before dropping them). This
/// process's own copies may still be in use by another read or upload, and are deleted on drop.
fn sweep_stale_copies(staging: &Path) {
    let Ok(dir) = std::fs::read_dir(staging) else {
        return;
    };
    let mine = format!("{COPY_PREFIX}{}-", std::process::id());
    for entry in dir.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(COPY_PREFIX) && !name.starts_with(&mine) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A file copied out through the folder grant; deleted when dropped.
struct CopiedOut {
    path: PathBuf,
}

impl Drop for CopiedOut {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn io_err(e: WaslError) -> io::Error {
    match e {
        WaslError::Permission { detail: message } => io::Error::new(io::ErrorKind::PermissionDenied, message),
        WaslError::NotFound { detail: message } => io::Error::new(io::ErrorKind::NotFound, message),
        other => io::Error::other(other.to_string()),
    }
}

fn entry(e: SafEntry) -> FsEntry {
    FsEntry {
        name: e.name,
        is_dir: e.is_dir,
        size: e.size,
        mtime_ms: e.mtime_ms,
    }
}

/// Closes the descriptor through Kotlin when the reader or path is dropped.
struct FdGuard {
    bridge: Arc<dyn SafBridge>,
    fd: i32,
}

impl Drop for FdGuard {
    fn drop(&mut self) {
        self.bridge.close_fd(self.fd);
    }
}

struct FdReader<G> {
    file: File,
    _guard: G,
}

impl<G> Read for FdReader<G> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

impl SafFs {
    fn open_fd(&self, rel: &str) -> io::Result<(PathBuf, FdGuard)> {
        let fd = self
            .bridge
            .open_read_fd(self.tree.clone(), check_rel(rel)?.to_owned())
            .map_err(io_err)?;
        let guard = FdGuard {
            bridge: self.bridge.clone(),
            fd,
        };
        if fd < 0 {
            return Err(io::Error::other("no file descriptor"));
        }
        Ok((PathBuf::from(format!("/proc/self/fd/{fd}")), guard))
    }

    /// `rel` opened for reading: by the descriptor's path when Android allows it, else from a copy
    /// made through the folder grant. Errors name the operation; callers name the file.
    fn open_readable(&self, rel: &str) -> io::Result<(PathBuf, File, Box<dyn Send>)> {
        let rel = check_rel(rel)?;
        let (path, guard) = self
            .open_fd(rel)
            .map_err(|e| context("open it through the folder access", e))?;
        let by_path = match (self.reopen)(&path) {
            Ok(file) => return Ok((path, file, Box::new(guard))),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => e,
            Err(e) => return Err(context("open it by its path", e)),
        };
        drop(guard);
        let copy = self.copy_out(rel).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "couldn't open it by its path ({by_path}) or copy it through the folder access ({e})"
                ),
            )
        })?;
        let file =
            File::open(&copy.path).map_err(|e| context("open the copy made through the folder access", e))?;
        Ok((copy.path.clone(), file, Box::new(copy)))
    }

    fn copy_out(&self, rel: &str) -> io::Result<CopiedOut> {
        std::fs::create_dir_all(&self.staging)?;
        // The size the provider reports; when it can't say, the copy goes ahead (a full disk then
        // fails the copy, which is deleted).
        let size = self
            .bridge
            .stat(self.tree.clone(), rel.to_owned())
            .ok()
            .flatten()
            .map(|e| e.size);
        if let (Some(size), Some(free)) = (size, (self.free_space)(&self.staging))
            && size.saturating_add(COPY_RESERVE) > free
        {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                format!(
                    "not enough free space on the phone to read it through the folder grant: it is {}, and {} is free (Wasl keeps {} spare)",
                    megabytes(size),
                    megabytes(free),
                    megabytes(COPY_RESERVE)
                ),
            ));
        }
        let digest = wasl_fs::hash_reader(rel.as_bytes())?.blake3;
        let n = COPIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let copy = CopiedOut {
            path: self.staging.join(format!(
                "{COPY_PREFIX}{}-{n}-{}",
                std::process::id(),
                &digest[..16]
            )),
        };
        self.bridge
            .copy_out(
                self.tree.clone(),
                rel.to_owned(),
                copy.path.to_string_lossy().into_owned(),
            )
            .map_err(io_err)?;
        Ok(copy)
    }
}

/// `e` with what Wasl was doing, keeping its kind.
fn context(what: &str, e: io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("couldn't {what}: {e}"))
}

impl LocalFs for SafFs {
    fn list(&self, dir: &str) -> io::Result<Vec<FsEntry>> {
        let mut out: Vec<FsEntry> = self
            .bridge
            .list(self.tree.clone(), check_rel(dir)?.to_owned())
            .map_err(io_err)?
            .into_iter()
            .filter(|e| {
                !wasl_fs::is_internal_name(&e.name) && check_rel(&e.name).is_ok() && !e.name.contains('/')
            })
            .map(entry)
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn stat(&self, rel: &str) -> io::Result<Option<FsEntry>> {
        Ok(self
            .bridge
            .stat(self.tree.clone(), check_rel(rel)?.to_owned())
            .map_err(io_err)?
            .map(entry))
    }

    fn open_read(&self, rel: &str) -> io::Result<Box<dyn Read + Send>> {
        let (_, file, guard) = self.open_readable(rel)?;
        Ok(Box::new(FdReader { file, _guard: guard }))
    }

    fn readable_path(&self, rel: &str) -> io::Result<ReadablePath> {
        let (path, file, guard) = self.open_readable(rel)?;
        // The caller opens the path itself; this check only finds out which path it may open.
        drop(file);
        Ok(ReadablePath {
            path,
            guard: Some(guard),
        })
    }

    fn staging_path(&self, rel: &str) -> io::Result<PathBuf> {
        let rel = check_rel(rel)?;
        std::fs::create_dir_all(&self.staging)?;
        // One flat folder; the name only has to be unique per target.
        let digest = wasl_fs::hash_reader(rel.as_bytes())?.blake3;
        Ok(self
            .staging
            .join(format!("{}{}", &digest[..32], wasl_fs::PART_SUFFIX)))
    }

    fn install(&self, staged: &Path, rel: &str, mtime_ms: Option<i64>) -> io::Result<FsEntry> {
        let e = self
            .bridge
            .install(
                self.tree.clone(),
                staged.to_string_lossy().into_owned(),
                check_rel(rel)?.to_owned(),
                mtime_ms,
            )
            .map_err(io_err)?;
        Ok(entry(e))
    }

    fn mkdir(&self, rel: &str) -> io::Result<()> {
        self.bridge
            .mkdir(self.tree.clone(), check_rel(rel)?.to_owned())
            .map_err(io_err)
    }

    fn remove_file(&self, rel: &str) -> io::Result<()> {
        if rel.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the pair root is not a file",
            ));
        }
        self.bridge
            .remove_file(self.tree.clone(), check_rel(rel)?.to_owned())
            .map_err(io_err)
    }

    fn remove_dir_if_empty(&self, rel: &str) -> io::Result<bool> {
        if rel.is_empty() {
            return Ok(false);
        }
        self.bridge
            .remove_dir_if_empty(self.tree.clone(), check_rel(rel)?.to_owned())
            .map_err(io_err)
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        self.bridge
            .rename(
                self.tree.clone(),
                check_rel(from)?.to_owned(),
                check_rel(to)?.to_owned(),
            )
            .map_err(io_err)
    }

    fn move_to_bin(&self, rel: &str) -> io::Result<String> {
        if rel.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the pair root cannot be binned",
            ));
        }
        self.bridge
            .move_to_bin(self.tree.clone(), check_rel(rel)?.to_owned())
            .map_err(io_err)
    }

    fn restore_from_bin(&self, location: &str, rel: &str) -> io::Result<()> {
        self.bridge
            .restore_from_bin(self.tree.clone(), location.to_owned(), check_rel(rel)?.to_owned())
            .map_err(io_err)
    }

    fn purge_from_bin(&self, location: &str) -> io::Result<()> {
        self.bridge
            .purge_from_bin(self.tree.clone(), location.to_owned())
            .map_err(io_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A folder grant over a plain folder: descriptors are numbers, copies are real.
    struct FakeBridge {
        root: PathBuf,
        closed: Mutex<Vec<i32>>,
        copy_denied: bool,
    }

    fn not_used<T>() -> Result<T, WaslError> {
        Err(WaslError::Other {
            detail: "not used".into(),
        })
    }

    impl SafBridge for FakeBridge {
        fn list(&self, _: String, _: String) -> Result<Vec<SafEntry>, WaslError> {
            not_used()
        }
        fn stat(&self, _: String, rel: String) -> Result<Option<SafEntry>, WaslError> {
            Ok(std::fs::metadata(self.root.join(&rel)).ok().map(|m| SafEntry {
                name: rel,
                is_dir: m.is_dir(),
                size: m.len(),
                mtime_ms: 0,
            }))
        }
        fn open_read_fd(&self, _: String, _: String) -> Result<i32, WaslError> {
            Ok(7)
        }
        fn close_fd(&self, fd: i32) {
            self.closed.lock().unwrap().push(fd);
        }
        fn copy_out(&self, _: String, rel: String, dest: String) -> Result<(), WaslError> {
            if self.copy_denied {
                return Err(WaslError::Permission {
                    detail: "Wasl no longer has access to this folder".into(),
                });
            }
            std::fs::copy(self.root.join(rel), dest)
                .map(|_| ())
                .map_err(|e| WaslError::Io {
                    detail: e.to_string(),
                })
        }
        fn install(&self, _: String, _: String, _: String, _: Option<i64>) -> Result<SafEntry, WaslError> {
            not_used()
        }
        fn mkdir(&self, _: String, _: String) -> Result<(), WaslError> {
            not_used()
        }
        fn remove_file(&self, _: String, _: String) -> Result<(), WaslError> {
            not_used()
        }
        fn remove_dir_if_empty(&self, _: String, _: String) -> Result<bool, WaslError> {
            not_used()
        }
        fn rename(&self, _: String, _: String, _: String) -> Result<(), WaslError> {
            not_used()
        }
        fn move_to_bin(&self, _: String, _: String) -> Result<String, WaslError> {
            not_used()
        }
        fn restore_from_bin(&self, _: String, _: String, _: String) -> Result<(), WaslError> {
            not_used()
        }
        fn purge_from_bin(&self, _: String, _: String) -> Result<(), WaslError> {
            not_used()
        }
    }

    /// What Android answers when an app without All files access opens `/proc/self/fd/N` of a
    /// file it didn't create (the test phone, 2026-11-02).
    fn denied(_: &Path) -> io::Result<File> {
        // EACCES as Linux words it (the raw number means something else on other test hosts).
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Permission denied (os error 13)",
        ))
    }

    fn setup(copy_denied: bool) -> (tempfile::TempDir, Arc<FakeBridge>, SafFs) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Notes");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("todo.txt"), b"changed on the phone").unwrap();
        let bridge = Arc::new(FakeBridge {
            root,
            closed: Mutex::new(Vec::new()),
            copy_denied,
        });
        let mut fs = SafFs::new(
            bridge.clone(),
            "content://tree".into(),
            dir.path().join("staging"),
        );
        fs.reopen = denied;
        fs.free_space = |_| Some(u64::MAX);
        (dir, bridge, fs)
    }

    fn staged(fs: &SafFs) -> usize {
        std::fs::read_dir(&fs.staging).map_or(0, |d| d.count())
    }

    #[test]
    fn a_file_android_wont_open_by_path_is_read_through_the_grant() {
        let (_dir, bridge, fs) = setup(false);
        let mut text = String::new();
        fs.open_read("todo.txt")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!("changed on the phone", text);
        assert_eq!(
            vec![7],
            *bridge.closed.lock().unwrap(),
            "the descriptor is closed"
        );
        assert_eq!(0, staged(&fs), "the copy is deleted once read");
    }

    #[test]
    fn an_upload_gets_a_path_it_can_open_while_it_needs_it() {
        let (_dir, _bridge, fs) = setup(false);
        let readable = fs.readable_path("todo.txt").unwrap();
        assert_eq!(
            b"changed on the phone".to_vec(),
            std::fs::read(&readable.path).unwrap()
        );
        drop(readable);
        assert_eq!(0, staged(&fs));
    }

    #[test]
    fn when_the_grant_refuses_too_the_error_says_both_and_stays_a_permission_error() {
        let (_dir, bridge, fs) = setup(true);
        let e = fs.open_read("todo.txt").err().unwrap();
        assert_eq!(io::ErrorKind::PermissionDenied, e.kind());
        let message = e.to_string();
        assert!(message.contains("by its path"), "{message}");
        assert!(message.contains("os error 13"), "{message}");
        assert!(message.contains("no longer has access"), "{message}");
        assert_eq!(vec![7], *bridge.closed.lock().unwrap());
        assert_eq!(0, staged(&fs));
    }

    #[test]
    fn a_copy_that_would_leave_the_phone_short_of_space_is_not_made() {
        let (_dir, bridge, mut fs) = setup(false);
        // The 20-byte file fits, but not with the reserve kept free.
        fs.free_space = |_| Some(COPY_RESERVE + 10);
        let e = fs.open_read("todo.txt").err().unwrap();
        assert_eq!(io::ErrorKind::StorageFull, e.kind());
        let message = e.to_string();
        assert!(
            message.contains("not enough free space on the phone to read it through the folder grant"),
            "{message}"
        );
        assert_eq!(vec![7], *bridge.closed.lock().unwrap());
        assert_eq!(0, staged(&fs), "nothing copied");

        fs.free_space = |_| Some(COPY_RESERVE + 20);
        let mut text = String::new();
        fs.open_read("todo.txt")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!("changed on the phone", text, "exactly enough room");
    }

    #[test]
    fn copies_an_earlier_process_left_behind_are_deleted_when_a_pair_opens() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let mine = format!("read-{}-0-abcd", std::process::id());
        // Not this process: a pid can't be 0 for a running app, and u32::MAX is never handed out.
        for name in [
            "read-0-3-abcd",
            &format!("read-{}-1-abcd", u32::MAX),
            &mine,
            "x.wasl-part",
        ] {
            std::fs::write(staging.join(name), b"copy").unwrap();
        }
        let bridge = Arc::new(FakeBridge {
            root: dir.path().to_owned(),
            closed: Mutex::new(Vec::new()),
            copy_denied: false,
        });
        let _fs = SafFs::new(bridge, "content://tree".into(), staging.clone());
        let mut left: Vec<String> = std::fs::read_dir(&staging)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            vec![mine, "x.wasl-part".to_owned()],
            left,
            "a download in progress stays too"
        );
    }

    #[test]
    fn a_path_android_allows_is_read_directly_without_a_copy() {
        let (_dir, bridge, mut fs) = setup(true);
        // Stands in for the descriptor's path: any readable file.
        fs.reopen = |_| File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/src/saf.rs"));
        let mut text = String::new();
        fs.open_read("todo.txt")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert!(text.contains("a_path_android_allows_is_read_directly"));
        assert_eq!(0, staged(&fs), "no copy");
        assert_eq!(vec![7], *bridge.closed.lock().unwrap());
    }
}
