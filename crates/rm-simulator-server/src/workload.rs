// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! The deterministic field the bandwidth probe and the measurement examples drive.
//!
//! Every bandwidth number in `docs/bandwidth-experiments.md` and every training
//! workload behind the compressed-dictionary checkpoints has to start from the
//! same field, or the numbers stop being comparable. The builder lives in the
//! library rather than in each harness so that "the same workload" is a
//! compiler-checked fact instead of a copied block: the two-rune, two-team
//! referee layout with no CAD, and `players` chassis that alternate teams in
//! spawn order.
//!
//! This is measurement scaffolding, not gameplay: nothing in the running host
//! calls it, and it reads no assets.
use crate::layout::ChassisSpawner;
use crate::simulation::Simulation;
use rm_simulator_world::{ChassisCommand, ChassisConfig, Field, FieldConfig, RefereeConfig, Team};

/// Build one measurement field and its chassis.
///
/// No CAD is read: the implicit floor and the rule layout are enough for chassis
/// motion, projectiles and the referee, so a workload needs no assets and runs
/// the same everywhere. Chassis are spawned in index order and alternate teams,
/// so index parity alone fixes every team assignment. Returns the simulation and
/// the chassis ids in spawn order.
///
/// ```
/// use rm_simulator_server::workload;
///
/// let (simulation, chassis) = workload::simulation(2);
/// assert_eq!(chassis.len(), 2);
/// assert_eq!(simulation.state().field.chassis.len(), 2);
/// ```
pub fn simulation(players: usize) -> (Simulation, Vec<u32>) {
    let mut config = FieldConfig {
        referee: Some(RefereeConfig::alternating(2, 2)),
        ..Default::default()
    };
    config.runes.push(config.runes[0]);
    let mut simulation = Simulation::new(
        Field::new(&config).expect("the measurement field configuration is valid"),
        false,
    )
    .with_spawner(ChassisSpawner {
        config: ChassisConfig::default(),
        terrain: None,
    });
    let chassis = (0..players)
        .map(|index| {
            let team = if index.is_multiple_of(2) {
                Team::Red
            } else {
                Team::Blue
            };
            simulation
                .spawn_chassis(team)
                .expect("the measurement field offers a chassis per player")
        })
        .collect();
    (simulation, chassis)
}

/// The command the constant-drive measurement workloads apply to every chassis:
/// a steady forward speed with a small yaw rate, so a run carries motion without
/// depending on how many frames have elapsed.
pub fn constant_drive() -> ChassisCommand {
    ChassisCommand {
        forward_m_s: 1.0,
        yaw_rate_rad_s: 0.4,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn players_alternate_teams_in_spawn_order() {
        let (simulation, chassis) = simulation(4);
        assert_eq!(chassis.len(), 4);
        let state = simulation.state();
        let teams: Vec<Team> = state
            .field
            .chassis
            .iter()
            .map(|chassis| chassis.team)
            .collect();
        assert_eq!(teams, vec![Team::Red, Team::Blue, Team::Red, Team::Blue]);
    }

    #[test]
    fn an_empty_workload_needs_no_chassis() {
        let (_, chassis) = simulation(0);
        assert!(chassis.is_empty());
    }

    #[test]
    fn the_field_carries_two_runes_and_a_referee() {
        let (simulation, _) = simulation(1);
        let state = simulation.state();
        assert_eq!(state.field.runes.len(), 2);
        assert!(state.field.referee.is_some());
    }
}
