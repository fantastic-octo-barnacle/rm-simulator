// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Projectile spheres drawn from the caller's
//! scene state. Requires the default plugins (meshes, materials and gizmos).
use crate::{
    RenderingConfig, flu_position,
    sync::{SceneInput, SceneSyncSet},
};
use bevy::prelude::*;

/// One pooled sphere; hidden when there are fewer projectiles than spheres.
#[derive(Component)]
pub struct ProjectileVisual;

/// Icosphere subdivision, a visual-only setting.
#[derive(Resource)]
pub struct ProjectileDetail(pub u32);
impl Default for ProjectileDetail {
    fn default() -> Self {
        Self(3)
    }
}

/// Shared unit sphere and material; each visual scales the mesh to one radius.
#[derive(Resource)]
struct ProjectileAssets {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

/// Draws the caller's projectiles as a pool of emissive spheres.
pub struct ProjectileVisualsPlugin {
    /// Appearance used for the sphere material.
    pub rendering: RenderingConfig,
}
/// Appearance captured from the plugin at build time.
#[derive(Resource, Clone, Copy)]
struct Config(RenderingConfig);
impl Plugin for ProjectileVisualsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Config(self.rendering))
            .init_resource::<ProjectileDetail>()
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (update_detail, sync_projectiles)
                    .chain()
                    .in_set(SceneSyncSet),
            );
    }
}

/// Build the shared sphere and material, then register their emission.
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    config: Res<Config>,
    detail: Res<ProjectileDetail>,
) {
    let lit = config.0.is_lit();
    let assets = ProjectileAssets {
        // Unit sphere; each visual scales it to the projectile radius.
        mesh: meshes.add(Sphere::new(1.0).mesh().ico(detail.0.clamp(1, 4)).unwrap()),
        // Table 4-1: fluorescent yellow-green.
        material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.80, 1.0, 0.35),
            emissive: if lit {
                LinearRgba::rgb(0.5, 1.0, 0.15) * (config.0.emissive_strength * 0.01)
            } else {
                LinearRgba::BLACK
            },
            emissive_exposure_weight: 1.0,
            perceptual_roughness: 0.6,
            unlit: !lit,
            ..default()
        }),
    };
    crate::quality::register_emission(
        &mut commands,
        &materials,
        &assets.material,
        config.0.emissive_strength,
    );
    commands.insert_resource(assets);
}

/// Rebuild the shared sphere when the detail setting changes.
fn update_detail(
    detail: Res<ProjectileDetail>,
    assets: Res<ProjectileAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if detail.is_changed() {
        let mesh = Sphere::new(1.0).mesh().ico(detail.0.clamp(1, 4)).unwrap();
        let _ = meshes.insert(assets.mesh.id(), mesh);
    }
}

/// Fill the pool from the caller's projectiles, spawning any beyond its size and
/// hiding the rest.
fn sync_projectiles(
    input: Res<SceneInput>,
    assets: Res<ProjectileAssets>,
    mut commands: Commands,
    mut pool: Query<(Entity, &mut Transform, &mut Visibility), With<ProjectileVisual>>,
) {
    let Some(scene) = &input.0 else { return };
    let mut wanted = scene.projectiles.iter();
    for (_, mut transform, mut visibility) in &mut pool {
        match wanted.next() {
            Some(projectile) => {
                *transform = Transform::from_translation(flu_position(projectile.position_m))
                    .with_scale(Vec3::splat(projectile.radius_m.max(0.001)));
                *visibility = Visibility::Inherited;
            }
            None => *visibility = Visibility::Hidden,
        }
    }
    for projectile in wanted {
        commands.spawn((
            ProjectileVisual,
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(assets.material.clone()),
            Transform::from_translation(flu_position(projectile.position_m))
                .with_scale(Vec3::splat(projectile.radius_m.max(0.001))),
            Visibility::Inherited,
        ));
    }
}
