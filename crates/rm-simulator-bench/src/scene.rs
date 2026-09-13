// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Fixed camera, lighting and geometry preparation for the frozen CAD scene.
//! Nothing here advances gameplay or physics; the camera is placed once and never moves.
use crate::{
    assets::Loaded,
    config::{Args, Case, Geometry},
};
use bevy::{
    camera::{Exposure, Hdr},
    core_pipeline::{prepass::DepthPrepass, tonemapping::Tonemapping},
    light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap},
    post_process::bloom::Bloom,
    prelude::*,
    render::occlusion_culling::OcclusionCulling,
};
use rm_simulator_render::{
    cad::{CadMesh, CadSceneStatus},
    flu_position, lighting,
};
use std::collections::HashMap;
/// Image the camera renders into when `--headless` is set; `None` renders to the primary window.
#[derive(Resource, Default)]
pub struct Target(
    /// Handle of the image created at the case resolution, used again for the screenshot.
    pub Option<Handle<Image>>,
);
/// Whether geometry preparation has run, plus the counts it recorded for the report.
#[derive(Resource, Default)]
pub struct GeometryState {
    /// Set once `prepare_geometry` has measured and rewritten the scene meshes.
    pub ready: bool,
    /// Mesh, triangle and bounds figures copied into `report.json`.
    pub report: serde_json::Value,
}
/// Spawns the lighting, the offscreen target and the single fixed camera.
///
/// The camera keeps the case's vertical field of view, exposure, MSAA and a skybox, and looks
/// from `camera_position_flu_m` toward `camera_target_flu_m` with world up. Bloom, depth prepass
/// and occlusion culling are attached only when the resolved settings ask for them; occlusion
/// culling also requires the depth prepass.
pub fn setup(
    mut commands: Commands,
    case: Res<Case>,
    args: Res<Args>,
    mut images: ResMut<Assets<Image>>,
    mut target: ResMut<Target>,
) {
    let settings = case.graphics.resolved();
    lighting::spawn_lighting(&mut commands);
    commands.insert_resource(DirectionalLightShadowMap {
        size: settings.shadow_map_size,
    });
    let render_target = if args.headless {
        let handle = images.add(Image::new_target_texture(
            case.resolution[0],
            case.resolution[1],
            bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
            None,
        ));
        target.0 = Some(handle.clone());
        bevy::camera::RenderTarget::Image(handle.into())
    } else {
        default()
    };
    let mut camera = commands.spawn((
        Camera3d::default(),
        Camera::default(),
        render_target,
        Hdr,
        Exposure {
            ev100: settings.exposure_ev100,
        },
        Projection::Perspective(PerspectiveProjection {
            fov: case.vertical_fov_deg.to_radians(),
            ..default()
        }),
        Msaa::from_samples(settings.msaa_samples),
        Tonemapping::None,
        lighting::skybox(&mut images),
        Transform::from_translation(flu_position(case.camera_position_flu_m))
            .looking_at(flu_position(case.camera_target_flu_m), Vec3::Y),
    ));
    if settings.bloom {
        camera.insert(Bloom {
            intensity: settings.bloom_intensity,
            ..default()
        });
    }
    if settings.depth_prepass || settings.occlusion_culling {
        camera.insert(DepthPrepass);
    }
    if settings.occlusion_culling {
        camera.insert(OcclusionCulling);
    }
}
/// Applies the resolved shadow toggle and cascade layout to each newly added key light.
///
/// The first cascade far bound is half the shadow distance, capped at 10 m.
pub fn configure_lights(
    mut commands: Commands,
    case: Res<Case>,
    mut lights: Query<(Entity, &mut DirectionalLight), Added<lighting::ShadowKeyLight>>,
) {
    let settings = case.graphics.resolved();
    for (entity, mut light) in &mut lights {
        light.shadow_maps_enabled = settings.shadows;
        commands.entity(entity).insert(
            CascadeShadowConfigBuilder {
                num_cascades: settings.shadow_cascades,
                first_cascade_far_bound: (settings.shadow_distance_m * 0.5).min(10.),
                maximum_distance: settings.shadow_distance_m,
                ..default()
            }
            .build(),
        );
    }
}
/// Rewrites untextured CAD meshes for the case's geometry mode, adds the stadium, and records counts.
///
/// Runs once, after the CAD scene is ready and at least one mesh exists. Meshes whose material
/// has a base colour texture or is missing, and meshes of 12 or fewer triangles, keep their
/// authored geometry, and no asset file is modified. The recorded report holds mesh entity,
/// unique mesh and triangle counts before and after the rewrite, the loaded instance count, and
/// the scene bounds in Bevy axes.
#[allow(clippy::too_many_arguments)]
pub fn prepare_geometry(
    mut commands: Commands,
    case: Res<Case>,
    loaded: Res<Loaded>,
    status: Res<CadSceneStatus>,
    mut state: ResMut<GeometryState>,
    query: Query<(&Mesh3d, &MeshMaterial3d<StandardMaterial>, &GlobalTransform), With<CadMesh>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if state.ready || !status.ready() || query.is_empty() {
        return;
    }
    let mut original = HashMap::new();
    let mut low = Vec3::splat(f32::INFINITY);
    let mut high = Vec3::splat(f32::NEG_INFINITY);
    for (handle, material, transform) in &query {
        if let Some(mesh) = meshes.get(handle)
            && let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        {
            for p in positions {
                let p = transform.transform_point(Vec3::from_array(*p));
                low = low.min(p);
                high = high.max(p);
            }
        }
        if original.contains_key(&handle.id()) {
            continue;
        }
        let Some(mut mesh) = meshes.get_mut(handle) else {
            return;
        };
        let triangles = count(&mesh);
        original.insert(handle.id(), triangles);
        let textured = materials
            .get(&material.0)
            .is_none_or(|m| m.base_color_texture.is_some());
        if textured || triangles <= 12 || case.geometry == Geometry::Normal {
            continue;
        }
        match case.geometry {
            Geometry::Sparse => {
                let indices: Vec<u32> = mesh.indices().map_or_else(
                    || (0..mesh.count_vertices() as u32).collect(),
                    |indices| indices.iter().map(|i| i as u32).collect(),
                );
                mesh.insert_indices(bevy::mesh::Indices::U32(
                    indices
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .step_by(100)
                        .flatten()
                        .copied()
                        .collect(),
                ));
            }
            Geometry::Boxes => {
                if let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
                    mesh.attribute(Mesh::ATTRIBUTE_POSITION)
                {
                    let min = positions.iter().fold(Vec3::splat(f32::INFINITY), |a, p| {
                        a.min(Vec3::from_array(*p))
                    });
                    let max = positions
                        .iter()
                        .fold(Vec3::splat(f32::NEG_INFINITY), |a, p| {
                            a.max(Vec3::from_array(*p))
                        });
                    *mesh = Mesh::from(Cuboid::from_size((max - min).max(Vec3::splat(1e-6))))
                        .translated_by((min + max) * 0.5);
                }
            }
            Geometry::Normal => {}
        }
    }
    let before: usize = query.iter().map(|(h, _, _)| original[&h.id()]).sum();
    let after: usize = query
        .iter()
        .filter_map(|(h, _, _)| meshes.get(h))
        .map(count)
        .sum();
    if case.stadium {
        lighting::spawn_stadium(&mut commands, &mut meshes, &mut materials, low, high);
    }
    state.report = serde_json::json!({"cad_mesh_entities":query.iter().count(),"cad_unique_meshes":original.len(),"cad_triangles_before":before,"cad_triangles_after":after,"cad_instances":loaded.instances.len(),"bounds_bevy_m":[low.to_array(),high.to_array()]});
    state.ready = true;
}
fn count(mesh: &Mesh) -> usize {
    mesh.indices().map_or(mesh.count_vertices(), |i| i.len()) / 3
}
