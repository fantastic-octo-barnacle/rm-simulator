// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Measure reconciliation cost on the installed CAD package, without a window.
use rm_simulator_server::{
    cad_assets, layout::LayoutOptions, prediction::Replay, simulation::Simulation,
};
use rm_simulator_world::{ChassisCommand, ChassisConfig, Team};
fn main() -> anyhow::Result<()> {
    let cad = cad_assets::load(&cad_assets::default_cad_assets())?;
    let mut host = Simulation::from_cad(
        &cad,
        &LayoutOptions {
            rune: None,
            terrain: true,
            referee: true,
            projectile_policy: Default::default(),
            outpost_speed_rad_s: 0.0,
        },
        Some(ChassisConfig::default()),
        false,
        |_| {},
    )?;
    let id = host.spawn_chassis(Team::Red).map_err(anyhow::Error::msg)?;
    host.apply(&rm_simulator_server::protocol::Command::Chassis {
        chassis: id,
        command: ChassisCommand {
            forward_m_s: 1.0,
            ..Default::default()
        },
    })
    .map_err(anyhow::Error::msg)?;
    let geometry = host.field().static_geometry_snapshot();
    for ticks in [32, 64, 128, 200] {
        let mut costs = Vec::new();
        let mut correction: f64 = 0.0;
        for _ in 0..20 {
            let state = host.snapshot();
            let replay = Replay {
                snapshot: std::sync::Arc::new(state.clone()),
                snapshot_id: 0,
                chassis: id,
                states: state.chassis.clone(),
                snapshot_time_ns: state.time_ns,
                context_time_ns: state.time_ns,
                context_id: 0,
                target_time_ns: state.time_ns + ticks * 1_000_000,
                inputs: Vec::new(),
            };
            let started = std::time::Instant::now();
            let predicted = replay
                .run(&geometry, rm_simulator_server::layout::CATCH_FLOOR_M)
                .map_err(anyhow::Error::msg)?;
            costs.push(started.elapsed().as_secs_f64() * 1000.0);
            host.step(ticks)?;
            let actual = host.snapshot().chassis.remove(0);
            let distance = actual
                .pose
                .translation_m
                .into_iter()
                .zip(predicted.pose.translation_m)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f64>()
                .sqrt();
            correction = correction.max(distance);
        }
        costs.sort_by(f64::total_cmp);
        println!(
            "{ticks} ms replay: median {:.2} ms, max {:.2} ms; max correction {:.6} m",
            costs[10], costs[19], correction
        );
    }
    Ok(())
}
