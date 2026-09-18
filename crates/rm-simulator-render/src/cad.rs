// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Extracted RMUC CAD scenery. Static instances are rigid; rune faces and outpost
//! rotors follow the caller's scene state. Loading is asynchronous and reported
//! through `CadSceneStatus`. No rules, collision or scoring live here.
use crate::sync::{SceneInput, SceneSyncSet};
use bevy::{
    asset::{LoadState, RecursiveDependencyLoadState},
    log::warn_once,
    prelude::*,
    world_serialization::WorldInstanceReady,
};
use std::collections::HashMap;

/// One rune face node inside an imported rune asset.
#[derive(Clone, Debug, PartialEq)]
pub struct CadRuneFace {
    /// glTF node name, for example `face_1`.
    pub node: String,
    /// Index into `SceneState::runes`.
    pub rune: u32,
    /// +1 when the node's local +Z is the face normal, -1 for the rear face.
    pub spin: f32,
    /// Owning team, supplied by the application.
    pub color: crate::TeamColor,
}
/// What one imported glTF scene contributes, and how its nodes are treated.
#[derive(Clone, Debug, PartialEq)]
pub enum CadRole {
    /// Bind exported joint motion nodes by their stable extras.rm IDs.
    Semantic {
        /// glTF scene index inside the asset.
        scene: usize,
        /// Exported joints to bind, by `extras.rm.id`.
        joints: Vec<CadJoint>,
        /// Node ids whose imported meshes the caller's overlays replace.
        hidden_ids: Vec<String>,
        /// Preserve rune optics when joint binding replaces the legacy motion path.
        rune_faces: Vec<CadRuneFace>,
    },
    /// Rigid scenery. An override recolours the materials the CAD left
    /// unpainted (the converter's near-white cream, see [`is_unpainted`])
    /// and leaves painted ones alone.
    Static {
        /// Replacement colour for the exporter's unpainted cream, or none.
        base_color: Option<Color>,
    },
    /// A legacy rune asset whose named face nodes rotate with the scene state.
    Rune {
        /// One entry per imported face node.
        faces: Vec<CadRuneFace>,
    },
    /// The `rotor` node follows `SceneState::outposts[outpost].angle_rad`;
    /// imported `rotor/armor_*` meshes are hidden so caller-owned optics replace them.
    Outpost {
        /// Index into `SceneState::outposts`.
        outpost: u32,
    },
}
/// A caller-owned link from rule state to an exported joint coordinate.
#[derive(Clone, Debug, PartialEq)]
pub enum JointDriver {
    /// Normalized position along the base dart target rail.
    DartTarget,
    /// Base opening endpoints for one team; 0 is red and 1 is blue.
    BaseOpening {
        /// Team slot whose gate this joint opens.
        team: usize,
    },
    /// Dart door endpoints for one team; 0 is red and 1 is blue.
    DartDoor {
        /// Team slot whose door this joint opens.
        team: usize,
    },
    /// Rune rotation for one rune.
    Rune {
        /// Index into `SceneState::runes`.
        index: u32,
        /// Multiplier applied to the rune angle, normally 1 or -1.
        sign: f32,
    },
    /// Outpost rotor angle for one outpost.
    Outpost {
        /// Index into `SceneState::outposts`.
        index: u32,
    },
}
/// One exported joint coordinate that the caller can drive.
#[derive(Clone, Debug, PartialEq)]
pub struct CadJoint {
    /// `extras.rm.id` of the imported node that carries the joint.
    pub id: String,
    /// Local axis in the node's own frame.
    pub axis: Vec3,
    /// True slides along `axis` in metres, false rotates about it in radians.
    pub prismatic: bool,
    /// Optional travel limits as `[min, max]` in metres or radians.
    pub limits: Option<[f32; 2]>,
    /// Caller-owned value that drives the joint; none holds it at zero.
    pub driver: Option<JointDriver>,
}
/// An imported joint node and the transform it rests at.
#[derive(Component)]
struct JointRest {
    binding: CadJoint,
    rest: Transform,
}

/// One CAD file to load, where to place it and what it contributes.
#[derive(Clone, Debug, PartialEq)]
pub struct CadInstance {
    /// Team paint for duplicated equipment; arena scenery keeps its CAD colours.
    pub team: Option<crate::TeamColor>,
    /// Path relative to the asset root, for example `rune.glb`.
    pub file: String,
    /// Placement in Bevy space, applied to the scene root.
    pub transform: Transform,
    /// What the scene contributes and how its nodes are treated.
    pub role: CadRole,
}

/// Loads and instances the CAD scenery, at most one asset per frame.
pub struct CadScenePlugin {
    /// Assets to load, in spawn order; indices must stay stable.
    pub instances: Vec<CadInstance>,
    /// Appearance applied to scenery and rune emblem materials.
    pub rendering: crate::RenderingConfig,
}
/// Initial scene instances, or instances supplied before loading begins.
/// Once spawning starts, callers must preserve existing instance indices.
#[derive(Resource)]
pub struct CadSceneConfig {
    /// Assets to load, in spawn order.
    pub instances: Vec<CadInstance>,
    /// Appearance applied to scenery and rune emblem materials.
    pub rendering: crate::RenderingConfig,
}
/// Asynchronous load progress, polled by the application.
#[derive(Resource)]
pub struct CadSceneStatus {
    /// Instances the plugin was configured with.
    pub expected: usize,
    /// Instances that have reported a ready event so far.
    pub loaded: usize,
    /// First fatal error; a failure stops further spawning.
    pub failed: Option<String>,
}
impl CadSceneStatus {
    /// Monotonic: a duplicate ready event must not latch loading off forever.
    pub fn ready(&self) -> bool {
        self.loaded >= self.expected
    }
}
/// A mesh entity loaded from field CAD, excluding procedural simulator visuals.
#[derive(Component)]
pub struct CadMesh;

/// Index of the spawned instance in the scene configuration.
#[derive(Component)]
struct CadRoot(usize);

/// Whether the CAD scenery draws at all; hide it to look at overlays alone.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneryVisibility(pub bool);
/// A rune face node that turns about its local +Z by `spin` times the rune angle.
#[derive(Component)]
struct FaceRest {
    rune: u32,
    spin: f32,
    rest: Transform,
}
/// Cap primitives inherit the rotating face through the identity hub node and
/// counter-rotate so the centre emblem stays upright.
#[derive(Component)]
struct CapRest {
    rune: u32,
    spin: f32,
    rest: Transform,
}
/// An outpost rotor node that turns about its local +Y by the caller's angle.
#[derive(Component)]
struct RotorRest {
    outpost: u32,
    rest: Transform,
}

impl Plugin for CadScenePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CadSceneConfig {
            instances: self.instances.clone(),
            rendering: self.rendering,
        })
        .insert_resource(CadSceneStatus {
            expected: self.instances.len(),
            loaded: 0,
            failed: None,
        })
        .insert_resource(SceneryVisibility(true))
        .add_systems(Update, spawn)
        .add_observer(loaded)
        .add_systems(
            Update,
            (
                check_loading,
                sync_cad.after(SceneSyncSet),
                sync_joints.after(SceneSyncSet),
                apply_scenery_visibility.run_if(resource_changed::<SceneryVisibility>),
            ),
        );
    }
}

/// Show or hide every imported CAD root.
fn apply_scenery_visibility(
    shown: Res<SceneryVisibility>,
    mut roots: Query<&mut Visibility, With<CadRoot>>,
) {
    for mut visibility in &mut roots {
        visibility.set_if_neq(if shown.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

/// Spawn at most one instance per frame to spread scene preparation out.
fn spawn(
    mut commands: Commands,
    server: Res<AssetServer>,
    config: Res<CadSceneConfig>,
    mut status: ResMut<CadSceneStatus>,
    mut next: Local<usize>,
) {
    status.expected = config.instances.len();
    if let Some(instance) = config.instances.get(*next) {
        let scene = match instance.role {
            CadRole::Semantic { scene, .. } => scene,
            _ => 0,
        };
        commands.spawn((
            WorldAssetRoot(
                server.load(GltfAssetLabel::Scene(scene).from_asset(instance.file.clone())),
            ),
            instance.transform,
            Visibility::default(),
            CadRoot(*next),
        ));
        *next += 1;
    }
}

/// Extracted hub caps: 53 mm radius, |z| = 124.8..172.3 mm in hub coordinates.
/// Require the whole primitive inside that envelope so the large hub housing keeps rotating.
fn is_rune_center_cap(mesh: &Mesh) -> bool {
    let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return false;
    };
    const CAP_RADIUS_M: f32 = 0.054;
    !positions.is_empty()
        && positions.iter().all(|[x, y, z]| {
            x * x + y * y <= CAP_RADIUS_M * CAP_RADIUS_M && (0.124..=0.173).contains(&z.abs())
        })
}
/// The flat emblem on the cap face: radius under 52 mm on the outer plane.
fn is_rune_center_emblem(mesh: &Mesh) -> bool {
    let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return false;
    };
    !positions.is_empty()
        && positions
            .iter()
            .all(|[x, y, z]| x * x + y * y <= 0.052 * 0.052 && (0.171..=0.173).contains(&z.abs()))
}

/// Index a ready instance: tag meshes, bind joints and swap scenery materials.
#[allow(clippy::too_many_arguments)]
fn loaded(
    event: On<WorldInstanceReady>,
    roots: Query<&CadRoot>,
    children: Query<&Children>,
    handles: Query<&MeshMaterial3d<StandardMaterial>>,
    mesh_handles: Query<&Mesh3d>,
    meshes: Res<Assets<Mesh>>,
    names: Query<(&Name, &Transform)>,
    semantics: Query<(&bevy::gltf::GltfExtras, &Transform)>,
    config: Res<CadSceneConfig>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    mut status: ResMut<CadSceneStatus>,
) {
    let Ok(root) = roots.get(event.entity) else {
        return;
    };
    for child in children.iter_descendants(event.entity) {
        if mesh_handles.contains(child) {
            commands.entity(child).insert(CadMesh);
        }
    }
    let instance = &config.instances[root.0];
    if let CadRole::Semantic {
        joints,
        hidden_ids,
        rune_faces,
        ..
    } = &instance.role
    {
        let mut bound = std::collections::HashSet::new();
        for child in children.iter_descendants(event.entity) {
            let Ok((extras, transform)) = semantics.get(child) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&extras.value) else {
                continue;
            };
            let Some(id) = value["rm"]["id"].as_str() else {
                continue;
            };
            if let Some(joint) = joints.iter().find(|j| j.id == id) {
                if !bound.insert(id.to_owned()) {
                    status.failed = Some(format!("{}: duplicate joint ID {id}", instance.file));
                    return;
                }
                commands.entity(child).insert(JointRest {
                    binding: joint.clone(),
                    rest: *transform,
                });
            }
            if hidden_ids.iter().any(|hidden| hidden == id) {
                commands.entity(child).insert(Visibility::Hidden);
            }
        }
        if bound.len() != joints.len() {
            status.failed = Some(format!("{}: missing exported joint nodes", instance.file));
            return;
        }
        if rune_faces.is_empty() {
            status.loaded += 1;
            info!(
                file = instance.file,
                joints = bound.len(),
                "semantic CAD instance ready"
            );
            return;
        }
    }
    let faces = match &instance.role {
        CadRole::Rune { faces } => faces.as_slice(),
        CadRole::Semantic { rune_faces, .. } => rune_faces.as_slice(),
        _ => &[],
    };
    let emblems_by_face: Vec<_> = faces
        .iter()
        .map(|face| {
            let (base_color, emission) = face.color.light();
            materials.add(StandardMaterial {
                base_color,
                emissive: if config.rendering.is_lit() {
                    emission * config.rendering.emissive_strength
                } else {
                    LinearRgba::BLACK
                },
                unlit: !config.rendering.is_lit(),
                emissive_exposure_weight: 1.0,
                ..default()
            })
        })
        .collect();
    for emblem in &emblems_by_face {
        crate::quality::register_emission(
            &mut commands,
            &materials,
            emblem,
            config.rendering.emissive_strength,
        );
    }
    let mut cache = HashMap::new();
    let mut emblems = Vec::new();
    for child in children.iter_descendants(event.entity) {
        if let Ok((name, transform)) = names.get(child) {
            match &instance.role {
                CadRole::Static { .. } | CadRole::Semantic { .. } => {}
                CadRole::Rune { faces } => {
                    for (face, emblem) in faces.iter().zip(&emblems_by_face) {
                        if name.as_str() == face.node {
                            commands.entity(child).insert(FaceRest {
                                rune: face.rune,
                                spin: face.spin,
                                rest: *transform,
                            });
                        }
                        if name.as_str() == format!("{}/hub", face.node) {
                            for primitive in children.iter_descendants(child) {
                                let Ok(handle) = mesh_handles.get(primitive) else {
                                    continue;
                                };
                                let Some(mesh) = meshes.get(handle.id()) else {
                                    continue;
                                };
                                if is_rune_center_cap(mesh)
                                    && let Ok((_, rest)) = names.get(primitive)
                                {
                                    commands.entity(primitive).insert(CapRest {
                                        rune: face.rune,
                                        spin: face.spin,
                                        rest: *rest,
                                    });
                                }
                                if is_rune_center_emblem(mesh) {
                                    emblems.push((primitive, face.clone(), emblem.clone()));
                                }
                            }
                        }
                    }
                }
                CadRole::Outpost { outpost } => {
                    if name.as_str() == "rotor" {
                        commands.entity(child).insert(RotorRest {
                            outpost: *outpost,
                            rest: *transform,
                        });
                    }
                    if name.as_str().starts_with("rotor/armor_") {
                        commands.entity(child).insert(Visibility::Hidden);
                    }
                }
            }
        }
        // Fixed semantic logos retain hub-local geometry, but are no longer
        // descendants of the spinning face. Recolour only the emblem primitive.
        if let Ok((name, _)) = names.get(child) {
            for (face, emblem) in faces.iter().zip(&emblems_by_face) {
                if name.as_str() == format!("rune/fixed/face_{}/logo", face.rune) {
                    for primitive in children.iter_descendants(child) {
                        if mesh_handles
                            .get(primitive)
                            .ok()
                            .and_then(|h| meshes.get(h.id()))
                            .is_some_and(is_rune_center_emblem)
                        {
                            emblems.push((primitive, face.clone(), emblem.clone()));
                        }
                    }
                }
            }
        }
        if let Ok(handle) = handles.get(child) {
            let replacement = cache.entry(handle.id()).or_insert_with(|| {
                let original = materials.get(handle.id()).expect("loaded CAD material");
                let material =
                    scenery_material(original, &instance.role, !faces.is_empty(), instance.team);
                materials.add(material)
            });
            commands
                .entity(child)
                .insert(MeshMaterial3d(replacement.clone()));
        }
    }
    for (primitive, face, emblem) in emblems {
        if let Ok((_, original)) = names.get(primitive) {
            let mut rest = *original;
            // The imported R sits 0.1 mm behind its backing disk. Lift the
            // artwork just past the disk, toward this face's viewer.
            rest.translation.z += face.spin * 0.0003;
            commands.entity(primitive).insert(rest);
            if matches!(instance.role, CadRole::Rune { .. }) {
                commands.entity(primitive).insert(CapRest {
                    rune: face.rune,
                    spin: face.spin,
                    rest,
                });
            }
        }
        commands
            .entity(primitive)
            .insert(MeshMaterial3d(emblem.clone()));
    }
    status.loaded += 1;
    info!(
        file = instance.file,
        loaded = status.loaded,
        expected = status.expected,
        "CAD scenery instance ready"
    );
}

/// Recolour imported scenery for team paint, unpainted-cream overrides and rune
/// faces.
// Embedded artwork carries its colour, alpha mask and PBR settings in glTF.
// Legacy colour-only scenery keeps the existing CAD presentation overrides.
fn scenery_material(
    original: &StandardMaterial,
    role: &CadRole,
    has_rune_faces: bool,
    team: Option<crate::TeamColor>,
) -> StandardMaterial {
    let mut material = original.clone();
    if material.base_color_texture.is_some() {
        return material;
    }
    if let Some(team) = team {
        let c = material.base_color.to_srgba();
        if (c.red > c.green * 1.6 && c.red > c.blue * 1.6 && c.red > 0.3)
            || (c.blue > c.red * 1.6 && c.blue > c.green * 1.3 && c.blue > 0.3)
        {
            material.base_color = match team {
                crate::TeamColor::Red => Color::srgb(0.9, 0.035, 0.035),
                crate::TeamColor::Blue => Color::srgb(0.035, 0.22, 0.95),
            };
        }
    }
    match role {
        CadRole::Static {
            base_color: Some(color),
        } if is_unpainted(material.base_color) => material.base_color = *color,
        _ if has_rune_faces => {
            // Lit blade artwork is supplied by the caller's overlay; keep the
            // CAD's painted red/blue regions dark so only real lights glow.
            let c = material.base_color.to_srgba();
            let min = c.red.min(c.green).min(c.blue);
            let max = c.red.max(c.green).max(c.blue);
            if c.red.max(c.blue) - c.green > 0.15 {
                material.base_color = Color::srgb(0.06, 0.06, 0.06);
            } else if min > 0.7 && max - min < 0.15 {
                material.base_color = Color::srgb(0.22, 0.22, 0.22);
            }
        }
        _ => {}
    }
    material.metallic = 0.;
    material.perceptual_roughness = 0.8;
    material.unlit = false;
    material
}

/// Record the first asset load failure, which stops further spawning.
fn check_loading(
    mut status: ResMut<CadSceneStatus>,
    server: Res<AssetServer>,
    roots: Query<(&WorldAssetRoot, &CadRoot)>,
    config: Res<CadSceneConfig>,
) {
    if status.ready() || status.failed.is_some() {
        return;
    }
    for (root, index) in &roots {
        let failure = match server.get_load_state(root.0.id()) {
            Some(LoadState::Failed(error)) => Some(error),
            _ => match server.get_recursive_dependency_load_state(root.0.id()) {
                Some(RecursiveDependencyLoadState::Failed(error)) => Some(error),
                _ => None,
            },
        };
        if let Some(error) = failure {
            let message = format!("{} failed to load: {error}", config.instances[index.0].file);
            error!("{message}");
            status.failed = Some(message);
            return;
        }
    }
}

/// Turn rune faces, hub caps and outpost rotors from the caller's scene state.
#[allow(clippy::type_complexity)]
fn sync_cad(
    input: Res<SceneInput>,
    mut faces: Query<(&FaceRest, &mut Transform), (Without<CapRest>, Without<RotorRest>)>,
    mut caps: Query<(&CapRest, &mut Transform), (Without<FaceRest>, Without<RotorRest>)>,
    mut rotors: Query<(&RotorRest, &mut Transform), (Without<FaceRest>, Without<CapRest>)>,
) {
    let Some(scene) = &input.0 else { return };
    let rune_angle = |rune: u32| {
        let angle = scene.runes.get(rune as usize).map(|r| r.angle_rad as f32);
        if angle.is_none() {
            warn_once!("CAD rune face {rune} is absent from scene input");
        }
        angle
    };
    for (face, mut pose) in &mut faces {
        if let Some(angle) = rune_angle(face.rune) {
            *pose = face.rest;
            pose.rotate_local_z(face.spin * angle);
        }
    }
    for (cap, mut pose) in &mut caps {
        if let Some(angle) = rune_angle(cap.rune) {
            *pose = Transform::from_rotation(Quat::from_rotation_z(-cap.spin * angle))
                .mul_transform(cap.rest);
        }
    }
    for (rotor, mut pose) in &mut rotors {
        match scene.outposts.get(rotor.outpost as usize) {
            Some(outpost) => {
                *pose = rotor.rest;
                pose.rotate_local_y(outpost.angle_rad as f32);
            }
            None => warn_once!("CAD outpost {} is absent from scene input", rotor.outpost),
        }
    }
}

/// Apply one joint value to the node's rest transform, clamped to its limits.
fn joint_transform(binding: &CadJoint, rest: Transform, value: f32) -> Transform {
    let value = binding.limits.map_or(value, |[lo, hi]| value.clamp(lo, hi));
    if binding.prismatic {
        rest.mul_transform(Transform::from_translation(binding.axis * value))
    } else {
        rest.mul_transform(Transform::from_rotation(Quat::from_axis_angle(
            binding.axis,
            value,
        )))
    }
}
/// Drive every bound joint from its caller-owned value.
fn sync_joints(input: Res<SceneInput>, mut joints: Query<(&JointRest, &mut Transform)>) {
    let Some(scene) = &input.0 else {
        return;
    };
    for (joint, mut pose) in &mut joints {
        let value = match &joint.binding.driver {
            Some(JointDriver::Rune { index, sign }) => scene
                .runes
                .get(*index as usize)
                .map(|r| r.angle_rad as f32 * sign),
            Some(JointDriver::Outpost { index }) => scene
                .outposts
                .get(*index as usize)
                .map(|o| o.angle_rad as f32),
            Some(JointDriver::DartTarget) => joint
                .binding
                .limits
                .map(|[low, high]| low + (high - low) * scene.dart_target_fraction),
            Some(JointDriver::BaseOpening { team }) => joint
                .binding
                .limits
                .map(|[closed, open]| closed + (open - closed) * scene.base_open_fraction[*team]),
            Some(JointDriver::DartDoor { team }) => joint.binding.limits.map(|[closed, open]| {
                if scene.dart_door_open[*team] {
                    open
                } else {
                    closed
                }
            }),
            None => Some(0.),
        };
        if let Some(value) = value {
            pose.set_if_neq(joint_transform(&joint.binding, joint.rest, value));
        }
    }
}

/// The CAD exporter's default for faces nobody coloured: a near-white cream
/// (about `(1, 1, 0.95)` sRGB, yellow-tinted). Painted faces are darker,
/// saturated, or plain white like the field's line markings.
///
/// ```
/// use bevy::prelude::Color;
/// use rm_simulator_render::cad::is_unpainted;
///
/// // A scenery override may recolour only the exporter's cream.
/// assert!(is_unpainted(Color::srgb(1.0, 1.0, 0.949)));
/// // Field line markings are pure white and keep their colour.
/// assert!(!is_unpainted(Color::WHITE));
/// assert!(!is_unpainted(Color::srgb(1.0, 0.03, 0.03)));
/// ```
pub fn is_unpainted(color: Color) -> bool {
    let c = color.to_srgba();
    let min = c.red.min(c.green).min(c.blue);
    let max = c.red.max(c.green).max(c.blue);
    min > 0.7 && max - min < 0.15 && c.green - c.blue > 0.02
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn equipment_paint_follows_team_without_recoloring_arena_or_neutral_materials() {
        let red = StandardMaterial {
            base_color: Color::srgb(1.0, 0.0, 0.0),
            ..default()
        };
        let role = CadRole::Static { base_color: None };
        let blue = scenery_material(&red, &role, false, Some(crate::TeamColor::Blue))
            .base_color
            .to_srgba();
        assert!(blue.blue > blue.red);
        assert_eq!(
            scenery_material(&red, &role, false, None).base_color,
            red.base_color
        );
        let white = StandardMaterial {
            base_color: Color::WHITE,
            ..default()
        };
        assert_eq!(
            scenery_material(&white, &role, false, Some(crate::TeamColor::Blue)).base_color,
            Color::WHITE
        );
    }

    use crate::sync::{OutpostAppearance, RuneAppearance, SceneState};
    fn mesh(points: Vec<[f32; 3]>) -> Mesh {
        Mesh::new(bevy::mesh::PrimitiveTopology::TriangleList, default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, points)
    }
    #[test]
    fn textured_artwork_preserves_tint_alpha_and_pbr_settings() {
        let original = StandardMaterial {
            base_color_texture: Some(Handle::default()),
            base_color: Color::srgb(1.0, 1.0, 0.949),
            alpha_mode: AlphaMode::Mask(0.5),
            metallic: 0.2,
            perceptual_roughness: 0.6,
            double_sided: true,
            ..default()
        };
        let role = CadRole::Static {
            base_color: Some(Color::BLACK),
        };
        let textured = scenery_material(&original, &role, true, None);
        assert_eq!(textured.base_color_texture, original.base_color_texture);
        assert_eq!(textured.base_color, original.base_color);
        assert_eq!(textured.alpha_mode, original.alpha_mode);
        assert_eq!(textured.metallic, original.metallic);
        assert_eq!(textured.perceptual_roughness, original.perceptual_roughness);
        assert!(textured.double_sided);
        let untextured = StandardMaterial {
            base_color_texture: None,
            ..original
        };
        assert_eq!(
            scenery_material(&untextured, &role, false, None).base_color,
            Color::BLACK
        );
    }

    #[test]
    fn dart_target_joint_applies_caller_position_and_holds_without_updates() {
        let mut app = App::new();
        app.init_resource::<SceneInput>()
            .add_systems(Update, sync_joints);
        let target = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                JointRest {
                    rest: Transform::IDENTITY,
                    binding: CadJoint {
                        id: "base.dart_target.slide".into(),
                        axis: Vec3::X,
                        prismatic: true,
                        limits: Some([-0.28, 0.28]),
                        driver: Some(JointDriver::DartTarget),
                    },
                },
            ))
            .id();
        for fraction in [0., 0.5, 1., 0.5, 0.] {
            app.world_mut().resource_mut::<SceneInput>().0 = Some(SceneState {
                dart_target_fraction: fraction,
                ..Default::default()
            });
            app.update();
            let pose = *app.world().get::<Transform>(target).unwrap();
            assert!((pose.translation.x - (-0.28 + 0.56 * fraction)).abs() < 1e-6);
            app.update();
            assert_eq!(*app.world().get::<Transform>(target).unwrap(), pose);
        }
    }
    #[test]
    fn mechanism_joints_follow_team_endpoints_without_advancing_time() {
        let mut app = App::new();
        app.init_resource::<SceneInput>()
            .add_systems(Update, sync_joints);
        let gate = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                JointRest {
                    rest: Transform::IDENTITY,
                    binding: CadJoint {
                        id: "gate".into(),
                        axis: Vec3::Y,
                        prismatic: true,
                        limits: Some([-1.2, 0.]),
                        driver: Some(JointDriver::DartDoor { team: 1 }),
                    },
                },
            ))
            .id();
        let base = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                JointRest {
                    rest: Transform::IDENTITY,
                    binding: CadJoint {
                        id: "shield".into(),
                        axis: Vec3::X,
                        prismatic: true,
                        limits: Some([0., 0.17]),
                        driver: Some(JointDriver::BaseOpening { team: 0 }),
                    },
                },
            ))
            .id();
        for open in [false, true, false] {
            app.world_mut().resource_mut::<SceneInput>().0 = Some(SceneState {
                base_open_fraction: [f32::from(u8::from(open)), f32::from(u8::from(!open))],
                dart_door_open: [!open, open],
                ..Default::default()
            });
            app.update();
            assert_eq!(
                app.world().get::<Transform>(gate).unwrap().translation.y,
                if open { 0. } else { -1.2 }
            );
            assert_eq!(
                app.world().get::<Transform>(base).unwrap().translation.x,
                if open { 0.17 } else { 0. }
            );
        }
    }
    #[test]
    fn exported_joint_pivot_moves_children_and_leaves_fixed_siblings_untouched() {
        let mut app = App::new();
        app.add_plugins(bevy::transform::TransformPlugin)
            .init_resource::<SceneInput>()
            .add_systems(Update, sync_joints);
        let pivot = Transform::from_xyz(2., 3., 4.);
        let origin = app.world_mut().spawn(pivot).id();
        let binding = CadJoint {
            id: "stable.spin".into(),
            axis: Vec3::Y,
            prismatic: false,
            limits: None,
            driver: Some(JointDriver::Outpost { index: 0 }),
        };
        let motion = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                ChildOf(origin),
                JointRest {
                    binding,
                    rest: Transform::IDENTITY,
                },
            ))
            .id();
        let rotor = app
            .world_mut()
            .spawn((Transform::from_xyz(1., 0., 0.), ChildOf(motion)))
            .id();
        let fixed = app
            .world_mut()
            .spawn((Transform::from_xyz(0., 0., 0.2), ChildOf(origin)))
            .id();
        for angle in [0., 0.7, 2.4] {
            app.world_mut().resource_mut::<SceneInput>().0 = Some(SceneState {
                outposts: vec![OutpostAppearance {
                    disabled: false,
                    origin: crate::PoseFlu::default(),
                    angle_rad: angle,
                    armors: [crate::PoseFlu::default(); 3],
                    hit_flash: [false; 3],
                }],
                ..default()
            });
            app.update();
            let actual = app
                .world()
                .get::<GlobalTransform>(rotor)
                .unwrap()
                .translation();
            assert!(
                (actual - (pivot.translation + Quat::from_rotation_y(angle as f32) * Vec3::X))
                    .length()
                    < 1e-6
            );
            let fixed_pose = app
                .world()
                .get::<GlobalTransform>(fixed)
                .unwrap()
                .compute_transform();
            assert!((fixed_pose.translation - Vec3::new(2., 3., 4.2)).length() < 1e-6);
            assert!(fixed_pose.rotation.angle_between(Quat::IDENTITY) < 1e-6);
        }
        let slide = CadJoint {
            id: "gate".into(),
            axis: Vec3::new(0.6, 0., 0.8),
            prismatic: true,
            limits: Some([-1., 0.]),
            driver: None,
        };
        let pose = joint_transform(&slide, Transform::IDENTITY, -2.);
        assert!((pose.translation - Vec3::new(-0.6, 0., -0.8)).length() < 1e-6);
        assert_eq!(
            joint_transform(&slide, Transform::IDENTITY, 0.),
            Transform::IDENTITY
        );
    }

    #[test]
    fn status_is_monotonic_past_the_expected_instance_count() {
        let status = |loaded| CadSceneStatus {
            expected: 2,
            loaded,
            failed: None,
        };
        assert!(!status(1).ready());
        assert!(status(2).ready());
        assert!(status(3).ready());
    }
    #[test]
    fn center_cap_envelope_matches_both_faces_and_excludes_housing() {
        assert!(is_rune_center_cap(&mesh(vec![
            [0.053, 0., 0.1248],
            [-0.053, 0., 0.1723],
            [0., 0.053, 0.1723],
        ])));
        assert!(is_rune_center_cap(&mesh(vec![[0., 0., -0.1722]])));
        assert!(!is_rune_center_cap(&mesh(vec![
            [0., 0., 0.1723],
            [0.303, 0., 0.1723]
        ])));
        assert!(!is_rune_center_cap(&mesh(vec![[0., 0., -0.0438]])));
        assert!(!is_rune_center_cap(&mesh(vec![])));
        assert!(!is_rune_center_cap(&mesh(vec![[f32::NAN, 0., 0.17]])));
        assert!(is_rune_center_emblem(&mesh(vec![
            [0.038, 0., 0.1722],
            [0., 0., -0.1722]
        ])));
        assert!(!is_rune_center_emblem(&mesh(vec![[0.053, 0., 0.1248]])));
        // Actual R artwork reaches 50.93 mm; the black disk reaches 53 mm.
        assert!(is_rune_center_emblem(&mesh(vec![[0.05093, 0., 0.1722]])));
        assert!(!is_rune_center_emblem(&mesh(vec![[0.053, 0., 0.1723]])));
    }
    #[test]
    fn faces_caps_and_rotors_follow_scene_state_with_their_spin_sign() {
        let mut app = App::new();
        app.add_plugins(bevy::transform::TransformPlugin)
            .init_resource::<SceneInput>()
            .add_systems(Update, sync_cad);
        let base = Transform::from_xyz(0.1, 2., -5.);
        let front = app
            .world_mut()
            .spawn((
                FaceRest {
                    rune: 0,
                    spin: 1.,
                    rest: base,
                },
                base,
            ))
            .id();
        let rear = app
            .world_mut()
            .spawn((
                FaceRest {
                    rune: 1,
                    spin: -1.,
                    rest: base,
                },
                base,
            ))
            .id();
        let hub = app
            .world_mut()
            .spawn((Transform::default(), ChildOf(front)))
            .id();
        let rest = Transform::from_xyz(0., 0., 0.17);
        let cap = app
            .world_mut()
            .spawn((
                CapRest {
                    rune: 0,
                    spin: 1.,
                    rest,
                },
                rest,
                ChildOf(hub),
            ))
            .id();
        let rotor_rest = Transform::from_xyz(0., 1.14, 0.);
        let rotor = app
            .world_mut()
            .spawn((
                RotorRest {
                    outpost: 0,
                    rest: rotor_rest,
                },
                rotor_rest,
            ))
            .id();
        let appearance = |angle_rad| RuneAppearance {
            hub_pose: crate::PoseFlu::default(),
            angle_rad,
            active_blades: [false; 5],
            activated_blades: [false; 5],
            fully_activated: false,
            progress_stages: [false; 5],
            hit_flash: [false; 5],
        };
        for angle in [0., 0.7, 1.2, 3.1_f32] {
            app.world_mut().resource_mut::<SceneInput>().0 = Some(SceneState {
                runes: vec![appearance(angle as f64), appearance(angle as f64)],
                outposts: vec![OutpostAppearance {
                    disabled: false,
                    origin: crate::PoseFlu::default(),
                    angle_rad: angle as f64,
                    armors: [crate::PoseFlu::default(); 3],
                    hit_flash: [false; 3],
                }],
                ..Default::default()
            });
            app.update();
            let check = |entity, expected: Quat| {
                let actual = app.world().get::<Transform>(entity).unwrap().rotation;
                for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                    assert!((actual * axis - expected * axis).length() < 1e-6);
                }
            };
            check(front, Quat::from_rotation_z(angle));
            check(rear, Quat::from_rotation_z(-angle));
            check(rotor, Quat::from_rotation_y(angle));
            let cap_pose = app
                .world()
                .get::<GlobalTransform>(cap)
                .unwrap()
                .compute_transform();
            assert!((cap_pose.translation - base.mul_transform(rest).translation).length() < 1e-6);
            for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                assert!((cap_pose.rotation * axis - axis).length() < 1e-6);
            }
        }
    }
    #[test]
    fn unpainted_is_the_exporters_cream_only() {
        assert!(is_unpainted(Color::srgb(1.0, 1.0, 0.949)));
        assert!(is_unpainted(Color::srgb(1.0, 1.0, 0.888)));
        // Field line markings are pure white and stay white.
        assert!(!is_unpainted(Color::WHITE));
        assert!(!is_unpainted(Color::srgb(0.98, 0.98, 0.98)));
        assert!(!is_unpainted(Color::srgb(0.2, 0.2, 0.2)));
        assert!(!is_unpainted(Color::srgb(1.0, 0.03, 0.03)));
        assert!(!is_unpainted(Color::srgb(0.6, 0.6, 1.0)));
    }
}
