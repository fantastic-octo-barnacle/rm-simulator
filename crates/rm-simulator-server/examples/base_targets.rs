// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Reproduce the scoring-frame fit; add --check to shoot every plate with CAD collision.
use rm_simulator_server::{base_layout, cad_assets, layout, math};
use rm_simulator_world::{ArmorTarget, Caliber, Field, Pose, RefereeCommand, Shot, Team};
fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let root = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(cad_assets::default_cad_assets);
    let cad = cad_assets::load(&root)?;
    let bases = base_layout::load(&cad)?;
    println!("{}", serde_json::to_string_pretty(&bases)?);
    if args.iter().any(|a| a == "--check") {
        let options = layout::LayoutOptions {
            rune: None,
            outpost_speed_rad_s: 0.,
            terrain: true,
            referee: true,
            physics_rate_hz: 1000,
        };
        let mut config = layout::field_config(&cad, &options);
        config.bases = bases;
        let mut field = Field::new(&config)?;
        layout::add_terrain(&mut field, &layout::load_terrain(&cad)?)?;
        for team in [Team::Red, Team::Blue] {
            field.referee_command(RefereeCommand::SetBaseOpen { team, open: true })?;
        }
        let mut failures = 0;
        for base in 0..config.bases.len() {
            for plate in 0..7 {
                let target = field.snapshot().bases[base].pose(plate, field.time_ns());
                let normal = math::rotate(target.rotation_wxyz, [1., 0., 0.]);
                let muzzle = Pose {
                    translation_m: std::array::from_fn(|i| {
                        target.translation_m[i] + normal[i] * 0.05
                    }),
                    rotation_wxyz: math::quat_multiply(
                        target.rotation_wxyz,
                        math::quat_axis_angle([0., 0., 1.], std::f64::consts::PI),
                    ),
                };
                let id = field.fire(muzzle, Shot::at_limit(Caliber::Mm17), None)?;
                field.step(60)?;
                let snapshot = field.snapshot();
                let hit = snapshot.hits.iter().find(|h| {
                    h.projectile == id
                        && h.detected
                        && h.target
                            == ArmorTarget::Base {
                                base: base as u32,
                                plate: plate as u32,
                            }
                });
                eprintln!(
                    "base {base} plate {plate}: {}",
                    if hit.is_some() { "hit" } else { "MISSED" }
                );
                failures += usize::from(hit.is_none());
            }
        }
        anyhow::ensure!(failures == 0, "{failures} base scoring checks failed");
    }
    Ok(())
}
