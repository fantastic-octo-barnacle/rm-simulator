// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Where host-side code reads the wall clock.
//!
//! Simulation time advances only through explicit ticks; the readings here pace
//! those ticks, expire pending work and estimate transit. Production reads
//! [`Instant::now`]. A test hands the same components a [`ManualTime`] it steps
//! by hand, so cadence assertions need no sleeps and no deadlines.
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A shared reading of the wall clock. Cloning shares one source.
#[derive(Clone)]
pub struct TimeSource(Arc<dyn Fn() -> Instant + Send + Sync>);

impl TimeSource {
    /// The real clock. Every production path uses this.
    pub fn system() -> Self {
        Self(Arc::new(Instant::now))
    }
    /// Wrap `read` as a clock source. Tests inject a manual clock this way.
    pub fn new(read: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        Self(Arc::new(read))
    }
    /// The source's current reading.
    pub fn now(&self) -> Instant {
        (self.0)()
    }
    /// How long ago `at` was, never negative even if the source rewinds.
    pub fn since(&self, at: Instant) -> Duration {
        self.now().saturating_duration_since(at)
    }
}
impl Default for TimeSource {
    fn default() -> Self {
        Self::system()
    }
}
impl std::fmt::Debug for TimeSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimeSource").finish_non_exhaustive()
    }
}

/// A clock that only moves when a test advances it. Readings are an offset from
/// a real instant, so mixing one real `Instant` in stays ordered rather than
/// panicking; cloning shares the offset with every holder of its [`TimeSource`].
#[derive(Clone)]
pub struct ManualTime {
    epoch: Instant,
    offset_ns: Arc<AtomicU64>,
}
impl ManualTime {
    /// A clock stopped at the real instant it was created. It moves only when
    /// [`advance`](ManualTime::advance) or [`advance_ms`](ManualTime::advance_ms)
    /// is called.
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            offset_ns: Arc::new(AtomicU64::new(0)),
        }
    }
    /// Move the clock forward by `by`, clamping a duration longer than
    /// `u64::MAX` nanoseconds.
    pub fn advance(&self, by: Duration) {
        let by = u64::try_from(by.as_nanos()).unwrap_or(u64::MAX);
        self.offset_ns.fetch_add(by, Ordering::Release);
    }
    /// Move the clock forward by `ms` milliseconds.
    pub fn advance_ms(&self, ms: u64) {
        self.advance(Duration::from_millis(ms));
    }
    /// The epoch plus the accumulated offset.
    pub fn now(&self) -> Instant {
        self.epoch + Duration::from_nanos(self.offset_ns.load(Ordering::Acquire))
    }
    /// A source every component can hold; it follows later advances.
    pub fn source(&self) -> TimeSource {
        let clock = self.clone();
        TimeSource::new(move || clock.now())
    }
}
impl Default for ManualTime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_readings_move_only_when_advanced_and_reach_every_holder() {
        let time = ManualTime::new();
        let source = time.source();
        let start = source.now();
        assert_eq!(source.now(), start);
        time.advance_ms(16);
        assert_eq!(source.since(start), Duration::from_millis(16));
        time.clone().advance(Duration::from_nanos(4));
        assert_eq!(source.since(start), Duration::from_nanos(16_000_004));
        assert!(TimeSource::system().now() >= start);
    }
}
