// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Measurement harness: restitution and post-contact projectile retirement.
//!
//! Runs the real CAD field headlessly on the ordinary 1 ms tick, fires a
//! deterministic scripted volley that is identical for every arm, and records
//! active counts, lifetimes, removal reasons, contact workload, physics and
//! replay CPU, snapshot bytes at a fixed publication rate, and every armor
//! contact so arms can be diffed against the control.
//!
//! Usage: `projectile_retirement --cad <package> --out <dir> [--reps 5]
//! [--warmup-s 10] [--measure-s 60] [--arms a,b,c] [--trajectories]`.
use rm_simulator_server::{
    base_layout, cad_assets,
    layout::{self, LayoutOptions},
    protocol::ServerMessage,
    simulation::SimulationState,
    snapshot_codec::encode_player_message,
};
use rm_simulator_world::{
    ArmorTarget, Caliber, ChassisCommand, Field, FieldSnapshot, Pose, RuneKind, Shot,
    StaticGeometry, Team,
    projectile::{ProjectilePolicy, RemovalReason},
    tick_ns,
};
use std::{collections::BTreeMap, fmt::Write as _, path::PathBuf, time::Instant};

/// Publication rate the byte measurement holds constant, in hertz.
const PUBLISH_HZ: u64 = 30;
/// Replay probe length, in ticks.
const REPLAY_TICKS: u64 = 200;

/// One measured configuration.
#[derive(Clone, Copy, Debug)]
struct Arm {
    name: &'static str,
    policy: ProjectilePolicy,
}

fn arms() -> Vec<Arm> {
    let control = ProjectilePolicy::default();
    let mut arms = vec![Arm {
        name: "control",
        policy: control,
    }];
    for restitution in [0.30, 0.15] {
        arms.push(Arm {
            name: if restitution == 0.30 { "R30" } else { "R15" },
            policy: ProjectilePolicy {
                restitution,
                ..control
            },
        });
    }
    for (name, speed) in [("D05", 0.5), ("D10", 1.0), ("D20", 2.0)] {
        arms.push(Arm {
            name,
            policy: ProjectilePolicy {
                retire_speed_m_s: Some(speed),
                ..control
            },
        });
    }
    arms.push(Arm {
        name: "D10_immediate",
        policy: ProjectilePolicy {
            retire_speed_m_s: Some(1.0),
            retire_dwell_ns: 0,
            ..control
        },
    });
    arms.push(Arm {
        name: "R30_D10",
        policy: ProjectilePolicy {
            restitution: 0.30,
            retire_speed_m_s: Some(1.0),
            ..control
        },
    });
    for (name, ttl_s) in [("TTL3", 3), ("TTL2", 2)] {
        arms.push(Arm {
            name,
            policy: ProjectilePolicy {
                max_flight_ns: ttl_s * 1_000_000_000,
                ..control
            },
        });
    }
    arms
}

/// Deterministic xorshift so the same seed gives the same volley in every arm.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }
    fn below(&mut self, high: usize) -> usize {
        (self.next_u64() % high as u64) as usize
    }
}

/// A launch the schedule decided on, independent of any arm's physics.
#[derive(Clone, Copy, Debug)]
struct Launch {
    tick: u64,
    scenario: usize,
    /// Which target of the scenario's kind to aim at.
    pick: usize,
    /// Aim jitter in radians, yaw then pitch.
    jitter_rad: [f64; 2],
    caliber: Caliber,
    /// Range the muzzle stands off the aim point, in metres.
    standoff_m: f64,
    speed_scale: f64,
}

const SCENARIOS: [&str; 10] = [
    "direct_outpost",
    "direct_rune",
    "direct_base",
    "direct_chassis",
    "ground_skip",
    "grazing_slide",
    "ramp_roll",
    "ledge_toss",
    "miss_volley",
    "heavy_42mm",
];

/// Build the whole volley up front so every arm fires the same shots at the
/// same ticks, whatever its physics does with them.
fn schedule(seed: u64, ticks: u64, period_ticks: u64) -> Vec<Launch> {
    let mut rng = Rng(seed | 1);
    let mut launches = Vec::new();
    let mut tick = 0;
    while tick < ticks {
        let scenario = rng.below(SCENARIOS.len());
        launches.push(Launch {
            tick,
            scenario,
            pick: rng.below(8),
            jitter_rad: [rng.range(-0.02, 0.02), rng.range(-0.02, 0.02)],
            caliber: if scenario == 9 {
                Caliber::Mm42
            } else {
                Caliber::Mm17
            },
            standoff_m: rng.range(2.0, 6.0),
            speed_scale: rng.range(0.75, 1.0),
        });
        tick += period_ticks;
    }
    launches
}

/// Rotate `v` by a wxyz unit quaternion.
fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let [w, x, y, z] = q;
    let t = [
        2.0 * (y * v[2] - z * v[1]),
        2.0 * (z * v[0] - x * v[2]),
        2.0 * (x * v[1] - y * v[0]),
    ];
    [
        v[0] + w * t[0] + (y * t[2] - z * t[1]),
        v[1] + w * t[1] + (z * t[0] - x * t[2]),
        v[2] + w * t[2] + (x * t[1] - y * t[0]),
    ]
}

/// Point a muzzle at `target_m` from `from_m`, with jitter.
fn aim(from_m: [f64; 3], target_m: [f64; 3], jitter_rad: [f64; 2]) -> Pose {
    let delta = [
        target_m[0] - from_m[0],
        target_m[1] - from_m[1],
        target_m[2] - from_m[2],
    ];
    let yaw = delta[1].atan2(delta[0]) + jitter_rad[0];
    let pitch = -delta[2].atan2(delta[0].hypot(delta[1])) + jitter_rad[1];
    // Yaw about world up, then pitch about the turned +y (positive aims down).
    let (sy, cy) = (yaw / 2.0).sin_cos();
    let (sp, cp) = (pitch / 2.0).sin_cos();
    let q = [cy * cp, -sy * sp, cy * sp, sy * cp];
    Pose {
        translation_m: from_m,
        rotation_wxyz: q,
    }
}

/// Resolve a scheduled launch against the field as it stands now.
fn resolve(field: &Field, snapshot: &FieldSnapshot, launch: &Launch) -> Option<(Pose, Shot)> {
    let pick = launch.pick;
    // Armor scenarios stand the muzzle off along the face's outward normal, so
    // a scripted shot actually meets the scoring side.
    let mut normal: Option<[f64; 3]> = None;
    let (target_m, speed_limit) = match launch.scenario {
        0 => {
            let outpost = &snapshot.outposts[pick % snapshot.outposts.len().max(1)];
            let armor = outpost.armors.get(pick % outpost.armors.len().max(1))?;
            normal = Some(rotate(armor.pose.rotation_wxyz, [1.0, 0.0, 0.0]));
            (armor.pose.translation_m, 25.0)
        }
        1 => {
            let rune = snapshot.runes.first()?;
            let pose = rune.target_poses[pick % 5];
            // A rune target's local +x points into the wheel, away from the shooter.
            normal = Some(rotate(pose.rotation_wxyz, [-1.0, 0.0, 0.0]));
            (pose.translation_m, 25.0)
        }
        2 => {
            let base = snapshot.bases.get(pick % snapshot.bases.len().max(1))?;
            let pose = base.config.plates[pick % 7];
            normal = Some(rotate(pose.rotation_wxyz, [1.0, 0.0, 0.0]));
            (pose.translation_m, 25.0)
        }
        3 => {
            let chassis = snapshot.chassis.get(pick % snapshot.chassis.len().max(1))?;
            let mut point = chassis.pose.translation_m;
            point[2] += 0.25;
            normal = Some(rotate(chassis.pose.rotation_wxyz, [1.0, 0.0, 0.0]));
            (point, 25.0)
        }
        4 => {
            // A shallow skip across the floor from one side of the arena.
            let side = if pick.is_multiple_of(2) { 1.0 } else { -1.0 };
            ([side * 3.0, side * 2.0, 0.0], 25.0)
        }
        5 => {
            // Nearly level, just above the floor: a long grazing slide.
            let side = if pick.is_multiple_of(2) { 1.0 } else { -1.0 };
            ([side * 8.0, -side * 4.0, 0.03], 25.0)
        }
        6 => {
            // The hex-marked pyramid on the centre line.
            let side = if pick.is_multiple_of(2) { 1.0 } else { -1.0 };
            ([side * 7.4, 0.0, 0.15], 25.0)
        }
        7 => {
            // Lobbed onto a highland deck, where it can roll off an edge.
            let side = if pick.is_multiple_of(2) { 1.0 } else { -1.0 };
            ([side * 10.5, side * 4.5, 0.6], 25.0)
        }
        8 => {
            // Over everything: a missed volley that only meets a wall.
            let side = if pick.is_multiple_of(2) { 1.0 } else { -1.0 };
            ([side * 12.0, side * 6.0, 2.2], 25.0)
        }
        _ => {
            let base = snapshot.bases.get(pick % snapshot.bases.len().max(1))?;
            (base.config.plates[pick % 7].translation_m, 12.0)
        }
    };
    // Stand off the aim point along the face normal, or along a
    // scenario-dependent bearing when the target is terrain.
    let standoff = launch.standoff_m;
    let from = match normal {
        Some(n) => [
            target_m[0] + standoff * n[0],
            target_m[1] + standoff * n[1],
            (target_m[2] + standoff * n[2] * 0.25).max(0.2),
        ],
        None => {
            let bearing = (launch.pick as f64) * std::f64::consts::FRAC_PI_4;
            [
                target_m[0] - standoff * bearing.cos(),
                target_m[1] - standoff * bearing.sin(),
                (target_m[2] + 0.35).max(0.15),
            ]
        }
    };
    // Keep the muzzle inside the arena and clear of the floor.
    let from = [
        from[0].clamp(-13.0, 13.0),
        from[1].clamp(-7.0, 7.0),
        from[2],
    ];
    let _ = field;
    Some((
        aim(from, target_m, launch.jitter_rad),
        Shot {
            caliber: launch.caliber,
            speed_m_s: speed_limit * launch.speed_scale,
        },
    ))
}

/// A comma-free CSV key for one scoring face.
fn target_key(target: ArmorTarget) -> String {
    match target {
        ArmorTarget::Base { base, plate } => format!("base:{base}:{plate}"),
        ArmorTarget::Outpost { outpost, face } => format!("outpost:{outpost}:{face}"),
        ArmorTarget::Rune { rune, blade } => format!("rune:{rune}:{blade}"),
        ArmorTarget::Chassis { chassis, plate } => format!("chassis:{chassis}:{plate}"),
    }
}

/// One armor contact, keyed so arms can be compared shot for shot.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HitRecord {
    projectile: u64,
    target: ArmorTarget,
    detected: bool,
    damage: u32,
    /// The ball had touched nothing before this contact.
    direct: bool,
}

/// Everything one run measured.
#[derive(Default)]
struct Metrics {
    active: Vec<usize>,
    lifetimes_ns: Vec<u64>,
    after_contact_ns: Vec<u64>,
    removals: BTreeMap<&'static str, u64>,
    contacts_seen: u64,
    launched: u64,
    step_ns: u128,
    replay_ns: u128,
    replay_samples: u64,
    snapshot_bytes: u64,
    projectile_bytes: u64,
    publications: u64,
    hits: Vec<HitRecord>,
}

fn percentile(values: &mut [f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let index = ((values.len() - 1) as f64 * fraction).round() as usize;
    values[index]
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

#[allow(clippy::too_many_arguments)]
fn run_arm(
    cad: &cad_assets::CadAssets,
    terrain: &layout::Terrain,
    arm: &Arm,
    launches: &[Launch],
    warmup_ticks: u64,
    measure_ticks: u64,
    trajectory_ids: &[u64],
    trajectories: &mut String,
) -> anyhow::Result<Metrics> {
    let options = LayoutOptions {
        rune: Some(RuneKind::Small),
        outpost_speed_rad_s: 0.4,
        terrain: true,
        // No referee: the volley must not be throttled by ammo allowances, and
        // no robot may be defeated out of the workload half way through a run.
        referee: false,
        physics_rate_hz: layout::DEFAULT_RATE_HZ,
        projectile_policy: arm.policy,
    };
    let mut config = layout::field_config(cad, &options);
    config.bases = base_layout::load(cad)?;
    let mut field = Field::new(&config)?;
    layout::add_terrain(&mut field, terrain)?;

    // Two robots, driving slowly and turning, for moving armor and kicks.
    let chassis_config = rm_simulator_world::ChassisConfig::default();
    let mut ids = Vec::new();
    for team in Team::BOTH {
        let (spawn, yaw) = layout::spawn_slot(team, 0);
        let placement = layout::chassis_placement(
            chassis_config.clone(),
            rm_simulator_world::RobotKind::Infantry,
            Some(terrain),
            team,
            spawn,
            yaw,
        );
        ids.push(field.add_chassis(&placement)?);
    }
    for (index, id) in ids.iter().enumerate() {
        field.command_chassis(
            *id,
            ChassisCommand {
                forward_m_s: if index == 0 { 0.6 } else { -0.4 },
                yaw_rate_rad_s: if index == 0 { 0.5 } else { -0.3 },
                ..Default::default()
            },
        )?;
    }

    let geometry: StaticGeometry = field.static_geometry_snapshot();
    let floor_m = layout::CATCH_FLOOR_M;
    let mut metrics = Metrics::default();
    let mut launch_index = 0;
    let publish_every = 1_000 / PUBLISH_HZ;
    let total_ticks = warmup_ticks + measure_ticks;
    let mut hits_this_tick: Vec<(u64, ArmorTarget, bool, u32)> = Vec::new();
    // First contact time per live ball at the end of the previous tick, so a
    // ball removed inside this tick can still be classified.
    let mut first_contact: BTreeMap<u64, Option<u64>> = BTreeMap::new();

    for tick in 0..total_ticks {
        let measuring = tick >= warmup_ticks;
        while launch_index < launches.len() && launches[launch_index].tick == tick {
            let launch = launches[launch_index];
            launch_index += 1;
            let snapshot = field.snapshot();
            if let Some((muzzle, shot)) = resolve(&field, &snapshot, &launch)
                && field.fire(muzzle, shot, None).is_ok()
                && measuring
            {
                metrics.launched += 1;
            }
        }
        hits_this_tick.clear();
        let started = Instant::now();
        field.step_with_hits(1, &mut |hit| {
            hits_this_tick.push((hit.projectile, hit.target, hit.detected, hit.damage));
        })?;
        let elapsed = started.elapsed().as_nanos();
        if !measuring {
            let _ = field.take_projectile_removals();
            continue;
        }
        metrics.step_ns += elapsed;
        let now_ns = field.time_ns();
        let balls = field.projectile_snapshots();
        metrics.active.push(balls.len());
        metrics.contacts_seen += hits_this_tick.len() as u64;
        for (projectile, target, detected, damage) in &hits_this_tick {
            // A direct hit is a ball whose very first contact is this armor.
            let known = balls
                .iter()
                .find(|ball| ball.id == *projectile)
                .map(|ball| ball.first_contact_ns)
                .or_else(|| first_contact.get(projectile).copied());
            let direct = match known {
                Some(Some(first)) => first == now_ns,
                // Never seen touching before this tick: this contact is its first.
                _ => true,
            };
            metrics.hits.push(HitRecord {
                projectile: *projectile,
                target: *target,
                detected: *detected,
                damage: *damage,
                direct,
            });
        }
        first_contact.clear();
        first_contact.extend(balls.iter().map(|ball| (ball.id, ball.first_contact_ns)));
        for removal in field.take_projectile_removals() {
            metrics.lifetimes_ns.push(removal.lifetime_ns);
            if let Some(after) = removal.since_first_contact_ns {
                metrics.after_contact_ns.push(after);
            }
            let reason = match removal.reason {
                RemovalReason::Expired => "expired",
                RemovalReason::OutOfWorld => "out_of_world",
                RemovalReason::OutOfBounds => "out_of_bounds",
                RemovalReason::Capped => "capped",
                RemovalReason::Retired => "retired",
            };
            *metrics.removals.entry(reason).or_default() += 1;
        }
        if tick % publish_every == 0 {
            let mut state = SimulationState {
                bots: Vec::new(),
                snapshot_id: tick,
                input_epoch: 0,
                shot_results: Vec::new(),
                paused: false,
                field: field.snapshot(),
            };
            let with = encode_player_message(&ServerMessage::Snapshot(Box::new(state.clone())));
            state.field.projectiles.clear();
            let without = encode_player_message(&ServerMessage::Snapshot(Box::new(state)));
            metrics.snapshot_bytes += with.len() as u64;
            metrics.projectile_bytes += with.len().saturating_sub(without.len()) as u64;
            metrics.publications += 1;
        }
        // Replay probe: restore the published checkpoint and step it on.
        if (tick - warmup_ticks).is_multiple_of(10_000) {
            let checkpoint = field.snapshot();
            let mut replay = Field::restore(&checkpoint, &geometry, floor_m)?;
            let started = Instant::now();
            replay.step(REPLAY_TICKS)?;
            metrics.replay_ns += started.elapsed().as_nanos();
            metrics.replay_samples += 1;
        }
        if !trajectory_ids.is_empty() {
            for ball in &balls {
                if trajectory_ids.contains(&ball.id) {
                    let _ = writeln!(
                        trajectories,
                        "{},{},{},{:.4},{:.4},{:.4},{:.4}",
                        arm.name,
                        ball.id,
                        now_ns,
                        ball.position_m[0],
                        ball.position_m[1],
                        ball.position_m[2],
                        (ball.velocity_m_s[0].powi(2)
                            + ball.velocity_m_s[1].powi(2)
                            + ball.velocity_m_s[2].powi(2))
                        .sqrt()
                    );
                }
            }
        }
    }
    Ok(metrics)
}

fn main() -> anyhow::Result<()> {
    let mut cad_path: Option<PathBuf> = None;
    let mut out = PathBuf::from("target/projectile-retirement");
    let mut reps = 5;
    let mut warmup_s = 10.0;
    let mut measure_s = 60.0;
    let mut rate_hz: Vec<u64> = vec![20, 8];
    let mut selected: Option<Vec<String>> = None;
    let mut trajectories = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cad" => cad_path = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from).unwrap(),
            "--reps" => reps = args.next().unwrap().parse()?,
            "--warmup-s" => warmup_s = args.next().unwrap().parse()?,
            "--measure-s" => measure_s = args.next().unwrap().parse()?,
            "--rates" => {
                rate_hz = args
                    .next()
                    .unwrap()
                    .split(',')
                    .map(|value| value.parse::<u64>().unwrap())
                    .collect()
            }
            "--arms" => {
                selected = Some(
                    args.next()
                        .unwrap()
                        .split(',')
                        .map(str::to_string)
                        .collect(),
                )
            }
            "--trajectories" => trajectories = true,
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    let cad_path = cad_path.unwrap_or_else(cad_assets::default_cad_assets);
    let cad = cad_assets::load(&cad_path)?;
    let terrain = layout::load_terrain(&cad)?;
    std::fs::create_dir_all(&out)?;

    let arms: Vec<Arm> = arms()
        .into_iter()
        .filter(|arm| {
            selected
                .as_ref()
                .is_none_or(|names| names.iter().any(|name| name == arm.name))
        })
        .collect();
    let warmup_ticks = (warmup_s * 1_000.0) as u64;
    let measure_ticks = (measure_s * 1_000.0) as u64;

    let mut runs = String::from(
        "rate_hz,rep,arm,restitution,retire_speed_m_s,retire_dwell_ns,max_flight_ns,\
launched,mean_active,p95_active,mean_lifetime_ms,p95_lifetime_ms,mean_after_contact_ms,\
p95_after_contact_ms,expired,out_of_world,out_of_bounds,capped,retired,contacts,\
direct_hits,ricochet_hits,detected_hits,\
step_ms_per_sim_s,replay_ms_per_sim_s,snapshot_bytes_per_s,projectile_bytes_per_s\n",
    );
    let mut disagreements = String::from(
        "rate_hz,rep,arm,projectile,target,kind,control_detected,arm_detected,change\n",
    );
    let mut trajectory_text = String::from("arm,projectile,time_ns,x_m,y_m,z_m,speed_m_s\n");

    for rate in &rate_hz {
        if *rate == 0 || *rate > 1_000 || 1_000 % rate != 0 {
            anyhow::bail!(
                "--rates {rate}: a launch rate must be a divisor of 1000 so that its \
                 period is a whole number of 1 ms ticks"
            );
        }
        let period_ticks = 1_000 / rate;
        for rep in 0..reps {
            let seed = 0x5EED_0000 + rep as u64 * 7919 + rate;
            let launches = schedule(seed, warmup_ticks + measure_ticks, period_ticks);
            // Randomize arm order within the repetition.
            let mut order: Vec<usize> = (0..arms.len()).collect();
            let mut rng = Rng(seed ^ 0xA5A5_A5A5);
            for index in (1..order.len()).rev() {
                order.swap(index, rng.below(index + 1));
            }
            let trajectory_ids: Vec<u64> = if trajectories && rep == 0 && *rate == rate_hz[0] {
                // Ball ids are assigned in launch order, so the first shots of
                // the measured window have the same ids in every arm.
                let first = launches.iter().filter(|l| l.tick < warmup_ticks).count() as u64;
                (first..first + 8).collect()
            } else {
                Vec::new()
            };
            let mut by_arm: BTreeMap<String, Metrics> = BTreeMap::new();
            for index in order {
                let arm = arms[index];
                eprintln!("rate {rate} Hz rep {rep} arm {}", arm.name);
                let metrics = run_arm(
                    &cad,
                    &terrain,
                    &arm,
                    &launches,
                    warmup_ticks,
                    measure_ticks,
                    &trajectory_ids,
                    &mut trajectory_text,
                )?;
                by_arm.insert(arm.name.to_string(), metrics);
            }
            for arm in &arms {
                let Some(metrics) = by_arm.get(arm.name) else {
                    continue;
                };
                let mut active: Vec<f64> =
                    metrics.active.iter().map(|count| *count as f64).collect();
                let mut lifetimes: Vec<f64> = metrics
                    .lifetimes_ns
                    .iter()
                    .map(|ns| *ns as f64 / 1e6)
                    .collect();
                let mut after: Vec<f64> = metrics
                    .after_contact_ns
                    .iter()
                    .map(|ns| *ns as f64 / 1e6)
                    .collect();
                let sim_s = measure_ticks as f64 * tick_ns() as f64 / 1e9;
                let publications = metrics.publications.max(1) as f64;
                let published_s = publications / PUBLISH_HZ as f64;
                let _ = writeln!(
                    runs,
                    "{rate},{rep},{},{},{},{},{},{},{:.3},{:.1},{:.1},{:.1},{:.1},{:.1},\
{},{},{},{},{},{},{},{},{},{:.3},{:.3},{:.0},{:.0}",
                    arm.name,
                    arm.policy.restitution,
                    arm.policy
                        .retire_speed_m_s
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "none".into()),
                    arm.policy.retire_dwell_ns,
                    arm.policy.max_flight_ns,
                    metrics.launched,
                    mean(&active),
                    percentile(&mut active, 0.95),
                    mean(&lifetimes),
                    percentile(&mut lifetimes, 0.95),
                    mean(&after),
                    percentile(&mut after, 0.95),
                    metrics.removals.get("expired").copied().unwrap_or(0),
                    metrics.removals.get("out_of_world").copied().unwrap_or(0),
                    metrics.removals.get("out_of_bounds").copied().unwrap_or(0),
                    metrics.removals.get("capped").copied().unwrap_or(0),
                    metrics.removals.get("retired").copied().unwrap_or(0),
                    metrics.contacts_seen,
                    metrics.hits.iter().filter(|hit| hit.direct).count(),
                    metrics.hits.iter().filter(|hit| !hit.direct).count(),
                    metrics.hits.iter().filter(|hit| hit.detected).count(),
                    metrics.step_ns as f64 / 1e6 / sim_s,
                    if metrics.replay_samples == 0 {
                        0.0
                    } else {
                        // Per replayed simulated second, not per measured one.
                        let replay_sim_s =
                            metrics.replay_samples as f64 * REPLAY_TICKS as f64 * tick_ns() as f64
                                / 1e9;
                        metrics.replay_ns as f64 / 1e6 / replay_sim_s
                    },
                    metrics.snapshot_bytes as f64 / published_s,
                    metrics.projectile_bytes as f64 / published_s,
                );
            }
            // Scoring disagreements against this repetition's control run.
            if let Some(control) = by_arm.get("control") {
                for arm in &arms {
                    if arm.name == "control" {
                        continue;
                    }
                    let Some(metrics) = by_arm.get(arm.name) else {
                        continue;
                    };
                    // A ball can strike the same plate more than once, so each
                    // key holds the ordered sequence of its contacts and the
                    // arms are compared contact by contact.
                    let key = |hit: &HitRecord| (hit.projectile, hit.target, hit.direct);
                    let mut control_map: BTreeMap<_, Vec<HitRecord>> = BTreeMap::new();
                    for hit in &control.hits {
                        control_map.entry(key(hit)).or_default().push(*hit);
                    }
                    let mut arm_map: BTreeMap<_, Vec<HitRecord>> = BTreeMap::new();
                    for hit in &metrics.hits {
                        arm_map.entry(key(hit)).or_default().push(*hit);
                    }
                    let empty = Vec::new();
                    for (id, control_hits) in &control_map {
                        let arm_hits = arm_map.get(id).unwrap_or(&empty);
                        for (index, hit) in control_hits.iter().enumerate() {
                            let (change, arm_detected) = match arm_hits.get(index) {
                                None => ("lost", false),
                                Some(other) if other.detected != hit.detected => {
                                    ("detection_changed", other.detected)
                                }
                                Some(other) if other.damage != hit.damage => {
                                    ("damage_changed", other.detected)
                                }
                                Some(_) => continue,
                            };
                            let _ = writeln!(
                                disagreements,
                                "{rate},{rep},{},{},{},{},{},{},{change}",
                                arm.name,
                                hit.projectile,
                                target_key(hit.target),
                                if hit.direct { "direct" } else { "ricochet" },
                                hit.detected,
                                arm_detected,
                            );
                        }
                    }
                    for (id, arm_hits) in &arm_map {
                        let known = control_map.get(id).map_or(0, Vec::len);
                        for hit in arm_hits.iter().skip(known) {
                            let _ = writeln!(
                                disagreements,
                                "{rate},{rep},{},{},{},{},false,{},gained",
                                arm.name,
                                hit.projectile,
                                target_key(hit.target),
                                if hit.direct { "direct" } else { "ricochet" },
                                hit.detected,
                            );
                        }
                    }
                }
            }
            std::fs::write(out.join("runs.csv"), &runs)?;
            std::fs::write(out.join("disagreements.csv"), &disagreements)?;
            if trajectories {
                std::fs::write(out.join("trajectories.csv"), &trajectory_text)?;
            }
        }
    }
    println!("wrote {}", out.display());
    Ok(())
}
