// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Visual-only manifest loading. No world, server, collider, or referee dependency.
use anyhow::{Context, ensure};
use bevy::prelude::*;
use rm_simulator_render::{
    TeamColor,
    cad::{CadInstance, CadRole},
    flu_position,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct Manifest {
    schema_version: u32,
    units: String,
    floor_top_source_z_m: Option<f64>,
    assets: BTreeMap<String, Asset>,
    #[serde(default)]
    static_assets: Vec<String>,
}
#[derive(Deserialize)]
struct Asset {
    visual: String,
    visual_sha256: String,
    #[serde(default)]
    placements_in_source_arena_frame: Vec<Placement>,
}
#[derive(Deserialize)]
struct Placement {
    translation_m: [f64; 3],
    rotation_xyzw: [f64; 4],
}
/// CAD scene instances and verified file hashes for one asset package.
#[derive(Resource)]
pub struct Loaded {
    /// One entry per arena or equipment placement, already in FLU metres.
    pub instances: Vec<CadInstance>,
    /// SHA-256 of both manifests and every visual file, keyed by package-relative path.
    pub hashes: BTreeMap<String, String>,
}
/// SHA-256 of one file as lowercase hexadecimal.
pub fn hash(path: &Path) -> anyhow::Result<String> {
    Ok(format!("{:x}", Sha256::digest(std::fs::read(path)?)))
}
fn read(path: &Path) -> anyhow::Result<Manifest> {
    let data: Manifest = serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("reading {}", path.display()))?,
    )?;
    ensure!(
        data.schema_version == 1 && data.units == "metres",
        "unsupported manifest units/schema"
    );
    Ok(data)
}
fn safe(root: &Path, file: &str) -> anyhow::Result<PathBuf> {
    let path = root.join(file).canonicalize()?;
    ensure!(
        path.starts_with(root.canonicalize()?),
        "asset escapes package"
    );
    Ok(path)
}
/// Load one CAD package into scene instances, verifying every visual against its manifest hash.
///
/// Reads `manifest.json` and `equipment/manifest.json`, then places `floor`, `arena-static`,
/// `rune`, `outpost`, the manifest's extra static assets, `base` and `tech-core`. The playing
/// floor top comes from the manifest's `floor_top_source_z_m` and falls back to the V2.0.0 pad
/// height of -1.5304 m. Placements are source-arena coordinates: `y` is shifted by the arena
/// centre 1.6243436 m and `z` by the floor top, then converted to Bevy axes. `floor` and
/// `arena-static` use the identity placement because their manifests carry none. Bases, energy
/// cores and the outpost take the red team colour at x >= 0 m and blue below. The floor and
/// arena-static override unpainted materials with a fixed grey, which replaces the V2.0.0
/// exporter's cream with a neutral surface.
pub fn load(root: &Path) -> anyhow::Result<Loaded> {
    let arena = read(&root.join("manifest.json"))?;
    let equipment = read(&root.join("equipment/manifest.json"))?;
    // Same fitted CAD frame as server/cad_assets.rs, but no simulator dependency.
    let floor = arena.floor_top_source_z_m.unwrap_or(-1.5304);
    ensure!(
        floor.is_finite() && (-3. ..=0.).contains(&floor),
        "invalid floor reference"
    );
    let conversion = Quat::from_mat3(&Mat3::from_cols(Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y));
    let mut loaded = Loaded {
        instances: Vec::new(),
        hashes: BTreeMap::new(),
    };
    for file in ["manifest.json", "equipment/manifest.json"] {
        loaded.hashes.insert(file.into(), hash(&root.join(file))?);
    }
    let names = ["floor", "arena-static", "rune", "outpost"]
        .into_iter()
        .map(str::to_string)
        .chain(arena.static_assets.clone());
    for (prefix, manifest, names) in [
        ("", &arena, names.collect::<Vec<_>>()),
        (
            "equipment/",
            &equipment,
            vec!["base".into(), "tech-core".into()],
        ),
    ] {
        for name in names {
            let asset = manifest
                .assets
                .get(&name)
                .with_context(|| format!("missing asset {name}"))?;
            let file = format!("{prefix}{}", asset.visual);
            let actual = hash(&safe(root, &file)?)?;
            ensure!(
                actual == asset.visual_sha256,
                "visual hash mismatch: {file}"
            );
            loaded.hashes.insert(file.clone(), actual);
            let origin = Placement {
                translation_m: [0.; 3],
                rotation_xyzw: [0., 0., 0., 1.],
            };
            let placements: Vec<_> = if name == "floor" || name == "arena-static" {
                vec![&origin]
            } else {
                asset.placements_in_source_arena_frame.iter().collect()
            };
            ensure!(!placements.is_empty(), "missing placements: {name}");
            for p in placements {
                ensure!(
                    p.translation_m
                        .iter()
                        .chain(&p.rotation_xyzw)
                        .all(|v| v.is_finite()),
                    "invalid placement"
                );
                let q = Quat::from_xyzw(
                    p.rotation_xyzw[0] as f32,
                    p.rotation_xyzw[1] as f32,
                    p.rotation_xyzw[2] as f32,
                    p.rotation_xyzw[3] as f32,
                );
                ensure!(
                    (q.length() - 1.).abs() < 1e-4,
                    "non-unit placement quaternion"
                );
                let t = [
                    p.translation_m[0],
                    p.translation_m[1] - 1.624_343_6,
                    p.translation_m[2] - floor,
                ];
                loaded.instances.push(CadInstance {
                    file: file.clone(),
                    transform: Transform {
                        translation: flu_position(t),
                        rotation: conversion * q,
                        ..default()
                    },
                    team: matches!(name.as_str(), "base" | "tech-core" | "outpost").then_some(
                        if t[0] >= 0. {
                            TeamColor::Red
                        } else {
                            TeamColor::Blue
                        },
                    ),
                    role: CadRole::Static {
                        base_color: match name.as_str() {
                            "floor" => Some(Color::srgb(0.24, 0.25, 0.27)),
                            "arena-static" => Some(Color::srgb(0.60, 0.61, 0.63)),
                            _ => None,
                        },
                    },
                });
            }
        }
    }
    Ok(loaded)
}
