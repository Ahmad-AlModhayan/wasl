//! Port of `BandwidthLimiterTest.kt`. Transfers ask the limiter for up to 256 KB at a time. A limit
//! below that (a restored backup can carry any value) must slow transfers down, never stop them.
//!
//! Each test uses its own limiter instead of the global one, so tests cannot interfere. The async
//! tests run on paused tokio time; the blocking variant necessarily sleeps for real (about 1 s).

use std::sync::mpsc;
use std::time::Duration;

use tokio::time::{Instant, timeout};
use wasl_drive::BandwidthLimiter;

const CHUNK: usize = 256 * 1024;

#[tokio::test(start_paused = true)]
async fn a_chunk_larger_than_one_second_of_bandwidth_still_goes_through() {
    let limiter = BandwidthLimiter::new();
    limiter.set_bytes_per_second(100 * 1024);

    let done = timeout(Duration::from_secs(3), limiter.acquire(CHUNK)).await;

    assert!(
        done.is_ok(),
        "a 256 KB chunk waited forever under a 100 KB/s limit"
    );
}

#[test]
fn the_blocking_variant_also_lets_a_large_chunk_through() {
    let limiter = BandwidthLimiter::new();
    limiter.set_bytes_per_second(100 * 1024);
    let (tx, rx) = mpsc::channel();
    let l = limiter.clone();
    std::thread::spawn(move || {
        l.acquire_blocking(CHUNK);
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(3))
        .expect("the blocking acquire did not return within 3 s");
}

#[tokio::test(start_paused = true)]
async fn large_chunks_still_keep_to_the_limit_on_average() {
    let limiter = BandwidthLimiter::new();
    limiter.set_bytes_per_second(100 * 1024);
    assert!(
        timeout(Duration::from_secs(3), limiter.acquire(CHUNK))
            .await
            .is_ok(),
        "never hang the suite on a regression"
    );

    // The first chunk borrowed about 156 KB; the next one waits for that to be paid back.
    let start = Instant::now();
    let _ = timeout(Duration::from_secs(5), limiter.acquire(CHUNK)).await;
    let second = start.elapsed().as_millis();

    assert!(
        second >= 1_500,
        "the second chunk went through in {second} ms, faster than the limit allows"
    );
}

#[tokio::test(start_paused = true)]
async fn unlimited_never_waits() {
    let limiter = BandwidthLimiter::new();
    let start = Instant::now();
    for _ in 0..100 {
        limiter.acquire(CHUNK).await;
    }
    assert_eq!(Duration::ZERO, start.elapsed());
}
