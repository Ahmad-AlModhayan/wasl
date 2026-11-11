//! [`LocalFs`] over the operating system's file system (desktop, Android All files access).

use crate::PART_SUFFIX;
use crate::local::{FsEntry, LocalFs, ReadablePath, check_rel, is_internal_name};
use filetime::FileTime;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A pair folder on a plain file system, with its recycle bin in `bin_dir`. Keep the bin on the
/// same volume as the pair so binning is a rename, not a copy.
pub struct StdFs {
    root: PathBuf,
    bin_dir: PathBuf,
}

/// Bin locations are unique in the process, not per [`StdFs`]: the engine opens a new one for each
/// operation, and two files with one name binned in the same millisecond must not share a location
/// (the second move would replace the first file in the bin).
static BIN_COUNTER: AtomicU64 = AtomicU64::new(0);

impl StdFs {
    pub fn new(root: impl Into<PathBuf>, bin_dir: impl Into<PathBuf>) -> StdFs {
        StdFs {
            root: root.into(),
            bin_dir: bin_dir.into(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The path for `rel`, refusing it when any existing component under the root is a symbolic
    /// link or junction: a link made after the scan (or one a Drive folder name lands on) must not
    /// let a read or write leave the pair folder.
    fn path(&self, rel: &str) -> io::Result<PathBuf> {
        let rel = check_rel(rel)?;
        let mut p = self.root.clone();
        let mut exists = true;
        for part in rel.split('/').filter(|s| !s.is_empty()) {
            p.push(part);
            if !exists {
                continue;
            }
            match fs::symlink_metadata(&p) {
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{rel:?} goes through a symbolic link"),
                    ));
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => exists = false,
                Err(e) => return Err(e),
            }
        }
        Ok(p)
    }

    fn bin_path(&self, location: &str) -> io::Result<PathBuf> {
        // A bin location is one file name we made; anything else is refused.
        if location.is_empty() || location.contains(['/', '\\', '\0']) || location == "." || location == ".."
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("bad bin location {location:?}"),
            ));
        }
        Ok(self.bin_dir.join(location))
    }
}

fn mtime_ms(meta: &fs::Metadata) -> i64 {
    let t = FileTime::from_last_modification_time(meta);
    t.unix_seconds() * 1000 + i64::from(t.nanoseconds() / 1_000_000)
}

fn entry(name: String, meta: &fs::Metadata) -> FsEntry {
    let is_dir = meta.is_dir();
    FsEntry {
        name,
        is_dir,
        size: if is_dir { 0 } else { meta.len() },
        mtime_ms: mtime_ms(meta),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

/// True when `p`'s folder lists an entry spelled exactly as `p`'s last part.
fn listed_exactly(p: &Path) -> bool {
    let (Some(dir), Some(name)) = (p.parent(), p.file_name()) else {
        return false;
    };
    fs::read_dir(dir).is_ok_and(|rd| rd.filter_map(|e| e.ok()).any(|e| e.file_name() == name))
}

/// Moves `from` to `to`, copying when they are on different volumes.
fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(from, to)?;
            fs::remove_file(from)
        }
    }
}

impl LocalFs for StdFs {
    fn list(&self, dir: &str) -> io::Result<Vec<FsEntry>> {
        let mut out = Vec::new();
        for item in fs::read_dir(self.path(dir)?)? {
            let item = item?;
            let Ok(name) = item.file_name().into_string() else {
                continue;
            }; // not UTF-8: cannot be synced
            if is_internal_name(&name) {
                continue;
            }
            // Symlinks are not followed: a link could lead outside the pair folder.
            let ft = item.file_type()?;
            if ft.is_symlink() {
                continue;
            }
            let meta = item.metadata()?;
            if meta.is_dir() || meta.is_file() {
                out.push(entry(name, &meta));
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn stat(&self, rel: &str) -> io::Result<Option<FsEntry>> {
        // A link (at any level) counts as nothing there: it is never synced.
        let p = match self.path(rel) {
            Ok(p) => p,
            Err(e) if e.kind() == io::ErrorKind::InvalidInput && check_rel(rel).is_ok() => return Ok(None),
            Err(e) => return Err(e),
        };
        match fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => Ok(None),
            Ok(m) => Ok(Some(entry(rel.rsplit('/').next().unwrap_or(rel).to_owned(), &m))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn open_read(&self, rel: &str) -> io::Result<Box<dyn Read + Send>> {
        Ok(Box::new(fs::File::open(self.path(rel)?)?))
    }

    fn readable_path(&self, rel: &str) -> io::Result<ReadablePath> {
        Ok(ReadablePath::plain(self.path(rel)?))
    }

    fn staging_path(&self, rel: &str) -> io::Result<PathBuf> {
        let target = self.path(rel)?;
        let mut name = target.file_name().unwrap_or_default().to_os_string();
        name.push(PART_SUFFIX);
        Ok(target.with_file_name(name))
    }

    fn install(&self, staged: &Path, rel: &str, mtime_ms: Option<i64>) -> io::Result<FsEntry> {
        let target = self.path(rel)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Some(ms) = mtime_ms.filter(|ms| *ms > 0) {
            filetime::set_file_mtime(
                staged,
                FileTime::from_unix_time(ms.div_euclid(1000), (ms.rem_euclid(1000) * 1_000_000) as u32),
            )?;
        }
        // Same folder: an atomic rename, so a crash leaves either the old state or the new file.
        move_file(staged, &target)?;
        let meta = fs::metadata(&target)?;
        Ok(entry(rel.rsplit('/').next().unwrap_or(rel).to_owned(), &meta))
    }

    fn mkdir(&self, rel: &str) -> io::Result<()> {
        fs::create_dir_all(self.path(rel)?)
    }

    fn remove_file(&self, rel: &str) -> io::Result<()> {
        if rel.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the pair root is not a file",
            ));
        }
        fs::remove_file(self.path(rel)?)
    }

    fn remove_dir_if_empty(&self, rel: &str) -> io::Result<bool> {
        let p = self.path(rel)?;
        if rel.is_empty() {
            return Ok(false); // never the pair root
        }
        let entries: Vec<_> = match fs::read_dir(&p) {
            Ok(it) => it.collect::<io::Result<_>>()?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        let leftovers: Vec<_> = entries
            .iter()
            .filter(|e| !is_internal_name(&e.file_name().to_string_lossy()))
            .collect();
        if !leftovers.is_empty() {
            return Ok(false);
        }
        for e in &entries {
            let _ = fs::remove_file(e.path());
        }
        fs::remove_dir(&p)?;
        Ok(true)
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        // `to` may name `from` itself on a folder that folds case or accents (`Foo.txt` →
        // `foo.txt` on Windows or macOS): a rename in place, only when it is that same file.
        let in_place =
            from != to && wasl_core::identity::same_name(from, to) && self.is_same_file(from, to)?;
        let (from, to) = (self.path(from)?, self.path(to)?);
        if to.exists() && !in_place {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} exists", to.display()),
            ));
        }
        fs::rename(from, to)
    }

    fn is_same_file(&self, a: &str, b: &str) -> io::Result<bool> {
        let (pa, pb) = (self.path(a)?, self.path(b)?);
        if !pa.exists() || !pb.exists() {
            return Ok(false);
        }
        // Some network drives (WebDAV, some NAS) report every file as the same file: two names the
        // folder lists exactly are two entries, whatever the identity says.
        if a != b && listed_exactly(&pa) && listed_exactly(&pb) {
            return Ok(false);
        }
        same_file::is_same_file(pa, pb)
    }

    fn move_to_bin(&self, rel: &str) -> io::Result<String> {
        let src = self.path(rel)?;
        if rel.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the pair root cannot be binned",
            ));
        }
        fs::create_dir_all(&self.bin_dir)?;
        let name = rel.rsplit('/').next().unwrap_or(rel);
        // Never over a binned file, even one another process put there.
        loop {
            let location = format!(
                "{}_{}_{name}",
                now_ms(),
                BIN_COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let target = self.bin_path(&location)?;
            if fs::symlink_metadata(&target).is_err() {
                move_file(&src, &target)?;
                return Ok(location);
            }
        }
    }

    fn restore_from_bin(&self, location: &str, rel: &str) -> io::Result<()> {
        let target = self.path(rel)?;
        if target.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{rel} exists"),
            ));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        move_file(&self.bin_path(location)?, &target)
    }

    fn purge_from_bin(&self, location: &str) -> io::Result<()> {
        match fs::remove_file(self.bin_path(location)?) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}
