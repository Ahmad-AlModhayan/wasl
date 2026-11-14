//! A preview's progress and stopping it (the owner's phone, 2026-10-30: a 33 s LifeOS preview
//! showed only "Comparing" and a clock, and leaving the screen did not stop the work).

mod common;

use common::{PAIR, Slow, SlowOpens, World};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use wasl_engine::{EngineError, Phase};

fn stuck_listing() -> World {
    World::with(
        |_| {},
        Some(Box::new(|m| {
            Arc::new(Slow {
                inner: m,
                delay: Duration::ZERO,
                hang_downloads: false,
                hang_listing: true,
            })
        })),
    )
}

async fn until_previewing(w: &World) -> wasl_engine::Progress {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(p) = w.engine.preview_progress(PAIR) {
                return p;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the preview reports its progress")
}

#[tokio::test]
async fn a_preview_says_which_step_it_is_on() {
    let w = stuck_listing();
    let preview = w.engine.preview(PAIR);
    tokio::pin!(preview);
    let seen = tokio::select! {
        _ = &mut preview => panic!("the listing never answers"),
        p = until_previewing(&w) => p,
    };
    assert_eq!(Phase::ListingRemote, seen.phase);
    // A preview is not a run: nothing reads as syncing.
    assert!(w.engine.progress(PAIR).is_none());
    assert!(!w.engine.is_running(PAIR));
}

#[tokio::test]
async fn a_finished_preview_reports_nothing() {
    let w = World::new();
    w.write("a.txt", "a");
    w.engine.preview(PAIR).await.unwrap();
    assert!(w.engine.preview_progress(PAIR).is_none());
}

#[tokio::test]
async fn stopping_a_preview_ends_it_whatever_step_it_is_in() {
    let w = Arc::new(stuck_listing());
    let previewing = {
        let w = w.clone();
        tokio::spawn(async move { w.engine.preview(PAIR).await.map(|_| ()) })
    };
    until_previewing(&w).await;
    w.engine.cancel_preview(PAIR);
    let r = tokio::time::timeout(Duration::from_secs(5), previewing)
        .await
        .expect("the preview ends once stopped")
        .unwrap();
    assert!(matches!(r, Err(EngineError::Cancelled)), "{r:?}");
    assert!(w.engine.preview_progress(PAIR).is_none());
}

/// Leaving the screen drops the preview (the app cancels its coroutine): its work stops with it.
#[tokio::test]
async fn a_dropped_preview_is_stopped_and_forgotten() {
    let w = stuck_listing();
    {
        let preview = w.engine.preview(PAIR);
        tokio::pin!(preview);
        tokio::select! {
            _ = &mut preview => panic!("the listing never answers"),
            _ = until_previewing(&w) => {},
        }
    }
    assert!(w.engine.preview_progress(PAIR).is_none());
}

/// Two previews of one pair (another window, the CLI): the newer one is reported, and the earlier
/// one ending does not hide it.
#[tokio::test]
async fn the_newest_preview_of_a_pair_is_the_one_reported() {
    let w = Arc::new(stuck_listing());
    let first = {
        let w = w.clone();
        tokio::spawn(async move { w.engine.preview(PAIR).await.map(|_| ()) })
    };
    until_previewing(&w).await;
    let second = w.engine.preview(PAIR);
    tokio::pin!(second);
    tokio::select! {
        _ = &mut second => panic!("the listing never answers"),
        () = tokio::time::sleep(Duration::from_millis(50)) => {},
    }
    first.abort();
    let _ = first.await;
    assert!(
        w.engine.preview_progress(PAIR).is_some(),
        "the newer preview is still reported"
    );
}

/// A first preview reads every file both sides have (5.3 GB for the owner's LifeOS): leaving the
/// screen stops those reads between files, not after the last one.
#[tokio::test]
async fn a_dropped_preview_stops_reading_files() {
    let w = World::new();
    for i in 0..40 {
        w.write(&format!("f{i}.txt"), &format!("same {i}"));
        w.put_drive(&format!("f{i}.txt"), &format!("same {i}"));
    }
    let slow = Arc::new(SlowOpens {
        delay: Duration::from_millis(50),
        ..SlowOpens::default()
    });
    *w.env.slow_opens.lock().unwrap() = Some(slow.clone());
    {
        let preview = w.engine.preview(PAIR);
        tokio::pin!(preview);
        tokio::select! {
            _ = &mut preview => panic!("40 slow files take longer"),
            () = async {
                while slow.opened.load(Ordering::SeqCst) == 0 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            } => {},
        }
        let p = w.engine.preview_progress(PAIR).unwrap();
        assert_eq!(Phase::Planning, p.phase, "checking files is the comparing step");
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    let opened = slow.opened.load(Ordering::SeqCst);
    assert!(
        opened <= 8,
        "{opened} of 40 files read after the preview was dropped"
    );
}
