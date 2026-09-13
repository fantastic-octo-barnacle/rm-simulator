// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Emissive rune overlays for the imported CAD rune faces: active target, activated
//! outline, and Big Rune progress lights on each blade. The CAD owns the backing,
//! hub and stand. Active target geometry follows the Vision solver's 270/150 mm
//! luminous boundaries and targets orbit at the CAD's 698.5 mm radius.
use crate::{RenderingConfig, TeamColor};
use bevy::{
    asset::RenderAssetUsages, mesh::Indices, prelude::*, render::render_resource::PrimitiveTopology,
};
/// Rotating root of one rune; `rune` indexes the caller's scene state.
#[derive(Component)]
pub struct RuneVisual {
    /// Index into `SceneState::runes`.
    pub rune: u32,
}
/// One Big Rune progress segment along an arm outline.
#[derive(Component)]
pub struct RuneProgressLight {
    /// Index into `SceneState::runes`.
    pub rune: u32,
    /// Progress stage, 0 to 4, growing from the hub outward.
    pub stage: u32,
}
/// Parent of one blade's lights; its local rotation places the blade.
#[derive(Component)]
pub struct RuneBlade {
    /// Blade index, 0 to 4.
    pub id: u32,
}
/// A light that shows while the blade is still an available target.
#[derive(Component)]
pub struct RuneActiveLight {
    /// Index into `SceneState::runes`.
    pub rune: u32,
    /// Blade index, 0 to 4.
    pub id: u32,
}
/// A light that shows while the blade's target is completed.
#[derive(Component)]
pub struct RuneActivatedLight {
    /// Index into `SceneState::runes`.
    pub rune: u32,
    /// Blade index, 0 to 4.
    pub id: u32,
}
/// Phase-specific target rings and framing (Figures 5-20 and 5-23).
#[derive(Component)]
pub struct RuneCompletionDetail {
    /// Index into `SceneState::runes`.
    pub rune: u32,
    /// False selects the thin completed-arm ring used during activation.
    pub full: bool,
}
/// Arrow row phase. Flow speed is a rendering assumption, not a rule constant.
#[derive(Component)]
pub struct RuneFlowArrow {
    /// Row index along the arm, 0 nearest the hub.
    pub row: u32,
}

/// One lit part of an active target, with its normal and struck materials.
#[derive(Component)]
pub struct RuneTargetLight {
    /// Index into `SceneState::runes`.
    pub rune: u32,
    /// Target or blade index, 0 to 4.
    pub id: u32,
    /// Powered team-coloured material.
    pub lit: Handle<StandardMaterial>,
    /// Grey strike-flash material.
    pub struck: Handle<StandardMaterial>,
}

/// Spawn the five-blade light overlay. The caller owns the hub pose and wheel rotation.
/// Local +Y is the initial active blade; local +Z is the visible front normal.
pub fn spawn_rune(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    rune: u32,
    rendering: RenderingConfig,
    color: TeamColor,
) {
    let (base_color, emission) = color.light();
    let active = materials.add(StandardMaterial {
        base_color,
        emissive: if rendering.is_lit() {
            emission * rendering.emissive_strength
        } else {
            LinearRgba::BLACK
        },
        emissive_exposure_weight: 1.0,
        unlit: !rendering.is_lit(),
        ..default()
    });
    crate::quality::register_emission(commands, materials, &active, rendering.emissive_strength);
    let struck = materials.add(crate::outpost::struck_material(rendering));
    // Front-facing strips keep the opposite team's lights from shining
    // through the open arm channels. All optical meshes face local +Z.
    let strip = meshes.add(Rectangle::new(1.0, 1.0));
    let spoke = meshes.add(cardinal_spoke());
    // Figure 5-22: 270/150/70 mm outer diameters, 20 mm radial widths.
    let rings = [0.135, 0.075, 0.035].map(|radius| meshes.add(annulus(radius - 0.02, radius)));
    let completed_ring = meshes.add(annulus(0.13, 0.135));
    let centre_ring = meshes.add(annulus(0.025, 0.03));
    let root = commands
        .spawn((
            RuneVisual { rune },
            Transform::from_xyz(0.0, 1.5, -5.0),
            Visibility::default(),
        ))
        .id();
    for blade in 0..5 {
        let angle = blade as f32 * std::f32::consts::TAU / 5.0;
        let parent = commands
            .spawn((
                Transform::from_rotation(Quat::from_rotation_z(angle)),
                Visibility::default(),
                ChildOf(root),
                RuneBlade { id: blade },
            ))
            .id();
        let active_parent = commands
            .spawn((
                Transform::default(),
                if blade == 0 {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                },
                RuneActiveLight { rune, id: blade },
                ChildOf(parent),
            ))
            .id();
        let activated_parent = commands
            .spawn((
                Transform::default(),
                Visibility::Hidden,
                RuneActivatedLight { rune, id: blade },
                ChildOf(parent),
            ))
            .id();
        let fully_activated_parent = commands
            .spawn((
                Transform::default(),
                Visibility::Hidden,
                RuneCompletionDetail { rune, full: true },
                ChildOf(parent),
            ))
            .id();
        // Figure 5-21 shows progress growing from the hub along the arm
        // outlines. Five equal path lengths are a diagram-based approximation.
        for stage in 0..5 {
            for side in [-1.0, 1.0] {
                let points = [
                    Vec3::new(side * 0.025, 0.10, 0.0015),
                    Vec3::new(side * 0.13, 0.32, 0.0015),
                    Vec3::new(side * 0.13, 0.51, 0.0015),
                ];
                let lengths = [
                    (points[1] - points[0]).length(),
                    (points[2] - points[1]).length(),
                ];
                let total = lengths[0] + lengths[1];
                let start = stage as f32 * total / 5.0;
                let end = (stage + 1) as f32 * total / 5.0;
                let mut offset = 0.0;
                for segment in 0..2 {
                    let lo = start.max(offset);
                    let hi = end.min(offset + lengths[segment]);
                    if hi > lo {
                        let a = points[segment]
                            .lerp(points[segment + 1], (lo - offset) / lengths[segment]);
                        let b = points[segment]
                            .lerp(points[segment + 1], (hi - offset) / lengths[segment]);
                        commands.spawn((
                            Mesh3d(strip.clone()),
                            MeshMaterial3d(active.clone()),
                            beam(a, b, 0.012, 0.002),
                            RuneProgressLight { rune, stage },
                            Visibility::Hidden,
                            ChildOf(parent),
                        ));
                    }
                    offset += lengths[segment];
                }
            }
        }
        let completed_ring_parent = commands
            .spawn((
                Transform::default(),
                Visibility::Hidden,
                RuneCompletionDetail { rune, full: false },
                ChildOf(activated_parent),
            ))
            .id();
        let draw_parent = std::cell::Cell::new(completed_ring_parent);
        let arrow_row = std::cell::Cell::new(None);
        let mut part =
            |mesh: Handle<Mesh>, material: Handle<StandardMaterial>, transform: Transform| {
                // Sit 1.5 mm ahead of the blade so the lights never z-fight the CAD.
                let mut transform = transform;
                transform.translation.z += 0.0015;
                let mut entity = commands.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(material.clone()),
                    transform,
                    ChildOf(draw_parent.get()),
                ));
                if let Some(row) = arrow_row.get() {
                    entity.insert(RuneFlowArrow { row });
                }
                if draw_parent.get() == active_parent {
                    entity.insert(RuneTargetLight {
                        rune,
                        id: blade,
                        lit: material,
                        struck: struck.clone(),
                    });
                }
            };
        // 372 x 382 mm framing envelope; bevel proportions are diagram-informed assumptions.
        let outline = [
            Vec3::new(-0.13, 0.891, 0.0),
            Vec3::new(0.13, 0.891, 0.0),
            Vec3::new(0.186, 0.85, 0.0),
            Vec3::new(0.186, 0.55, 0.0),
            Vec3::new(0.13, 0.509, 0.0),
            Vec3::new(-0.13, 0.509, 0.0),
            Vec3::new(-0.186, 0.55, 0.0),
            Vec3::new(-0.186, 0.85, 0.0),
        ];
        part(
            completed_ring.clone(),
            active.clone(),
            Transform::from_xyz(0.0, 0.6985, 0.0),
        );
        draw_parent.set(fully_activated_parent);
        part(
            centre_ring.clone(),
            active.clone(),
            Transform::from_xyz(0.0, 0.6985, 0.0),
        );
        for i in 0..outline.len() {
            part(
                strip.clone(),
                active.clone(),
                beam(outline[i], outline[(i + 1) % outline.len()], 0.008, 0.002),
            );
        }
        draw_parent.set(activated_parent);
        for side in [-1.0, 1.0] {
            let points = [
                Vec3::new(side * 0.025, 0.10, 0.0),
                Vec3::new(side * 0.13, 0.32, 0.0),
                Vec3::new(side * 0.13, 0.51, 0.0),
            ];
            for pair in points.windows(2) {
                part(
                    strip.clone(),
                    active.clone(),
                    beam(pair[0], pair[1], 0.012, 0.002),
                );
            }
        }
        part(
            strip.clone(),
            active.clone(),
            beam(
                Vec3::new(0.0, 0.1, 0.0),
                Vec3::new(0.0, 0.51, 0.0),
                0.026,
                0.002,
            ),
        );
        draw_parent.set(active_parent);
        {
            for ring in &rings {
                part(
                    ring.clone(),
                    active.clone(),
                    Transform::from_xyz(0.0, 0.6985, 0.0),
                );
            }
            for cardinal in 0..4 {
                let a = cardinal as f32 * std::f32::consts::FRAC_PI_2;
                let direction = Vec3::new(a.sin(), a.cos(), 0.0);
                let center = Vec3::new(0.0, 0.6985, 0.0);
                part(
                    spoke.clone(),
                    active.clone(),
                    Transform::from_translation(center)
                        .with_rotation(Quat::from_rotation_arc(Vec3::Y, direction)),
                );
            }
            for arrow in 0..9 {
                arrow_row.set(Some(arrow));
                let y = 0.15 + arrow as f32 * 0.036;
                for side in [-1.0, 1.0] {
                    part(
                        strip.clone(),
                        active.clone(),
                        beam(
                            Vec3::new(side * 0.022, y, 0.0),
                            Vec3::new(0.0, y + 0.018, 0.0),
                            0.005,
                            0.001,
                        ),
                    );
                }
            }
        }
    }
}
/// Place a unit square strip so local +Y runs from `start` to `end` while the
/// front stays on local +Z.
fn beam(start: Vec3, end: Vec3, width: f32, depth: f32) -> Transform {
    let delta = end - start;
    Transform::from_translation((start + end) * 0.5)
        .with_rotation(Quat::from_rotation_z(-delta.x.atan2(delta.y)))
        .with_scale(Vec3::new(width, delta.length(), depth))
}
// Figure 5-22 gives a 35 mm outer tab and 7 degree tapered sides.
// Clip its curved outer edge to the 300 mm effective circle, not the 308 mm backing.
/// One of four tapered tabs around an available target, facing local +Z.
fn cardinal_spoke() -> Mesh {
    let radius = 0.15_f32;
    let half_width = 0.0175_f32;
    let outer_y = (radius * radius - half_width * half_width).sqrt();
    let inner_y = 0.05_f32;
    let inner_half = half_width - (outer_y - inner_y) * 7_f32.to_radians().tan();
    let mut positions = vec![
        [-inner_half, inner_y, 0.0006],
        [inner_half, inner_y, 0.0006],
    ];
    for i in 0..=16 {
        let x = half_width * (1.0 - 2.0 * i as f32 / 16.0);
        positions.push([x, (radius * radius - x * x).sqrt(), 0.0006]);
    }
    let indices: Vec<u32> = (1..positions.len() as u32 - 1)
        .flat_map(|i| [0, i, i + 1])
        .collect();
    let count = positions.len();
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; count])
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; count])
    .with_inserted_indices(Indices::U32(indices))
}
/// Flat annulus in the local XY plane, facing local +Z. `inner` and `outer` are
/// radii in metres.
fn annulus(inner: f32, outer: f32) -> Mesh {
    const SEGMENTS: usize = 96;
    let mut positions = Vec::with_capacity((SEGMENTS + 1) * 2);
    let mut indices = Vec::with_capacity(SEGMENTS * 6);
    for i in 0..=SEGMENTS {
        let a = i as f32 * std::f32::consts::TAU / SEGMENTS as f32;
        for radius in [inner, outer] {
            positions.push([radius * a.cos(), radius * a.sin(), 0.0]);
        }
    }
    for i in 0..SEGMENTS {
        let n = (i * 2) as u32;
        indices.extend_from_slice(&[n, n + 1, n + 3, n, n + 3, n + 2]);
    }
    let len = positions.len();
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; len])
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; len])
    .with_inserted_indices(Indices::U32(indices))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn light_strips_face_forward_even_when_the_path_runs_downward() {
        for end in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y] {
            let transform = beam(Vec3::ZERO, end, 0.01, 0.002);
            assert!((transform.rotation * Vec3::Z - Vec3::Z).length() < 1e-6);
            assert!((transform.rotation * Vec3::Y - end).length() < 1e-6);
        }
    }
    #[test]
    fn ring_boundary_and_front_plane_match_solver() {
        let mesh = annulus(0.115, 0.135);
        let bevy::mesh::VertexAttributeValues::Float32x3(vertices) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
        else {
            panic!("positions")
        };
        assert!(vertices.iter().all(|p| p[2] == 0.0));
        let max = vertices
            .iter()
            .map(|p| p[0].hypot(p[1]))
            .fold(0.0_f32, f32::max);
        assert!((max - 0.135).abs() < 1e-7);
    }
}
