//! The engine's one retry policy (issue #30): when a failed run is tried again, how often, and when
//! the engine gives up and the folder "needs you".
//!
//! A run that fails for a passing reason (Google Drive can't be reached, it asks Wasl to slow down,
//! or it has a problem on its side) is not a problem the person has to know about. The folder reads
//! "waiting" and the engine runs it again after a pause that grows with each failure. Only when
//! the policy gives up does the folder become "needs you", once, with the entry for the last cause and
//! its fix. A lasting failure (signed out, the Drive folder gone, a full disk, a refused name) is
//! never retried: it needs the person at once.
//!
//! ## The policy
//!
//! - **Tries**: [`MAX_TRIES`] runs in a row, the first included. The run that fails for the
//!   [`MAX_TRIES`]th time in a row is the one the engine gives up on.
//! - **Pause**: after the 1st failure 30 s, after the 2nd 2 min, after the 3rd 10 min, after the 4th
//!   30 min ([`BACKOFF_SECS`]). A folder therefore keeps trying for about 42 minutes before it asks
//!   the person for anything.
//! - **Counted** by `PairStatus::consecutive_failures`, which a successful run, or a run that finished
//!   in part (Drive answered), resets to 0. The apps keep no count of their own: they read
//!   [`RetryInfo`] from the folder's status (`wasl status --json`: `retry.attempt`, `retry.max_tries`,
//!   `retry.next_at`).
//! - **While waiting**, nothing else starts the folder (neither its interval nor a change in the
//!   folder), so a run's pause is not cut short and tries are not used up faster than the policy says.
//!   "Sync now" always runs at once, and counts as a try.
//! - **After giving up**, the folder goes back to its own interval and its changes, and "Try again"
//!   runs it at once.
//!
//! This is the only place these numbers live; a test pins them. The decision about what a given run
//! error means (passing or lasting) is [`ErrorKind::transient`].

use crate::run::{ErrorKind, RunStatus};
use crate::store::PairStatus;
use serde::{Deserialize, Serialize};

/// Runs in a row, the first included, before the engine gives up.
pub const MAX_TRIES: u32 = 5;

/// Seconds the engine waits after the 1st, 2nd, 3rd and 4th failure in a row before the next try.
pub const BACKOFF_SECS: [u64; 4] = [30, 120, 600, 1800];

/// How many tries a folder gets and how long the engine waits between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Runs in a row, the first included, before the engine gives up.
    pub tries: u32,
    /// Milliseconds to wait after the 1st, 2nd, … failure in a row; one fewer than `tries`.
    pub delays_ms: &'static [u64],
}

impl RetryPolicy {
    /// The policy every device runs.
    pub const DEFAULT: RetryPolicy = RetryPolicy {
        tries: MAX_TRIES,
        delays_ms: &[
            BACKOFF_SECS[0] * 1000,
            BACKOFF_SECS[1] * 1000,
            BACKOFF_SECS[2] * 1000,
            BACKOFF_SECS[3] * 1000,
        ],
    };

    /// How long to wait before the next try after `failures` failures in a row; `None` when the
    /// policy has given up (or nothing has failed yet).
    pub fn delay_after_ms(&self, failures: u32) -> Option<u64> {
        if failures == 0 || failures >= self.tries {
            return None;
        }
        let i = (failures as usize - 1).min(self.delays_ms.len().saturating_sub(1));
        self.delays_ms.get(i).copied()
    }

    /// Where a folder with this last-run status stands: waiting to be tried again, or `None` when it
    /// is not in a retry (its last run didn't fail for a passing reason, or the policy has given up).
    pub fn plan(&self, status: &PairStatus) -> Option<RetryInfo> {
        if status.last_status != Some(RunStatus::Failed)
            || !status.last_error_kind.is_some_and(ErrorKind::transient)
        {
            return None;
        }
        let delay = self.delay_after_ms(status.consecutive_failures)?;
        Some(RetryInfo {
            attempt: status.consecutive_failures,
            max_tries: self.tries,
            next_at: status
                .last_run_at
                .map(|at| at.saturating_add(i64::try_from(delay).unwrap_or(i64::MAX))),
        })
    }

    /// Whether `status` is a passing failure the policy has given up on: the folder needs the person.
    pub fn gave_up(&self, status: &PairStatus) -> bool {
        status.last_status == Some(RunStatus::Failed)
            && status.last_error_kind.is_some_and(ErrorKind::transient)
            && status.consecutive_failures >= self.tries
    }
}

/// [`RetryPolicy::plan`] under the policy every device runs.
pub fn plan(status: &PairStatus) -> Option<RetryInfo> {
    RetryPolicy::DEFAULT.plan(status)
}

/// A folder whose last run failed for a passing reason and which the engine will try again. Part of
/// the folder's status, so an app or an agent reads it instead of counting tries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryInfo {
    /// Runs that have failed in a row (1 after the first failure).
    pub attempt: u32,
    /// How many runs in a row the engine tries before it gives up.
    pub max_tries: u32,
    /// When the engine tries again, milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(kind: ErrorKind, failures: u32) -> PairStatus {
        PairStatus {
            last_run_at: Some(1_000_000),
            last_status: Some(RunStatus::Failed),
            last_error_kind: Some(kind),
            last_error: Some("boom".into()),
            consecutive_failures: failures,
            ..PairStatus::default()
        }
    }

    #[test]
    fn the_policy_is_pinned() {
        assert_eq!(5, MAX_TRIES);
        assert_eq!([30, 120, 600, 1800], BACKOFF_SECS);
        let p = RetryPolicy::DEFAULT;
        assert_eq!(5, p.tries);
        assert_eq!(
            vec![Some(30_000), Some(120_000), Some(600_000), Some(1_800_000), None],
            (1..=5).map(|n| p.delay_after_ms(n)).collect::<Vec<_>>(),
            "the 5th failure in a row is where it gives up"
        );
        assert_eq!(None, p.delay_after_ms(0), "nothing failed yet");
        assert_eq!(None, p.delay_after_ms(40));
        assert_eq!(
            p.tries as usize - 1,
            p.delays_ms.len(),
            "one pause between each two tries"
        );
        let waited: u64 = BACKOFF_SECS.iter().sum();
        assert_eq!(2_550, waited, "about 42 minutes before it asks the person");
    }

    #[test]
    fn only_passing_causes_are_retried() {
        for kind in [ErrorKind::Network, ErrorKind::RateLimit, ErrorKind::Server] {
            assert!(plan(&failed(kind, 1)).is_some(), "{kind:?}");
        }
        for kind in [
            ErrorKind::Auth,
            ErrorKind::Permission,
            ErrorKind::Quota,
            ErrorKind::NotFound,
            ErrorKind::Locked,
            ErrorKind::Io,
            ErrorKind::Integrity,
            ErrorKind::Other,
        ] {
            assert_eq!(None, plan(&failed(kind, 1)), "{kind:?} is lasting: no retry");
            assert!(!RetryPolicy::DEFAULT.gave_up(&failed(kind, 9)), "{kind:?}");
        }
    }

    #[test]
    fn a_retry_says_which_try_failed_and_when_the_next_one_is() {
        let info = plan(&failed(ErrorKind::Network, 2)).unwrap();
        assert_eq!(2, info.attempt);
        assert_eq!(MAX_TRIES, info.max_tries);
        assert_eq!(Some(1_000_000 + 120_000), info.next_at);
    }

    #[test]
    fn the_policy_gives_up_on_the_last_try_and_stays_given_up() {
        let p = RetryPolicy::DEFAULT;
        for n in 1..MAX_TRIES {
            let s = failed(ErrorKind::Server, n);
            assert!(plan(&s).is_some() && !p.gave_up(&s), "failure {n}");
        }
        for n in [MAX_TRIES, MAX_TRIES + 3] {
            let s = failed(ErrorKind::Server, n);
            assert!(plan(&s).is_none() && p.gave_up(&s), "failure {n}");
        }
    }

    #[test]
    fn a_run_that_did_not_fail_is_not_a_retry() {
        let mut s = failed(ErrorKind::Network, 1);
        s.last_status = Some(RunStatus::Success);
        assert_eq!(None, plan(&s));
        s.last_status = Some(RunStatus::Partial);
        assert_eq!(None, plan(&s));
        s.last_status = None;
        assert_eq!(None, plan(&s));
    }
}
