// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Shared training workloads; kept separate from bandwidth evaluation runs.
use rm_simulator_server::{protocol::Command, simulation::SimulationState, workload};
use rm_simulator_world::{ChassisCommand, ChassisSnapshot, RefereeCommand, Team};

/// Production publication period in world time, in milliseconds. The run is
/// scheduled in duration, not ticks: a 32 ms frame is four whole 128 Hz ticks
/// plus a remainder, and the remainder accumulates.
const STEP_MS: u64 = 32;
/// Fixed command-jitter seed; unchanged since the original dictionary trainer.
const SEED: u64 = 0x5eed_2026_0914;
/// One deterministic synthetic gameplay run.
pub struct Scenario {
    /// Name used in the printed record.
    pub name: &'static str,
    /// Chassis spawned at the start, alternating teams.
    pub chassis: usize,
    /// Checkpoints published, one per `STEP_MS`.
    pub frames: u64,
    /// Frames between two training-bot join/leave events, or zero to disable.
    bot_churn: u64,
    /// Frames between two fire bursts, or zero for a run that never shoots.
    fire_period: u64,
    /// Whether the referee starts the match, which opens rune opportunities
    /// and the match clock.
    match_running: bool,
}

/// The training runs: an idle field, two duellists, a bot-churning skirmish, a
/// crowded melee and a referee round with rune activity.
pub const SCENARIOS: [Scenario; 5] = [
    Scenario {
        name: "idle",
        chassis: 0,
        frames: 500,
        bot_churn: 0,
        fire_period: 0,
        match_running: false,
    },
    Scenario {
        name: "patrol",
        chassis: 2,
        frames: 700,
        bot_churn: 0,
        fire_period: 96,
        match_running: false,
    },
    Scenario {
        name: "skirmish",
        chassis: 4,
        frames: 600,
        bot_churn: 48,
        fire_period: 32,
        match_running: true,
    },
    Scenario {
        name: "melee",
        chassis: 12,
        frames: 400,
        bot_churn: 0,
        fire_period: 24,
        match_running: true,
    },
    Scenario {
        name: "rune",
        chassis: 4,
        frames: 500,
        bot_churn: 64,
        fire_period: 40,
        match_running: true,
    },
];

/// Independent checkpoint states from one deterministic training run.
pub fn checkpoints(scenario: &Scenario) -> Vec<SimulationState> {
    let (mut simulation, pilots) = workload::simulation(scenario.chassis);
    let mut bots = Vec::new();
    if scenario.match_running {
        simulation
            .apply(&Command::Referee(RefereeCommand::StartMatch))
            .unwrap();
    }
    let mut rng = Rng::new(SEED);
    let mut schedule = rm_simulator_server::simulation::TickSchedule::default();
    let mut samples = Vec::new();
    for frame in 0..scenario.frames {
        // Drive from the last published checkpoint, the one-frame-old view a
        // real client acts on, so aiming lags the field the way it does live.
        let observed = simulation.state();
        for (index, pilot) in pilots.iter().copied().enumerate() {
            if observed
                .field
                .chassis
                .iter()
                .any(|c| c.id == pilot && c.defeated)
            {
                let _ = simulation.apply(&Command::Respawn { chassis: pilot });
                continue;
            }
            let command = drive(index, frame, pilot, &observed.field.chassis, &mut rng);
            let _ = simulation.apply(&Command::Chassis {
                chassis: pilot,
                command,
            });
            if scenario.fire_period != 0
                && frame.is_multiple_of(scenario.fire_period + index as u64)
            {
                let _ = simulation.apply(&Command::Fire { shooter: pilot });
            }
        }
        if scenario.bot_churn != 0 && frame.is_multiple_of(scenario.bot_churn) && frame > 0 {
            // Bots join and leave, which changes collection lengths and forces
            // whole-array replacements in the delta encoder.
            if frame / scenario.bot_churn % 2 == 1 && bots.len() < 4 {
                let _ = simulation.apply(&Command::SpawnBot {
                    team: if bots.len() % 2 == 0 {
                        Team::Red
                    } else {
                        Team::Blue
                    },
                    spin_rad_s: 1.5,
                });
                bots = simulation.state().bots;
            } else if let Some(bot) = bots.pop() {
                let _ = simulation.apply(&Command::RemoveBot { chassis: bot });
            }
        }
        let advance = schedule.advance(STEP_MS * 1_000_000);
        if advance > 0 {
            simulation.step(advance).unwrap();
        }
        let mut state = simulation.state();
        state.snapshot_id = frame + 1;
        samples.push(state);
    }
    samples
}

/// One chassis' control for this frame: a deliberately varied mix of driving
/// modes plus a turret that tracks the nearest enemy with a little wobble, so
/// the checkpoint stream exercises acceleration, braking, strafing, spinning
/// and turret motion instead of one constant command.
fn drive(
    index: usize,
    frame: u64,
    pilot: u32,
    chassis: &[ChassisSnapshot],
    rng: &mut Rng,
) -> ChassisCommand {
    // Change driving mode a few times a second; the sine terms vary the
    // command within a mode.
    let mode = (frame / 12 + index as u64 * 3) % 6;
    let phase = frame as f64 * 0.15 + index as f64 * 1.7;
    let (forward_m_s, left_m_s, yaw_rate_rad_s) = match mode {
        0 => (3.0, 0.0, 0.0),                             // straight at speed
        1 => (2.0, 1.6, 0.4),                             // circle strafe
        2 => (-1.5, 0.0, 0.0),                            // reverse out
        3 => (1.2 * phase.sin(), 1.2 * phase.cos(), 0.0), // weave
        4 => (0.0, 0.0, 2.2),                             // spin on the spot
        _ => (5.5, 0.0, -0.6),                            // sprint and turn
    };
    let translation = chassis
        .iter()
        .find(|c| c.id == pilot)
        .map(|c| c.pose.translation_m)
        .unwrap_or([0.0; 3]);
    let team = if index.is_multiple_of(2) {
        Team::Red
    } else {
        Team::Blue
    };
    let enemy = chassis.iter().filter(|c| c.team != team).min_by(|a, b| {
        let distance = |c: &ChassisSnapshot| {
            let dx = c.pose.translation_m[0] - translation[0];
            let dy = c.pose.translation_m[1] - translation[1];
            dx * dx + dy * dy
        };
        distance(a).total_cmp(&distance(b))
    });
    // Track the nearest enemy, with a slow sweep so the turret keeps moving
    // even when nobody is visible.
    let (bearing, elevation) = match enemy {
        Some(enemy) => {
            let dx = enemy.pose.translation_m[0] - translation[0];
            let dy = enemy.pose.translation_m[1] - translation[1];
            (
                dy.atan2(dx),
                (enemy.pose.translation_m[2] - translation[2]) * 0.05,
            )
        }
        None => (frame as f64 * 0.08, 0.0),
    };
    let wobble = (rng.next_f64() - 0.5) * 0.12;
    ChassisCommand {
        forward_m_s,
        left_m_s,
        yaw_rate_rad_s,
        aim_yaw_rad: bearing + wobble,
        aim_pitch_rad: (elevation + 0.02 * phase.sin()).clamp(-0.4, 0.4),
    }
}

/// A tiny deterministic xorshift generator, so the training set is exactly
/// reproducible without adding a dependency. It feeds command jitter only,
/// never the simulation itself.
struct Rng(u64);

impl Rng {
    /// Seeds the generator. Zero is remapped, because xorshift cannot escape it.
    fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        })
    }

    /// The next value in `0.0..1.0`.
    fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}
