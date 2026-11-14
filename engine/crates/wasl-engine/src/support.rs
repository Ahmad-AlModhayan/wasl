//! Small shared pieces: the clock, ids, cancellation, live progress and the per-run hash cache.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;
use wasl_core::LocalEntry;
use wasl_fs::{ContentHash, LocalFs};

/// Time for runs, bin expiry and conflict-copy names. Tests use a fixed clock.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;

    /// The stamp in conflict-copy names, e.g. `2026-09-30 14-03` (no `:` so it is valid everywhere).
    fn stamp(&self) -> String {
        stamp_utc(self.now_ms())
    }
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64)
    }

    /// In the device's own time zone: a copy made at 23:37 is named 23-37, not 20-37 (E2E 2026-10).
    fn stamp(&self) -> String {
        stamp_local(self.now_ms())
    }
}

/// `YYYY-MM-DD HH-MM` in this device's time zone; UTC when the zone can't be read.
pub fn stamp_local(ms: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms).single() {
        Some(t) => t.format("%Y-%m-%d %H-%M").to_string(),
        None => stamp_utc(ms),
    }
}

/// `YYYY-MM-DD HH-MM` in UTC for epoch milliseconds.
pub fn stamp_utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}-{:02}", tod / 3600, (tod % 3600) / 60)
}

/// Unique ids for runs, bin entries and conflicts: time plus a process-wide counter.
pub fn new_id(clock: &dyn Clock) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{:x}-{:x}",
        clock.now_ms(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// A run's stop button.
#[derive(Default)]
pub struct CancelToken {
    flag: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Resolves once [`CancelToken::cancel`] is called.
    pub async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

pub use wasl_proto::engine::{Phase, Progress};

/// Receives progress snapshots. Called often; implementations should be cheap.
pub type ProgressListener = Arc<dyn Fn(&Progress) + Send + Sync>;

pub(crate) struct Tracker {
    state: Mutex<Progress>,
    listener: Option<ProgressListener>,
    /// Bumped on every change; the watchdog compares it to see whether the run moves.
    pub(crate) ticks: AtomicU64,
}

impl Tracker {
    pub(crate) fn new(pair_id: &str, run_id: &str, listener: Option<ProgressListener>) -> Tracker {
        Tracker {
            state: Mutex::new(Progress {
                pair_id: pair_id.to_owned(),
                run_id: run_id.to_owned(),
                phase: Phase::Starting,
                total_actions: 0,
                done_actions: 0,
                bytes_total: 0,
                bytes_done: 0,
                checked: 0,
                errors: 0,
                current_file: None,
            }),
            listener,
            ticks: AtomicU64::new(0),
        }
    }

    pub(crate) fn update(&self, f: impl FnOnce(&mut Progress)) {
        let snapshot = {
            let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            f(&mut s);
            s.clone()
        };
        self.ticks.fetch_add(1, Ordering::Relaxed);
        if let Some(l) = &self.listener {
            l(&snapshot);
        }
    }

    pub(crate) fn snapshot(&self) -> Progress {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// Memo so a file is hashed at most once even when the planner and the executor both need it, and
/// across the runs of one pair (a preview and the sync started from it). A file whose size or time
/// changed since is hashed again.
#[derive(Default)]
pub(crate) struct HashCache {
    map: Mutex<HashMap<String, (u64, i64, ContentHash)>>,
}

/// What [`HashCache::md5_all_blocking`] found: `(rel, md5)` per file read, `(rel, why)` per file not.
#[derive(Debug)]
pub(crate) struct Hashed {
    pub md5s: Vec<(String, String)>,
    pub unreadable: Vec<(String, std::io::Error)>,
}

/// The hash of the device file at `rel`; an error names the file and keeps its kind.
pub(crate) fn read_hash(fs: &dyn LocalFs, rel: &str) -> std::io::Result<ContentHash> {
    fs.open_read(rel)
        .and_then(wasl_fs::hash_reader)
        .map_err(|e| unreadable(rel, e))
}

/// `e` from reading the device file at `rel`, naming it and keeping the error's kind.
pub(crate) fn unreadable(rel: &str, e: std::io::Error) -> std::io::Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        // Moved or deleted since the scan listed it (#154): said plainly, not as a fault. The
        // device's own words when it has them (Android's are in the app's language).
        let plain = match e.raw_os_error() {
            Some(_) => format!("{rel} was moved or deleted during the sync. The next sync checks it again."),
            None => e.to_string(),
        };
        return std::io::Error::new(e.kind(), plain);
    }
    std::io::Error::new(e.kind(), format!("Couldn't read {rel} on the device: {e}"))
}

/// The pair's own folder is gone (a memory card taken out, the folder deleted or moved): every
/// file in it reads as moved or deleted, which must fail the run, not pass as skips (#154 review).
pub(crate) fn root_gone(fs: &dyn LocalFs) -> Option<std::io::Error> {
    fs.list("").err().map(|e| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("The pair's folder on this device isn't there any more ({e}). If it is on a memory card, put the card back; otherwise choose the folder again in the pair's settings"),
        )
    })
}

/// Files hashed at once: opening a file on a phone's shared storage costs milliseconds that
/// overlap well, and hashing a big file keeps one core busy.
pub(crate) const HASH_THREADS: usize = 4;

impl HashCache {
    /// MD5s of `entries`, hashed [`HASH_THREADS`] at a time, with `done` called after each file,
    /// and the files that couldn't be read with why (one unreadable file doesn't stop the others,
    /// #127). `stop`, checked before each file, stops the rest with an `Interrupted` error.
    pub(crate) fn md5_all_blocking(
        &self,
        fs: &Arc<dyn LocalFs>,
        entries: &[&LocalEntry],
        done: &(dyn Fn() + Sync),
        stop: &(dyn Fn() -> bool + Sync),
    ) -> std::io::Result<Hashed> {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let failed = Mutex::new(None);
        let out = Mutex::new(Vec::with_capacity(entries.len()));
        let unreadable = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            for _ in 0..HASH_THREADS.min(entries.len()) {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(e) = entries.get(i) else { break };
                        if failed.lock().unwrap_or_else(|p| p.into_inner()).is_some() {
                            break;
                        }
                        if stop() {
                            failed.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert(
                                std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"),
                            );
                            break;
                        }
                        match self.hash_blocking(fs, e) {
                            Ok(h) => {
                                out.lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .push((e.rel.clone(), h.md5));
                                done();
                            }
                            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {
                                failed
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .get_or_insert(err);
                                break;
                            }
                            Err(err) => {
                                unreadable
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .push((e.rel.clone(), err));
                                done();
                            }
                        }
                    }
                });
            }
        });
        match failed.into_inner().unwrap_or_else(|p| p.into_inner()) {
            Some(e) => Err(e),
            None => Ok(Hashed {
                md5s: out.into_inner().unwrap_or_else(|p| p.into_inner()),
                unreadable: unreadable.into_inner().unwrap_or_else(|p| p.into_inner()),
            }),
        }
    }

    /// [`HashCache::hash`] for code already off the async threads (the planner).
    pub(crate) fn hash_blocking(
        &self,
        fs: &Arc<dyn LocalFs>,
        e: &LocalEntry,
    ) -> std::io::Result<ContentHash> {
        if let Some(h) = self.cached(e) {
            return Ok(h);
        }
        let h = read_hash(fs.as_ref(), &e.rel)?;
        self.map
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(e.rel.clone(), (e.size, e.mtime, h.clone()));
        Ok(h)
    }

    fn cached(&self, e: &LocalEntry) -> Option<ContentHash> {
        let map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        map.get(&e.rel)
            .filter(|(size, mtime, _)| *size == e.size && *mtime == e.mtime)
            .map(|(_, _, h)| h.clone())
    }

    pub(crate) async fn hash(&self, fs: &Arc<dyn LocalFs>, e: &LocalEntry) -> std::io::Result<ContentHash> {
        if let Some(h) = self.cached(e) {
            return Ok(h);
        }
        let fs = fs.clone();
        let rel = e.rel.clone();
        let h = tokio::task::spawn_blocking(move || read_hash(fs.as_ref(), &rel))
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))??;
        self.map
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(e.rel.clone(), (e.size, e.mtime, h.clone()));
        Ok(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A first preview hashes every file in one call (5.3 GB for the owner's LifeOS): stopping
    /// must not wait for all of them.
    #[test]
    fn hashing_stops_between_files_once_asked() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("pair");
        std::fs::create_dir_all(&root).unwrap();
        let entries: Vec<LocalEntry> = (0..40)
            .map(|i| {
                let rel = format!("f{i}.txt");
                std::fs::write(root.join(&rel), format!("file {i}")).unwrap();
                LocalEntry {
                    rel,
                    is_dir: false,
                    size: 6,
                    mtime: 1,
                }
            })
            .collect();
        let fs: Arc<dyn LocalFs> = Arc::new(wasl_fs::StdFs::new(&root, dir.path().join("bin")));
        let wanted: Vec<&LocalEntry> = entries.iter().collect();
        let hashed = AtomicUsize::new(0);
        let done = || {
            hashed.fetch_add(1, Ordering::SeqCst);
        };
        let stop = || hashed.load(Ordering::SeqCst) >= 1;
        let r = HashCache::default().md5_all_blocking(&fs, &wanted, &done, &stop);
        assert_eq!(std::io::ErrorKind::Interrupted, r.unwrap_err().kind());
        // At most the files already being read when it was asked finish.
        assert!(hashed.load(Ordering::SeqCst) <= 1 + HASH_THREADS, "{hashed:?}");
    }
}
