//! One lock per pair, shared by Drive sync and device sync (wasl#20).
//!
//! Whoever starts a run of a pair (the scheduler, a daemon request, the phone's Drive call or
//! device call, a peer's "I have changes" notice) takes the pair's lock first and holds it until
//! the run ends. The lock is released when its guard is dropped, so an error, a cancel, a dropped
//! call and a panic all free it. Runs of different pairs don't touch each other.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// A pair's lock, held until dropped.
pub type PairGuard = OwnedMutexGuard<()>;

/// The locks of every pair. Cloning shares them: hand the same value to the engine and to the
/// device sync so the two see one lock per pair.
#[derive(Clone, Default)]
pub struct PairLocks(Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>);

impl std::fmt::Debug for PairLocks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairLocks")
    }
}

impl PairLocks {
    fn slot(&self, pair_id: &str) -> Arc<AsyncMutex<()>> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(pair_id.to_owned())
            .or_default()
            .clone()
    }

    /// The pair's lock, or `None` when a run holds it. For a caller that reports "already
    /// syncing" instead of waiting (a sync the person pressed).
    pub fn try_acquire(&self, pair_id: &str) -> Option<PairGuard> {
        self.slot(pair_id).try_lock_owned().ok()
    }

    /// The pair's lock, waiting at most `limit` for the run that holds it. `None` when that run
    /// is still going at the deadline. For a caller that would rather wait its turn (a peer's
    /// notice, the scheduler).
    pub async fn acquire_within(&self, pair_id: &str, limit: Duration) -> Option<PairGuard> {
        tokio::time::timeout(limit, self.slot(pair_id).lock_owned())
            .await
            .ok()
    }

    /// Whether a run holds the pair's lock right now.
    pub fn is_held(&self, pair_id: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(pair_id)
            .is_some_and(|m| m.try_lock().is_err())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_second_run_of_a_pair_waits_or_reports() {
        let locks = PairLocks::default();
        let first = locks.try_acquire("p").unwrap();
        assert!(locks.is_held("p"));
        assert!(locks.try_acquire("p").is_none());
        assert!(
            locks
                .acquire_within("p", Duration::from_millis(20))
                .await
                .is_none()
        );
        drop(first);
        assert!(!locks.is_held("p"));
        assert!(
            locks
                .acquire_within("p", Duration::from_millis(20))
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_waiter_gets_the_lock_when_the_holder_ends() {
        let locks = PairLocks::default();
        let first = locks.try_acquire("p").unwrap();
        let l2 = locks.clone();
        let waiter =
            tokio::spawn(async move { l2.acquire_within("p", Duration::from_secs(5)).await.is_some() });
        tokio::task::yield_now().await;
        drop(first);
        assert!(waiter.await.unwrap());
    }

    #[tokio::test]
    async fn different_pairs_do_not_wait_for_each_other() {
        let locks = PairLocks::default();
        let _a = locks.try_acquire("a").unwrap();
        assert!(locks.try_acquire("b").is_some());
    }

    #[tokio::test]
    async fn a_panicking_run_frees_the_lock() {
        let locks = PairLocks::default();
        let l2 = locks.clone();
        let run = tokio::spawn(async move {
            let _g = l2.try_acquire("p").unwrap();
            panic!("run failed");
        });
        assert!(run.await.is_err());
        assert!(locks.try_acquire("p").is_some());
    }
}
