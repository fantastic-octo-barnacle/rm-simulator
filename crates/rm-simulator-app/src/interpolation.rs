// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Remote view timing. Local input and prediction use their own immediate timeline.
/// The bounded record of remote states the session samples a view time into.
pub use rm_simulator_server::view::RemoteHistory;
use std::collections::VecDeque;

/// Remote delay in ns before arrival jitter has been measured. Local input and
/// prediction never use it.
pub const DEFAULT_DELAY_NS: u64 = 64_000_000;
const MAX_DELAY_NS: u64 = 250_000_000;
/// How far back the arrival-age percentile looks. A span, not a frame count:
/// the same seconds of jitter inform the delay at 60 and at 240 fps.
const AGE_WINDOW_NS: u64 = 2_000_000_000;
/// Memory bound for that span at any frame rate; a faster client thins its
/// history rather than growing it.
const MAX_AGE_SAMPLES: usize = 1024;

/// Remote view timing for one session. It chooses which world time to draw, so
/// peers and equipment appear at a steady rate instead of the arrival pattern
/// of their snapshots. Own input never goes through it.
pub struct Buffer {
    /// Whether the delay follows measured arrival jitter. `adjust_manual`
    /// clears it, and `reset_timeline` preserves it.
    pub automatic: bool,
    /// The delay the player chose, in ms, clamped to 0..=250. It applies while
    /// `automatic` is false.
    pub manual_ms: u64,
    delay_ns: u64,
    /// Sample time and the arrival age observed then.
    ages: VecDeque<(u64, u64)>,
    view_ns: Option<u64>,
    last_frame_ns: u64,
    /// Age of the presented view time in ms, for the network overlay and the
    /// console's `state` reply.
    pub view_age_ms: f64,
    /// Underruns so far, each counted on entry into an underrun rather than once
    /// per frame.
    pub underruns: u64,
    underrunning: bool,
}
impl Default for Buffer {
    fn default() -> Self {
        Self {
            automatic: true,
            manual_ms: 64,
            delay_ns: DEFAULT_DELAY_NS,
            ages: VecDeque::new(),
            view_ns: None,
            last_frame_ns: 0,
            view_age_ms: 0.,
            underruns: 0,
            underrunning: false,
        }
    }
}
impl Buffer {
    /// The delay in force, in whole ms.
    pub fn delay_ms(&self) -> u64 {
        self.delay_ns / 1_000_000
    }
    /// Move the manual delay by `delta_ms`, saturating at 0 and 250 ms, and
    /// select manual mode. Dropping the view time lets the next sample move the
    /// view backwards to the new delay.
    pub fn adjust_manual(&mut self, delta_ms: i64) {
        self.manual_ms = self.manual_ms.saturating_add_signed(delta_ms).min(250);
        self.automatic = false;
        self.view_ns = None;
    }
    /// Forget the arrival history and the view time, keeping the mode and the
    /// manual delay. A new life or a changed clock starts from a clean view.
    pub fn reset_timeline(&mut self) {
        let automatic = self.automatic;
        let manual_ms = self.manual_ms;
        *self = Self::default();
        self.automatic = automatic;
        self.manual_ms = manual_ms;
    }
    /// The world time in ns to present this frame. `now_ns` is the app clock,
    /// `bounds` the oldest and newest snapshot times the session holds, and
    /// `remote` enables underrun counting; without bounds the app clock is
    /// returned unchanged. Pausing pins the view to the newest snapshot. The
    /// result never moves backwards, never precedes the oldest snapshot and
    /// runs at most 100 ms past the newest. In automatic mode the delay is the
    /// 95th percentile arrival age plus 16 ms, clamped to 32..=250 ms, rising at
    /// once and falling at 50 ms per second.
    pub fn sample_time(
        &mut self,
        now_ns: u64,
        bounds: Option<(u64, u64)>,
        paused: bool,
        remote: bool,
    ) -> u64 {
        let Some((first, latest)) = bounds else {
            return now_ns;
        };
        if paused {
            self.view_ns = Some(latest);
            self.last_frame_ns = now_ns;
            self.ages.clear();
            self.underrunning = false;
            self.view_age_ms = 0.;
            return latest;
        }
        let elapsed = now_ns.saturating_sub(self.last_frame_ns).min(100_000_000);
        self.last_frame_ns = now_ns;
        self.ages.push_back((now_ns, now_ns.saturating_sub(latest)));
        while self
            .ages
            .front()
            .is_some_and(|(at, _)| now_ns.saturating_sub(*at) > AGE_WINDOW_NS)
            || self.ages.len() > MAX_AGE_SAMPLES
        {
            self.ages.pop_front();
        }
        if self.automatic {
            let mut ages: Vec<_> = self.ages.iter().map(|(_, age)| *age).collect();
            ages.sort_unstable();
            let mut desired = ages[(ages.len() - 1) * 95 / 100]
                .saturating_add(16_000_000)
                .clamp(32_000_000, MAX_DELAY_NS);
            if self.underrunning {
                desired = desired
                    .max(now_ns.saturating_sub(latest).saturating_add(16_000_000))
                    .min(MAX_DELAY_NS);
            }
            // Increase promptly; release delay at 50 ms per second.
            self.delay_ns = if desired > self.delay_ns {
                desired
            } else {
                self.delay_ns.saturating_sub(elapsed / 20).max(desired)
            };
        } else {
            self.delay_ns = self.manual_ms.min(250) * 1_000_000;
        }
        let target = now_ns.saturating_sub(self.delay_ns);
        let view = target
            .max(self.view_ns.unwrap_or(target))
            .max(first)
            .min(latest.saturating_add(100_000_000));
        let underrunning = remote && view > latest;
        if underrunning && !self.underrunning {
            self.underruns += 1;
        }
        self.underrunning = underrunning;
        self.view_ns = Some(view);
        self.view_age_ms = now_ns.saturating_sub(view) as f64 / 1e6;
        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jitter_increases_delay_and_recovery_is_bounded_and_monotonic() {
        let mut buffer = Buffer::default();
        let mut previous = 0;
        for tick in 1..300 {
            let now = tick * 16_000_000;
            let age = if tick < 130 { 140_000_000 } else { 16_000_000 };
            let view = buffer.sample_time(now, Some((0, now.saturating_sub(age))), false, true);
            assert!(view >= previous);
            previous = view;
        }
        assert!(buffer.delay_ms() > 100);
        assert!(buffer.delay_ms() < 140);
    }
    #[test]
    fn the_jitter_window_is_a_time_span_not_a_frame_count() {
        let mut buffer = Buffer::default();
        // Three seconds at 240 fps: a frame-count window would hold half a second.
        for tick in 1..=720 {
            let now = tick * 4_166_666;
            buffer.sample_time(now, Some((0, now.saturating_sub(16_000_000))), false, true);
        }
        let span = buffer.ages.back().unwrap().0 - buffer.ages.front().unwrap().0;
        assert!(buffer.ages.len() > 120);
        assert!(
            span > AGE_WINDOW_NS - 10_000_000 && span <= AGE_WINDOW_NS,
            "{span}"
        );
    }
    #[test]
    fn manual_bounds_extrapolation_hold_and_pause_reset() {
        let mut buffer = Buffer::default();
        buffer.adjust_manual(-1000);
        assert_eq!(buffer.manual_ms, 0);
        let time = buffer.sample_time(500_000_000, Some((0, 100_000_000)), false, true);
        assert_eq!(time, 200_000_000);
        assert_eq!(buffer.underruns, 1);
        assert_eq!(
            buffer.sample_time(600_000_000, Some((0, 100_000_000)), false, true),
            time
        );
        assert_eq!(buffer.underruns, 1);
        assert_eq!(
            buffer.sample_time(600_000_000, Some((0, 100_000_000)), true, true),
            100_000_000
        );
        buffer.adjust_manual(1000);
        assert_eq!(buffer.manual_ms, 250);
        buffer.reset_timeline();
        assert!(!buffer.automatic);
        assert_eq!(buffer.manual_ms, 250);
    }
}
