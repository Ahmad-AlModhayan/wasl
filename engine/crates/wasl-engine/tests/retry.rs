//! A passing failure is retried by the engine and becomes "needs you" only when the retry policy
//! gives up (issue #30). Whole runs against a real folder and the in-memory Drive, then the folder's
//! status as the daemon hands it out.

mod common;

use common::World;
use wasl_core::{ErrorKind, RunStatus};
use wasl_drive::DriveError;
use wasl_engine::Op;
use wasl_proto::retry::MAX_TRIES;
use wasl_proto::{Fix, FolderState, FolderStatus, Lang, OverallState, WaitReason, overall_status};

fn status(w: &World) -> FolderStatus {
    w.engine
        .folder_statuses(Lang::En, &|_| Some(10_000_000), &mut |_| {})
        .unwrap()
        .remove(0)
}

fn keys(s: &FolderStatus) -> Vec<&str> {
    s.needs_you.iter().map(|r| r.key.as_str()).collect()
}

/// The ways Drive fails that pass: it can't be reached, asks Wasl to slow down, has a problem.
fn passing(n: u32) -> DriveError {
    match n % 3 {
        0 => DriveError::Network("connection reset".into()),
        1 => DriveError::RateLimited {
            retry_after_ms: 0,
            message: "slow down".into(),
        },
        _ => DriveError::Http {
            code: 503,
            reason: None,
            message: "backend error".into(),
        },
    }
}

#[tokio::test]
async fn one_passing_failure_then_success_is_never_a_problem() {
    let w = World::new();
    w.write("a.txt", "a");
    assert_eq!(RunStatus::Success, w.sync().await.status);

    w.write("b.txt", "b");
    w.drive
        .fail_next(Op::List, DriveError::Network("connection reset".into()));
    let run = w.sync().await;
    assert_eq!(RunStatus::Failed, run.status, "{:?}", run.message);

    let s = status(&w);
    assert_eq!(FolderState::Waiting, s.state, "{s:?}");
    assert!(s.needs_you.is_empty(), "no problem item: {s:?}");
    let retry = s.retry.expect("the engine says it will try again");
    assert_eq!((1, MAX_TRIES), (retry.attempt, retry.max_tries));
    assert!(retry.next_at.is_some());
    let overall = overall_status(&[s], 0, Lang::En);
    assert_eq!((OverallState::Waiting, 0), (overall.state, overall.needs_you));

    assert_eq!(RunStatus::Success, w.sync().await.status);
    let s = status(&w);
    assert_eq!(FolderState::Synced, s.state, "{s:?}");
    assert!(s.needs_you.is_empty() && s.retry.is_none());
    assert_eq!(0, w.pair().status.consecutive_failures);
}

#[tokio::test]
async fn failures_that_pass_ask_for_the_person_once_at_give_up() {
    let w = World::new();
    w.write("a.txt", "a");
    assert_eq!(RunStatus::Success, w.sync().await.status);

    for n in 1..=MAX_TRIES {
        w.drive.fail_next(Op::List, passing(n));
        assert_eq!(RunStatus::Failed, w.sync().await.status, "failure {n}");
        let s = status(&w);
        if n < MAX_TRIES {
            assert!(
                matches!(s.state, FolderState::Waiting),
                "failure {n} is still being retried: {s:?}"
            );
            assert!(s.needs_you.is_empty(), "failure {n} raises no item: {s:?}");
            assert_eq!(Some(n), s.retry.map(|r| r.attempt));
            assert_eq!(0, overall_status(&[s], 0, Lang::En).needs_you);
        } else {
            assert_eq!(FolderState::NeedsYou, s.state, "give-up: {s:?}");
            assert_eq!(1, s.needs_you.len(), "exactly one item: {s:?}");
            assert_eq!(Some(Fix::Retry), s.needs_you[0].fix);
            assert!(s.retry.is_none());
            assert_eq!(1, overall_status(&[s], 0, Lang::En).needs_you);
        }
    }

    // Still failing after it gave up: still the one item, not a new one each run.
    w.drive.fail_next(Op::List, passing(0));
    assert_eq!(RunStatus::Failed, w.sync().await.status);
    let s = status(&w);
    assert_eq!((FolderState::NeedsYou, 1), (s.state, s.needs_you.len()), "{s:?}");

    // "Try again" works once Drive does.
    assert_eq!(RunStatus::Success, w.sync().await.status);
    let s = status(&w);
    assert_eq!((FolderState::Synced, 0), (s.state, s.needs_you.len()), "{s:?}");
}

#[tokio::test]
async fn the_cause_of_the_give_up_is_the_last_one() {
    let w = World::new();
    w.write("a.txt", "a");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    for _ in 0..MAX_TRIES {
        w.drive
            .fail_next(Op::List, DriveError::Network("connection reset".into()));
        w.sync().await;
    }
    let s = status(&w);
    assert_eq!(vec!["error.network"], keys(&s));
    assert_eq!(Some("connection reset"), s.needs_you[0].details.as_deref());
}

#[tokio::test]
async fn a_lasting_failure_needs_you_at_once_without_a_retry() {
    // Signed out.
    let w = World::new();
    w.write("a.txt", "a");
    w.drive.fail_next(
        Op::Upload,
        DriveError::Auth {
            message: "Sign in again".into(),
            user_action_required: true,
        },
    );
    let run = w.sync().await;
    assert_eq!(
        (RunStatus::Failed, Some(ErrorKind::Auth)),
        (run.status, run.error_kind)
    );
    let s = status(&w);
    assert_eq!(FolderState::NeedsYou, s.state, "{s:?}");
    assert_eq!(vec!["error.auth"], keys(&s));
    assert_eq!(Some(Fix::SignInAgain), s.needs_you[0].fix);
    assert!(s.retry.is_none(), "nothing to retry: {s:?}");
    assert_eq!(1, w.pair().status.consecutive_failures);

    // The Drive folder is gone.
    let w = World::new();
    w.write("a.txt", "a");
    w.drive
        .fail_next(Op::List, DriveError::NotFound("File not found".into()));
    let run = w.sync().await;
    assert_eq!(
        (RunStatus::Failed, Some(ErrorKind::NotFound)),
        (run.status, run.error_kind),
        "{:?}",
        run.message
    );
    let s = status(&w);
    assert_eq!(FolderState::NeedsYou, s.state, "{s:?}");
    assert_eq!(vec!["error.not_found"], keys(&s));
    assert!(s.retry.is_none());
}

#[tokio::test]
async fn a_run_that_finishes_in_part_starts_the_count_again() {
    let w = World::new();
    w.write("good.txt", "1");
    w.write("refused.txt", "x");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    w.drive.fail_next(Op::List, passing(0));
    w.sync().await;
    w.drive.fail_next(Op::List, passing(0));
    w.sync().await;
    assert_eq!(2, w.pair().status.consecutive_failures);

    // Drive answers, and one file it refuses: the run finished, so it is not a failure in a row.
    w.write("refused2.txt", "y");
    w.drive.poison("refused2.txt");
    assert_eq!(RunStatus::Partial, w.sync().await.status);
    assert_eq!(0, w.pair().status.consecutive_failures);
    let s = status(&w);
    assert_eq!(FolderState::PartlySynced, s.state, "{s:?}");
    assert!(s.retry.is_none());

    w.drive.fail_next(Op::List, passing(0));
    w.sync().await;
    assert_eq!(Some(1), status(&w).retry.map(|r| r.attempt));
}

#[tokio::test]
async fn a_failure_while_the_device_is_offline_waits_for_the_network_not_for_the_person() {
    let w = World::new();
    w.write("a.txt", "a");
    assert_eq!(RunStatus::Success, w.sync().await.status);
    w.drive
        .fail_next(Op::List, DriveError::Network("unreachable".into()));
    w.sync().await;
    let s = status(&w);
    assert_eq!(
        (FolderState::Waiting, Some(WaitReason::NoNetwork)),
        (s.state, s.wait)
    );
}
