//! When pairs run without anyone asking (SRS FR-B1, FR-C2): on their interval, and soon after a
//! change in their folder. Folder events are hints only; the scheduled run's full scan is the
//! source of truth, so a missed event costs at most one interval.
//!
//! Events are kept per path until a run that began after them, and finished, takes them. A change
//! while a run is in flight may come after that run's scan, so it stays and runs the pair once more
//! when the run ends, debounced like any change (issue #50). Before a change runs the pair, the
//! paths that are as the last sync left them are dropped (`Engine::as_last_synced`): a run's own
//! downloads pass, and an edit that changes a file's size or modification time does not, even to a
//! file the run just wrote (issue #88). A file modified within 2 s of the last run always counts,
//! since a coarse clock can give a same-size edit the time the run recorded. An event that names no
//! path in the folder (a rescan, an overflow, a watcher error) always counts, and so does a flood
//! of more than `MAX_PATHS` paths.
//!
//! A run that is skipped (its conditions, device sync off), fails, is blocked or is cancelled takes
//! nothing, and the pair waits `Timing::skip_wait` before it tries again. A partial run takes what
//! was seen before it and also waits, so its failed items are not retried on every tick.

use crate::daemon::Daemon;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wasl_core::RunStatus;
use wasl_engine::Trigger;
use wasl_store::Pair;

/// Global interval when a pair says "follow the global setting" (interval 0).
pub const DEFAULT_INTERVAL_MIN: u32 = 15;
/// How long a folder must be quiet after a change before the pair runs.
pub const SETTLE: Duration = Duration::from_secs(5);
/// How long a pair whose run was skipped waits before it tries again.
pub const SKIP_WAIT: Duration = Duration::from_secs(60);
/// The most paths kept per pair; past it the events count as one that names no path.
pub const MAX_PATHS: usize = 10_000;

#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub tick: Duration,
    pub settle: Duration,
    pub skip_wait: Duration,
    pub bin_purge_every: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            tick: Duration::from_secs(2),
            settle: SETTLE,
            skip_wait: SKIP_WAIT,
            bin_purge_every: Duration::from_secs(6 * 3600),
        }
    }
}

/// The pair's interval in minutes; 0 means the global setting `interval_minutes`.
pub fn interval_minutes(pair: &Pair, global: Option<u32>) -> u32 {
    if pair.interval_minutes > 0 {
        pair.interval_minutes
    } else {
        global.filter(|m| *m > 0).unwrap_or(DEFAULT_INTERVAL_MIN)
    }
}

/// A pair runs without being asked only once it is on, not paused, and its first plan was seen
/// (a new pair is held for the preview).
pub fn runs_on_its_own(pair: &Pair) -> bool {
    pair.enabled && !pair.paused && !pair.status.hold_for_preview
}

/// Whether a pair's scheduled run is due at `now_ms`.
pub fn due(pair: &Pair, global: Option<u32>, now_ms: i64) -> bool {
    if !runs_on_its_own(pair) {
        return false;
    }
    match pair.status.last_run_at {
        None => true,
        Some(last) => now_ms - last >= i64::from(interval_minutes(pair, global)) * 60_000,
    }
}

struct Watching {
    root: PathBuf,
    _watcher: RecommendedWatcher,
}

/// One pair's folder events since a run took them: the paths they named, relative to the pair's
/// folder with `/`, and an event that named none (a rescan, an overflow), each with when it was
/// last seen.
#[derive(Debug, Default)]
struct Seen {
    paths: HashMap<String, Instant>,
    unnamed: Option<Instant>,
}

impl Seen {
    fn latest(&self) -> Option<Instant> {
        self.paths.values().copied().chain(self.unnamed).max()
    }

    /// A run that began at `start` scanned the folder, so it took what was seen before then.
    fn taken_by_run(&mut self, start: Instant) {
        self.paths.retain(|_, t| *t >= start);
        self.unnamed = self.unnamed.filter(|t| *t >= start);
    }

    /// Notes an event seen at `now`: the paths it named, or `None` for one that named none.
    fn note(&mut self, named: Option<Vec<String>>, now: Instant) {
        match named {
            Some(rels) => self.paths.extend(rels.into_iter().map(|r| (r, now))),
            None => self.unnamed = Some(now),
        }
        if self.paths.len() > MAX_PATHS {
            self.paths.clear();
            self.unnamed = Some(now);
        }
    }

    /// Drops the `synced` paths, each found as the last sync left it when seen at its time here.
    /// A path seen again since then stays.
    fn drop_synced(&mut self, synced: &[(String, Instant)]) {
        for (rel, t) in synced {
            if self.paths.get(rel) == Some(t) {
                self.paths.remove(rel);
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.unnamed.is_none()
    }
}

type Changes = Arc<Mutex<HashMap<String, Seen>>>;

/// Runs the schedule until the daemon shuts down.
pub async fn run(daemon: Arc<Daemon>, timing: Timing) {
    let changed: Changes = Arc::new(Mutex::new(HashMap::new()));
    // Pairs whose last run was skipped, and when they may try again.
    let retry_at: Arc<Mutex<HashMap<String, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
    // Pairs this loop started a run for, until that run's bookkeeping is done.
    let started: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let mut watchers: HashMap<String, Watching> = HashMap::new();
    let mut last_purge = Instant::now();

    loop {
        tokio::time::sleep(timing.tick).await;
        let Ok(pairs) = daemon.pairs() else { continue };
        let global: Option<u32> = daemon
            .store
            .lock()
            .setting("interval_minutes")
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok());

        // Watch exactly the active pairs that sync on change.
        let wanted: HashMap<&str, &Pair> = pairs
            .iter()
            .filter(|p| runs_on_its_own(p) && p.instant_sync)
            .map(|p| (p.id.as_str(), p))
            .collect();
        watchers.retain(|id, w| {
            wanted
                .get(id.as_str())
                .is_some_and(|p| w.root.as_path() == Path::new(&p.local_root))
        });
        // A pair that stopped syncing on change (paused, instant sync off) leaves its changes to
        // its interval; a moved folder starts afresh.
        lock(&changed).retain(|id, _| watchers.contains_key(id));
        for (id, p) in &wanted {
            if watchers.contains_key(*id) {
                continue;
            }
            if let Some(w) = watch(id, &p.local_root, changed.clone()) {
                watchers.insert((*id).to_owned(), w);
            }
        }

        let now = Instant::now();
        let now_ms = {
            use wasl_engine::Clock as _;
            wasl_engine::SystemClock.now_ms()
        };
        for p in &pairs {
            if daemon.engine.is_running(&p.id)
                || lock(&started).contains(&p.id)
                || lock(&retry_at).get(&p.id).is_some_and(|t| now < *t)
            {
                continue;
            }
            let trigger = if due(p, global, now_ms) {
                Some(Trigger::Schedule)
            } else if wanted.contains_key(p.id.as_str())
                && settled(&daemon, &changed, &p.id, now, timing.settle)
            {
                Some(Trigger::Instant)
            } else {
                None
            };
            if let Some(trigger) = trigger {
                lock(&started).insert(p.id.clone());
                let d = daemon.clone();
                let id = p.id.clone();
                let (ch, retry) = (changed.clone(), retry_at.clone());
                let skip_wait = timing.skip_wait;
                let _started = Started(started.clone(), id.clone());
                tokio::spawn(async move {
                    let _started = _started;
                    let outcome = d.run(&id, trigger).await;
                    let wait = after_run(lock(&ch).get_mut(&id), &outcome, now);
                    let mut waiting = lock(&retry);
                    if wait {
                        waiting.insert(id, Instant::now() + skip_wait);
                    } else {
                        waiting.remove(&id);
                    }
                });
            }
        }

        if now.duration_since(last_purge) >= timing.bin_purge_every {
            last_purge = now;
            let _ = daemon.engine.purge_expired_bin().await;
        }
    }
}

/// Books a run's outcome and says whether the pair waits before trying again. A run that finished
/// (in full or in part) took what was seen before it began at `start`; a partial one waits. A run
/// that failed, was blocked or cancelled, or was skipped took nothing and waits, unless the skip
/// was for a run already in flight, which is followed like any run.
fn after_run(seen: Option<&mut Seen>, outcome: &wasl_engine::RunOutcome, start: Instant) -> bool {
    match outcome.status {
        RunStatus::Success | RunStatus::Partial => {
            if let Some(seen) = seen {
                seen.taken_by_run(start);
            }
            outcome.status == RunStatus::Partial
        }
        RunStatus::Skipped => outcome.message.as_deref() != Some(wasl_engine::ALREADY_RUNNING),
        _ => true,
    }
}

/// Whether the pair's folder changed and has been quiet for `settle` since. The paths as the last
/// sync left them go first; when nothing is left, there is nothing to run for. They are checked
/// off the lock, so the watcher's callbacks don't wait on the store and the disk.
fn settled(
    daemon: &Daemon,
    changed: &Mutex<HashMap<String, Seen>>,
    id: &str,
    now: Instant,
    settle: Duration,
) -> bool {
    let snapshot: Vec<(String, Instant)> = {
        let ch = lock(changed);
        let Some(seen) = ch.get(id) else {
            return false;
        };
        if seen
            .latest()
            .is_none_or(|t| now.saturating_duration_since(t) < settle)
        {
            return false;
        }
        if seen.unnamed.is_some() {
            return true;
        }
        seen.paths.iter().map(|(r, t)| (r.clone(), *t)).collect()
    };
    let synced: Vec<(String, Instant)> = snapshot
        .into_iter()
        .filter(|(rel, _)| daemon.engine.as_last_synced(id, rel))
        .collect();
    let mut ch = lock(changed);
    let Some(seen) = ch.get_mut(id) else {
        return false;
    };
    seen.drop_synced(&synced);
    if seen.is_empty() {
        ch.remove(id);
        return false;
    }
    true
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Keeps a pair marked started until its run task ends, even if the run panics.
struct Started(Arc<Mutex<HashSet<String>>>, String);

impl Drop for Started {
    fn drop(&mut self) {
        lock(&self.0).remove(&self.1);
    }
}

/// `path` relative to the first of `roots` it is under, with `/`; `None` outside them.
fn rel_in(roots: &[PathBuf], path: &Path) -> Option<String> {
    let rest = roots.iter().find_map(|r| path.strip_prefix(r).ok())?;
    let parts: Option<Vec<&str>> = rest
        .components()
        .map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    Some(parts?.join("/"))
}

fn is_part(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(wasl_fs::PART_SUFFIX))
}

fn watch(pair_id: &str, root: &str, changed: Changes) -> Option<Watching> {
    let id = pair_id.to_owned();
    // Events can name the folder by its resolved path (macOS gives /private/var for /var).
    let mut roots = vec![PathBuf::from(root)];
    if let Ok(real) = std::fs::canonicalize(root)
        && real != roots[0]
    {
        roots.push(real);
    }
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        // A watcher error may have lost events: it counts as a rescan.
        let named: Option<Vec<String>> = match res {
            Err(_) => None,
            Ok(ev) => {
                if matches!(ev.kind, notify::EventKind::Access(_)) {
                    return;
                }
                // Wasl's own partial downloads are not changes.
                if !ev.paths.is_empty() && ev.paths.iter().all(|p| is_part(p)) {
                    return;
                }
                if ev.need_rescan() || ev.paths.is_empty() {
                    None
                } else {
                    ev.paths
                        .iter()
                        .filter(|p| !is_part(p))
                        .map(|p| rel_in(&roots, p))
                        .collect()
                }
            }
        };
        let now = Instant::now();
        let mut ch = changed.lock().unwrap_or_else(|e| e.into_inner());
        ch.entry(id.clone()).or_default().note(named, now);
    })
    .ok()?;
    watcher.watch(Path::new(root), RecursiveMode::Recursive).ok()?;
    Some(Watching {
        root: PathBuf::from(root),
        _watcher: watcher,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(interval: u32, last: Option<i64>) -> Pair {
        let mut p = Pair::new("p", "P", "/tmp/p");
        p.interval_minutes = interval;
        p.status.last_run_at = last;
        p
    }

    #[test]
    fn a_pair_is_due_on_its_interval_or_the_global_one() {
        let now = 10_000_000;
        assert!(due(&pair(5, None), None, now), "never ran");
        assert!(!due(&pair(5, Some(now - 4 * 60_000)), None, now));
        assert!(due(&pair(5, Some(now - 5 * 60_000)), None, now));
        assert!(
            !due(&pair(0, Some(now - 14 * 60_000)), None, now),
            "global default 15"
        );
        assert!(due(&pair(0, Some(now - 30 * 60_000)), Some(30), now));
        let mut paused = pair(5, None);
        paused.paused = true;
        assert!(!due(&paused, None, now));
        let mut held = pair(5, None);
        held.status.hold_for_preview = true;
        assert!(!due(&held, None, now), "a new pair waits for its preview");
    }

    fn seen(paths: &[(&str, Instant)], unnamed: Option<Instant>) -> Seen {
        Seen {
            paths: paths.iter().map(|(p, t)| ((*p).to_owned(), *t)).collect(),
            unnamed,
        }
    }

    #[test]
    fn a_run_takes_what_was_seen_before_it_began_and_leaves_the_rest() {
        let start = Instant::now();
        let (before, during) = (start - Duration::from_secs(1), start + Duration::from_secs(1));
        let mut s = seen(&[("a.txt", before), ("b.txt", during)], Some(before));
        s.taken_by_run(start);
        assert_eq!(vec!["b.txt"], s.paths.keys().collect::<Vec<_>>());
        assert_eq!(None, s.unnamed);
        assert_eq!(Some(during), s.latest());
        s.taken_by_run(during + Duration::from_secs(1));
        assert!(
            s.is_empty(),
            "the follow-up run takes the change made during the first"
        );
    }

    #[test]
    fn a_runs_own_writes_are_dropped_and_a_persons_edits_are_kept() {
        let t = Instant::now();
        let mut s = seen(&[("downloaded.txt", t), ("edited.txt", t)], None);
        s.drop_synced(&[("downloaded.txt".to_owned(), t)]);
        assert_eq!(vec!["edited.txt"], s.paths.keys().collect::<Vec<_>>());
        s.drop_synced(&[("edited.txt".to_owned(), t)]);
        assert!(s.is_empty());
    }

    #[test]
    fn a_path_seen_again_while_it_was_checked_is_kept() {
        let t = Instant::now();
        let later = t + Duration::from_millis(10);
        let mut s = seen(&[("a.txt", later)], None);
        s.drop_synced(&[("a.txt".to_owned(), t)]);
        assert_eq!(vec!["a.txt"], s.paths.keys().collect::<Vec<_>>());
    }

    #[test]
    fn an_event_that_names_no_path_is_never_taken_for_an_own_write() {
        let t = Instant::now();
        let mut s = seen(&[("downloaded.txt", t)], Some(t));
        s.drop_synced(&[("downloaded.txt".to_owned(), t)]);
        assert!(!s.is_empty(), "a rescan still runs the pair");
        assert_eq!(Some(t), s.latest());
    }

    #[test]
    fn a_flood_of_paths_counts_as_one_event_that_names_none() {
        let t = Instant::now();
        let mut s = Seen::default();
        s.note(Some((0..MAX_PATHS).map(|i| format!("{i}.txt")).collect()), t);
        assert_eq!(MAX_PATHS, s.paths.len());
        assert_eq!(None, s.unnamed);
        let later = t + Duration::from_millis(10);
        s.note(Some(vec!["one-more.txt".to_owned()]), later);
        assert!(s.paths.is_empty(), "the paths are let go");
        assert_eq!(Some(later), s.unnamed);
        s.note(None, later);
        assert_eq!(Some(later), s.latest());
    }

    fn outcome(status: RunStatus, message: Option<&str>) -> wasl_engine::RunOutcome {
        wasl_engine::RunOutcome {
            run_id: String::new(),
            status,
            message: message.map(str::to_owned),
            summary: None,
            error_kind: None,
            blocker: None,
        }
    }

    #[test]
    fn a_skipped_run_takes_no_changes_and_the_pair_waits_before_trying_again() {
        let start = Instant::now();
        let mut s = seen(&[("a.txt", start - Duration::from_secs(1))], None);
        let skipped = outcome(RunStatus::Skipped, Some("Waiting for the pair's conditions"));
        assert!(after_run(Some(&mut s), &skipped, start), "the pair waits");
        assert!(!s.is_empty(), "the change waits for a run that happens");
        let done = outcome(RunStatus::Success, None);
        assert!(!after_run(Some(&mut s), &done, start));
        assert!(s.is_empty(), "the run took it");
        assert!(!after_run(None, &done, start));
    }

    #[test]
    fn a_run_that_did_not_finish_takes_no_changes_and_the_pair_waits() {
        let start = Instant::now();
        for status in [RunStatus::Failed, RunStatus::Blocked, RunStatus::Cancelled] {
            let mut s = seen(&[("a.txt", start - Duration::from_secs(1))], None);
            assert!(
                after_run(Some(&mut s), &outcome(status, None), start),
                "{status:?}"
            );
            assert!(
                !s.is_empty(),
                "{status:?} keeps the change for a run that finishes"
            );
        }
    }

    #[test]
    fn a_partial_run_takes_the_changes_and_the_pair_waits() {
        let start = Instant::now();
        let mut s = seen(&[("a.txt", start - Duration::from_secs(1))], None);
        let partial = outcome(RunStatus::Partial, Some("1 item failed"));
        assert!(after_run(Some(&mut s), &partial, start), "no retry on every tick");
        assert!(s.is_empty());
    }

    #[test]
    fn a_skip_for_a_run_already_in_flight_does_not_make_the_pair_wait() {
        let start = Instant::now();
        let mut s = seen(&[("a.txt", start - Duration::from_secs(1))], None);
        let busy = outcome(RunStatus::Skipped, Some(wasl_engine::ALREADY_RUNNING));
        assert!(!after_run(Some(&mut s), &busy, start));
        assert!(!s.is_empty(), "the run in flight is followed like any run");
    }

    #[test]
    fn event_paths_are_made_relative_to_the_folder_or_its_resolved_path() {
        let roots = [PathBuf::from("/var/x/Sync"), PathBuf::from("/private/var/x/Sync")];
        assert_eq!(
            Some("a/b.txt".to_owned()),
            rel_in(&roots, Path::new("/private/var/x/Sync/a/b.txt"))
        );
        assert_eq!(
            Some("c.txt".to_owned()),
            rel_in(&roots, Path::new("/var/x/Sync/c.txt"))
        );
        assert_eq!(Some(String::new()), rel_in(&roots, Path::new("/var/x/Sync")));
        assert_eq!(None, rel_in(&roots, Path::new("/var/x/Other/c.txt")));
    }
}
