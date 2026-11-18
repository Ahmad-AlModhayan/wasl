//! The kill-point harness: runs a scenario once to count the points of the apply path it passes,
//! then once per point, stopping there as if the process had died, starting the engine again on
//! the same store and folder, running again, and handing back what a person would see.
//!
//! One command runs the tests that use it: `cargo test -p wasl-engine --test killpoints`.
#![allow(dead_code)]

use super::World;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use wasl_core::RunStatus;
use wasl_engine::fault::{FaultPlan, Point};

pub type Step<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;

/// What a person would see after a run was stopped at one point and the engine ran again.
pub struct Seen {
    /// Which point (in the order a counting run saw them) and what it was.
    pub index: usize,
    pub point: Point,
    /// Local files and the Drive's live files as they were before the scenario ran.
    pub local_before: BTreeMap<String, String>,
    pub drive_before: BTreeMap<String, String>,
    /// Straight after the stop, before the engine started again: what a person would find if the
    /// machine stayed off.
    pub local_at_stop: BTreeMap<String, String>,
    /// After the stop and one more run.
    pub local: BTreeMap<String, String>,
    pub drive: BTreeMap<String, String>,
    /// Names of the files in Drive's bin.
    pub trashed: Vec<String>,
    /// The content of every file in the local bin folder, indexed or not.
    pub bin_contents: Vec<String>,
    /// How many files the local bin index holds.
    pub binned: usize,
    /// The status of the run after the stop.
    pub status: RunStatus,
    /// The world, to look further.
    pub world: World,
}

impl Seen {
    /// Files that were on the device before and are not there at all now, under any content the
    /// `acceptable` list for that path doesn't allow. `acceptable(path)` gives the contents a
    /// person may find there (the old one, the new one).
    pub fn lost_locally(&self, acceptable: impl Fn(&str) -> Vec<String>) -> Vec<String> {
        self.local_before
            .keys()
            .filter(|p| match self.local.get(*p) {
                Some(c) => !acceptable(p).contains(c),
                None => true,
            })
            .cloned()
            .collect()
    }
}

/// The plain scenario step: one sync.
pub fn sync_once(w: &World, _plan: Arc<FaultPlan>) -> Step<'_> {
    Box::pin(async move {
        w.sync().await;
    })
}

/// The points `run` passes on a world `make` builds, in order.
pub async fn points_of<M, R>(make: &M, run: &R) -> Vec<Point>
where
    M: Fn() -> World,
    R: for<'a> Fn(&'a World, Arc<FaultPlan>) -> Step<'a>,
{
    let mut w = make();
    let plan = FaultPlan::counting();
    w.reopen(Some(plan.clone()));
    run(&w, plan.clone()).await;
    plan.seen()
}

/// Runs the scenario once per point it passes, stopped there. `make` builds a fresh world with its
/// files and Drive ready (and already synced, if the scenario wants a baseline); `run` is the step
/// that is stopped (usually [`sync_once`]); `check` gets what a person would see afterwards.
pub async fn for_each_kill_point_in<M, R>(make: M, run: R, mut check: impl FnMut(&Seen))
where
    M: Fn() -> World,
    R: for<'a> Fn(&'a World, Arc<FaultPlan>) -> Step<'a>,
{
    let points = points_of(&make, &run).await;
    assert!(
        !points.is_empty(),
        "the scenario passes no point of the apply path"
    );
    for (index, point) in points.iter().copied().enumerate() {
        let mut w = make();
        let local_before = w.local_files();
        let drive_before = w.drive_files();
        let plan = FaultPlan::stop_at(index);
        w.reopen(Some(plan.clone()));
        run(&w, plan.clone()).await;
        assert_eq!(
            Some(point),
            plan.stopped_at(),
            "run {index} did not stop where counted"
        );
        let local_at_stop = w.local_files();
        // The process starts again, on the same data.
        w.reopen(None);
        let status = w.sync().await.status;
        let bin_contents = std::fs::read_dir(&w.env.bin)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                    .collect()
            })
            .unwrap_or_default();
        let binned = w.engine.store().lock().bin_entries(None).unwrap().len();
        let seen = Seen {
            index,
            point,
            local_before,
            drive_before,
            local_at_stop,
            local: w.local_files(),
            drive: w.drive_files(),
            trashed: w.drive.trashed_files(),
            bin_contents,
            binned,
            status,
            world: w,
        };
        check(&seen);
    }
}

/// [`for_each_kill_point_in`] for a scenario that is one sync.
pub async fn for_each_kill_point(make: impl Fn() -> World, check: impl FnMut(&Seen)) {
    for_each_kill_point_in(make, sync_once, check).await;
}
