// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Verify a package and report semantic bindings and actual static collider counts.
use rm_simulator_server::{cad_assets, layout};
fn main() -> anyhow::Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .expect("usage: inspect_assets ASSET_DIRECTORY");
    let start = std::time::Instant::now();
    let cad = cad_assets::load(std::path::Path::new(&root))?;
    println!(
        "manifest + semantic verification: {:.3}s",
        start.elapsed().as_secs_f64()
    );
    for asset in [&cad.rune, &cad.outpost, &cad.base, &cad.tech_core]
        .into_iter()
        .chain(&cad.static_assets)
    {
        if let Some(data) = asset.visual_semantics() {
            println!(
                "{}: {} semantic nodes, {} joints",
                asset.file,
                data.nodes.len(),
                data.joints.len()
            );
            for joint in &data.joints {
                println!(
                    "  {}: {} origin={:?}, axis={:?}, limits={:?}, verification={:?}",
                    joint.id,
                    joint.kind,
                    joint.origin_m,
                    joint.axis,
                    joint.limits,
                    joint.verification
                );
            }
            for pending in &data.pending {
                println!("  pending: {pending}");
            }
        }
    }
    let start = std::time::Instant::now();
    let terrain = layout::load_terrain(&cad)?;
    println!(
        "{}; parsed in {:.3}s",
        terrain.describe(),
        start.elapsed().as_secs_f64()
    );
    Ok(())
}
