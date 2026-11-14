//! The engine's front door (SRS FR-E4): preview, sync, cancel, approve, bin. Port of 1.1.4
//! `SyncEngine.kt`. Platforms provide file access, the remote, the device state and a clock through
//! [`Environment`]; every decision is made here (FR-E3).

use crate::error::{EngineError, Result};
use crate::executor::Executor;
use crate::remote::Remote;
use crate::support::{CancelToken, Clock, HashCache, Phase, Progress, ProgressListener, Tracker, new_id};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wasl_core::conditions::{Blocker, DeviceState, blocker};
use wasl_core::guards::GuardConfig;
use wasl_core::identity::{adopt_drive_identity, name_key, same_name, same_name_or_under};
use wasl_core::planner::{SIZE_LIMIT_SKIP, mapped_local_rel, over_size_limit};
use wasl_core::{
    ErrorKind, LocalEntry, PlanInput, PlanSummary, Planner, PlannerConfig, RemoteEntry, RunStatus, SyncPlan,
};
use wasl_fs::LocalFs;
use wasl_ignore::legacy::PathFilter;
use wasl_ignore::{IgnoreRules, Preset};
use wasl_store::{BinSide, EventLevel, Pair, PairStatus, RunEvent, RunStats, SharedStore};

/// What the platform provides.
pub trait Environment: Send + Sync {
    /// File access for a pair's local folder (a path, or an Android SAF tree).
    fn local_fs(&self, pair: &Pair) -> Result<Arc<dyn LocalFs>>;
    /// The remote side of a pair (Drive for its account).
    fn remote(&self, pair: &Pair) -> Result<Arc<dyn Remote>>;
    /// Network, power and battery right now.
    fn device_state(&self) -> DeviceState;
}

pub use wasl_proto::engine::{Preview, RunOutcome, Trigger};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EngineConfig {
    pub device_name: String,
    /// Parallel transfers per run (1..=8).
    pub concurrency: usize,
    /// A run without progress this long is abandoned and retried (FR-B4).
    pub stall_limit: Duration,
    pub watchdog_tick: Duration,
    /// Where earlier versions of files changed by agents are kept (docs/AGENT_CHANGES.md). Agent
    /// sessions are refused without it.
    #[serde(default)]
    pub agent_dir: Option<std::path::PathBuf>,
    /// The agent undo store (docs/AGENT_UNDO.md): snapshots of watched folders. Outside every
    /// watched folder; `None` turns snapshots off.
    #[serde(default)]
    pub undo_dir: Option<std::path::PathBuf>,
    /// This device's id (`Devices::id()`, `WASL-XXXX-…`), handed to the store so agent changes
    /// are written to the op-log under it (ADR-0002). `None` (no device key): agent changes keep
    /// only their working detail until a start with an id copies them in
    /// ([`Engine::copy_agent_changesets_to_oplog`]).
    #[serde(default)]
    pub device_id: Option<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            device_name: "this device".into(),
            concurrency: 3,
            stall_limit: Duration::from_secs(15 * 60),
            watchdog_tick: Duration::from_secs(30),
            agent_dir: None,
            undo_dir: None,
            device_id: None,
        }
    }
}
pub use wasl_proto::engine::{ALREADY_RUNNING, HOLD_MESSAGE, INTERRUPTED, SAVING_MESSAGE, STALLED, STOPPED};
/// How near a file's modification time may be to the pair's last run before `as_last_synced`
/// stops trusting it: the coarsest file clock Wasl meets (FAT's 2 s).
pub const RACY_MS: i64 = 2_000;

struct Active {
    cancel: Arc<CancelToken>,
    tracker: Arc<Tracker>,
}

/// Stops a preview and forgets it when it ends, also when its caller drops it: the app drops the
/// call when the preview screen is left, and the files still being read must stop with it.
struct PreviewGuard<'a> {
    engine: &'a Engine,
    pair_id: &'a str,
    previewing: Arc<Active>,
}

impl Drop for PreviewGuard<'_> {
    fn drop(&mut self) {
        self.previewing.cancel.cancel();
        let mut map = self.engine.previews.lock().unwrap_or_else(|p| p.into_inner());
        if map
            .get(self.pair_id)
            .is_some_and(|a| Arc::ptr_eq(a, &self.previewing))
        {
            map.remove(self.pair_id);
        }
    }
}

/// Frees the pair when its run ends, and closes the run when the caller drops it midway: Android
/// stops a sync worker by dropping its call (the owner's phone, 2026-10-29: a run stayed "Syncing"
/// for two hours and every later sync of the pair was skipped as already running).
struct RunGuard<'a> {
    engine: &'a Engine,
    pair: &'a Pair,
    run_id: &'a str,
    active: &'a Arc<Active>,
    finished: bool,
}

/// The watchdog ends with its run, also when the run is dropped.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            // Work still on a blocking thread sees the flag and stops.
            self.active.cancel.cancel();
            self.engine.close_run(
                self.pair,
                self.run_id,
                RunStatus::Cancelled,
                Some(INTERRUPTED),
                None,
                &RunStats::default(),
            );
        }
    }
}

/// A pair's place in `Engine::active`, held from before `sync` reads the pair until its run is
/// closed, and freed however `sync` ends (also when its call is dropped). While it is held,
/// `Engine::update_pair` won't point the pair at other files (#94).
struct Slot<'a> {
    engine: &'a Engine,
    pair_id: &'a str,
    active: &'a Arc<Active>,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut map = self.engine.active.lock().unwrap_or_else(|p| p.into_inner());
        if map.get(self.pair_id).is_some_and(|a| Arc::ptr_eq(a, self.active)) {
            map.remove(self.pair_id);
        }
    }
}

/// A pair `Engine::update_pair` is saving, taken under the `active` lock and cleared however the
/// save ends; a run doesn't start while it is held.
struct Saving<'a> {
    engine: &'a Engine,
    pair_id: &'a str,
}

impl Drop for Saving<'_> {
    fn drop(&mut self) {
        let mut map = self.engine.saving.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(n) = map.get_mut(self.pair_id) {
            *n -= 1;
            if *n == 0 {
                map.remove(self.pair_id);
            }
        }
    }
}

pub struct Engine {
    pub(crate) store: SharedStore,
    pub(crate) env: Arc<dyn Environment>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) config: EngineConfig,
    listener: Option<ProgressListener>,
    active: Mutex<HashMap<String, Arc<Active>>>,
    /// Pairs whose options are being saved, with how many saves: the save runs outside the
    /// `active` lock, so a status poll doesn't wait for it.
    saving: Mutex<HashMap<String, usize>>,
    /// Previews in progress, apart from runs: a preview is not a sync, but its screen shows which
    /// step it is on and leaving the screen stops it.
    previews: Mutex<HashMap<String, Arc<Active>>>,
    /// Each pair's file hashes, kept between runs: the sync started from a preview reuses the
    /// preview's hashes instead of reading every file again.
    hash_memos: Mutex<HashMap<String, Arc<HashCache>>>,
}

/// Everything a plan was built from.
pub(crate) struct Prepared {
    pub(crate) plan: SyncPlan,
    pub(crate) notes: Vec<String>,
    /// What the run did on its own that is not a problem (renames followed, shortcuts left out).
    pub(crate) infos: Vec<String>,
    pub(crate) local: Arc<dyn LocalFs>,
    pub(crate) remote: Arc<dyn Remote>,
    pub(crate) remote_ids: HashMap<String, String>,
    pub(crate) hashes: Arc<HashCache>,
    /// The trees the plan was made from (ignored and unreadable keys already removed).
    pub(crate) input: PlanInput,
    /// Device files left out of the plan because they couldn't be read, each with why: the run
    /// ends Partial, never a silent Success (#127).
    pub(crate) unreadable: Vec<String>,
}

/// The planner's result: the plan, how many files it hashed, the files it couldn't read and the
/// ones gone since the scan (each with why), and the trees it planned from.
type Planned = (SyncPlan, usize, Vec<String>, Vec<String>, PlanInput);

impl Engine {
    /// This device's name: the `device_name` setting, else the configured one.
    pub(crate) fn device_name(&self) -> Result<String> {
        Ok(self
            .store
            .lock()
            .setting("device_name")?
            .unwrap_or_else(|| self.config.device_name.clone()))
    }

    pub fn new(
        store: SharedStore,
        env: Arc<dyn Environment>,
        clock: Arc<dyn Clock>,
        config: EngineConfig,
        listener: Option<ProgressListener>,
    ) -> Engine {
        if let Some(id) = &config.device_id {
            store.lock().set_oplog_device(Some(id.clone()));
        }
        Engine {
            store,
            env,
            clock,
            config,
            listener,
            active: Mutex::new(HashMap::new()),
            saving: Mutex::new(HashMap::new()),
            previews: Mutex::new(HashMap::new()),
            hash_memos: Mutex::new(HashMap::new()),
        }
    }

    fn hash_memo(&self, pair_id: &str) -> Arc<HashCache> {
        self.hash_memos
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(pair_id.to_owned())
            .or_default()
            .clone()
    }

    pub fn store(&self) -> &SharedStore {
        &self.store
    }

    /// Closes runs a killed process left open. Call once at start-up.
    pub fn recover(&self) -> Result<usize> {
        Ok(self
            .store
            .lock()
            .close_stale_runs(INTERRUPTED, self.clock.now_ms())?)
    }

    pub fn is_running(&self, pair_id: &str) -> bool {
        self.active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(pair_id)
    }

    /// Whether the local path `rel` (local names, `/`-separated) is as the last sync left it: a
    /// file with the size and mtime the baseline holds, a folder the baseline holds, or nothing
    /// where the baseline has nothing. A run's own writes pass once the run has recorded them; an
    /// edit made after the run wrote the file does not (issue #88). A file modified within
    /// `RACY_MS` of the pair's last run counts as changed: a coarse file clock (FAT 2 s, HFS+ 1 s)
    /// can give an edit made just after the run the very time the run recorded (git's racy-clean
    /// rule). An edit that keeps both the size and an older time (`cp -p`, `touch -r`) is not seen.
    /// The pair's folder itself, and anything that can't be read, count as changed.
    pub fn as_last_synced(&self, pair_id: &str, rel: &str) -> bool {
        if rel.is_empty() {
            return false;
        }
        let Ok(pair) = self.pair(pair_id) else {
            return false;
        };
        let Ok(fs) = self.env.local_fs(&pair) else {
            return false;
        };
        let Ok(base) = self.store.lock().baseline_by_local_rel(pair_id, rel) else {
            return false;
        };
        match (fs.stat(rel), base) {
            (Ok(None), None) => true,
            (Ok(Some(d)), Some(b)) if d.is_dir => b.is_dir,
            (Ok(Some(d)), Some(b)) => {
                let racy = pair
                    .status
                    .last_run_at
                    .is_none_or(|run| d.mtime_ms > run - RACY_MS);
                !b.is_dir && d.size == b.local_size && d.mtime_ms == b.local_mtime && !racy
            }
            _ => false,
        }
    }

    /// Live progress of a pair's run.
    pub fn progress(&self, pair_id: &str) -> Option<Progress> {
        self.active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(pair_id)
            .map(|a| a.tracker.snapshot())
    }

    /// Stops a pair's run. Files done so far stay synced.
    pub fn cancel(&self, pair_id: &str) {
        if let Some(a) = self.active.lock().unwrap_or_else(|p| p.into_inner()).get(pair_id) {
            a.cancel.cancel();
        }
    }

    pub fn cancel_all(&self) {
        for a in self.active.lock().unwrap_or_else(|p| p.into_inner()).values() {
            a.cancel.cancel();
        }
        for a in self.previews.lock().unwrap_or_else(|p| p.into_inner()).values() {
            a.cancel.cancel();
        }
    }

    /// Live progress of a pair's preview: the step, and how many files were checked.
    pub fn preview_progress(&self, pair_id: &str) -> Option<Progress> {
        self.previews
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(pair_id)
            .map(|a| a.tracker.snapshot())
    }

    /// Saves a pair's options; every app saves through here. When they point the pair at other
    /// files, the store starts its sync state over and holds it for its preview
    /// (`Store::update_pair`); that move is refused while the pair runs, since the run would write
    /// the old files' state back after the reset. Under the lock a run takes before it reads its
    /// pair (`sync`), the save either finds the pair running or marks it as saving, which a run
    /// start refuses, so the two can't interleave. Returns whether the pair moved.
    pub fn update_pair(&self, pair: &Pair) -> Result<bool> {
        let reserved = {
            let running = self.active.lock().unwrap_or_else(|p| p.into_inner());
            let free = !running.contains_key(&pair.id);
            if free {
                *self
                    .saving
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .entry(pair.id.clone())
                    .or_default() += 1;
            }
            free.then_some(Saving {
                engine: self,
                pair_id: &pair.id,
            })
        };
        let s = self.store.lock();
        let moves = s
            .get_pair(&pair.id)?
            .is_some_and(|before| wasl_store::repointed(&before, pair));
        if moves && reserved.is_none() {
            return Err(EngineError::Other(format!(
                "{} is syncing. Stop it or let it finish, then change its folders.",
                pair.name
            )));
        }
        // Checked under the same store lock as the save, so no changeset can open in between
        // (security review M1, #134).
        if moves && s.pair_has_agent_changes(&pair.id)? {
            return Err(EngineError::Store(wasl_store::StoreError::Refused(
                wasl_store::AGENT_CHANGES_PENDING.into(),
            )));
        }
        if moves {
            // A preview of the old folders would show a plan that no longer applies.
            self.cancel_preview(&pair.id);
        }
        Ok(s.update_pair(pair)?)
    }

    /// Stops a pair's preview; it ends with [`EngineError::Cancelled`].
    pub fn cancel_preview(&self, pair_id: &str) {
        if let Some(a) = self
            .previews
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(pair_id)
        {
            a.cancel.cancel();
        }
    }

    /// The user approved the stopped plan (or started from the preview): the next run goes ahead
    /// once (FR-F2, FR-F3).
    pub fn approve_once(&self, pair_id: &str) -> Result<()> {
        let s = self.store.lock();
        let p = s
            .get_pair(pair_id)?
            .ok_or_else(|| EngineError::NotFound(format!("pair {pair_id}")))?;
        s.set_pair_status(
            pair_id,
            &PairStatus {
                approve_once: true,
                ..p.status
            },
        )?;
        Ok(())
    }

    pub(crate) fn pair(&self, pair_id: &str) -> Result<Pair> {
        self.store
            .lock()
            .get_pair(pair_id)?
            .ok_or_else(|| EngineError::NotFound(format!("pair {pair_id}")))
    }

    /// Plans without changing anything. Guards are evaluated as a scheduled run would. While it
    /// runs, [`Engine::preview_progress`] reports its step; dropping it stops its work.
    pub async fn preview(&self, pair_id: &str) -> Result<Preview> {
        // No listener: a preview's progress must not read as a running sync.
        let previewing = Arc::new(Active {
            cancel: Arc::new(CancelToken::default()),
            tracker: Arc::new(Tracker::new(pair_id, "", None)),
        });
        // A newer preview of the pair is the one reported; an earlier one still running (another
        // window, the CLI) carries on unreported.
        self.previews
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(pair_id.to_owned(), previewing.clone());
        let _guard = PreviewGuard {
            engine: self,
            pair_id,
            previewing: previewing.clone(),
        };
        // Read after the preview is listed: a folder change saved from here on cancels it
        // (`update_pair`), and one saved before is what it reads.
        let pair = self.pair(pair_id)?;
        let (tracker, cancel) = (&previewing.tracker, &previewing.cancel);
        // Like a run's Stop: ends the preview whatever step it is in (a Drive listing never looks
        // at the flag).
        let p = tokio::select! {
            r = self.prepare(&pair, pair.status.approve_once, tracker, cancel, false) => r?,
            () = cancel.cancelled() => return Err(EngineError::Cancelled),
        };
        let mut sizes = HashMap::new();
        for a in &p.plan.actions {
            let k = a.key();
            let size = match a {
                wasl_core::Action::DeleteLocal { is_dir: false, .. } => p.input.local.get(k).map(|l| l.size),
                wasl_core::Action::DeleteRemote { is_dir: false, .. } => {
                    p.input.remote.get(k).and_then(|r| r.size)
                }
                wasl_core::Action::Skip { .. } => p
                    .input
                    .local
                    .get(k)
                    .map(|l| l.size)
                    .or_else(|| p.input.remote.get(k).and_then(|r| r.size)),
                _ => None,
            };
            if let Some(s) = size {
                sizes.insert(k.to_owned(), s);
            }
        }
        Ok(Preview {
            plan: p.plan,
            notes: p.notes,
            infos: p.infos,
            sizes,
        })
    }

    /// Runs a pair once.
    pub async fn sync(&self, pair_id: &str, trigger: Trigger) -> RunOutcome {
        let skipped = |message: &str, blocker: Option<Blocker>| RunOutcome {
            run_id: String::new(),
            status: RunStatus::Skipped,
            message: Some(message.to_owned()),
            summary: None,
            error_kind: None,
            blocker,
        };
        // Take the pair's place before reading it: a folder change checks and commits under the
        // same lock (`update_pair`), so either it lands first and this run reads the moved pair
        // (held for its preview), or this run holds the place and the change is refused until it
        // ends. Reading first let a run sync the old folder and then clear the new hold (#94).
        let run_id = new_id(self.clock.as_ref());
        let active = Arc::new(Active {
            cancel: Arc::new(CancelToken::default()),
            tracker: Arc::new(Tracker::new(pair_id, &run_id, self.listener.clone())),
        });
        {
            let mut map = self.active.lock().unwrap_or_else(|p| p.into_inner());
            if map.contains_key(pair_id) {
                return skipped(ALREADY_RUNNING, None);
            }
            if self
                .saving
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains_key(pair_id)
            {
                return skipped(SAVING_MESSAGE, None);
            }
            map.insert(pair_id.to_owned(), active.clone());
        }
        let _slot = Slot {
            engine: self,
            pair_id,
            active: &active,
        };
        let mut pair = match self.pair(pair_id) {
            Ok(p) => p,
            Err(e) => return skipped(&e.to_string(), None),
        };
        if !pair.enabled {
            return skipped("Turned off", None);
        }
        if pair.paused && !trigger.manual() {
            return skipped(pair.pause_reason.as_deref().unwrap_or("Paused"), None);
        }
        if let Some(b) = blocker(&pair.conditions, &self.env.device_state(), trigger.manual()) {
            return skipped("Waiting for the pair's conditions", Some(b));
        }
        if trigger == Trigger::Preview && !pair.status.approve_once {
            // Starting from the preview is the approval the first sync waits for.
            // Only the approval is written: the rest of the status is not this copy's to write.
            pair.status.approve_once = true;
            if let Err(e) = self.store.lock().set_approve_once(pair_id) {
                return skipped(&e.to_string(), None);
            }
        }
        if pair.status.hold_for_preview && !pair.status.approve_once {
            return skipped(HOLD_MESSAGE, None);
        }

        let mut guard = RunGuard {
            engine: self,
            pair: &pair,
            run_id: &run_id,
            active: &active,
            finished: false,
        };
        let outcome = self.run(&pair, &run_id, trigger, &active).await;
        guard.finished = true;
        outcome
    }

    async fn run(&self, pair: &Pair, run_id: &str, trigger: Trigger, active: &Arc<Active>) -> RunOutcome {
        let started = self.clock.now_ms();
        if let Err(e) = self
            .store
            .lock()
            .start_run(run_id, &pair.id, trigger.as_str(), started)
        {
            return RunOutcome {
                run_id: String::new(),
                status: RunStatus::Failed,
                message: Some(e.to_string()),
                summary: None,
                error_kind: Some(ErrorKind::Io),
                blocker: None,
            };
        }

        // The watchdog abandons a run whose progress has not moved for the stall limit (FR-B4).
        let stalled = Arc::new(AtomicBool::new(false));
        let watchdog = {
            let tracker = active.tracker.clone();
            let cancel = active.cancel.clone();
            let stalled = stalled.clone();
            let (tick, limit) = (self.config.watchdog_tick, self.config.stall_limit);
            AbortOnDrop(tokio::spawn(async move {
                let mut last = tracker.ticks.load(Ordering::Relaxed);
                let mut quiet = Duration::ZERO;
                loop {
                    tokio::time::sleep(tick).await;
                    let now = tracker.ticks.load(Ordering::Relaxed);
                    if now != last {
                        last = now;
                        quiet = Duration::ZERO;
                        continue;
                    }
                    quiet += tick;
                    if quiet >= limit {
                        stalled.store(true, Ordering::SeqCst);
                        cancel.cancel();
                        return;
                    }
                }
            }))
        };

        // Stop, Pause and the watchdog end the run whatever step it is in: a step that never looks
        // at the flag (a Drive call stuck past its timeouts) is dropped, and with it the request
        // (E2E 2026-10: a phone run stayed "Syncing" for 40 minutes). Nothing is lost: each file is
        // recorded as it finishes, as when the process is killed.
        let stop = active.cancel.clone();
        let inner = async {
            tokio::select! {
                r = self.run_inner(pair, run_id, active) => r,
                () = stop.cancelled() => Err(EngineError::Cancelled),
            }
        };
        let (status, message, summary, kind, stats) = match inner.await {
            Ok(r) => r,
            Err(EngineError::Cancelled) if stalled.load(Ordering::SeqCst) => (
                RunStatus::Failed,
                Some(STALLED.to_owned()),
                None,
                Some(ErrorKind::Network),
                RunStats::default(),
            ),
            Err(EngineError::Cancelled) => (
                RunStatus::Cancelled,
                Some(STOPPED.to_owned()),
                None,
                None,
                RunStats::default(),
            ),
            Err(e) => {
                let kind = e.kind();
                self.log(run_id, EventLevel::Error, "ERROR", &e.to_string());
                if kind == ErrorKind::Auth
                    && let Some(a) = &pair.account_id
                {
                    let _ = self.store.lock().set_account_auth_error(a, Some(&e.to_string()));
                }
                (
                    RunStatus::Failed,
                    Some(e.to_string()),
                    None,
                    Some(kind),
                    RunStats::default(),
                )
            }
        };
        drop(watchdog);
        let (status, message, kind) = if stalled.load(Ordering::SeqCst) && status == RunStatus::Cancelled {
            (
                RunStatus::Failed,
                Some(STALLED.to_owned()),
                Some(ErrorKind::Network),
            )
        } else {
            (status, message, kind)
        };
        self.close_run(pair, run_id, status, message.as_deref(), kind, &stats);
        RunOutcome {
            run_id: run_id.to_owned(),
            status,
            message,
            summary,
            error_kind: kind,
            blocker: None,
        }
    }

    #[allow(clippy::type_complexity)]
    async fn run_inner(
        &self,
        pair: &Pair,
        run_id: &str,
        active: &Arc<Active>,
    ) -> Result<(
        RunStatus,
        Option<String>,
        Option<PlanSummary>,
        Option<ErrorKind>,
        RunStats,
    )> {
        let approve = pair.status.approve_once;
        let prepared = self
            .prepare(pair, approve, &active.tracker, &active.cancel, true)
            .await?;
        let summary = prepared.plan.summary.clone();
        for n in &prepared.notes {
            self.log(run_id, EventLevel::Warn, "INFO", n);
        }
        // Not problems: kept out of the Problems list.
        for n in &prepared.infos {
            self.log(run_id, EventLevel::Info, "INFO", n);
        }

        if prepared.plan.blocked {
            let reason = summary
                .blocked_reason
                .clone()
                .unwrap_or_else(|| "Stopped to ask before a risky change".into());
            {
                let s = self.store.lock();
                s.put_blocked_plan(
                    &pair.id,
                    &reason,
                    &serde_json::to_string(&summary).unwrap_or_default(),
                    self.clock.now_ms(),
                )?;
                let mut st = s.get_pair(&pair.id)?.map(|p| p.status).unwrap_or_default();
                st.pending_approval = true;
                s.set_pair_status(&pair.id, &st)?;
            }
            self.log(run_id, EventLevel::Warn, "BLOCKED", &reason);
            return Ok((
                RunStatus::Blocked,
                Some(reason),
                Some(summary),
                None,
                RunStats::default(),
            ));
        }
        {
            // The plan may run: an approval is used up, and a stop that no longer applies clears itself.
            let s = self.store.lock();
            if let Some(mut st) = s.get_pair(&pair.id)?.map(|p| p.status)
                && (st.approve_once || st.pending_approval || st.hold_for_preview)
            {
                st.approve_once = false;
                st.pending_approval = false;
                st.hold_for_preview = false;
                s.set_pair_status(&pair.id, &st)?;
                s.delete_blocked_plan(&pair.id)?;
            }
        }

        let concurrency = self
            .store
            .lock()
            .setting("concurrency")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(self.config.concurrency);
        let device_name = self.device_name()?;
        let exec = Arc::new(Executor {
            pair: pair.clone(),
            run_id: run_id.to_owned(),
            store: self.store.clone(),
            local: prepared.local,
            remote: prepared.remote,
            remote_ids: Mutex::new(prepared.remote_ids),
            hashes: prepared.hashes,
            concurrency,
            device_name,
            clock: self.clock.clone(),
            tracker: active.tracker.clone(),
            cancel: active.cancel.clone(),
            stats: Mutex::new(RunStats::default()),
            events: Mutex::new(Vec::new()),
            fatal: Mutex::new(None),
            first_error: Mutex::new(None),
            incoming: Mutex::new(HashSet::new()),
        });
        exec.execute(&prepared.plan).await;
        active.tracker.update(|p| p.phase = Phase::Finishing);
        if active.cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let mut stats = exec.stats.lock().unwrap_or_else(|p| p.into_inner()).clone();
        // Files the planner couldn't read count as items that couldn't sync.
        stats.errors = stats
            .errors
            .saturating_add(u32::try_from(prepared.unreadable.len()).unwrap_or(u32::MAX));
        let fatal = exec.fatal.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(f) = fatal {
            let kind = f.kind();
            if kind == ErrorKind::Auth
                && let Some(a) = &pair.account_id
            {
                self.store
                    .lock()
                    .set_account_auth_error(a, Some(&f.to_string()))?;
            }
            if kind == ErrorKind::Quota {
                self.store
                    .lock()
                    .set_paused(&pair.id, true, Some("Your Google Drive is full"))?;
            }
            return Ok((
                RunStatus::Failed,
                Some(f.to_string()),
                Some(summary),
                Some(kind),
                stats,
            ));
        }
        if stats.errors > 0 {
            // The first failure itself, so the result says what went wrong and what to do.
            let first = exec
                .first_error
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take()
                .or_else(|| prepared.unreadable.first().cloned())
                .unwrap_or_default();
            return Ok((
                RunStatus::Partial,
                Some(match stats.errors {
                    1 => first,
                    n => format!("{n} items couldn't sync. First: {first}"),
                }),
                Some(summary),
                Some(ErrorKind::Other),
                stats,
            ));
        }
        if let Some(a) = &pair.account_id {
            // Drive answered, so an earlier "sign in again" was a passing problem.
            self.store.lock().set_account_auth_error(a, None)?;
        }
        Ok((RunStatus::Success, None, Some(summary), None, stats))
    }

    fn log(&self, run_id: &str, level: EventLevel, action: &str, message: &str) {
        let e = RunEvent {
            at: self.clock.now_ms(),
            level,
            action: action.to_owned(),
            key: None,
            message: message.to_owned(),
            detail: None,
            bytes: 0,
        };
        let _ = self.store.lock().append_events(run_id, &[e]);
    }

    fn close_run(
        &self,
        pair: &Pair,
        run_id: &str,
        status: RunStatus,
        message: Option<&str>,
        kind: Option<ErrorKind>,
        stats: &RunStats,
    ) {
        let now = self.clock.now_ms();
        let s = self.store.lock();
        let _ = s.finish_run(run_id, status, stats, message, now);
        let Ok(Some(current)) = s.get_pair(&pair.id) else {
            return;
        };
        let mut st = current.status;
        st.last_run_at = Some(now);
        st.last_status = Some(status);
        match status {
            RunStatus::Success => {
                st.last_success_at = Some(now);
                st.last_error = None;
                st.last_error_kind = None;
                st.consecutive_failures = 0;
            }
            RunStatus::Failed | RunStatus::Partial => {
                st.last_error = message.map(str::to_owned);
                st.last_error_kind = kind.or(Some(ErrorKind::Other));
                st.consecutive_failures += 1;
            }
            _ => {}
        }
        let _ = s.set_pair_status(&pair.id, &st);
    }

    /// Lists Drive, scans the device and plans (port of `SyncEngine.prepare`).
    pub(crate) async fn prepare(
        &self,
        pair: &Pair,
        approve: bool,
        tracker: &Arc<Tracker>,
        cancel: &Arc<CancelToken>,
        follow_renames: bool,
    ) -> Result<Prepared> {
        let local = self.env.local_fs(pair)?;
        let remote = self.env.remote(pair)?;
        let mut notes = Vec::new();
        let mut infos = Vec::new();
        let root_id = pair.drive_folder_id.clone().ok_or_else(|| {
            EngineError::NotFound("This pair has no Drive folder. Edit the pair and choose one.".into())
        })?;

        let started = std::time::Instant::now();
        tracker.update(|p| p.phase = Phase::ListingRemote);
        let gone = |_| {
            EngineError::NotFound(format!(
                "The Drive folder for this pair no longer exists ({}). Edit the pair and choose a folder.",
                pair.remote_path
            ))
        };
        match remote.prepare(&root_id, pair.drive_id.as_deref()).await {
            Err(wasl_drive::DriveError::NotFound(m)) => return Err(gone(m)),
            other => other?,
        }
        let listing = match remote.list_subtree(&root_id, pair.drive_id.as_deref()).await {
            Err(wasl_drive::DriveError::NotFound(_)) => {
                return Err(EngineError::NotFound(format!(
                    "The Drive folder for this pair no longer exists ({}). Edit the pair and choose a folder.",
                    pair.remote_path
                )));
            }
            other => other?,
        };
        // Same-named items (exactly) in one Drive folder: the newest syncs, the rest are reported.
        // Names that differ only in case or accents are clashes instead (docs/adr/0004).
        let listed_keys: std::collections::HashSet<&str> =
            listing.entries.iter().map(|e| e.key.as_str()).collect();
        let (exact_dups, drive_clashes): (Vec<String>, Vec<String>) = listing
            .duplicates
            .iter()
            .cloned()
            .partition(|d| listed_keys.contains(d.as_str()));
        if !exact_dups.is_empty() {
            notes.push(format!(
                "{} Drive item(s) weren't synced: another item in the same Drive folder has the same name, which Drive allows but a device folder doesn't. Rename or remove one of each on Drive and the next sync picks it up: {}",
                exact_dups.len(),
                exact_dups
                    .iter()
                    .take(5)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !listing.shortcuts.is_empty() {
            infos.push(format!(
                "Drive shortcuts aren't synced ({}). To sync what one points to, pair that folder itself: {}",
                listing.shortcuts.len(),
                listing
                    .shortcuts
                    .iter()
                    .take(5)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        if follow_renames {
            infos.extend(
                self.follow_drive_renames(pair, &local, &listing.entries, &drive_clashes)
                    .await?,
            );
        }
        let baseline = self.store.lock().load_baseline(&pair.id)?;
        let mut local_rel_by_key: BTreeMap<String, String> = baseline
            .iter()
            .map(|(k, b)| (k.clone(), b.local_rel.clone()))
            .collect();
        let key_by_local_rel: HashMap<String, String> = baseline
            .iter()
            .map(|(k, b)| (b.local_rel.clone(), k.clone()))
            .collect();
        let remote_by_key: BTreeMap<String, RemoteEntry> = listing
            .entries
            .iter()
            .map(|e| (e.key.clone(), e.clone()))
            .collect();
        let base_input = PlanInput {
            baseline: baseline.clone(),
            local_rel_by_key: local_rel_by_key.clone(),
            ..PlanInput::default()
        };
        let mut key_by_mapped_rel: HashMap<String, String> = HashMap::new();
        for k in remote_by_key.keys() {
            let rel = mapped_local_rel(k, &base_input);
            if &rel != k {
                key_by_mapped_rel.insert(rel, k.clone());
            }
        }
        // Canonical keys: the baseline's mapping first, then a Drive name that maps to this local
        // name, else the local name itself.
        let key_of = {
            let a = key_by_local_rel.clone();
            let b = key_by_mapped_rel.clone();
            move |rel: &str| {
                a.get(rel)
                    .or_else(|| b.get(rel))
                    .cloned()
                    .unwrap_or_else(|| rel.to_owned())
            }
        };

        let listed = started.elapsed();
        tracker.update(|p| p.phase = Phase::ScanningLocal);
        let builder = rules_builder(pair);
        let scan = {
            let fs = local.clone();
            let key_of = key_of.clone();
            let cancel = cancel.clone();
            tokio::task::spawn_blocking(move || {
                wasl_fs::scan(fs.as_ref(), builder, &key_of, || cancel.is_cancelled())
            })
            .await
            .map_err(|e| EngineError::Other(e.to_string()))??
        };
        if !scan.unreadable.is_empty() {
            notes.push(format!(
                "Couldn't read {} folder(s) on the device, so nothing in them changed this time. Check that Wasl still has access to the pair's folder (Edit pair, then choose the folder again) and that nothing else is locking them: {}",
                scan.unreadable.len(),
                scan.unreadable
                    .iter()
                    .take(5)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        let mut local_by_key: BTreeMap<String, LocalEntry> = BTreeMap::new();
        for e in scan.entries {
            let key = key_of(&e.rel);
            local_rel_by_key.insert(key.clone(), e.rel.clone());
            local_by_key.insert(key, e);
        }
        let mut input = PlanInput {
            baseline,
            local: local_by_key,
            local_rel_by_key,
            remote: remote_by_key,
        };
        for k in input.remote.keys().cloned().collect::<Vec<_>>() {
            if !input.local_rel_by_key.contains_key(&k) {
                let rel = mapped_local_rel(&k, &input);
                input.local_rel_by_key.insert(k, rel);
            }
        }
        let rules = scan.rules;
        let unreadable = scan.unreadable;
        let input = input
            .without_ignored(|k, d| rules.is_ignored(k, d))
            .without_ignored(|k, _| {
                // Everything under a folder that could not be read stays out of this plan, so nothing
                // there reads as deleted.
                unreadable
                    .iter()
                    .any(|u| k == u || k.starts_with(&format!("{u}/")))
            });
        // One name, one item (docs/adr/0004): the same file spelled two ways is one key, a name held
        // twice on one side stays out of the plan.
        let checked = wasl_core::identity::check_names(input, &drive_clashes);
        let input = checked.input;
        if follow_renames && !checked.rekeyed.is_empty() {
            let mut s = self.store.lock();
            for (old, row) in &checked.rekeyed {
                s.put_baseline(&pair.id, row)?;
                s.remove_baseline(&pair.id, old)?;
            }
        }
        if !checked.clashes.is_empty() {
            notes.push(format!(
                "{} name(s) weren't synced: one folder holds the same name twice, differing only in upper and lower case or in how accents are written, which Windows, macOS and phones treat as one name. Rename one of each (on Drive or on this device) and the next sync picks them up. An old folder that only files the pair ignores keep here counts too: move or delete those files. {}",
                checked.clashes.len(),
                checked.clashes.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        // An agent's changes wait for the user (docs/AGENT_CHANGES.md).
        let held = self.agent_held_keys(&pair.id, &input)?;
        let input = if held.is_empty() {
            input
        } else {
            notes.push(format!(
                "Holding {} item(s) an agent changed until you accept or revert them: {}",
                held.len(),
                held.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
            ));
            input.without_ignored(|k, _| held.contains(k))
        };

        let scanned = started.elapsed();
        tracker.update(|p| p.phase = Phase::Planning);
        let cfg = self.planner_config(pair, approve)?;
        let hashes = self.hash_memo(&pair.id);
        let plan = {
            let fs = local.clone();
            let hashes = hashes.clone();
            let tracker = tracker.clone();
            let input = input.clone();
            let cancel = cancel.clone();
            tokio::task::spawn_blocking(move || -> std::io::Result<Planned> {
                let planner = Planner::new(cfg);
                let mut input = input;
                let mut known = BTreeMap::new();
                let mut unreadable = Vec::new();
                let mut vanished = Vec::new();
                loop {
                    match planner.plan(&input, &known) {
                        wasl_core::PlanOutcome::Ready(plan) => {
                            return Ok((plan, known.len(), unreadable, vanished, input));
                        }
                        wasl_core::PlanOutcome::NeedHashes(rels) => {
                            if cancel.is_cancelled() {
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::Interrupted,
                                    "cancelled",
                                ));
                            }
                            let key_by_rel: HashMap<&str, &str> = input
                                .local
                                .iter()
                                .map(|(k, l)| (l.rel.as_str(), k.as_str()))
                                .collect();
                            let wanted: Vec<&LocalEntry> = rels
                                .iter()
                                .filter_map(|r| input.local.get(*key_by_rel.get(r.as_str())?))
                                .collect();
                            let done = || tracker.update(|p| p.checked += 1);
                            let stop = || cancel.is_cancelled();
                            let hashed = hashes.md5_all_blocking(&fs, &wanted, &done, &stop)?;
                            known.extend(hashed.md5s);
                            if hashed.unreadable.is_empty() {
                                continue;
                            }
                            // A file Android won't let Wasl read stays as it is on both sides this
                            // time, named in the run's notes; the rest of the pair syncs (#127).
                            // One moved or deleted since the scan is skipped by name, not
                            // counted as a failure; the next run sees what is there (#154).
                            let (gone, refused): (Vec<_>, Vec<_>) = hashed
                                .unreadable
                                .into_iter()
                                .partition(|(_, e)| e.kind() == std::io::ErrorKind::NotFound);
                            let skip: std::collections::HashSet<String> = gone
                                .iter()
                                .chain(refused.iter())
                                .filter_map(|(rel, _)| key_by_rel.get(rel.as_str()).map(|k| (*k).to_owned()))
                                .collect();
                            if !gone.is_empty()
                                && let Some(e) = crate::support::root_gone(fs.as_ref())
                            {
                                return Err(e);
                            }
                            vanished.extend(gone.into_iter().map(|(_, e)| e.to_string()));
                            unreadable.extend(refused.into_iter().map(|(_, e)| e.to_string()));
                            input = input.without_ignored(|k, _| skip.contains(k));
                        }
                    }
                }
            })
            .await
            .map_err(|e| EngineError::Other(e.to_string()))??
        };
        let (plan, compared, unreadable, vanished, input) = plan;
        // Each by name in the log, as a skip: nothing failed (#154).
        notes.extend(vanished);
        if !unreadable.is_empty() {
            notes.push(format!(
                "Skipped {} file(s) on the device that couldn't be read this time; they stay as they are on both sides until the next sync: {}",
                unreadable.len(),
                unreadable.iter().take(5).cloned().collect::<Vec<_>>().join("; ")
            ));
        }
        // Where a slow comparison spent its time, in the run's log (the owner's phone, 2026-10-29:
        // a 12,000-file preview took minutes and nothing showed which step).
        let total = started.elapsed();
        infos.push(format!(
            "Compared {} files in {:.1} s: Drive list {:.1} s, device scan {:.1} s, content check of {compared} files {:.1} s",
            input.local.len().max(input.remote.len()),
            total.as_secs_f64(),
            listed.as_secs_f64(),
            (scanned - listed).as_secs_f64(),
            (total - scanned).as_secs_f64(),
        ));
        notes.extend(plan.summary.warnings.iter().cloned());

        let mut remote_ids: HashMap<String, String> = HashMap::new();
        remote_ids.insert(String::new(), root_id);
        for (k, e) in &input.remote {
            remote_ids.insert(k.clone(), e.id.clone());
        }
        Ok(Prepared {
            plan,
            notes,
            infos,
            local,
            remote,
            remote_ids,
            hashes,
            input,
            unreadable,
        })
    }

    /// A file renamed or moved on Drive keeps its Drive id: rename it here too, instead of
    /// downloading it under the new name and binning the old one. Only for a file synced before
    /// (its baseline has that id), unchanged here since, whose new place is free on this device.
    /// The baseline follows the rename, so the planner then sees nothing to do (or only the
    /// content change, if there was one). A rename the folder cannot do (Android's folder picker
    /// renames only within one folder) falls back to the planner's download.
    ///
    /// Only where Drive may rearrange the device's files (two-way and mirror-down pairs), and
    /// never from or into a path the pair's rules ignore (rule files along both paths included).
    /// A rename loses nothing, so it does not count toward the deletion guards.
    async fn follow_drive_renames(
        &self,
        pair: &Pair,
        local: &Arc<dyn LocalFs>,
        remote: &[RemoteEntry],
        drive_clashes: &[String],
    ) -> Result<Vec<String>> {
        if !matches!(
            pair.method,
            wasl_core::SyncMode::TwoWay | wasl_core::SyncMode::DownloadMirror
        ) {
            return Ok(Vec::new());
        }
        let baseline = self.store.lock().load_baseline(&pair.id)?;
        let listed: std::collections::HashSet<&str> = remote.iter().map(|e| e.key.as_str()).collect();
        let by_id: HashMap<&str, &wasl_core::BaselineEntry> = baseline
            .values()
            .filter(|b| !b.is_dir && b.local_rel == b.key)
            .filter_map(|b| b.remote_id.as_deref().map(|id| (id, b)))
            .collect();
        // A synced name Drive now spells differently (case, accents) is the same file even under a
        // new id: a device that renamed it uploaded it again (#137).
        let by_name: HashMap<String, &wasl_core::BaselineEntry> = baseline
            .values()
            .filter(|b| !b.is_dir && b.local_rel == b.key && !listed.contains(b.key.as_str()))
            .map(|b| (name_key(&b.key), b))
            .collect();
        let mut moves: Vec<(wasl_core::BaselineEntry, String)> = Vec::new();
        for e in remote.iter().filter(|e| !e.is_dir) {
            let Some(b) = by_id
                .get(e.id.as_str())
                .or_else(|| by_name.get(&name_key(&e.key)))
            else {
                continue;
            };
            if b.key == e.key
                || listed.contains(b.key.as_str())
                || baseline.contains_key(&e.key)
                || drive_clashes.iter().any(|c| same_name_or_under(&e.key, c))
            {
                continue;
            }
            let mut b = (*b).clone();
            if b.remote_id.as_deref() != Some(e.id.as_str()) {
                // Matched by name: take Drive's id when the content is the one synced, so the
                // planner moves nothing; otherwise the planner sees Drive's change.
                if !adopt_drive_identity(&mut b, e) {
                    continue;
                }
            }
            if wasl_fs::check_rel(&e.key).is_err() || e.key.split('/').any(wasl_fs::is_internal_name) {
                continue;
            }
            moves.push((b, e.key.clone()));
        }
        if moves.is_empty() {
            return Ok(Vec::new());
        }
        let fs = local.clone();
        let builder = rules_builder(pair);
        let done: Vec<(wasl_core::BaselineEntry, String)> = tokio::task::spawn_blocking(move || {
            let paths: Vec<&str> = moves
                .iter()
                .flat_map(|(b, to)| [b.key.as_str(), to.as_str()])
                .collect();
            let Some(rules) = rules_along(fs.as_ref(), builder, &paths) else {
                return Vec::new();
            };
            let done: Vec<(wasl_core::BaselineEntry, String)> = moves
                .into_iter()
                .filter(|(b, to)| !ignored_or_under(&rules, &b.key) && !ignored_or_under(&rules, to))
                .filter(|(b, to)| {
                    // Unchanged here since the last sync, and nothing at the new place.
                    let clean = matches!(fs.stat(&b.local_rel), Ok(Some(d))
                        if !d.is_dir && d.size == b.local_size && d.mtime_ms == b.local_mtime);
                    if !clean || !place_is_free(fs.as_ref(), &b.local_rel, to) {
                        return false;
                    }
                    if let Some((parent, _)) = to.rsplit_once('/')
                        && fs.mkdir(parent).is_err()
                    {
                        return false;
                    }
                    fs.rename(&b.local_rel, to).is_ok()
                })
                .collect();
            // A folder Drive respelled (`Docs` → `docs`) is a new folder on a device that keeps
            // both spellings apart: remove the old one once the renames emptied it.
            for (b, to) in &done {
                let (mut from, mut to) = (b.local_rel.as_str(), to.as_str());
                while let (Some((old, _)), Some((new, _))) = (from.rsplit_once('/'), to.rsplit_once('/')) {
                    if old == new || !same_name(old, new) || !matches!(fs.remove_dir_if_empty(old), Ok(true))
                    {
                        break;
                    }
                    (from, to) = (old, new);
                }
            }
            done
        })
        .await
        .map_err(|e| EngineError::Other(e.to_string()))?;
        let mut notes = Vec::new();
        {
            let mut s = self.store.lock();
            for (b, to) in &done {
                let mut moved = b.clone();
                moved.key = to.clone();
                moved.local_rel = to.clone();
                s.put_baseline(&pair.id, &moved)?;
                s.remove_baseline(&pair.id, &b.key)?;
            }
        }
        if !done.is_empty() {
            notes.push(format!(
                "Followed {} rename(s) made on Drive: {}",
                done.len(),
                done.iter()
                    .take(5)
                    .map(|(b, to)| format!("{} → {}", b.key, to))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(notes)
    }

    fn planner_config(&self, pair: &Pair, approve: bool) -> Result<PlannerConfig> {
        let s = self.store.lock();
        let num = |key: &str, default: u32| -> Result<u32> {
            Ok(s.setting(key)?.and_then(|v| v.parse().ok()).unwrap_or(default))
        };
        Ok(PlannerConfig {
            mode: pair.method,
            conflict_policy: pair.conflict_policy,
            propagate_deletions: pair.propagate_deletions,
            max_file_size: pair.size_limit,
            docs_export: pair.docs_export,
            docs_format: pair.docs_format,
            guards: GuardConfig {
                mass_delete_guard: pair.mass_delete_guard,
                mass_delete_min_files: num("mass_delete_min_files", 20)?,
                mass_delete_threshold_percent: num("mass_delete_threshold_percent", 5)?,
                first_merge_guard: true,
                approved_once: approve,
            },
        })
    }

    // ------------------------------------------------------------------ recycle bin

    /// Puts a binned file back where it was (FR-F5). A newer file in its place goes to the bin first.
    pub async fn restore_from_bin(&self, entry_id: &str) -> Result<()> {
        self.restore_one(entry_id).await.map(|_| ())
    }

    /// Deletes local bin files past their retention and drops expired entries (Drive empties its
    /// own bin). Returns how many entries went.
    pub async fn purge_expired_bin(&self) -> Result<usize> {
        let expired = self.store.lock().expired_bin_entries(self.clock.now_ms())?;
        let mut n = 0;
        for b in expired {
            if b.entry.side == BinSide::Local
                && let Ok(pair) = self.pair(&b.entry.pair_id)
            {
                let fs = self.env.local_fs(&pair)?;
                let loc = b.entry.location.clone();
                tokio::task::spawn_blocking(move || fs.purge_from_bin(&loc))
                    .await
                    .map_err(|e| EngineError::Other(e.to_string()))??;
            }
            self.store.lock().remove_from_bin(&b.entry.id)?;
            n += 1;
        }
        let _ = self.agent_housekeeping();
        // Remote upkeep runs with the purge (vault v2 sweeps unreferenced chunks). Best effort: a
        // locked vault or an unreachable Drive waits for the next purge.
        let vault_pairs: Vec<Pair> = self
            .store
            .lock()
            .list_pairs()?
            .into_iter()
            .filter(|p| p.vault_id.is_some())
            .collect();
        for pair in vault_pairs {
            if let (Some(root), Ok(remote)) = (pair.drive_folder_id.as_deref(), self.env.remote(&pair)) {
                let _ = remote.housekeeping(root, pair.drive_id.as_deref()).await;
            }
        }
        Ok(n)
    }
}

/// Why a path is ignored or synced (SRS FR-A3 `wasl ignore --explain`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IgnoreVerdict {
    pub ignored: bool,
    /// The deciding line as written, e.g. `node_modules/` or `!keep.log`.
    pub pattern: Option<String>,
    /// Where it came from: `app/.waslignore:3`, `preset node:2`, `pair settings` or
    /// [`INTERNAL_SOURCE`].
    pub source: Option<String>,
    /// The folder the rule matched, when it was an ancestor of the path.
    pub via_parent: Option<String>,
    /// What explain could not check, e.g. the size of a file that is only on Drive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Explain's source for Wasl's own partial and temporary files: no setting changes it.
pub const INTERNAL_SOURCE: &str = "Wasl's own temporary files";

/// Explain's note for a path that is not on the device and was never synced: the planner also
/// skips a regular Drive file over the size limit, and explain does not list Drive.
pub const SIZE_LIMIT_LOCAL_ONLY: &str = "The size limit was checked against the file on this device only. \
     It is not here, so a regular file on Drive larger than the limit is skipped.";

/// Follows [`SIZE_LIMIT_LOCAL_ONLY`] when the pair exports Google files: the planner never checks
/// their size.
pub const GOOGLE_ANY_SIZE: &str = "A Google file is exported whatever its size.";

/// Nothing else is at `to`. On a folder that folds case or accents, `to` may name the file being
/// renamed itself (`Foo.txt` → `foo.txt`, #138): free only when it is that same file, never when a
/// device that keeps both spellings apart holds another file there.
fn place_is_free(fs: &dyn LocalFs, from: &str, to: &str) -> bool {
    match fs.stat(to) {
        Ok(None) => true,
        Ok(Some(_)) => same_name(from, to) && fs.is_same_file(from, to).unwrap_or(false),
        Err(_) => false,
    }
}

/// "`Notes` or `Notes.docx`": the titles of the Google files a note names.
fn google_titles(titles: &[&str]) -> String {
    titles
        .iter()
        .map(|t| format!("`{t}`"))
        .collect::<Vec<_>>()
        .join(" or ")
}

/// Explain's note for a never-synced path under a name an export writes (`Notes.docx`): a Google
/// file on Drive titled `Notes` or `Notes.docx` (an existing extension is not doubled), which
/// explain does not list, is exported to it whatever its size. `titles` are those whose own key
/// the rules keep; `replaces` when the file is on the device. The verdict stays the local file's.
pub fn export_here_note(titles: &[&str], replaces: bool) -> String {
    let mut note = format!(
        "If a Google file {} is on Drive, it is exported here",
        google_titles(titles)
    );
    note.push_str(if replaces {
        " and this file goes to the recycle bin."
    } else {
        "."
    });
    note
}

/// Explain's note for a never-synced file here that a rule keeps: a Google file of one of these
/// `titles` on Drive is exported beside it under a name of its own, `Notes (Google Doc).docx`, and
/// the file keeps its name (#139).
pub fn export_beside_note(titles: &[&str]) -> String {
    format!(
        "If a Google file {} is on Drive, it is exported beside this file under a name of its own, and this file stays.",
        google_titles(titles)
    )
}

/// Explain's note for a never-synced path in a pair that exports no Google files: the planner
/// skips a Google file of one of these `titles` on Drive, which explain does not list.
pub fn google_skipped_note(titles: &[&str]) -> String {
    format!(
        "If a Google file {} is on Drive, it is skipped: this pair does not export Google files.",
        google_titles(titles)
    )
}

/// Explain's note for a never-synced file on the device in a pair that only downloads: the
/// planner never uploads it, and a mirror from Drive bins it when Drive has no file there. `None`
/// for a pair that uploads.
pub fn local_only_note(mode: wasl_core::SyncMode) -> Option<String> {
    use wasl_core::SyncMode;
    let kept = |name: &str| {
        format!(
            "This pair is set to {name}: a file only on this device is never uploaded, so it stays on this device only."
        )
    };
    let name = mode.names().0;
    match mode {
        SyncMode::DownloadOnly | SyncMode::DownloadThenDelete => Some(kept(name)),
        SyncMode::DownloadMirror => Some(format!(
            "This pair is set to {name}: if Drive has no file here, the next sync moves this one to the recycle bin."
        )),
        _ => None,
    }
}

impl Engine {
    /// Explains the ignore decision for `key` (pair-relative, `/`-separated) with the rule files
    /// on the device now: the pair root's and every ancestor folder's.
    pub async fn explain_ignore(&self, pair_id: &str, key: &str, is_dir: bool) -> Result<IgnoreVerdict> {
        use std::io::Read;
        use wasl_core::DocsFormat;
        use wasl_core::docs::export_stem;
        use wasl_core::paths::{name_of, parent_of};
        use wasl_ignore::RuleOrigin;
        use wasl_ignore::legacy::PairRule;
        use wasl_ignore::rules::FileKind;
        let pair = self.pair(pair_id)?;
        let fs = self.env.local_fs(&pair)?;
        let key = key.replace('\\', "/").trim_matches('/').to_owned();
        // Baseline rows are keyed by canonical key (`Notes` for the export `Notes.docx`): find the
        // row by its local name, then by the path as a key.
        let base = {
            let store = self.store.lock();
            match store.baseline_by_local_rel(pair_id, &key)? {
                Some(b) => Some(b),
                None => store.baseline_entry(pair_id, &key)?,
            }
        };
        // Like the scan and the planner, read files by local name and check rules on the key.
        let (local_rel, rule_key) = match &base {
            Some(b) => (b.local_rel.clone(), b.key.clone()),
            None => (key.clone(), key),
        };
        tokio::task::spawn_blocking(move || {
            let mut builder = rules_builder(&pair);
            let parts: Vec<&str> = local_rel.split('/').filter(|p| !p.is_empty()).collect();
            let key_parts: Vec<&str> = rule_key.split('/').filter(|p| !p.is_empty()).collect();
            for depth in 0..parts.len().max(1) {
                let dir = parts[..depth].join("/");
                let dir_key = if key_parts.len() == parts.len() {
                    key_parts[..depth].join("/")
                } else {
                    dir.clone()
                };
                for kind in [FileKind::WaslIgnore, FileKind::GitIgnore] {
                    let rel = if dir.is_empty() {
                        kind.file_name().to_owned()
                    } else {
                        format!("{dir}/{}", kind.file_name())
                    };
                    if let Ok(mut r) = fs.open_read(&rel) {
                        let mut text = String::new();
                        if r.read_to_string(&mut text).is_ok() {
                            builder = builder.file(&dir_key, kind, &text);
                        }
                    }
                }
            }
            let rules = builder
                .build()
                .map_err(|e| EngineError::Other(format!("The ignore rules have an error: {e:?}")))?;
            let v = rules.check(&rule_key, is_dir);
            let (pattern, source, via_parent) = match v.rule {
                Some(m) => {
                    let source = match m.origin {
                        RuleOrigin::File { file, line } => format!("{file}:{line}"),
                        RuleOrigin::Preset { preset, line } => format!("preset {}:{line}", preset.id()),
                        RuleOrigin::PairPatterns if m.pair_rule == Some(PairRule::Internal) => {
                            INTERNAL_SOURCE.to_owned()
                        }
                        RuleOrigin::PairPatterns => "pair settings".to_owned(),
                        RuleOrigin::Unknown => "unknown".to_owned(),
                    };
                    // Pair settings carry no line of their own: name the setting that decided.
                    let pattern = m.pair_rule.map_or(m.pattern, |why| why.to_string());
                    (Some(pattern), Some(source), m.via_parent)
                }
                None => (None, None, None),
            };
            // The rest depends on Drive, which explain does not list, so a note names what Drive
            // decides. Only a never-synced path needs one: a baseline row says what a synced one is.
            let never_synced = base.is_none() && !is_dir;
            let synced = base.as_ref().is_some_and(|b| b.local_hash.is_some());
            let exports = pair.docs_export && pair.method.downloads();
            let stat = if is_dir {
                None
            } else {
                fs.stat(&local_rel).ok().flatten()
            };
            let here = stat.as_ref().filter(|e| !e.is_dir);
            // The planner's size limit comes after the rules, so it can skip a file they keep. It
            // never applies to a Google file: the planner sends those to plan_export, and an
            // export's baseline row always has a hash, so it reads as synced here.
            let over_limit =
                here.is_some_and(|e| pair.size_limit > 0 && over_size_limit(pair.size_limit, e.size, synced));
            // The verdict is the local file's, as the scan and the planner treat it.
            let (ignored, pattern, source, via_parent) = if !v.ignored && over_limit {
                let pattern = Some(SIZE_LIMIT_SKIP.to_owned());
                (true, pattern, Some("pair settings".to_owned()), None)
            } else {
                (v.ignored, pattern, source, via_parent)
            };
            // A Google file on Drive can land on a never-synced path: a doc `Notes` or `Notes.docx`
            // is exported to `Notes.docx` (an existing extension is not doubled), and any Google
            // file sits at its own name. Explain does not list Drive, so these never change the
            // verdict; a note names the titles whose own key the rules keep.
            let name = name_of(&local_rel);
            let stem = export_stem(name, pair.docs_format == DocsFormat::Pdf);
            let titles: Vec<&str> = if never_synced {
                let key_of = |title: &str| match parent_of(&rule_key) {
                    "" => title.to_owned(),
                    parent => format!("{parent}/{title}"),
                };
                stem.into_iter()
                    .chain([name])
                    .filter(|&title| !rules.check(&key_of(title), false).ignored)
                    .collect()
            } else {
                Vec::new()
            };
            let mut notes = Vec::new();
            // A pair that only downloads never uploads a file it has not synced (#99): say what its
            // mode does with it, as the planner will.
            if never_synced
                && here.is_some()
                && !ignored
                && let Some(note) = local_only_note(pair.method)
            {
                notes.push(note);
            }
            if never_synced && stat.is_none() && !v.ignored && pair.size_limit > 0 {
                notes.push(SIZE_LIMIT_LOCAL_ONLY.to_owned());
                if exports {
                    notes.push(GOOGLE_ANY_SIZE.to_owned());
                }
            }
            if !titles.is_empty() {
                if exports && stem.is_some() {
                    // A real file here keeps its name, so a Google file of another title is
                    // exported beside it (#139). One with this very name shares its key and lands
                    // here, as does any while a rule leaves this file out of the plan.
                    let (beside, onto): (Vec<&str>, Vec<&str>) = if here.is_some() && !v.ignored {
                        titles.iter().partition(|&&t| t != name)
                    } else {
                        (Vec::new(), titles.clone())
                    };
                    if !beside.is_empty() {
                        notes.push(export_beside_note(&beside));
                    }
                    if !onto.is_empty() {
                        notes.push(export_here_note(&onto, here.is_some()));
                    }
                } else if !exports && (stat.is_none() || stem.is_some()) {
                    // On the device, only a name an export writes is a copy of a Google file.
                    notes.push(google_skipped_note(&titles));
                }
            }
            let note = (!notes.is_empty()).then(|| notes.join(" "));
            Ok(IgnoreVerdict {
                ignored,
                pattern,
                source,
                via_parent,
                note,
            })
        })
        .await
        .map_err(|e| EngineError::Other(e.to_string()))?
    }
}

/// The pair's rules with every rule file in the folders above `paths` (the only ones that can
/// decide about them). `None` if the rules do not build.
fn rules_along(
    fs: &dyn LocalFs,
    mut builder: wasl_ignore::rules::IgnoreRulesBuilder,
    paths: &[&str],
) -> Option<IgnoreRules> {
    use wasl_ignore::rules::FileKind;
    let mut dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::from([String::new()]);
    for p in paths {
        let mut at = 0;
        while let Some(i) = p[at..].find('/') {
            dirs.insert(p[..at + i].to_owned());
            at += i + 1;
        }
    }
    for d in dirs {
        for kind in [FileKind::WaslIgnore, FileKind::GitIgnore] {
            let rel = if d.is_empty() {
                kind.file_name().to_owned()
            } else {
                format!("{d}/{}", kind.file_name())
            };
            let Ok(r) = fs.open_read(&rel) else { continue };
            let mut text = String::new();
            if std::io::Read::read_to_string(&mut std::io::Read::take(r, 1 << 20), &mut text).is_ok() {
                builder = builder.file(&d, kind, &text);
            }
        }
    }
    builder.build().ok()
}

/// Whether `key` or a folder above it is ignored.
fn ignored_or_under(rules: &IgnoreRules, key: &str) -> bool {
    let mut at = 0;
    while let Some(i) = key[at..].find('/') {
        if rules.is_ignored(&key[..at + i], true) {
            return true;
        }
        at += i + 1;
    }
    rules.is_ignored(key, false)
}

/// Whether the pair's own patterns and excluded subfolders leave the folder `folder` out, judged
/// as a folder, itself or through a folder above it. Only these are final: wasl-ignore lets no rule
/// file bring back anything under a folder they reject, so the planner never lists, uploads or
/// downloads below it. Presets and rule files don't count here (a rule file could re-include).
pub fn excluded_by_pair_settings(pair: &Pair, folder: &str) -> bool {
    let folder = folder.trim_matches('/');
    if folder.is_empty() {
        return false;
    }
    let filter = PathFilter::new(
        &pair.include_patterns,
        &pair.exclude_patterns,
        &pair.excluded_subfolders,
        pair.sync_hidden,
    );
    folder
        .match_indices('/')
        .map(|(i, _)| &folder[..i])
        .chain(std::iter::once(folder))
        .any(|dir| !filter.accept(dir, true))
}

/// The pair's rules before any rule file is read: presets, `.gitignore` option, 1.1.4 patterns.
pub(crate) fn rules_builder(pair: &Pair) -> wasl_ignore::rules::IgnoreRulesBuilder {
    let presets: Vec<Preset> = pair.presets.iter().filter_map(|id| Preset::from_id(id)).collect();
    IgnoreRules::builder()
        .presets(presets)
        .honor_gitignore(pair.honor_gitignore)
        .pair_patterns(PathFilter::new(
            &pair.include_patterns,
            &pair.exclude_patterns,
            &pair.excluded_subfolders,
            pair.sync_hidden,
        ))
}
