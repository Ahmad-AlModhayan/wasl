//! A test-only fault plan: stops a run at a named point of the apply path as if the process had
//! died there. It exists only under `cfg(test)` or the `test-support` feature, so a production
//! build has no such point.

use crate::error::{EngineError, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A place in the apply path where a run can be stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// The new content is staged next to its target.
    AfterStage,
    /// The staged content is flushed to disk.
    AfterFlush,
    /// The old copy is in the bin and still at its path (the swap has not happened).
    AfterBin,
    /// The item is about to be renamed (it still has its old name).
    BeforeRename,
    /// The new file has taken the name (or the item has been renamed).
    AfterRename,
    /// The baseline row is gone; the remote is not yet in Drive's bin (move mode).
    AfterRowRemoved,
    /// The remote was moved to Drive's bin (move mode).
    AfterRemoteTrash,
    /// Everything is done on disk; the result is not yet in the baseline.
    BeforeRecord,
}

/// Counts the points a run passes, and optionally stops the run at one of them. After the stop the
/// plan is dead: every later point fails too and the executor starts nothing more, as after a kill.
#[derive(Debug, Default)]
pub struct FaultPlan {
    stop_at: Option<usize>,
    seen: Mutex<Vec<Point>>,
    dead: AtomicBool,
}

impl FaultPlan {
    /// A plan that only counts.
    pub fn counting() -> Arc<FaultPlan> {
        Arc::new(FaultPlan::default())
    }

    /// A plan that stops the run at its `index`th point (0-based, in the order a counting run saw).
    pub fn stop_at(index: usize) -> Arc<FaultPlan> {
        Arc::new(FaultPlan {
            stop_at: Some(index),
            ..FaultPlan::default()
        })
    }

    pub(crate) fn hit(&self, point: Point) -> Result<()> {
        if self.dead.load(Ordering::SeqCst) {
            return Err(stopped(point));
        }
        let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        seen.push(point);
        if self.stop_at == Some(seen.len() - 1) {
            self.dead.store(true, Ordering::SeqCst);
            return Err(stopped(point));
        }
        Ok(())
    }

    /// Passes a point from a test's own scenario (the harness's self-test).
    pub fn pass(&self, point: Point) -> Result<()> {
        self.hit(point)
    }

    /// Every point passed so far, in order.
    pub fn seen(&self) -> Vec<Point> {
        self.seen.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// The point the run was stopped at, if it was.
    pub fn stopped_at(&self) -> Option<Point> {
        let stop = self.stop_at?;
        self.seen().get(stop).copied()
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }
}

fn stopped(point: Point) -> EngineError {
    EngineError::Other(format!("fault plan: stopped at {point:?}"))
}
