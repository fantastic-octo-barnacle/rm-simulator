// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Experiment 1 (shared physics rate) harness: deterministic fidelity fixtures
//! and headless CPU cells at whatever rate `RM_SIM_TICK_NS` selected.
//!
//! Every mode prints long-format CSV on stdout, one row per observation, so a
//! run at another rate can be joined on `fixture` and `key`.
//!
//! ```text
//! physics_rate probe    <CAD_PATH>
//! physics_rate fixtures <CAD_PATH>
//! physics_rate cpu      <CAD_PATH> <ROBOTS> <idle|drive|fire> <WARMUP_S> <MEASURE_S> <REP>
//! ```
use rm_simulator_server::{cad_assets, layout};
use rm_simulator_world::{
    ArmorHit, ArmorTarget, Caliber, ChassisCommand, ChassisConfig, Field, Pose, RuneKind, Shot,
    Team, tick_ns,
};
use std::{hint::black_box, path::PathBuf, time::Instant};

/// Sampling period for trajectory observations. 500 ms is the smallest whole
/// number of ticks at 1 kHz, 500 Hz, 250 Hz and exactly 128 Hz alike, so every
/// variant lands a sample on the same simulated instant.
const SAMPLE_NS: u64 = 500_000_000;

fn row(fixture: &str, key: &str, value: f64) {
    println!("{},{fixture},{key},{value:.9},", tick_ns());
}
fn row_text(fixture: &str, key: &str, text: &str) {
    println!("{},{fixture},{key},,{text}", tick_ns());
}
fn header() {
    println!("rate_ns,fixture,key,value,text");
}

struct Loaded {
    terrain: layout::Terrain,
    cad: cad_assets::CadAssets,
}
fn load(root: &PathBuf) -> anyhow::Result<Loaded> {
    let cad = cad_assets::load(root)?;
    let terrain = layout::load_terrain(&cad)?;
    Ok(Loaded { terrain, cad })
}
fn build(loaded: &Loaded, outpost_speed_rad_s: f64, referee: bool) -> anyhow::Result<Field> {
    let config = layout::field_config(
        &loaded.cad,
        &layout::LayoutOptions {
            rune: Some(RuneKind::Small),
            outpost_speed_rad_s,
            terrain: true,
            referee,
        },
    );
    let mut field = Field::new(&config)?;
    layout::add_terrain(&mut field, &loaded.terrain)?;
    Ok(field)
}
fn spawn(
    field: &mut Field,
    loaded: &Loaded,
    team: Team,
    point: [f64; 3],
    yaw_deg: f64,
) -> anyhow::Result<u32> {
    Ok(field.add_chassis(&layout::chassis_placement(
        ChassisConfig::default(),
        Some(&loaded.terrain),
        team,
        point,
        yaw_deg,
    ))?)
}
fn yaw_of(pose: Pose) -> f64 {
    let [w, x, y, z] = pose.rotation_wxyz;
    (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z))
}

/// One scripted control change, timestamped in world nanoseconds. The harness
/// applies it on the first tick boundary at or after `at_ns` and records the
/// quantization delay that rounding cost.
struct Event {
    at_ns: u64,
    chassis: usize,
    command: ChassisCommand,
}
fn drive(forward_m_s: f64, left_m_s: f64, yaw_rate_rad_s: f64) -> ChassisCommand {
    ChassisCommand {
        forward_m_s,
        left_m_s,
        yaw_rate_rad_s,
        ..Default::default()
    }
}

struct Trajectory {
    name: &'static str,
    /// Spawn point and heading of each chassis in the fixture.
    spawns: Vec<([f64; 3], f64)>,
    duration_ns: u64,
    events: Vec<Event>,
}

fn trajectories() -> Vec<Trajectory> {
    // Coordinates come from the `probe` mode's ground scan of the loaded CAD.
    let straight = |name, point: [f64; 3], yaw, ms: u64, speed| Trajectory {
        name,
        spawns: vec![(point, yaw)],
        duration_ns: ms * 1_000_000,
        events: vec![Event {
            at_ns: 250_000_001,
            chassis: 0,
            command: drive(speed, 0.0, 0.0),
        }],
    };
    vec![
        // Flat slab: x = 5.5 m is level from y = 0 to y = 5.2 m.
        straight("flat", [5.5, 0.5, 0.0], 90.0, 3_500, 1.5),
        // The hex-marked truncated pyramid, x 6.3..8.5 m on the centre line.
        straight("ramp", [5.0, 0.0, 0.0], 0.0, 3_500, 1.5),
        // The same pyramid taken fast enough to unload the wheels.
        straight("ramp_launch", [5.0, 0.0, 0.0], 0.0, 2_000, 3.5),
        // The undulating road on the 0.2 m deck, x 6.5..8.5 m at y = 6.4 m.
        straight("waves", [5.5, 6.4, 0.0], 0.0, 3_500, 1.5),
        // Off the 0.2 m deck's y edge at y = 7.6 m.
        straight("edge_drop", [5.5, 6.0, 0.0], 90.0, 3_000, 2.0),
        // Under the roof the probe found at x = 10.5 m, y = 4.5 m (0.88 m up).
        straight("passage", [9.0, 4.5, 0.0], 0.0, 3_000, 1.5),
        Trajectory {
            name: "braking",
            spawns: vec![([5.5, 0.5, 0.0], 90.0)],
            duration_ns: 4_000_000_000,
            events: vec![
                Event {
                    at_ns: 250_000_001,
                    chassis: 0,
                    command: drive(3.0, 0.0, 0.0),
                },
                Event {
                    at_ns: 3_000_000_001,
                    chassis: 0,
                    command: drive(0.0, 0.0, 0.0),
                },
            ],
        },
        Trajectory {
            name: "rotation",
            spawns: vec![([5.5, 0.5, 0.0], 90.0)],
            duration_ns: 4_000_000_000,
            events: vec![Event {
                at_ns: 250_000_001,
                chassis: 0,
                command: drive(1.5, 0.0, 6.0),
            }],
        },
        Trajectory {
            name: "robot_contact",
            spawns: vec![([5.5, 1.0, 0.0], 90.0), ([5.5, 4.0, 0.0], -90.0)],
            duration_ns: 4_000_000_000,
            events: vec![
                Event {
                    at_ns: 250_000_001,
                    chassis: 0,
                    command: drive(2.0, 0.0, 0.0),
                },
                Event {
                    at_ns: 250_000_001,
                    chassis: 1,
                    command: drive(2.0, 0.0, 0.0),
                },
            ],
        },
        // No command at all: the suspension settles from the placement height.
        Trajectory {
            name: "settling",
            spawns: vec![([5.5, 2.5, 0.0], 0.0)],
            duration_ns: 3_000_000_000,
            events: Vec::new(),
        },
    ]
}

fn run_trajectory(loaded: &Loaded, scenario: &Trajectory) -> anyhow::Result<()> {
    let mut field = build(loaded, 0.4, false)?;
    let mut ids = Vec::new();
    for (index, (point, yaw)) in scenario.spawns.iter().enumerate() {
        let team = if index % 2 == 0 {
            Team::Red
        } else {
            Team::Blue
        };
        ids.push(spawn(&mut field, loaded, team, *point, *yaw)?);
    }
    let mut pending = 0usize;
    let mut quantization_ns: Vec<u64> = Vec::new();
    let mut min_z = f64::INFINITY;
    let mut max_z = f64::NEG_INFINITY;
    let mut airborne_ticks = 0u64;
    let mut ticks = 0u64;
    let mut settled_ns: Option<u64> = None;
    let mut calm_since: Option<u64> = None;
    while field.time_ns() < scenario.duration_ns {
        let now = field.time_ns();
        while pending < scenario.events.len() && scenario.events[pending].at_ns <= now {
            let event = &scenario.events[pending];
            quantization_ns.push(now - event.at_ns);
            field.command_chassis(ids[event.chassis], event.command)?;
            pending += 1;
        }
        field.step(1)?;
        ticks += 1;
        let now = field.time_ns();
        let state = field.chassis_snapshot(ids[0]).expect("chassis present");
        min_z = min_z.min(state.pose.translation_m[2]);
        max_z = max_z.max(state.pose.translation_m[2]);
        let grounded = state.wheels.iter().filter(|w| w.contact.is_some()).count();
        airborne_ticks += u64::from(grounded < 4);
        if state.velocity_m_s[2].abs() < 0.01 {
            let since = *calm_since.get_or_insert(now);
            if settled_ns.is_none() && now - since >= 200_000_000 {
                settled_ns = Some(since);
            }
        } else {
            calm_since = None;
        }
        if now % SAMPLE_NS == 0 {
            let sample = now / SAMPLE_NS;
            let name = scenario.name;
            let p = state.pose.translation_m;
            row(name, &format!("s{sample}.x_m"), p[0]);
            row(name, &format!("s{sample}.y_m"), p[1]);
            row(name, &format!("s{sample}.z_m"), p[2]);
            row(name, &format!("s{sample}.yaw_rad"), yaw_of(state.pose));
            let v = state.velocity_m_s;
            row(
                name,
                &format!("s{sample}.speed_m_s"),
                (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt(),
            );
            row(
                name,
                &format!("s{sample}.yaw_rate_rad_s"),
                state.angular_velocity_rad_s[2],
            );
            row(name, &format!("s{sample}.wheels_grounded"), grounded as f64);
            if ids.len() > 1 {
                let other = field.chassis_snapshot(ids[1]).expect("second chassis");
                let o = other.pose.translation_m;
                row(name, &format!("s{sample}.other_x_m"), o[0]);
                row(name, &format!("s{sample}.other_y_m"), o[1]);
            }
        }
    }
    row(scenario.name, "ticks", ticks as f64);
    row(scenario.name, "min_z_m", min_z);
    row(scenario.name, "max_z_m", max_z);
    row(
        scenario.name,
        "airborne_fraction",
        airborne_ticks as f64 / ticks as f64,
    );
    row(
        scenario.name,
        "settled_at_ns",
        settled_ns.map_or(f64::NAN, |ns| ns as f64),
    );
    row(
        scenario.name,
        "quantization_max_ns",
        quantization_ns.iter().copied().max().unwrap_or(0) as f64,
    );
    row(
        scenario.name,
        "quantization_mean_ns",
        if quantization_ns.is_empty() {
            0.0
        } else {
            quantization_ns.iter().sum::<u64>() as f64 / quantization_ns.len() as f64
        },
    );
    Ok(())
}

fn target_name(target: ArmorTarget) -> String {
    match target {
        ArmorTarget::Outpost { outpost, face } => format!("outpost{outpost}f{face}"),
        ArmorTarget::Base { base, plate } => format!("base{base}p{plate}"),
        ArmorTarget::Rune { rune, blade } => format!("rune{rune}b{blade}"),
        ArmorTarget::Chassis { chassis, plate } => format!("chassis{chassis}p{plate}"),
    }
}
fn report_hit(fixture: &str, order: usize, hit: &ArmorHit) {
    let key = |k: &str| format!("hit{order}.{k}");
    row_text(fixture, &key("target"), &target_name(hit.target));
    row(fixture, &key("time_ns"), hit.time_ns as f64);
    row(fixture, &key("detected"), f64::from(u8::from(hit.detected)));
    row(fixture, &key("normal_speed_m_s"), hit.normal_speed_m_s);
    row(fixture, &key("offset_u_m"), hit.local_offset_m[0]);
    row(fixture, &key("offset_v_m"), hit.local_offset_m[1]);
    row(fixture, &key("damage_hp"), f64::from(hit.damage));
    row(fixture, &key("projectile"), hit.projectile as f64);
    row_text(
        fixture,
        &key("rejection"),
        &hit.rejection
            .map_or("none".to_string(), |r| format!("{r:?}")),
    );
}

/// Place a muzzle `distance_m` out along a scoring face's outward normal,
/// offset by `offset_m` across the face, aimed back at it. The barrel is level,
/// so the muzzle is raised by the ballistic drop of a `speed_m_s` ball over the
/// flight: without that every fixture would strike the plate's bottom edge.
fn muzzle_for(face: Pose, distance_m: f64, speed_m_s: f64, offset_m: [f64; 2]) -> Pose {
    let yaw = yaw_of(face);
    let normal = [yaw.cos(), yaw.sin(), 0.0];
    let across = [-yaw.sin(), yaw.cos(), 0.0];
    let flight_s = distance_m / speed_m_s;
    let drop_m = 0.5 * 9.81 * flight_s * flight_s;
    let c = face.translation_m;
    Pose::yawed(
        [
            c[0] + normal[0] * distance_m + across[0] * offset_m[0],
            c[1] + normal[1] * distance_m + across[1] * offset_m[0],
            c[2] + offset_m[1] + drop_m,
        ],
        yaw + std::f64::consts::PI,
    )
}
/// Hamilton product of two wxyz quaternions.
fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let [aw, ax, ay, az] = a;
    let [bw, bx, by, bz] = b;
    [
        aw * bw - ax * bx - ay * by - az * bz,
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
    ]
}
/// Rotate `v` by a wxyz quaternion.
fn quat_rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let [w, x, y, z] = q;
    let t = [
        2.0 * (y * v[2] - z * v[1]),
        2.0 * (z * v[0] - x * v[2]),
        2.0 * (x * v[1] - y * v[0]),
    ];
    [
        v[0] + w * t[0] + y * t[2] - z * t[1],
        v[1] + w * t[1] + z * t[0] - x * t[2],
        v[2] + w * t[2] + x * t[1] - y * t[0],
    ]
}
/// Carry a body-local pose into the world by a body pose.
fn compose(body: Pose, local: Pose) -> Pose {
    let offset = quat_rotate(body.rotation_wxyz, local.translation_m);
    Pose {
        translation_m: [
            body.translation_m[0] + offset[0],
            body.translation_m[1] + offset[1],
            body.translation_m[2] + offset[2],
        ],
        rotation_wxyz: quat_mul(body.rotation_wxyz, local.rotation_wxyz),
    }
}

struct ShotFixture {
    name: &'static str,
    outpost_speed_rad_s: f64,
    caliber: Caliber,
    speed_m_s: f64,
    /// Metres out from the plate centre along its normal.
    distance_m: f64,
    /// Across-face and vertical offsets of the muzzle, in metres.
    offset_m: [f64; 2],
    /// Extra muzzle rise in metres, hand-fitted for the drag the level-barrel
    /// drop estimate leaves out on long shots.
    extra_drop_m: f64,
    /// Lead the plate by the flight time, so a fast rotor is still struck near
    /// its centre rather than on the housing edge.
    lead: bool,
    /// Fire a second ball on the same tick at the other outpost.
    pair: bool,
    duration_ns: u64,
}
fn shot_fixtures() -> Vec<ShotFixture> {
    let base = ShotFixture {
        name: "",
        outpost_speed_rad_s: 0.4,
        caliber: Caliber::Mm17,
        speed_m_s: 25.0,
        distance_m: 3.0,
        offset_m: [0.0, 0.0],
        extra_drop_m: 0.0,
        lead: false,
        pair: false,
        duration_ns: 1_500_000_000,
    };
    vec![
        ShotFixture {
            name: "shot_normal_17",
            ..base
        },
        ShotFixture {
            name: "shot_normal_42",
            caliber: Caliber::Mm42,
            speed_m_s: 16.0,
            ..base
        },
        // The detection rectangle of Figure 5-16 is 101 x 94 mm, so its half
        // extents are 50.5 and 47 mm. These four aims bracket both edges.
        ShotFixture {
            name: "shot_grazing_edge_in",
            offset_m: [0.050, 0.0],
            ..base
        },
        ShotFixture {
            name: "shot_grazing_edge_out",
            offset_m: [0.066, 0.0],
            ..base
        },
        ShotFixture {
            name: "shot_grazing_top_in",
            offset_m: [0.0, 0.040],
            ..base
        },
        ShotFixture {
            name: "shot_grazing_top_out",
            offset_m: [0.0, 0.062],
            ..base
        },
        // 195 mm of travel per 128 Hz step against a 19 mm deep housing.
        ShotFixture {
            name: "shot_sweep_close",
            distance_m: 0.25,
            ..base
        },
        ShotFixture {
            name: "shot_sweep_far",
            distance_m: 8.0,
            extra_drop_m: 0.052,
            ..base
        },
        // A plate crossing the line of fire at 2 rad/s, led by the flight time.
        ShotFixture {
            name: "shot_spinning_fast",
            outpost_speed_rad_s: 2.0,
            lead: true,
            ..base
        },
        // The same fast rotor without a lead: a glancing edge contact.
        ShotFixture {
            name: "shot_spinning_fast_nolead",
            outpost_speed_rad_s: 2.0,
            ..base
        },
        ShotFixture {
            name: "shot_spinning_slow",
            outpost_speed_rad_s: 0.1,
            ..base
        },
        // Two balls launched on the same tick at the two outposts.
        ShotFixture {
            name: "shot_simultaneous",
            pair: true,
            ..base
        },
        // Short of the plate and into the floor: the bounce path.
        ShotFixture {
            name: "shot_bounce",
            offset_m: [0.0, -0.6],
            duration_ns: 3_000_000_000,
            ..base
        },
    ]
}

fn run_shot(loaded: &Loaded, fixture: &ShotFixture) -> anyhow::Result<()> {
    let mut field = build(loaded, fixture.outpost_speed_rad_s, false)?;
    // Let the rotors leave their starting angle so the plate is genuinely moving.
    field.step(250_000_000 / tick_ns())?;
    // Aim at where the plate stands when the ball should arrive. The lead is a
    // nanosecond time, so it is identical at every rate.
    let flight_ns = (fixture.distance_m / fixture.speed_m_s * 1e9) as u64;
    let aim_ns = field.time_ns() + if fixture.lead { flight_ns } else { 0 };
    let mut aims = Vec::new();
    for index in 0..field.outposts().len() {
        let face = field.outposts()[index].snapshot(aim_ns).armors[0].pose;
        aims.push((
            index,
            muzzle_for(
                face,
                fixture.distance_m,
                fixture.speed_m_s,
                [
                    fixture.offset_m[0],
                    fixture.offset_m[1] + fixture.extra_drop_m,
                ],
            ),
        ));
        if !fixture.pair {
            break;
        }
    }
    let shot = Shot {
        caliber: fixture.caliber,
        speed_m_s: fixture.speed_m_s,
    };
    for (_, muzzle) in &aims {
        field.fire(*muzzle, shot, None)?;
    }
    let end = field.time_ns() + fixture.duration_ns;
    let mut order = 0usize;
    let mut hits = Vec::new();
    while field.time_ns() < end {
        field.step_with_hits(1, &mut |hit| hits.push(hit.clone()))?;
        for hit in hits.drain(..) {
            report_hit(fixture.name, order, &hit);
            order += 1;
        }
        let now = field.time_ns();
        if now % SAMPLE_NS == 0
            && let Some(ball) = field.projectile_snapshots().first()
        {
            let sample = now / SAMPLE_NS;
            for (axis, value) in ["x", "y", "z"].iter().zip(ball.position_m) {
                row(fixture.name, &format!("s{sample}.ball_{axis}_m"), value);
            }
        }
    }
    row(fixture.name, "hits", order as f64);
    let left = field.projectile_snapshots();
    row(fixture.name, "in_flight_at_end", left.len() as f64);
    for (index, ball) in left.iter().enumerate() {
        row(
            fixture.name,
            &format!("rest{index}.x_m"),
            ball.position_m[0],
        );
        row(
            fixture.name,
            &format!("rest{index}.y_m"),
            ball.position_m[1],
        );
        row(
            fixture.name,
            &format!("rest{index}.z_m"),
            ball.position_m[2],
        );
    }
    Ok(())
}

/// A ball fired straight down the barrel of a stationary chassis' own armor,
/// so the scorer is a chassis plate on a dynamic body rather than a kinematic one.
fn run_chassis_armor(loaded: &Loaded, name: &'static str, moving: bool) -> anyhow::Result<()> {
    let mut field = build(loaded, 0.4, false)?;
    // The strip at x = 5.5 m is level from y = 0 to 5.2 m, so the muzzle three
    // metres in front of the victim is over flat ground too.
    let victim = spawn(&mut field, loaded, Team::Blue, [5.5, 4.5, 0.0], -90.0)?;
    if moving {
        // Spin in place, so the plate crosses the line of fire without the
        // whole robot leaving it.
        field.command_chassis(victim, drive(0.0, 0.0, 4.0))?;
    }
    field.step(500_000_000 / tick_ns())?;
    let state = field.chassis_snapshot(victim).expect("victim");
    let shot = Shot::at_limit(Caliber::Mm17);
    let local = state.config.armor_faces()[0];
    let face = compose(state.pose, local);
    let muzzle = muzzle_for(face, 3.0, shot.speed_m_s, [0.0, 0.0]);
    field.fire(muzzle, shot, None)?;
    let end = field.time_ns() + 1_500_000_000;
    let mut order = 0usize;
    let mut hits = Vec::new();
    while field.time_ns() < end {
        field.step_with_hits(1, &mut |hit| hits.push(hit.clone()))?;
        for hit in hits.drain(..) {
            report_hit(name, order, &hit);
            order += 1;
        }
    }
    row(name, "hits", order as f64);
    Ok(())
}

fn fixtures(root: &PathBuf) -> anyhow::Result<()> {
    let loaded = load(root)?;
    header();
    row_text(
        "meta",
        "terrain",
        &loaded.terrain.describe().replace(',', ";"),
    );
    for scenario in trajectories() {
        run_trajectory(&loaded, &scenario)?;
    }
    for fixture in shot_fixtures() {
        run_shot(&loaded, &fixture)?;
    }
    run_chassis_armor(&loaded, "shot_chassis_static", false)?;
    run_chassis_armor(&loaded, "shot_chassis_moving", true)?;
    Ok(())
}

fn probe(root: &PathBuf) -> anyhow::Result<()> {
    let loaded = load(root)?;
    println!("terrain: {}", loaded.terrain.describe());
    // Where a roof stands over a low floor there is a passage to drive through.
    for y in [0.0, 1.5, 3.0, 4.5] {
        let mut line = String::new();
        let mut x = 8.0;
        while x <= 15.0 {
            let floor = loaded.terrain.ground.ground_height_below(x, y, 0.55);
            let roof = loaded.terrain.ground.ground_height_below(x, y, 3.0);
            if let (Some(floor), Some(roof)) = (floor, roof)
                && roof - floor > 0.5
            {
                line.push_str(&format!("{x:.2}:{floor:.2}/{roof:.2} "));
            }
            x += 0.25;
        }
        println!("passage y={y}: {line}");
    }
    for (name, x) in [("across_deck_x5.5", 5.5), ("across_centre_x9.5", 9.5)] {
        let mut line = String::new();
        let mut y = 0.0;
        while y <= 8.0 {
            let h = loaded
                .terrain
                .ground
                .ground_height_below(x, y, 3.0)
                .unwrap_or(f64::NAN);
            line.push_str(&format!("{y:.1}:{h:.3} "));
            y += 0.25;
        }
        println!("{name}: {line}");
    }
    for (name, y) in [("centre", 0.0), ("deck", 6.4)] {
        let mut line = String::new();
        let mut x = 0.0;
        while x <= 15.0 {
            let h = loaded
                .terrain
                .ground
                .ground_height_below(x, y, 3.0)
                .unwrap_or(f64::NAN);
            line.push_str(&format!("{x:.1}:{h:.3} "));
            x += 0.5;
        }
        println!("{name} y={y}: {line}");
    }
    Ok(())
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[(((sorted.len() - 1) as f64) * p).ceil() as usize]
}

fn cpu(root: &PathBuf, args: &[String]) -> anyhow::Result<()> {
    let robots: usize = args[0].parse()?;
    let mode = args[1].clone();
    let warmup_s: f64 = args[2].parse()?;
    let measure_s: f64 = args[3].parse()?;
    let rep: u32 = args[4].parse()?;
    let loaded = load(root)?;
    let mut field = build(&loaded, 0.4, true)?;
    let mut ids = Vec::new();
    for index in 0..robots {
        let team = if index % 2 == 0 {
            Team::Red
        } else {
            Team::Blue
        };
        let (point, yaw) = layout::spawn_slot(team, index / 2);
        let id = spawn(&mut field, &loaded, team, point, yaw)?;
        if mode != "idle" {
            field.command_chassis(id, drive(1.5, 0.0, 0.6))?;
        }
        ids.push(id);
    }
    let batch = (16_000_000 / tick_ns()).max(1);
    let batch_ns = batch * tick_ns();
    let warmup_batches = (warmup_s * 1e9 / batch_ns as f64).round() as u64;
    let measure_batches = (measure_s * 1e9 / batch_ns as f64).round() as u64;
    // Sustained fire: every chassis launches at 20 Hz, the default cadence.
    let fire_every = (50_000_000 / batch_ns).max(1);
    let fire = |field: &mut Field, index: u64| -> anyhow::Result<()> {
        if mode == "fire" && index % fire_every == 0 {
            for id in &ids {
                if let Some(muzzle) = field.chassis_muzzle_pose(*id) {
                    field.fire(muzzle, Shot::at_limit(Caliber::Mm17), Some(*id))?;
                }
            }
        }
        Ok(())
    };
    for index in 0..warmup_batches {
        fire(&mut field, index)?;
        field.step(batch)?;
    }
    let checkpoint = field.snapshot();
    let geometry = field.static_geometry_snapshot();
    let floor = field.floor_height_m();

    let mut batches = Vec::with_capacity(measure_batches as usize);
    let wall = Instant::now();
    for index in 0..measure_batches {
        fire(&mut field, index)?;
        let start = Instant::now();
        field.step(black_box(batch))?;
        batches.push(start.elapsed().as_secs_f64() * 1e3);
    }
    let wall_s = wall.elapsed().as_secs_f64();
    let simulated_s = measure_batches as f64 * batch_ns as f64 * 1e-9;

    // Restored replay: rebuild the whole field from the checkpoint and replay
    // 250 ms through the ordinary step path, as a client's prediction does.
    let replay_ns = 250_000_000u64;
    let replay_batches = (replay_ns / batch_ns).max(1);
    let mut restores = Vec::new();
    let mut replays = Vec::new();
    for _ in 0..20 {
        let start = Instant::now();
        let mut replay = Field::restore(&checkpoint, &geometry, floor)?;
        restores.push(start.elapsed().as_secs_f64() * 1e3);
        let start = Instant::now();
        for _ in 0..replay_batches {
            replay.step(black_box(batch))?;
        }
        replays.push(start.elapsed().as_secs_f64() * 1e3);
        black_box(&replay);
    }

    batches.sort_by(f64::total_cmp);
    restores.sort_by(f64::total_cmp);
    replays.sort_by(f64::total_cmp);
    let total_ms: f64 = batches.iter().sum();
    let cell = format!("{mode}_{robots}");
    println!(
        "{},{cell},{rep},{},{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{}",
        tick_ns(),
        robots,
        mode,
        simulated_s,
        total_ms / simulated_s,
        percentile(&batches, 0.5),
        percentile(&batches, 0.95),
        percentile(&batches, 0.99),
        percentile(&batches, 1.0),
        percentile(&restores, 0.5),
        percentile(&replays, 0.5),
        percentile(&replays, 1.0),
        replay_batches as f64 * batch_ns as f64 * 1e-9,
        wall_s,
        batches.iter().filter(|ms| **ms > 16.0).count(),
    );
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(args.len() >= 2, "usage: physics_rate MODE CAD_PATH [..]");
    let root = PathBuf::from(&args[1]);
    match args[0].as_str() {
        "probe" => probe(&root),
        "fixtures" => fixtures(&root),
        "cpu" => cpu(&root, &args[2..]),
        "header" => {
            println!(
                "rate_ns,cell,rep,robots,mode,simulated_s,cpu_ms_per_sim_s,batch_p50_ms,\
                 batch_p95_ms,batch_p99_ms,batch_max_ms,restore_p50_ms,replay_p50_ms,\
                 replay_max_ms,replay_simulated_s,wall_s,batches_over_16ms"
            );
            Ok(())
        }
        other => anyhow::bail!("unknown mode `{other}`"),
    }
}
