// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Compare ground rays through the real package loader before deploying a derivative.
use rm_simulator_server::{cad_assets, collision_mesh, layout};

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    anyhow::ensure!(args.len() == 2, "usage: compare_ground BEFORE AFTER");
    let before = layout::load_terrain(&cad_assets::load(std::path::Path::new(&args[0]))?)?;
    let after = layout::load_terrain(&cad_assets::load(std::path::Path::new(&args[1]))?)?;
    let mut samples = 0;
    let mut missing = 0;
    let mut max_delta_m = 0.0_f64;
    let mut worst = [0.0; 3];
    let mut max_near_ceiling_delta_m = 0.0_f64;
    for ix in -58..=58 {
        for iy in -34..=34 {
            for ceiling in [0.15, 0.6, 2.0] {
                let x = f64::from(ix) * 0.25;
                let y = f64::from(iy) * 0.25;
                let a = before.ground.ground_height_below(x, y, ceiling);
                let b = after.ground.ground_height_below(x, y, ceiling);
                samples += 1;
                if let Some(a) = a {
                    let nearby = after.ground.ground_height_below(x, y, ceiling + 0.005);
                    let nearest = b
                        .into_iter()
                        .chain(nearby)
                        .map(|b| (a - b).abs())
                        .reduce(f64::min);
                    if let Some(delta) = nearest {
                        max_near_ceiling_delta_m = max_near_ceiling_delta_m.max(delta);
                    }
                }
                match (a, b) {
                    (Some(a), Some(b)) if (a - b).abs() > max_delta_m => {
                        max_delta_m = (a - b).abs();
                        worst = [x, y, ceiling];
                    }
                    (Some(_), None) | (None, Some(_)) => missing += 1,
                    _ => {}
                }
            }
        }
    }
    for (label, path) in [("before", &args[0]), ("after", &args[1])] {
        let cad = cad_assets::load(std::path::Path::new(path))?;
        for asset in &cad.static_assets {
            for part in collision_mesh::load_glb_parts(&asset.physics_file(&cad.root))? {
                for pose in &asset.placements {
                    if let Some(z) = part
                        .mesh
                        .placed(pose)
                        .ground_height_below(worst[0], worst[1], 2.0)
                        && z > 0.01
                    {
                        eprintln!("{label} {} {} height={z}", asset.file, part.name);
                    }
                }
            }
        }
    }
    let count = |terrain: &layout::Terrain| {
        terrain.boundary.triangles.len()
            + terrain.ground.triangles.len()
            + terrain.fixtures.triangles.len()
            + terrain
                .equipment
                .iter()
                .map(|mesh| mesh.triangles.len())
                .sum::<usize>()
            + terrain
                .mechanisms
                .iter()
                .map(|mechanism| mechanism.mesh.triangles.len())
                .sum::<usize>()
    };
    println!(
        "{}",
        serde_json::json!({
            "before_collision_triangles": count(&before),
            "after_collision_triangles": count(&after),
            "ground_ray_samples": samples,
            "surface_presence_changes": missing,
            "maximum_height_difference_m": max_delta_m,
            "maximum_difference_with_5mm_ceiling_slack_m": max_near_ceiling_delta_m,
            "worst_ray_x_y_ceiling_m": worst,
            "worst_ray_nearby_ceilings": ([worst[2]-0.005, worst[2], worst[2]+0.005, 2.0].map(|ceiling| {
                serde_json::json!({"ceiling_m": ceiling,
                    "before_m": before.ground.ground_height_below(worst[0], worst[1], ceiling),
                    "after_m": after.ground.ground_height_below(worst[0], worst[1], ceiling)})
            })),
            "scope": "0.25 m whole-field grid, three ray ceilings; sampled check, not clearance proof"
        })
    );
    Ok(())
}
