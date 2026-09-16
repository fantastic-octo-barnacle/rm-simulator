// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Bounded local network display; opening it never sends a request to the host.
use crate::{hud::HudState, session::Session};
use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
};
use std::collections::VecDeque;

/// How much of the network display is shown. It is a display choice only:
/// opening it never sends a request to the host.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum NetworkStatsMode {
    /// No overlay.
    #[default]
    Off,
    /// One line with the round trip in ms and the receive and send rates in KiB/s.
    Compact,
    /// Adds frame time, send queue, update gap, input lead and backlog, plus 60 s
    /// p95-and-trace rows.
    Detailed,
}
impl NetworkStatsMode {
    /// The next mode in the settings cycle: off to compact to detailed to off.
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Compact,
            Self::Compact => Self::Detailed,
            Self::Detailed => Self::Off,
        }
    }
}
/// Marks the overlay text node this module spawns on its first update.
#[derive(Component)]
pub struct NetworkOverlay;
/// Rolling samples behind the detailed traces. One sample is pushed every
/// 0.25 s and holds a round trip, a receive rate, an update gap and a correction
/// distance; the oldest is dropped past 240 samples, so a trace spans 60 s.
#[derive(Default)]
pub struct History {
    last: f64,
    samples: VecDeque<[Option<f64>; 4]>,
}
fn number(value: Option<f64>) -> String {
    value.map_or_else(
        || "--".into(),
        |v| {
            if v > 0. && v < 10. {
                format!("{v:.2}")
            } else {
                format!("{v:.0}")
            }
        },
    )
}
fn spark(samples: &VecDeque<[Option<f64>; 4]>, column: usize) -> String {
    let max = samples.iter().filter_map(|s| s[column]).fold(1., f64::max);
    // 60 columns, each the maximum of four 4 Hz observations, spanning 60 seconds.
    let values: Vec<_> = samples.iter().collect();
    values
        .chunks(4)
        .map(|chunk| {
            let peak = chunk.iter().filter_map(|s| s[column]).reduce(f64::max);
            peak.map_or(' ', |v| {
                ['_', '.', ':', '-', '=', '+', '*', '#'][(v / max * 7.).round() as usize]
            })
        })
        .collect()
}
fn p95(samples: &VecDeque<[Option<f64>; 4]>, column: usize) -> Option<f64> {
    let mut values: Vec<_> = samples.iter().filter_map(|s| s[column]).collect();
    if values.len() < 20 {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(values[((values.len() as f64 * 0.95).ceil() as usize - 1).min(values.len() - 1)])
}
/// Spawn the hidden overlay on the first run, then refresh it every 0.25 s from
/// the session's local stats, the correction diagnostics and the frame-time
/// diagnostic. Rates come from the transport's own accounting and are shown as
/// KiB/s; loss is shown as unavailable rather than as zero.
#[allow(clippy::too_many_arguments)]
pub fn update(
    mut commands: Commands,
    session: Res<Session>,
    ui: Res<HudState>,
    time: Res<Time>,
    diagnostics: Res<DiagnosticsStore>,
    mut history: Local<History>,
    mut overlay: Query<(&mut Text, &mut Node), With<NetworkOverlay>>,
    aim: Option<Res<crate::auto_aim::AutoAim>>,
) {
    if overlay.is_empty() {
        commands.spawn((
            NetworkOverlay,
            Text::new(""),
            TextFont {
                font_size: bevy::text::FontSize::Px(13.),
                ..default()
            },
            TextColor(Color::srgb(0.88, 0.94, 0.96)),
            BackgroundColor(Color::srgba(0.02, 0.03, 0.04, 0.8)),
            bevy::picking::Pickable::IGNORE,
            GlobalZIndex(3),
            Node {
                position_type: PositionType::Absolute,
                top: px(100),
                left: px(16),
                width: px(450),
                max_width: percent(45),
                padding: UiRect::all(px(6)),
                display: Display::None,
                ..default()
            },
        ));
        return;
    }
    let Ok((mut text, mut node)) = overlay.single_mut() else {
        return;
    };
    let display = if ui.network_stats == NetworkStatsMode::Off {
        Display::None
    } else {
        Display::Flex
    };
    if node.display != display {
        node.display = display;
    }
    if time.elapsed_secs_f64() - history.last < 0.25 {
        return;
    }
    history.last = time.elapsed_secs_f64();
    let stats = session.network_stats();
    let native = stats
        .native
        .as_ref()
        .filter(|_| !stats.stale && stats.native_sample_age_ms.is_some_and(|age| age < 1000.));
    let ping = (!stats.stale)
        .then(|| native.and_then(|n| n.transport_rtt_ms).or(stats.app_rtt_ms))
        .flatten();
    let rx = native
        .and_then(|n| n.receive_bytes_per_s)
        .map(|v| v / 1024.);
    let tx = native.and_then(|n| n.send_bytes_per_s).map(|v| v / 1024.);
    let detail = session.network_diagnostics();
    let gap = (!stats.stale)
        .then(|| detail["checkpoint_gap_ms"].as_f64())
        .flatten();
    let correction = detail["correction_position_m"].as_f64().map(|v| v * 1000.);
    if history.samples.len() == 240 {
        history.samples.pop_front();
    }
    history.samples.push_back([ping, rx, gap, correction]);
    let title = if stats.transport == "local" {
        "Local"
    } else if stats.stale {
        "Disconnected (stale)"
    } else if native.is_some() {
        "Ping"
    } else {
        "App RTT"
    };
    let mut value = format!(
        "{title} {} ms   Loss --\nReceive {} KiB/s   Send {} KiB/s",
        number(ping),
        number(rx),
        number(tx)
    );
    if ui.network_stats == NetworkStatsMode::Detailed {
        let trace = &detail["trace"];
        let status = if trace["error"].is_string() {
            "failed"
        } else if trace["capped"].as_bool() == Some(true) {
            "size limit reached"
        } else if trace["path"].is_string() {
            "recording"
        } else {
            "off"
        };
        value.push_str(&format!(
            "\nTrace {status}   Dropped {}   Contended {}",
            trace["dropped"].as_u64().unwrap_or(0),
            trace["contended"].as_u64().unwrap_or(0)
        ));

        let frame = diagnostics
            .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
            .and_then(|d| d.smoothed());
        value.push_str(&format!("\nFrame {} ms   Queue {} ms\nUpdate gap {} ms   Lead {} ms   Backlog {} ms\nLoss unavailable; rates are GNS estimates\n60 s, auto-scaled traces; p95 / sample count", number(frame), number(native.and_then(|n| n.send_queue_ms)), number(gap), number(detail["input_lead_ms"].as_f64()), number(detail["prediction_backlog_ms"].as_f64())));
        if let Some(aim) = &aim {
            value.push_str(&format!(
                "\nAim {:.0}ms {}   stale {} track {} blocked {} firing {}",
                aim.observation_age_ms.max(0.),
                if aim.last_gate.is_empty() {
                    "--"
                } else {
                    aim.last_gate.as_str()
                },
                aim.stale_frames,
                aim.tracking_only_frames,
                aim.blocked_frames,
                aim.firing_frames,
            ));
        }
        for (i, label) in ["RTT ms", "Rx KiB/s", "Gap ms", "Correction mm"]
            .iter()
            .enumerate()
        {
            let count = history.samples.iter().filter(|s| s[i].is_some()).count();
            value.push_str(&format!(
                "\n{label} {} / {count}\n{}",
                number(p95(&history.samples, i)),
                spark(&history.samples, i)
            ));
        }
    }
    text.set_if_neq(Text::new(value));
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_samples_are_gaps_and_percentiles_need_evidence() {
        let mut samples = VecDeque::from([[None; 4]; 20]);
        assert_eq!(spark(&samples, 0), "     ");
        assert_eq!(p95(&samples, 0), None);
        for (i, sample) in samples.iter_mut().enumerate() {
            sample[0] = Some(i as f64);
        }
        assert_eq!(p95(&samples, 0), Some(18.));
    }
}
