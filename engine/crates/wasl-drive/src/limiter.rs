//! Token-bucket bandwidth limiter shared by the client's streaming loops (port of
//! `BandwidthLimiter.kt`). A rate of 0 means unlimited.
//!
//! Unlike the Kotlin `object`, a limiter is a cheap, cloneable handle: the client uses
//! [`BandwidthLimiter::global`] unless configured otherwise, and tests use their own instance.
//! The bucket runs on tokio's clock, so it follows paused time in tests.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::time::Instant;

#[derive(Clone, Debug, Default)]
pub struct BandwidthLimiter {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    bytes_per_second: AtomicU64,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    /// Bytes that may be sent now. Negative after a request bigger than the bucket: that is debt.
    allowance: f64,
    last_refill: Option<Instant>,
}

impl BandwidthLimiter {
    /// A new, unlimited limiter.
    pub fn new() -> Self {
        Self::default()
    }

    /// The process-wide limiter (the Kotlin `object BandwidthLimiter`).
    pub fn global() -> &'static BandwidthLimiter {
        static GLOBAL: OnceLock<BandwidthLimiter> = OnceLock::new();
        GLOBAL.get_or_init(BandwidthLimiter::new)
    }

    pub fn bytes_per_second(&self) -> u64 {
        self.inner.bytes_per_second.load(Ordering::Relaxed)
    }

    /// Sets the limit; 0 = unlimited.
    pub fn set_bytes_per_second(&self, rate: u64) {
        self.inner.bytes_per_second.store(rate, Ordering::Relaxed);
    }

    /// Waits as long as needed so that `bytes` fit the configured rate.
    pub async fn acquire(&self, bytes: usize) {
        loop {
            let wait = self.take(bytes, Instant::now());
            if wait == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(wait)).await;
        }
    }

    /// Blocking variant for code that runs on a plain thread (never call it on the async runtime).
    pub fn acquire_blocking(&self, bytes: usize) {
        loop {
            let wait = self.take(bytes, Instant::now());
            if wait == 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(wait));
        }
    }

    /// Takes `bytes` from the bucket and returns 0, or returns how many milliseconds to wait before
    /// asking again.
    fn take(&self, bytes: usize, now: Instant) -> u64 {
        let rate = self.bytes_per_second();
        if rate == 0 || bytes == 0 {
            return 0;
        }
        let rate = rate as f64;
        let mut s = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
        let last = *s.last_refill.get_or_insert(now);
        let elapsed = now.saturating_duration_since(last).as_secs_f64();
        s.allowance = rate.min(s.allowance + elapsed * rate);
        s.last_refill = Some(now);
        // The bucket never holds more than one second of bandwidth, so a bigger request would wait
        // forever. Let it through once the bucket is full and carry the rest as debt: the average
        // rate still holds.
        let need = (bytes as f64).min(rate);
        if s.allowance >= need {
            s.allowance -= bytes as f64;
            0
        } else {
            ((((need - s.allowance) / rate) * 1000.0) as u64).max(5)
        }
    }
}
