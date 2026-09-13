// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Infantry omni and Hero mecanum prototypes with approximate referee equipment.
//! Body, wheels and gimbal follow caller-owned poses. Armor flashes and LI01 HP
//! segments follow snapshots. Equipment housings are decorative; colliders remain
//! caller-owned. See `docs/robot-equipment.md` for reference dimensions and limits.
use crate::armor::{
    ArmorArtwork, ArmorAtlas, DiffuserProfile, diffuser_material, diffuser_mesh,
    powered_diffuser_material,
};
use crate::equipment::{self, Finish};
use crate::{
    RenderingConfig, TeamColor, apply_pose,
    outpost::struck_material,
    sync::{ArmorState, SceneInput, SceneSyncSet},
};
use bevy::prelude::*;

/// Resolved caller-owned pose. None leaves the transform untouched.
#[derive(Component, Default, Clone, Copy, PartialEq)]
pub struct PresentedPose {
    /// Resolved FLU pose, or none to leave the transform alone.
    pub pose: Option<crate::PoseFlu>,
    /// Resolved visibility, or none to leave it alone.
    pub visible: Option<bool>,
}

/// Resolved armor light state, or none before the first snapshot.
#[derive(Component, Default, Clone, Copy, PartialEq, Eq)]
struct ArmorVisualState(Option<ArmorState>);
/// Resolved equipment power state, or none before the first snapshot.
#[derive(Component, Default, Clone, Copy, PartialEq, Eq)]
struct EquipmentVisualState(Option<bool>);

/// Snapshot indices are rebuilt once, and never used as persistent entity identities.
#[derive(Resource, Default)]
struct ChassisIndex(std::collections::HashMap<u32, usize>);
/// Rebuild the id-to-snapshot index before the apply systems run.
fn index_chassis(input: Res<SceneInput>, mut index: ResMut<ChassisIndex>) {
    index.0.clear();
    if let Some(scene) = &input.0 {
        index
            .0
            .extend(scene.chassis.iter().enumerate().map(|(i, c)| (c.id, i)));
    }
}

/// The chassis a body, wheel, yaw stage or turret entity belongs to.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub struct ChassisId(pub u32);
/// Body root for one chassis; follows `ChassisAppearance::pose`.
#[derive(Component)]
#[require(PresentedPose)]
pub struct ChassisBody;
/// Index into `ChassisAppearance::wheels`.
#[derive(Component)]
#[require(PresentedPose)]
pub struct ChassisWheel(pub usize);
/// The turntable and fork: follows `ChassisAppearance::yaw_stage`.
#[derive(Component)]
#[require(PresentedPose)]
pub struct ChassisYawStage;
/// The pitch cradle with the barrel: follows `ChassisAppearance::turret`.
#[derive(Component)]
#[require(PresentedPose)]
pub struct ChassisTurret;
/// VT03 lens position in the pitch cradle, in Bevy metres. Shared with the
/// caller so the first-person eye sits at the visible lens.
pub fn camera_eye(turret_half_m: [f32; 3]) -> Vec3 {
    camera_mount(turret_half_m) + equipment::CAMERA_LENS_M
}
/// Centered above the barrel, just ahead of the cradle's front face.
fn camera_mount(turret_half_m: [f32; 3]) -> Vec3 {
    Vec3::new(0.0, turret_half_m[2] + 0.023, -turret_half_m[0] - 0.020)
}
/// A light bar on armor module `plate` of a chassis, with its materials
/// for the lit, struck and unpowered white-plastic states.
#[derive(Component)]
#[require(ArmorVisualState)]
pub struct ChassisArmorLight {
    /// Chassis id this light belongs to.
    pub chassis: u32,
    /// Index into `ChassisAppearance::armor`.
    pub plate: usize,
    /// Powered diffuser material in the team colour.
    pub lit: Handle<StandardMaterial>,
    /// Grey strike-flash material.
    pub struck: Handle<StandardMaterial>,
    /// Unpowered white-plastic diffuser material.
    pub dark: Handle<StandardMaterial>,
}

/// Small armor module geometry, from the caller's rules: housing half
/// extents behind the face (x depth, y width, z height), the visible panel
/// and the two light bars either side of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArmorOptics {
    /// Housing half extents in FLU metres: x depth, y width, z height.
    pub housing_half_m: [f32; 3],
    /// Visible panel width and height in metres.
    pub face_size_m: [f32; 2],
    /// Centre-to-centre spacing of the two light bars in metres.
    pub light_span_m: f32,
    /// Length of each light bar in metres.
    pub light_length_m: f32,
}
/// Rollers around each omni wheel.
const ROLLER_COUNT: usize = 10;

/// One LI01 HP segment or status window; `segment` is none on fixed indicators.
#[derive(Component)]
#[require(EquipmentVisualState)]
struct EquipmentLight {
    chassis: u32,
    segment: Option<usize>,
    lit: Handle<StandardMaterial>,
    dark: Handle<StandardMaterial>,
}
/// Shared materials for every equipment part of one chassis.
struct EquipmentMaterials {
    shell: Handle<StandardMaterial>,
    metal: Handle<StandardMaterial>,
    black: Handle<StandardMaterial>,
    lens: Handle<StandardMaterial>,
    lit: Handle<StandardMaterial>,
    dark: Handle<StandardMaterial>,
}
/// Spawn one equipment module and register its lit parts.
fn attach_equipment(
    parent: &mut ChildSpawnerCommands,
    parts: Vec<equipment::Part>,
    transform: Transform,
    chassis: u32,
    meshes: &mut Assets<Mesh>,
    materials: &EquipmentMaterials,
) {
    parent
        .spawn((transform, Visibility::Inherited))
        .with_children(|module| {
            for part in parts {
                let mat = match part.finish {
                    Finish::Shell => &materials.shell,
                    Finish::Metal => &materials.metal,
                    Finish::Black => &materials.black,
                    Finish::Lens => &materials.lens,
                    Finish::Hp(_) | Finish::Status => &materials.lit,
                };
                let mut entity = module.spawn((
                    Mesh3d(meshes.add(part.mesh)),
                    MeshMaterial3d(mat.clone()),
                    part.transform,
                ));
                if matches!(part.finish, Finish::Hp(_) | Finish::Status) {
                    entity.insert(EquipmentLight {
                        chassis,
                        segment: if let Finish::Hp(i) = part.finish {
                            Some(i)
                        } else {
                            None
                        },
                        lit: materials.lit.clone(),
                        dark: materials.dark.clone(),
                    });
                }
            }
        });
}
/// Swap equipment light materials, only for states that changed.
fn sync_equipment_lights(
    mut lights: Query<
        (
            &EquipmentLight,
            &EquipmentVisualState,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        Changed<EquipmentVisualState>,
    >,
) {
    for (light, state, mut material) in &mut lights {
        let Some(on) = state.0 else { continue };
        let target = if on { &light.lit } else { &light.dark };
        if material.0 != *target {
            material.0 = target.clone();
        }
    }
}

/// Builds, poses and lights one visual set per chassis the scene state lists.
pub struct ChassisVisualsPlugin {
    /// Appearance used for every generated chassis material.
    pub rendering: RenderingConfig,
    /// Armor module dimensions and light placement.
    pub armor: ArmorOptics,
}
/// Appearance and armor geometry captured from the plugin at build time.
#[derive(Resource, Clone, Copy)]
struct Config {
    rendering: RenderingConfig,
    armor: ArmorOptics,
}
impl Plugin for ChassisVisualsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChassisIndex>()
            .init_resource::<Assets<Image>>()
            .init_resource::<ArmorAtlas>()
            .init_resource::<DiffuserProfile>()
            .insert_resource(Config {
                rendering: self.rendering,
                armor: self.armor,
            })
            .add_systems(
                Update,
                (
                    index_chassis,
                    spawn_chassis,
                    ingest_chassis,
                    sync_chassis,
                    sync_armor_lights,
                    sync_equipment_lights,
                )
                    .chain()
                    .in_set(SceneSyncSet),
            );
    }
}

fn material(config: RenderingConfig, color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.7,
        metallic: 0.1,
        unlit: !config.is_lit(),
        ..default()
    }
}
fn metal(config: RenderingConfig, color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.35,
        metallic: 0.8,
        unlit: !config.is_lit(),
        ..default()
    }
}
/// Team accent colour for the front bar and body trim.
fn accent_color(team: TeamColor) -> Color {
    match team {
        TeamColor::Red => Color::srgb(0.85, 0.12, 0.1),
        TeamColor::Blue => Color::srgb(0.1, 0.35, 0.95),
    }
}
/// Light bar in the team colour, as on the outposts.
fn light_material(config: RenderingConfig, team: TeamColor) -> StandardMaterial {
    let lit = config.is_lit();
    let (base_color, emission) = team.light();
    StandardMaterial {
        base_color,
        emissive: if lit {
            emission * config.emissive_strength
        } else {
            LinearRgba::BLACK
        },
        emissive_exposure_weight: 1.,
        unlit: !lit,
        ..default()
    }
}

/// Bevy sizes of an FLU box: (x depth, y width, z height) to (X, Y, Z).
fn box_mesh(meshes: &mut Assets<Mesh>, half_m: [f32; 3]) -> Handle<Mesh> {
    meshes.add(Cuboid::new(
        2.0 * half_m[1],
        2.0 * half_m[2],
        2.0 * half_m[0],
    ))
}
/// Cylinder along Bevy Z (an FLU +x barrel or axle-crossing pin).
fn along_z() -> Quat {
    Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
}
/// Cylinder along Bevy X (an FLU axle).
fn along_x() -> Quat {
    Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)
}

/// Build the visuals for every chassis seen for the first time, and remove
/// those whose id has left the scene.
fn spawn_chassis(
    scene: (Res<SceneInput>, Res<ChassisIndex>),
    config: Res<Config>,
    optics: (Res<ArmorAtlas>, Res<DiffuserProfile>),
    existing: Query<(Entity, &ChassisId)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let (input, index) = scene;
    let (atlas, diffuser) = optics;
    let Some(scene) = input.0.as_ref() else {
        return;
    };
    for (entity, id) in &existing {
        if !index.0.contains_key(&id.0) {
            commands.entity(entity).despawn();
        }
    }
    let existing_ids: std::collections::HashSet<_> = existing.iter().map(|(_, id)| id.0).collect();
    let rendering = config.rendering;
    for chassis in &scene.chassis {
        if existing_ids.contains(&chassis.id) {
            continue;
        }
        let id = ChassisId(chassis.id);
        let graphite = materials.add(material(rendering, Color::srgb(0.16, 0.17, 0.19)));
        let accent = materials.add(material(rendering, accent_color(chassis.team)));
        let black = materials.add(material(rendering, Color::srgb(0.05, 0.05, 0.06)));
        let rubber = materials.add(material(rendering, Color::srgb(0.22, 0.22, 0.23)));
        let aluminium = materials.add(metal(rendering, Color::srgb(0.72, 0.73, 0.75)));
        let steel = materials.add(metal(rendering, Color::srgb(0.30, 0.31, 0.34)));
        let panel = materials.add(material(rendering, Color::srgb(0.018, 0.022, 0.027)));
        let lit = materials.add(light_material(rendering, chassis.team));
        let armor_lit = materials.add(powered_diffuser_material(
            rendering,
            chassis.team,
            &diffuser,
        ));
        crate::quality::register_emission(
            &mut commands,
            &materials,
            &lit,
            rendering.emissive_strength,
        );
        crate::quality::register_emission(
            &mut commands,
            &materials,
            &armor_lit,
            rendering.emissive_strength,
        );
        let armor_off = materials.add(diffuser_material(rendering));
        let struck = materials.add(struck_material(rendering));
        let artwork = meshes.add(atlas.mesh(chassis.armor_pattern, config.armor.face_size_m[1]));
        let ink = materials.add(atlas.material(rendering));
        let dark = materials.add(material(rendering, Color::srgb(0.12, 0.12, 0.13)));

        let equipment_materials = EquipmentMaterials {
            shell: materials.add(material(rendering, Color::srgb(0.31, 0.33, 0.36))),
            metal: aluminium.clone(),
            black: panel.clone(),
            lens: materials.add(metal(rendering, Color::srgb(0.04, 0.15, 0.22))),
            lit: lit.clone(),
            dark: dark.clone(),
        };
        let [hx, hy, hz] = chassis.body_half_m;
        let armor = config.armor;
        let housing = meshes.add(equipment::housing(
            2.0 * armor.housing_half_m[1],
            2.0 * armor.housing_half_m[2],
            2.0 * armor.housing_half_m[0],
            0.008,
        ));
        let face = meshes.add(Cuboid::new(
            armor.face_size_m[0] * 0.64,
            armor.face_size_m[1] * 0.72,
            0.004,
        ));
        let bar = meshes.add(diffuser_mesh(0.010, armor.light_length_m, 0.004));
        commands
            .spawn((
                id,
                ChassisBody,
                Mesh3d(box_mesh(&mut meshes, [hx * 0.60, hy * 0.60, hz * 0.65])),
                MeshMaterial3d(graphite.clone()),
                Transform::default(),
                Visibility::Inherited,
            ))
            .with_children(|body| {
                // A narrow perimeter frame exposes the wheel hubs. The physics body
                // remains a conservative box enclosing the wheel footprint.
                body.spawn((
                    Mesh3d(meshes.add(Cuboid::new(hy * 1.6, 0.012, hx * 1.6))),
                    MeshMaterial3d(graphite.clone()),
                    Transform::from_xyz(0.0, hz - 0.006, 0.0),
                ));
                for side in [-1.0, 1.0] {
                    body.spawn((
                        Mesh3d(meshes.add(Cuboid::new(0.018, 0.025, 2.0 * hx))),
                        MeshMaterial3d(aluminium.clone()),
                        Transform::from_xyz(side * (hy - 0.025), 0.0, 0.0),
                    ));
                    body.spawn((
                        Mesh3d(meshes.add(Cuboid::new(2.0 * hy, 0.025, 0.018))),
                        MeshMaterial3d(aluminium.clone()),
                        Transform::from_xyz(0.0, 0.0, side * (hx - 0.025)),
                    ));
                }
                // Deck rails, visible electronics enclosure and battery straps.
                for side in [-1.0, 1.0] {
                    body.spawn((
                        Mesh3d(meshes.add(Cuboid::new(0.018, 0.026, hx * 1.55))),
                        MeshMaterial3d(aluminium.clone()),
                        Transform::from_xyz(side * hy * 0.65, hz + 0.013, 0.0),
                    ));
                    body.spawn((
                        Mesh3d(meshes.add(Cuboid::new(0.032, 0.045, 0.15))),
                        MeshMaterial3d(black.clone()),
                        Transform::from_xyz(side * 0.06, hz + 0.025, hx * 0.45),
                    ));
                }
                body.spawn((
                    Mesh3d(meshes.add(Cuboid::new(0.18, 0.04, 0.11))),
                    MeshMaterial3d(graphite.clone()),
                    Transform::from_xyz(0.0, hz + 0.02, hx * 0.45),
                ));
                attach_equipment(
                    body,
                    equipment::light_bar(),
                    Transform::from_xyz(0.0, hz + 0.01, hx - 0.028)
                        .with_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
                    chassis.id,
                    &mut meshes,
                    &equipment_materials,
                );
                // 30 mm below the metal floor; detection face points at the ground.
                attach_equipment(
                    body,
                    equipment::interaction(),
                    Transform::from_xyz(0.0, -hz - 0.040, 0.0),
                    chassis.id,
                    &mut meshes,
                    &equipment_materials,
                );
                // A team-coloured bar on the front edge marks the forward direction.
                body.spawn((
                    Mesh3d(meshes.add(Cuboid::new(1.6 * hy, 0.02, 0.02))),
                    MeshMaterial3d(accent.clone()),
                    Transform::from_xyz(0.0, hz + 0.01, -hx + 0.01),
                ));
                // Corner bumpers.
                for sx in [-1.0, 1.0] {
                    for sz in [-1.0, 1.0] {
                        body.spawn((
                            Mesh3d(meshes.add(Cuboid::new(0.05, 2.0 * hz + 0.01, 0.05))),
                            MeshMaterial3d(black.clone()),
                            Transform::from_xyz(sx * (hy - 0.02), 0.0, sz * (hx - 0.02)),
                        ));
                    }
                }
                for (plate, module) in chassis.armor.iter().enumerate() {
                    let mut transform = Transform::default();
                    apply_pose(&mut transform, module.local);
                    body.spawn((transform, Visibility::Inherited))
                        .with_children(|module| {
                            // Face +x is Bevy -Z: the housing sits behind it.
                            module.spawn((
                                Mesh3d(housing.clone()),
                                MeshMaterial3d(graphite.clone()),
                                Transform::from_xyz(0.0, 0.0, armor.housing_half_m[0]),
                            ));
                            module.spawn((
                                Mesh3d(face.clone()),
                                MeshMaterial3d(panel.clone()),
                                Transform::from_xyz(0.0, 0.0, -0.002),
                            ));
                            // Rectangle fronts face +Z; armor's outward face is -Z.
                            // Rotate the entire UV-mapped plane so the glyph reads normally.
                            module.spawn((
                                ArmorArtwork(chassis.armor_pattern),
                                Mesh3d(artwork.clone()),
                                MeshMaterial3d(ink.clone()),
                                Transform::from_xyz(0.0, 0.0, -0.0045)
                                    .with_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
                            ));
                            // Raised panel bezel, exposed corner fasteners and rear brackets.
                            for x in [-0.048, 0.048] {
                                for y in [-0.046, 0.046] {
                                    module.spawn((
                                        Mesh3d(
                                            meshes.add(
                                                Cylinder::new(0.0035, 0.002)
                                                    .mesh()
                                                    .resolution(8)
                                                    .build(),
                                            ),
                                        ),
                                        MeshMaterial3d(aluminium.clone()),
                                        Transform::from_xyz(x, y, -0.003).with_rotation(along_z()),
                                    ));
                                }
                                module.spawn((
                                    Mesh3d(meshes.add(Cuboid::new(0.012, 0.11, 0.018))),
                                    MeshMaterial3d(black.clone()),
                                    Transform::from_xyz(x, -0.006, 0.025),
                                ));
                            }
                            for side in [-1.0, 1.0] {
                                module.spawn((
                                    Mesh3d(bar.clone()),
                                    MeshMaterial3d(armor_lit.clone()),
                                    ChassisArmorLight {
                                        chassis: chassis.id,
                                        plate,
                                        lit: armor_lit.clone(),
                                        struck: struck.clone(),
                                        dark: armor_off.clone(),
                                    },
                                    Transform::from_xyz(
                                        side * armor.light_span_m / 2.0,
                                        0.0,
                                        -0.005,
                                    ),
                                ));
                            }
                        });
                }
            });
        for (index, wheel) in chassis.wheels.iter().enumerate() {
            let radius = wheel.radius_m;
            let width = wheel.width_m;
            let roller_radius = (radius * 0.16).max(0.004);
            let roller_length = (width * if chassis.mecanum { 1.15 } else { 0.7 }).max(0.01);
            let roller_ring = radius - roller_radius;
            commands
                .spawn((
                    id,
                    ChassisWheel(index),
                    Transform::default(),
                    Visibility::Inherited,
                ))
                .with_children(|hub| {
                    // The hub disc, its cap and a rim ring under the rollers;
                    // all turned onto the axle (FLU +y = Bevy -X).
                    hub.spawn((
                        Mesh3d(
                            meshes.add(
                                Cylinder::new(roller_ring - roller_radius, width * 0.8)
                                    .mesh()
                                    .resolution(16)
                                    .build(),
                            ),
                        ),
                        MeshMaterial3d(black.clone()),
                        Transform::from_rotation(along_x()),
                    ));
                    hub.spawn((
                        Mesh3d(
                            meshes.add(
                                Cylinder::new(radius * 0.3, width * 1.1)
                                    .mesh()
                                    .resolution(12)
                                    .build(),
                            ),
                        ),
                        MeshMaterial3d(aluminium.clone()),
                        Transform::from_rotation(along_x()),
                    ));
                    hub.spawn((
                        Mesh3d(
                            meshes.add(
                                Torus {
                                    minor_radius: width * 0.12,
                                    major_radius: roller_ring - roller_radius * 0.5,
                                }
                                .mesh()
                                .minor_resolution(6)
                                .major_resolution(20)
                                .build(),
                            ),
                        ),
                        MeshMaterial3d(aluminium.clone()),
                        Transform::from_rotation(along_x()),
                    ));
                    // Rollers lie tangent to the rim in the wheel plane (Bevy Y-Z).
                    let roller = meshes.add(
                        Capsule3d::new(
                            roller_radius,
                            (roller_length - 2.0 * roller_radius).max(0.002),
                        )
                        .mesh()
                        .longitudes(8)
                        .latitudes(4)
                        .rings(0)
                        .build(),
                    );
                    for step in 0..ROLLER_COUNT {
                        let angle = step as f32 / ROLLER_COUNT as f32 * std::f32::consts::TAU;
                        hub.spawn((
                            Mesh3d(roller.clone()),
                            MeshMaterial3d(rubber.clone()),
                            Transform::from_xyz(
                                0.0,
                                roller_ring * angle.cos(),
                                roller_ring * angle.sin(),
                            )
                            .with_rotation(
                                Quat::from_rotation_x(angle + std::f32::consts::FRAC_PI_2)
                                    * Quat::from_rotation_z(if chassis.mecanum {
                                        // Hub order is front-left, rear-left, rear-right, front-right.
                                        if index.is_multiple_of(2) {
                                            std::f32::consts::FRAC_PI_4
                                        } else {
                                            -std::f32::consts::FRAC_PI_4
                                        }
                                    } else {
                                        0.0
                                    }),
                            ),
                        ));
                    }
                });
        }
        // Yaw stage: a turntable on the body top, a pedestal and the fork
        // whose arms hold the pitch axle either side of the cradle.
        let [tx, ty, tz] = chassis.turret_half_m;
        let drop = chassis.pivot_above_body_m.max(0.02);
        let arm_gap = ty + 0.012;
        commands
            .spawn((
                id,
                ChassisYawStage,
                Transform::default(),
                Visibility::Inherited,
            ))
            .with_children(|stage| {
                stage.spawn((
                    Mesh3d(meshes.add(Cylinder::new(tx.max(ty) * 1.6, 0.012))),
                    MeshMaterial3d(steel.clone()),
                    Transform::from_xyz(0.0, -drop + 0.006, 0.0),
                ));
                stage.spawn((
                    Mesh3d(meshes.add(Cylinder::new(tx.max(ty) * 0.7, drop - tz - 0.012))),
                    MeshMaterial3d(graphite.clone()),
                    Transform::from_xyz(0.0, (-drop + 0.012 - tz) / 2.0, 0.0),
                ));
                // Fork base plate under the cradle and its two arms.
                stage.spawn((
                    Mesh3d(meshes.add(Cuboid::new(2.0 * arm_gap + 0.02, 0.012, 2.0 * tx * 0.8))),
                    MeshMaterial3d(aluminium.clone()),
                    Transform::from_xyz(0.0, -tz - 0.006, 0.0),
                ));
                for side in [-1.0, 1.0] {
                    stage.spawn((
                        Mesh3d(meshes.add(Cuboid::new(0.01, tz + 0.02, 2.0 * tx * 0.6))),
                        MeshMaterial3d(aluminium.clone()),
                        Transform::from_xyz(side * (arm_gap + 0.005), (-tz + 0.02) / 2.0, 0.0),
                    ));
                    // Pitch motor housing on the outside of the arm.
                    stage.spawn((
                        Mesh3d(meshes.add(Cylinder::new(tz * 0.6, 0.016))),
                        MeshMaterial3d(black.clone()),
                        Transform::from_xyz(side * (arm_gap + 0.018), 0.0, 0.0)
                            .with_rotation(along_x()),
                    ));
                }
            });
        // Pitch stage: the cradle with the barrel, feeder and camera.
        let barrel = chassis.barrel_length_m.max(0.01);
        commands
            .spawn((
                id,
                ChassisTurret,
                Mesh3d(box_mesh(&mut meshes, [tx, ty, tz])),
                MeshMaterial3d(graphite.clone()),
                Transform::default(),
                Visibility::Inherited,
            ))
            .with_children(|turret| {
                // Pitch axle through the cradle.
                turret.spawn((
                    Mesh3d(meshes.add(Cylinder::new(0.008, 2.0 * arm_gap + 0.01))),
                    MeshMaterial3d(steel.clone()),
                    Transform::from_rotation(along_x()),
                ));
                // Barrel along the turret's +x (Bevy -Z) with a muzzle ring.
                turret.spawn((
                    Mesh3d(
                        meshes.add(
                            Cylinder::new(
                                if chassis.mecanum { 0.025 } else { 0.012 },
                                barrel - 0.12,
                            )
                            .mesh()
                            .resolution(16)
                            .build(),
                        ),
                    ),
                    MeshMaterial3d(black.clone()),
                    Transform::from_xyz(0.0, 0.0, -(barrel - 0.12) / 2.0).with_rotation(along_z()),
                ));
                turret.spawn((
                    Mesh3d(meshes.add(Cylinder::new(0.018, 0.02))),
                    MeshMaterial3d(steel.clone()),
                    Transform::from_xyz(0.0, 0.0, -tx - 0.01).with_rotation(along_z()),
                ));
                // Feeder drum on top, camera block ahead of it.
                turret.spawn((
                    Mesh3d(meshes.add(Cylinder::new(ty * 0.7, 0.03))),
                    MeshMaterial3d(black.clone()),
                    Transform::from_xyz(0.0, tz + 0.015, tx * 0.3),
                ));
                attach_equipment(
                    turret,
                    equipment::camera(),
                    Transform::from_translation(camera_mount(chassis.turret_half_m)),
                    chassis.id,
                    &mut meshes,
                    &equipment_materials,
                );
                attach_equipment(
                    turret,
                    equipment::speed_monitor(chassis.mecanum),
                    Transform::from_xyz(0.0, 0.0, -barrel),
                    chassis.id,
                    &mut meshes,
                    &equipment_materials,
                );
            });
    }
}

/// Resolve snapshot data once into components; application systems below never search snapshots.
#[allow(clippy::type_complexity)]
fn ingest_chassis(
    input: Res<SceneInput>,
    index: Res<ChassisIndex>,
    mut parts: Query<(
        &ChassisId,
        Option<&ChassisBody>,
        Option<&ChassisWheel>,
        Option<&ChassisYawStage>,
        Option<&ChassisTurret>,
        &mut PresentedPose,
    )>,
    mut armor: Query<(&ChassisArmorLight, &mut ArmorVisualState)>,
    mut equipment: Query<(&EquipmentLight, &mut EquipmentVisualState)>,
) {
    let Some(scene) = &input.0 else { return };
    let appearance = |id: u32| index.0.get(&id).and_then(|i| scene.chassis.get(*i));
    for (id, body, wheel, yaw, turret, mut presented) in &mut parts {
        let Some(chassis) = appearance(id.0) else {
            continue;
        };
        let pose = if body.is_some() {
            Some(chassis.pose)
        } else if let Some(wheel) = wheel {
            chassis.wheels.get(wheel.0).map(|w| w.pose)
        } else if yaw.is_some() {
            Some(chassis.yaw_stage)
        } else if turret.is_some() {
            Some(chassis.turret)
        } else {
            continue;
        };
        presented.set_if_neq(PresentedPose {
            pose,
            visible: Some(pose.is_some()),
        });
    }
    for (light, mut state) in &mut armor {
        if let Some(chassis) = appearance(light.chassis) {
            let flash = chassis.armor.get(light.plate).is_some_and(|a| a.hit_flash);
            state.set_if_neq(ArmorVisualState(Some(ArmorState::resolve(
                chassis.defeated,
                flash,
            ))));
        }
    }
    for (light, mut state) in &mut equipment {
        if let Some(chassis) = appearance(light.chassis) {
            let on = !chassis.defeated
                && light
                    .segment
                    .is_none_or(|i| chassis.hp_fraction.clamp(0.0, 1.0) * 10.0 > i as f32);
            state.set_if_neq(EquipmentVisualState(Some(on)));
        }
    }
}

/// Apply resolved poses and visibility, only where `PresentedPose` changed.
fn sync_chassis(
    mut parts: Query<
        (&PresentedPose, &mut Transform, Option<&mut Visibility>),
        Changed<PresentedPose>,
    >,
) {
    for (presented, mut transform, visibility) in &mut parts {
        if let Some(pose) = presented.pose {
            apply_pose(&mut transform, pose);
        }
        if let (Some(mut visibility), Some(shown)) = (visibility, presented.visible) {
            visibility.set_if_neq(if shown {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            });
        }
    }
}

/// Swap armor light materials, only for states that changed.
fn sync_armor_lights(
    mut lights: Query<
        (
            &ChassisArmorLight,
            &ArmorVisualState,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        Changed<ArmorVisualState>,
    >,
) {
    for (light, state, mut material) in &mut lights {
        if let Some(state) = state.0 {
            state.apply(&mut material, &light.lit, &light.struck, &light.dark);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PoseFlu,
        sync::{ArmorAppearance, ChassisAppearance, SceneState, WheelAppearance},
    };
    fn appearance(id: u32, x_m: f64) -> ChassisAppearance {
        let h = std::f64::consts::FRAC_1_SQRT_2;
        ChassisAppearance {
            armor_pattern: crate::armor::ArmorPattern::Three,
            mecanum: false,
            hp_fraction: 1.0,
            id,
            team: TeamColor::Red,
            pose: PoseFlu {
                translation_m: [x_m, 2.0, 0.15],
                rotation_wxyz: [h, 0.0, 0.0, h],
            },
            body_half_m: [0.26, 0.26, 0.05],
            wheels: vec![
                WheelAppearance {
                    pose: PoseFlu::default(),
                    radius_m: 0.07,
                    width_m: 0.04,
                },
                WheelAppearance {
                    pose: PoseFlu {
                        translation_m: [x_m + 0.2, 2.2, 0.1],
                        rotation_wxyz: [1.0, 0.0, 0.0, 0.0],
                    },
                    radius_m: 0.07,
                    width_m: 0.04,
                },
            ],
            armor: vec![
                ArmorAppearance {
                    local: PoseFlu {
                        translation_m: [0.28, 0.0, 0.015],
                        rotation_wxyz: [1.0, 0.0, 0.0, 0.0],
                    },
                    hit_flash: false,
                },
                ArmorAppearance {
                    local: PoseFlu {
                        translation_m: [0.0, 0.28, 0.015],
                        rotation_wxyz: [h, 0.0, 0.0, h],
                    },
                    hit_flash: true,
                },
            ],
            yaw_stage: PoseFlu {
                translation_m: [x_m, 2.0, 0.4],
                rotation_wxyz: [h, 0.0, 0.0, h],
            },
            turret: PoseFlu {
                translation_m: [x_m, 2.0, 0.4],
                rotation_wxyz: [1.0, 0.0, 0.0, 0.0],
            },
            turret_half_m: [0.06, 0.06, 0.06],
            pivot_above_body_m: 0.15,
            barrel_length_m: 0.25,
            defeated: false,
        }
    }
    fn optics() -> ArmorOptics {
        ArmorOptics {
            housing_half_m: [0.0095, 0.0705, 0.0675],
            face_size_m: [0.128, 0.113],
            light_span_m: 0.130,
            light_length_m: 0.056,
        }
    }
    #[test]
    fn chassis_visuals_follow_their_appearance_and_hide_spare_wheels() {
        let mut app = App::new();
        app.add_plugins(bevy::transform::TransformPlugin)
            .init_resource::<SceneInput>()
            .init_resource::<ChassisIndex>()
            .add_systems(
                Update,
                (index_chassis, ingest_chassis, sync_chassis).chain(),
            );
        let body = app
            .world_mut()
            .spawn((ChassisId(7), ChassisBody, Transform::default()))
            .id();
        let wheel = app
            .world_mut()
            .spawn((
                ChassisId(7),
                ChassisWheel(1),
                Transform::default(),
                Visibility::Inherited,
            ))
            .id();
        let spare = app
            .world_mut()
            .spawn((
                ChassisId(7),
                ChassisWheel(5),
                Transform::default(),
                Visibility::Inherited,
            ))
            .id();
        let yaw_stage = app
            .world_mut()
            .spawn((ChassisId(7), ChassisYawStage, Transform::default()))
            .id();
        let turret = app
            .world_mut()
            .spawn((ChassisId(7), ChassisTurret, Transform::default()))
            .id();
        let other = app
            .world_mut()
            .spawn((ChassisId(8), ChassisBody, Transform::default()))
            .id();
        app.world_mut().resource_mut::<SceneInput>().0 = Some(SceneState {
            chassis: vec![appearance(7, 1.0)],
            ..Default::default()
        });
        app.update();
        let world = app.world();
        let body_transform = world.get::<Transform>(body).unwrap();
        assert_eq!(body_transform.translation, Vec3::new(-2.0, 0.15, -1.0));
        // Yawed a quarter turn left: Bevy forward (-Z) becomes FLU +y (Bevy -X).
        assert!((body_transform.rotation * Vec3::NEG_Z - Vec3::NEG_X).length() < 1e-6);
        assert_eq!(
            world.get::<Transform>(wheel).unwrap().translation,
            Vec3::new(-2.2, 0.1, -1.2)
        );
        assert_eq!(*world.get::<Visibility>(spare).unwrap(), Visibility::Hidden);
        let stage = world.get::<Transform>(yaw_stage).unwrap();
        assert_eq!(stage.translation, Vec3::new(-2.0, 0.4, -1.0));
        assert!((stage.rotation * Vec3::NEG_Z - Vec3::NEG_X).length() < 1e-6);
        let turret = world.get::<Transform>(turret).unwrap();
        assert_eq!(turret.translation, Vec3::new(-2.0, 0.4, -1.0));
        assert!((turret.rotation * Vec3::NEG_Z - Vec3::NEG_Z).length() < 1e-6);
        // A chassis the scene does not list is left where it was.
        assert_eq!(
            world.get::<Transform>(other).unwrap().translation,
            Vec3::ZERO
        );
    }
    #[test]
    fn chassis_sets_are_spawned_per_id_and_removed_when_the_id_leaves() {
        let mut app = App::new();
        app.add_plugins((bevy::asset::AssetPlugin::default(),))
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_resource::<Assets<Image>>()
            .init_resource::<ArmorAtlas>()
            .init_resource::<DiffuserProfile>()
            .init_resource::<SceneInput>()
            .insert_resource(Config {
                rendering: RenderingConfig::default(),
                armor: optics(),
            })
            .init_resource::<ChassisIndex>()
            .add_systems(Update, (index_chassis, spawn_chassis).chain());
        app.world_mut().resource_mut::<SceneInput>().0 = Some(SceneState {
            chassis: vec![appearance(1, 0.0), appearance(2, 3.0)],
            ..Default::default()
        });
        app.update();
        let ids = |app: &mut App| {
            let mut ids: Vec<u32> = app
                .world_mut()
                .query_filtered::<&ChassisId, With<ChassisBody>>()
                .iter(app.world())
                .map(|id| id.0)
                .collect();
            ids.sort_unstable();
            ids
        };
        assert_eq!(ids(&mut app), vec![1, 2]);
        // A body, two wheels, a yaw stage and a turret per chassis, once
        // each, and two light bars per armor module.
        app.update();
        let parts = |app: &mut App| {
            app.world_mut()
                .query::<&ChassisId>()
                .iter(app.world())
                .count()
        };
        assert_eq!(parts(&mut app), 2 * 5);
        let lights = app
            .world_mut()
            .query::<&ChassisArmorLight>()
            .iter(app.world())
            .filter(|light| light.chassis == 1)
            .count();
        assert_eq!(lights, 4);
        app.world_mut().resource_mut::<SceneInput>().0 = Some(SceneState {
            chassis: vec![appearance(2, 3.0)],
            ..Default::default()
        });
        app.update();
        assert_eq!(ids(&mut app), vec![2]);
        assert_eq!(parts(&mut app), 5);
    }
    #[test]
    fn equipment_hp_segments_follow_snapshot_and_defeat() {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_resource::<Assets<Image>>()
            .init_resource::<ArmorAtlas>()
            .init_resource::<DiffuserProfile>()
            .add_plugins(ChassisVisualsPlugin {
                rendering: RenderingConfig::default(),
                armor: optics(),
            })
            .init_resource::<SceneInput>();
        let mut chassis = appearance(7, 0.0);
        chassis.mecanum = true;
        for (fraction, defeated, expected) in [
            (1.0, false, 10),
            (0.6, false, 6),
            (0.01, false, 1),
            (0.0, true, 0),
        ] {
            chassis.hp_fraction = fraction;
            chassis.defeated = defeated;
            app.world_mut().resource_mut::<SceneInput>().0 = Some(SceneState {
                chassis: vec![chassis.clone()],
                ..default()
            });
            app.update();
            let count = app
                .world_mut()
                .query::<(&EquipmentLight, &MeshMaterial3d<StandardMaterial>)>()
                .iter(app.world())
                .filter(|(light, material)| light.segment.is_some() && material.0 == light.lit)
                .count();
            assert_eq!(count, expected);
            if defeated {
                assert!(
                    app.world_mut()
                        .query::<(&EquipmentLight, &MeshMaterial3d<StandardMaterial>)>()
                        .iter(app.world())
                        .all(|(l, m)| m.0 == l.dark)
                );
            }
        }
        app.world_mut().resource_mut::<SceneInput>().0 = Some(SceneState::default());
        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&EquipmentLight>()
                .iter(app.world())
                .count(),
            0
        );
    }
    #[test]
    fn unchanged_appearance_does_not_dirty_transform_or_resolved_pose() {
        #[derive(Resource, Default)]
        struct Writes(usize);
        type VisualChanges = Or<(Changed<Transform>, Changed<PresentedPose>)>;
        fn count(query: Query<Entity, VisualChanges>, mut writes: ResMut<Writes>) {
            writes.0 = query.iter().count();
        }
        let mut app = App::new();
        app.init_resource::<SceneInput>()
            .init_resource::<ChassisIndex>()
            .init_resource::<Writes>()
            .add_systems(
                Update,
                (index_chassis, ingest_chassis, sync_chassis, count).chain(),
            );
        app.world_mut()
            .spawn((ChassisId(7), ChassisBody, Transform::default()));
        app.world_mut().resource_mut::<SceneInput>().0 = Some(SceneState {
            chassis: vec![appearance(7, 0.0)],
            ..default()
        });
        app.update();
        assert_eq!(app.world().resource::<Writes>().0, 1);
        app.update();
        assert_eq!(app.world().resource::<Writes>().0, 0);
        app.world_mut()
            .resource_mut::<SceneInput>()
            .0
            .as_mut()
            .unwrap()
            .chassis[0]
            .pose
            .translation_m[0] = 3.0;
        app.update();
        assert_eq!(app.world().resource::<Writes>().0, 1);
        app.world_mut().resource_mut::<SceneInput>().0 = None;
        app.update();
        assert_eq!(app.world().resource::<Writes>().0, 0);
    }

    #[test]
    fn armor_lights_flash_when_struck_and_go_dark_when_defeated() {
        let mut app = App::new();
        app.init_resource::<SceneInput>()
            .init_resource::<ChassisIndex>()
            .add_systems(
                Update,
                (index_chassis, ingest_chassis, sync_armor_lights).chain(),
            );
        let handle = |n: u128| {
            Handle::<StandardMaterial>::Uuid(
                bevy::asset::uuid::Uuid::from_u128(n),
                Default::default(),
            )
        };
        let (lit, struck, dark) = (handle(1), handle(2), handle(3));
        let spawn = |app: &mut App, plate: usize| {
            app.world_mut()
                .spawn((
                    ChassisArmorLight {
                        chassis: 7,
                        plate,
                        lit: lit.clone(),
                        struck: struck.clone(),
                        dark: dark.clone(),
                    },
                    MeshMaterial3d(lit.clone()),
                ))
                .id()
        };
        let quiet = spawn(&mut app, 0);
        let hit = spawn(&mut app, 1);
        let mut chassis = appearance(7, 0.0);
        app.world_mut().resource_mut::<SceneInput>().0 = Some(SceneState {
            chassis: vec![chassis.clone()],
            ..Default::default()
        });
        app.update();
        let material = |app: &App, entity| {
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(entity)
                .unwrap()
                .0
                .clone()
        };
        assert_eq!(material(&app, quiet), lit);
        assert_eq!(material(&app, hit), struck);
        chassis.defeated = true;
        // Defeat also overrides an overlapping strike flash.
        chassis.armor[1].hit_flash = true;
        app.world_mut().resource_mut::<SceneInput>().0 = Some(SceneState {
            chassis: vec![chassis],
            ..Default::default()
        });
        app.update();
        assert_eq!(material(&app, quiet), dark);
        assert_eq!(material(&app, hit), dark);
    }
}
