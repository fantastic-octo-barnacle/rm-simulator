// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Small reproducible wall-clock comparison of the world's main stepping paths.
//! Run with `cargo run --release -p rm-simulator-world --example step_cost`.

use rm_simulator_world::{
    ChassisConfig, ChassisPlacement, Field, FieldConfig, RefereeConfig, Team,
};
use std::hint::black_box;
use std::time::{Duration, Instant};

const TICKS: u64 = 20_000;
const REPEATS: u32 = 5;

fn measure(mut build: impl FnMut() -> Field) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..REPEATS {
        let mut field = build();
        let started = Instant::now();
        field.step(black_box(TICKS)).unwrap();
        black_box(field.tick());
        best = best.min(started.elapsed());
    }
    best
}

fn report(name: &str, elapsed: Duration) {
    println!(
        "{name:20} {:9.3} ms total, {:9.1} ns/tick",
        elapsed.as_secs_f64() * 1_000.0,
        elapsed.as_nanos() as f64 / TICKS as f64
    );
}

fn main() {
    let empty = measure(|| Field::new(&FieldConfig::default()).unwrap());
    let referee = measure(|| {
        Field::new(&FieldConfig {
            referee: Some(RefereeConfig::alternating(1, 2)),
            ..FieldConfig::default()
        })
        .unwrap()
    });
    let chassis = measure(|| {
        let config = ChassisConfig::default();
        Field::new(&FieldConfig {
            chassis: vec![ChassisPlacement {
                spawn: rm_simulator_world::Pose::at([0.0, 0.0, config.rest_height_m()]),
                config,
                team: Team::Red,
            }],
            ..FieldConfig::default()
        })
        .unwrap()
    });

    println!("best of {REPEATS}, {TICKS} ticks each");
    report("empty fast-forward", empty);
    report("referee only", referee);
    report("one chassis", chassis);
}
