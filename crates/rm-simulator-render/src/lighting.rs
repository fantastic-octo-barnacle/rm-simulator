// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Field lighting: ambient plus a shadowed key and an unshadowed fill.
//! Neutral white lamps preserve the CAD paint colours. Levels are assumed for
//! an indoor venue, not measured.
use bevy::prelude::*;

/// The venue key light; the fill stays unshadowed at every graphics preset.
#[derive(Component)]
pub struct ShadowKeyLight;

/// Spawn the venue lamps and set white global ambient light. The shadowed key
/// light is the only one that casts shadows; illuminances are assumed for an
/// indoor venue, not measured.
pub fn spawn_lighting(commands: &mut Commands) {
    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 70.0,
        ..default()
    });
    commands.spawn((
        ShadowKeyLight,
        DirectionalLight {
            illuminance: 1700.0,
            color: Color::WHITE,
            shadow_maps_enabled: true,
            ..default()
        },
        // The 29 m field fits within 40 m of the camera; keep the 10 m near cascade.
        bevy::light::CascadeShadowConfigBuilder {
            num_cascades: 2,
            first_cascade_far_bound: 10.0,
            maximum_distance: 40.0,
            ..default()
        }
        .build(),
        Transform::from_xyz(-4.0, 8.0, 1.0)
            .looking_at(Vec3::new(0.0, crate::FLOOR_Y_M, -3.0), Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 450.0,
            color: Color::WHITE,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(5.0, 4.0, -5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Dark indoor backdrop behind the stadium geometry.
pub fn skybox(images: &mut Assets<Image>) -> bevy::light::Skybox {
    use bevy::{
        asset::RenderAssetUsages,
        render::render_resource::{
            Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
        },
    };
    const SIZE: u32 = 64;
    let mut pixels = Vec::with_capacity((SIZE * SIZE * 6 * 4) as usize);
    for face in 0..6 {
        for y in 0..SIZE {
            for x in 0..SIZE {
                let u = 2.0 * (x as f32 + 0.5) / SIZE as f32 - 1.0;
                let v = 2.0 * (y as f32 + 0.5) / SIZE as f32 - 1.0;
                let direction = match face {
                    0 => Vec3::new(1.0, -v, -u),
                    1 => Vec3::new(-1.0, -v, u),
                    2 => Vec3::new(u, 1.0, v),
                    3 => Vec3::new(u, -1.0, -v),
                    4 => Vec3::new(u, -v, 1.0),
                    _ => Vec3::new(-u, -v, -1.0),
                }
                .normalize();
                let horizon = Vec3::new(16.0, 20.0, 28.0);
                let color = if direction.y >= 0.0 {
                    horizon.lerp(Vec3::new(4.0, 6.0, 10.0), direction.y.sqrt())
                } else {
                    horizon.lerp(Vec3::new(8.0, 10.0, 14.0), -direction.y)
                };
                pixels.extend([color.x as u8, color.y as u8, color.z as u8, 255]);
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE * 6,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image
        .reinterpret_stacked_2d_as_array(6)
        .expect("six square skybox faces");
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    bevy::light::Skybox {
        image: Some(images.add(image)),
        brightness: 500.0,
        ..default()
    }
}

/// Decorative venue, retained with the CAD across matches.
#[derive(Component)]
pub struct Stadium;

/// Low-poly stands and welded mesh fencing. Bounds come from the same grounded CAD scenery
/// used to place the collision walls. Venue dimensions are artistic settings.
pub fn spawn_stadium(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    corner_a: Vec3,
    corner_b: Vec3,
) {
    let low = corner_a.min(corner_b);
    let high = corner_a.max(corner_b);
    let center = (low + high) * 0.5;
    let half = (high - low) * 0.5;
    let mut steel = None;
    let mut stands = None;
    let mut lamps = None;
    let mut trim = None;
    // Four local wall frames: x runs along the fence, z points out of the field.
    for (offset, rotation, length) in [
        (Vec3::new(0.0, 0.0, half.z + 0.05), 0.0, half.x * 2.0),
        (
            Vec3::new(0.0, 0.0, -half.z - 0.05),
            std::f32::consts::PI,
            half.x * 2.0,
        ),
        (
            Vec3::new(half.x + 0.05, 0.0, 0.0),
            std::f32::consts::FRAC_PI_2,
            half.z * 2.0,
        ),
        (
            Vec3::new(-half.x - 0.05, 0.0, 0.0),
            -std::f32::consts::FRAC_PI_2,
            half.z * 2.0,
        ),
    ] {
        let rotation = Quat::from_rotation_y(rotation);
        let origin = Vec3::new(center.x, low.y, center.z) + offset;
        let height = high.y + 2.0 - low.y;
        let bar = |batch: &mut Option<Mesh>, size: Vec3, position: Vec3| {
            let mesh = Mesh::from(Cuboid::from_size(size)).transformed_by(Transform {
                translation: origin + rotation * position,
                rotation,
                ..default()
            });
            if let Some(batch) = batch {
                batch.merge(&mesh).expect("cuboids share vertex attributes");
            } else {
                *batch = Some(mesh);
            }
        };
        let panels = (length / 2.0).ceil() as u32;
        for i in 0..=panels {
            bar(
                &mut steel,
                Vec3::new(0.055, height, 0.055),
                Vec3::new(
                    -length / 2.0 + length * i as f32 / panels as f32,
                    height / 2.0,
                    0.0,
                ),
            );
        }
        for y in [0.0, height] {
            bar(
                &mut steel,
                Vec3::new(length, 0.045, 0.045),
                Vec3::new(0.0, y, 0.0),
            );
        }
        // Actual open geometry, rather than a transparent solid panel.
        let wires = (length / 0.16).ceil() as u32;
        for i in 0..=wires {
            bar(
                &mut steel,
                Vec3::new(0.009, height, 0.009),
                Vec3::new(
                    -length / 2.0 + length * i as f32 / wires as f32,
                    height / 2.0,
                    0.0,
                ),
            );
        }
        let rows = (height / 0.16).ceil() as u32;
        for i in 1..rows {
            bar(
                &mut steel,
                Vec3::new(length, 0.009, 0.009),
                Vec3::new(0.0, height * i as f32 / rows as f32, 0.0),
            );
        }
        for row in 0..10 {
            let y = 0.8 + row as f32 * 0.9;
            let z = 32.0 + row as f32 * 1.8;
            bar(
                &mut stands,
                Vec3::new(length + 64.0, 0.9, 1.8),
                Vec3::new(0.0, y, z),
            );
            bar(
                &mut trim,
                Vec3::new(length + 64.0, 0.05, 0.05),
                Vec3::new(0.0, y + 0.46, z - 0.9),
            );
        }
        bar(
            &mut stands,
            Vec3::new(length + 130.0, 36.0, 0.4),
            Vec3::new(0.0, 17.8, 65.0),
        );
        bar(
            &mut steel,
            Vec3::new(length + 64.0, 0.25, 0.25),
            Vec3::new(0.0, 28.0, 28.0),
        );
        for i in -2..=2 {
            bar(
                &mut lamps,
                Vec3::new(3.5, 0.1, 1.0),
                Vec3::new(i as f32 * (length + 64.0) / 5.0, 27.9, 28.0),
            );
        }
    }
    // A broad dark floor and ceiling close the room beyond the CAD slab.
    for y in [low.y - 0.02, low.y + 36.0] {
        let mesh = Mesh::from(Cuboid::new(
            half.x * 2.0 + 130.0,
            0.04,
            half.z * 2.0 + 130.0,
        ))
        .transformed_by(Transform::from_xyz(center.x, y, center.z));
        stands
            .as_mut()
            .expect("stands")
            .merge(&mesh)
            .expect("cuboid attributes");
    }
    for (index, (mesh, color, emission)) in [
        (steel, Color::srgb(0.28, 0.32, 0.36), 0.0),
        (stands, Color::srgb(0.025, 0.033, 0.05), 0.0),
        (trim, Color::srgb(0.10, 0.18, 0.24), 0.4),
        (lamps, Color::srgb(0.85, 0.92, 1.0), 5.0),
    ]
    .into_iter()
    .enumerate()
    {
        let mut entity = commands.spawn((
            Stadium,
            Mesh3d(meshes.add(mesh.expect("venue batch"))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: color,
                emissive: color.to_linear() * emission,
                perceptual_roughness: 0.8,
                ..default()
            })),
            Transform::default(),
        ));
        if index == 1 {
            // Venue shells must not occlude the directional lights used as
            // indoor field lamps. Fence posts still cast their own shadows.
            entity.insert(bevy::light::NotShadowCaster);
        }
    }
}
