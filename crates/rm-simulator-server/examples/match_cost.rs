// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Loaded-field CPU and tail-cost probe. No rendering or wall-clock pacing.
//! Usage: match_cost CAD_PATH [MATCHES=1] [PILOTS=8] [SAMPLES=1000]
use rm_simulator_server::{cad_assets, layout};
use rm_simulator_world::{Caliber, ChassisCommand, ChassisConfig, Field, Shot, Team};
use std::{hint::black_box, path::PathBuf, time::Instant};

fn report(name: &str, samples: &mut [f64]) {
    samples.sort_by(f64::total_cmp);
    let at = |p: f64| samples[((samples.len() - 1) as f64 * p).ceil() as usize];
    println!(
        "{name}: median={:.3} p95={:.3} p99={:.3} max={:.3} ms",
        at(0.5),
        at(0.95),
        at(0.99),
        at(1.0)
    );
}
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("CAD_PATH required"));
    let matches: usize = args.next().unwrap_or("1".into()).parse()?;
    let pilots: usize = args.next().unwrap_or("8".into()).parse()?;
    let samples: usize = args.next().unwrap_or("1000".into()).parse()?;
    anyhow::ensure!(
        matches > 0 && pilots > 0 && samples > 0,
        "counts must be positive"
    );
    let started = Instant::now();
    let cad = cad_assets::load(&root)?;
    let terrain = layout::load_terrain(&cad)?;
    let config = layout::field_config(
        &cad,
        &layout::LayoutOptions {
            rune: Some(rm_simulator_world::RuneKind::Small),
            outpost_speed_rad_s: 0.4,
            terrain: true,
            referee: true,
            physics_rate_hz: 1000,
            projectile_policy: Default::default(),
        },
    );
    let mut template = Field::new(&config)?;
    layout::add_terrain(&mut template, &terrain)?;
    for index in 0..pilots {
        let team = if index % 2 == 0 {
            Team::Red
        } else {
            Team::Blue
        };
        let (point, yaw) = layout::spawn_slot(team, index / 2);
        let id = template.add_chassis(&layout::chassis_placement(
            ChassisConfig::default(),
            Some(&terrain),
            team,
            point,
            yaw,
        ))?;
        template.command_chassis(
            id,
            ChassisCommand {
                forward_m_s: 1.,
                yaw_rate_rad_s: 0.4,
                ..Default::default()
            },
        )?;
    }
    template.step(500)?;
    let snapshot = template.snapshot();
    let geometry = template.static_geometry_snapshot();
    println!(
        "{}; load={:.3}s; matches={matches}; pilots/match={pilots}; samples={samples}; batch=16 ticks",
        terrain.describe(),
        started.elapsed().as_secs_f64()
    );
    println!("CAD root={}", root.display());
    let wall = Instant::now();
    let results = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..matches)
            .map(|_| {
                let snapshot = &snapshot;
                let geometry = &geometry;
                let floor = template.floor_height_m();
                scope.spawn(move || {
                    let mut field = Field::restore(snapshot, geometry, floor).unwrap();
                    let mut steps = Vec::with_capacity(samples);
                    let mut snapshots = Vec::new();
                    let mut restores = Vec::new();
                    for sample in 0..samples {
                        if sample % 8 == 0 {
                            let shooters: Vec<_> =
                                field.snapshot().chassis.iter().map(|c| c.id).collect();
                            for id in shooters {
                                if let Some(muzzle) = field.chassis_muzzle_pose(id) {
                                    field
                                        .fire(muzzle, Shot::at_limit(Caliber::Mm17), Some(id))
                                        .unwrap();
                                }
                            }
                        }
                        let start = Instant::now();
                        field.step(black_box(16)).unwrap();
                        steps.push(start.elapsed().as_secs_f64() * 1000.);
                        if sample % 4 == 0 {
                            let start = Instant::now();
                            let checkpoint = field.snapshot();
                            snapshots.push(start.elapsed().as_secs_f64() * 1000.);
                            if sample % 64 == 0 {
                                let start = Instant::now();
                                black_box(Field::restore(&checkpoint, geometry, floor).unwrap());
                                restores.push(start.elapsed().as_secs_f64() * 1000.);
                            }
                        }
                    }
                    (steps, snapshots, restores)
                })
            })
            .collect();
        jobs.into_iter()
            .map(|job| job.join().unwrap())
            .collect::<Vec<_>>()
    });
    println!(
        "Concurrent run wall time {:.3}s, simulated seconds per match {:.3}",
        wall.elapsed().as_secs_f64(),
        samples as f64 * 0.016
    );
    for (index, (mut steps, mut snapshots, mut restores)) in results.into_iter().enumerate() {
        let over_budget = steps.iter().filter(|&&ms| ms > 16.).count();
        println!("match {index}: {over_budget}/{samples} step batches exceed 16 ms");
        report("step", &mut steps);
        report("snapshot", &mut snapshots);
        report("restore", &mut restores);
    }
    Ok(())
}
