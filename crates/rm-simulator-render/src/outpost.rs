// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Outpost armor optics overlaid on the imported tower. Geometry is supplied by the
//! caller's world model.
use crate::{
    RenderingConfig, TeamColor,
    armor::{
        ArmorArtwork, ArmorAtlas, ArmorPattern, DiffuserProfile, diffuser_material, diffuser_mesh,
        powered_diffuser_material,
    },
};
use bevy::prelude::*;

/// One armor face. Imported optical faces have the opposite local normal to
/// procedural armor, so synchronization flips them about local Y.
#[derive(Component)]
pub struct OutpostArmor {
    /// Index into `SceneState::outposts`.
    pub outpost: u32,
    /// Face index in the outpost's own order.
    pub face: u32,
}
/// A light bar of one face, with the material for its normal and struck states.
/// Synchronization swaps between them from the caller's hit flash flag.
#[derive(Component)]
pub struct OutpostArmorLight {
    /// Index into `SceneState::outposts`.
    pub outpost: u32,
    /// Face index in the outpost's own order.
    pub face: u32,
    /// Powered diffuser material in the outpost's team colour.
    pub lit: Handle<StandardMaterial>,
    /// Grey strike-flash material.
    pub struck: Handle<StandardMaterial>,
    /// Unpowered white-plastic diffuser material.
    pub disabled: Handle<StandardMaterial>,
}
/// One base armor face, placed from `SceneState::bases` like an outpost face.
#[derive(Component)]
pub struct BaseArmor(pub OutpostArmor);
/// One base armor light bar, driven from `SceneState::bases`.
#[derive(Component)]
pub struct BaseArmorLight(pub OutpostArmorLight);
/// Armor faces on one outpost; a base instead draws seven plates.
pub const FACE_COUNT: usize = 3;

/// Dull grey for a light bar that has just registered a strike.
pub fn struck_material(rendering: RenderingConfig) -> StandardMaterial {
    let lit = rendering.is_lit();
    StandardMaterial {
        base_color: Color::srgb(0.62, 0.63, 0.65),
        emissive: LinearRgba::BLACK,
        perceptual_roughness: 0.7,
        unlit: !lit,
        ..default()
    }
}

/// Caller-supplied geometry and appearance for one outpost or base armor face.
pub struct OutpostOptics {
    /// Housing boxes as (offset, size) in the armor's FLU frame: x outward normal.
    pub boxes: Vec<([f32; 3], [f32; 3])>,
    /// Visible panel width and height in metres.
    pub face_size_m: [f32; 2],
    /// Centre-to-centre spacing of the two light bars in metres.
    pub light_span_m: f32,
    /// Length of each light bar in metres.
    pub light_length_m: f32,
    /// Appearance used for the generated materials.
    pub rendering: RenderingConfig,
    /// Emission texture shared with the chassis armor diffusers.
    pub emission_profile: Handle<Image>,
}
/// Spawn the caller-owned optics for one outpost. The CAD rotor supplies the
/// tower; the armor faces follow `SceneState::outposts[id]`.
pub fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    id: u32,
    config: &OutpostOptics,
    atlas: &ArmorAtlas,
    color: TeamColor,
) {
    spawn_optics(commands, meshes, materials, id, config, atlas, color, false);
}
/// Spawn the caller-owned optics for one live base. The seven plates follow
/// `SceneState::bases[id]`.
#[allow(clippy::too_many_arguments)]
pub fn spawn_base(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    id: u32,
    config: &OutpostOptics,
    atlas: &ArmorAtlas,
    color: TeamColor,
) {
    spawn_optics(commands, meshes, materials, id, config, atlas, color, true);
}
/// Shared builder behind `spawn` and `spawn_base`; `base` selects seven plates.
#[allow(clippy::too_many_arguments)]
fn spawn_optics(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    id: u32,
    config: &OutpostOptics,
    atlas: &ArmorAtlas,
    color: TeamColor,
    base: bool,
) {
    let lit = config.rendering.is_lit();
    let dark = materials.add(StandardMaterial {
        base_color: Color::srgb(0.018, 0.022, 0.027),
        perceptual_roughness: 0.8,
        unlit: !lit,
        ..default()
    });
    let frame = materials.add(StandardMaterial {
        base_color: Color::srgb(0.10, 0.11, 0.12),
        perceptual_roughness: 0.8,
        unlit: !lit,
        ..default()
    });
    let light = materials.add(powered_diffuser_material(
        config.rendering,
        color,
        &DiffuserProfile(config.emission_profile.clone()),
    ));
    crate::quality::register_emission(
        commands,
        materials,
        &light,
        config.rendering.emissive_strength,
    );
    let off = materials.add(diffuser_material(config.rendering));
    let struck = materials.add(struck_material(config.rendering));
    let ink = materials.add(atlas.material(config.rendering));
    let panel = meshes.add(Rectangle::new(config.face_size_m[0], config.face_size_m[1]));
    let pattern = if base {
        ArmorPattern::BaseSmall
    } else {
        ArmorPattern::Outpost
    };
    let artwork = meshes.add(atlas.mesh(pattern, config.face_size_m[1]));
    let bar = meshes.add(diffuser_mesh(0.007, config.light_length_m, 0.003));
    let boxes: Vec<_> = config
        .boxes
        .iter()
        .map(|(offset, size)| {
            (
                meshes.add(Cuboid::new(size[1], size[2], size[0])),
                Vec3::new(-offset[1], offset[2], offset[0]),
            )
        })
        .collect();
    for face in 0..if base { 7 } else { FACE_COUNT as u32 } {
        let mut root = commands.spawn((Transform::default(), Visibility::default()));
        let armor = OutpostArmor { outpost: id, face };
        if base {
            root.insert(BaseArmor(armor));
        } else {
            root.insert(armor);
        }
        root.with_children(|p| {
            for (mesh, position) in &boxes {
                p.spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(frame.clone()),
                    Transform::from_translation(*position),
                ));
            }
            p.spawn((
                Mesh3d(panel.clone()),
                MeshMaterial3d(dark.clone()),
                Transform::default(),
            ));
            p.spawn((
                ArmorArtwork(pattern),
                Mesh3d(artwork.clone()),
                MeshMaterial3d(ink.clone()),
                Transform::from_xyz(0., 0., 0.00005),
            ));
            for side in [-1., 1.] {
                let mut light_entity = p.spawn((
                    Mesh3d(bar.clone()),
                    MeshMaterial3d(light.clone()),
                    Transform::from_xyz(side * config.light_span_m / 2., 0., 0.0015),
                ));
                let light = OutpostArmorLight {
                    outpost: id,
                    face,
                    lit: light.clone(),
                    struck: struck.clone(),
                    disabled: off.clone(),
                };
                if base {
                    light_entity.insert(BaseArmorLight(light));
                } else {
                    light_entity.insert(light);
                }
            }
        });
    }
}
