// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Apply caller-owned scene state to already spawned rune and outpost visuals.
//! No rule stepping, clock or input handling lives here.
use crate::{
    PoseFlu, TeamColor, apply_pose,
    outpost::{FACE_COUNT, OutpostArmor, OutpostArmorLight},
    rune::{
        RuneActivatedLight, RuneActiveLight, RuneCompletionDetail, RuneFlowArrow,
        RuneProgressLight, RuneTargetLight, RuneVisual,
    },
};
use bevy::{log::warn_once, prelude::*};

/// Authoritative source time, copied unchanged from the caller, never derived from Bevy time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceTime {
    /// Authoritative tick count from the caller.
    pub tick: u64,
    /// Authoritative time in nanoseconds from the caller.
    pub time_ns: u64,
}

/// Resolved appearance. Activation/scoring rules belong to the caller.
#[derive(Clone, Debug, PartialEq)]
pub struct RuneAppearance {
    /// Hub pose in world FLU metres, before the wheel angle is applied.
    pub hub_pose: PoseFlu,
    /// Wheel rotation about the hub's local Z, in radians.
    pub angle_rad: f64,
    /// One flag per blade, true while that blade shows an available target.
    pub active_blades: [bool; 5],
    /// One flag per blade, true once its arm is completed.
    pub activated_blades: [bool; 5],
    /// One flag per stage, 0 to 4, true while that Big Rune segment shows.
    pub progress_stages: [bool; 5],
    /// Full-completion framing, including the caller's blink mask.
    pub fully_activated: bool,
    /// Targets whose lights show the grey strike flash.
    pub hit_flash: [bool; 5],
}

/// One outpost: the tower base, its rotor angle and its three armor faces.
#[derive(Clone, Debug, PartialEq)]
pub struct OutpostAppearance {
    /// True once the outpost is destroyed; its light bars go dark.
    pub disabled: bool,
    /// Tower base pose in world FLU metres.
    pub origin: PoseFlu,
    /// Rotor angle about the tower's up axis.
    pub angle_rad: f64,
    /// Scoring face poses, one per armor face, in world FLU metres.
    pub armors: [PoseFlu; FACE_COUNT],
    /// Faces whose light bars show the grey strike flash.
    pub hit_flash: [bool; FACE_COUNT],
}

/// One live base: the poses of its seven armor plates and their light state.
#[derive(Clone, Debug, PartialEq)]
pub struct BaseAppearance {
    /// True once the base has no HP left; its light bars go dark.
    pub disabled: bool,
    /// Plate poses in world FLU metres, one per plate.
    pub plates: [PoseFlu; 7],
    /// Plates whose light bars show the grey strike flash.
    pub hit_flash: [bool; 7],
}

/// A projectile at its current displayed position.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectileAppearance {
    /// Centre position in world FLU metres.
    pub position_m: [f64; 3],
    /// Sphere radius in metres; the renderer clamps it to at least 1 mm.
    pub radius_m: f32,
}
/// One omni wheel: its hub pose in the world with the axle along local +y and
/// the spin already applied.
#[derive(Clone, Debug, PartialEq)]
pub struct WheelAppearance {
    /// Hub pose in world FLU metres; the axle is local +y and the spin is applied.
    pub pose: PoseFlu,
    /// Wheel radius in metres.
    pub radius_m: f32,
    /// Wheel width along the axle in metres.
    pub width_m: f32,
}
/// One armor module on a chassis: its scoring face in the body frame (+x
/// the outward normal, +y width, +z height) and whether it is flashing.
#[derive(Clone, Debug, PartialEq)]
pub struct ArmorAppearance {
    /// Face pose in the chassis body frame, in FLU metres.
    pub local: PoseFlu,
    /// True while this plate shows the grey strike flash.
    pub hit_flash: bool,
}
/// One chassis: body pose and size, wheels, armor, and the gimbal bolted to
/// the body: the yaw stage turns about the body's up at the pivot, the
/// turret (pitch stage) about the yaw stage's left, carrying the barrel
/// along its +x. Visuals are keyed by `id` and coloured
/// by `team`; a defeated robot's lights go dark.
#[derive(Clone, Debug, PartialEq)]
pub struct ChassisAppearance {
    /// Printed identifier on each armor plate.
    pub armor_pattern: crate::armor::ArmorPattern,
    /// Hero prototype with parallel wheel axles and mirrored 45-degree rollers.
    pub mecanum: bool,
    /// Remaining HP in [0, 1], supplied by the caller.
    pub hp_fraction: f32,
    /// Chassis id from the caller; visuals are keyed by it.
    pub id: u32,
    /// Team whose colour the lights use.
    pub team: TeamColor,
    /// Body pose in world FLU metres.
    pub pose: PoseFlu,
    /// Body box half extents in FLU metres: x depth, y width, z height.
    pub body_half_m: [f32; 3],
    /// Wheels, indexed by [`crate::chassis::ChassisWheel`].
    pub wheels: Vec<WheelAppearance>,
    /// Armor plates, indexed by [`crate::chassis::ChassisArmorLight::plate`].
    pub armor: Vec<ArmorAppearance>,
    /// Pivot pose turned with the body and the yaw, without the pitch.
    pub yaw_stage: PoseFlu,
    /// Pitch cradle pose in world FLU metres; the barrel runs along its +x.
    pub turret: PoseFlu,
    /// Turret box half extents in FLU metres: x depth, y width, z height.
    pub turret_half_m: [f32; 3],
    /// How far the pivot sits above the body's top face.
    pub pivot_above_body_m: f32,
    /// Distance from the gun pivot forward to the muzzle in metres.
    pub barrel_length_m: f32,
    /// True when the robot is out of the match; its lights go dark.
    pub defeated: bool,
}

/// A complete update for an already spawned scene. Poses use metres, FLU and normalized wxyz.
/// `runes[i]` and `outposts[i]` drive the visuals spawned with index `i`.
/// Absent entries leave visuals alone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneState {
    /// Base visuals, indexed by the base's own order.
    pub bases: Vec<BaseAppearance>,
    /// Caller-owned normalized position along the base dart target rail.
    pub dart_target_fraction: f32,
    /// Whether each team's base gate is open; 0 is red and 1 is blue.
    pub base_open: [bool; 2],
    /// Whether each team's dart door is open; 0 is red and 1 is blue.
    pub dart_door_open: [bool; 2],
    /// Source time that phases the rune flow arrows.
    pub source: SourceTime,
    /// Rune appearances, indexed by the rune's own order.
    pub runes: Vec<RuneAppearance>,
    /// Outpost appearances, indexed by the outpost's own order.
    pub outposts: Vec<OutpostAppearance>,
    /// Displayed projectiles, in draw order.
    pub projectiles: Vec<ProjectileAppearance>,
    /// Every chassis on the field; visuals for ids not listed are removed.
    pub chassis: Vec<ChassisAppearance>,
}

/// Replace before `SceneSyncSet`. None leaves the existing scene untouched.
#[derive(Resource, Default)]
pub struct SceneInput(pub Option<SceneState>);

/// Systems in this set apply scene state during `Update`, before propagation.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SceneSyncSet;

/// Synchronize poses/lights during Update, before transform propagation.
/// Requires only spawned components; it can be tested without a GPU.
pub struct SceneSyncPlugin;
impl Plugin for SceneSyncPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SceneInput>().add_systems(
            Update,
            (sync_bases, sync_outposts, sync_rune, sync_rune_effects).in_set(SceneSyncSet),
        );
    }
}

/// Pose base plates and swap their light materials from `SceneState::bases`.
fn sync_bases(
    input: Res<SceneInput>,
    mut armor: Query<(&crate::outpost::BaseArmor, &mut Transform)>,
    mut lights: Query<(
        &crate::outpost::BaseArmorLight,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
) {
    let Some(scene) = &input.0 else { return };
    for (armor, mut transform) in &mut armor {
        if let Some(base) = scene.bases.get(armor.0.outpost as usize) {
            let mut wanted = Transform::default();
            apply_pose(&mut wanted, base.plates[armor.0.face as usize]);
            wanted.rotate_local_y(std::f32::consts::PI);
            if *transform != wanted {
                *transform = wanted;
            }
        }
    }
    for (light, mut material) in &mut lights {
        let light = &light.0;
        if let Some(base) = scene.bases.get(light.outpost as usize) {
            ArmorState::resolve(base.disabled, base.hit_flash[light.face as usize]).apply(
                &mut material,
                &light.lit,
                &light.struck,
                &light.disabled,
            );
        }
    }
}

/// Pose outpost armor and swap its light materials.
fn sync_outposts(
    input: Res<SceneInput>,
    mut armor: Query<(&OutpostArmor, &mut Transform)>,
    mut lights: Query<(&OutpostArmorLight, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    let Some(scene) = &input.0 else { return };
    for (armor, mut transform) in &mut armor {
        let Some(outpost) = scene.outposts.get(armor.outpost as usize) else {
            warn_once!("outpost {} is absent from scene input", armor.outpost);
            continue;
        };
        apply_pose(&mut transform, outpost.armors[armor.face as usize]);
        transform.rotate_local_y(std::f32::consts::PI);
    }
    for (light, mut material) in &mut lights {
        if let Some(outpost) = scene.outposts.get(light.outpost as usize) {
            ArmorState::resolve(outpost.disabled, outpost.hit_flash[light.face as usize]).apply(
                &mut material,
                &light.lit,
                &light.struck,
                &light.disabled,
            );
        }
    }
}

/// Reusable armor light state. Disabled armor ignores even a retained hit flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmorState {
    /// Normal powered appearance.
    Enabled,
    /// Grey strike flash material.
    Struck,
    /// Unpowered white-plastic diffuser material.
    Disabled,
}
impl ArmorState {
    /// Resolve the caller's flags; disabled wins over a retained hit flash.
    ///
    /// ```
    /// use rm_simulator_render::sync::ArmorState;
    ///
    /// assert_eq!(ArmorState::resolve(false, false), ArmorState::Enabled);
    /// assert_eq!(ArmorState::resolve(false, true), ArmorState::Struck);
    /// // A defeated target ignores a flash that has not cleared yet.
    /// assert_eq!(ArmorState::resolve(true, true), ArmorState::Disabled);
    /// ```
    pub fn resolve(disabled: bool, hit_flash: bool) -> Self {
        if disabled {
            Self::Disabled
        } else if hit_flash {
            Self::Struck
        } else {
            Self::Enabled
        }
    }
    /// Point the material at the matching handle, only when it differs.
    pub fn apply(
        self,
        material: &mut MeshMaterial3d<StandardMaterial>,
        lit: &Handle<StandardMaterial>,
        struck: &Handle<StandardMaterial>,
        disabled: &Handle<StandardMaterial>,
    ) {
        let wanted = match self {
            Self::Enabled => lit,
            Self::Struck => struck,
            Self::Disabled => disabled,
        };
        if material.0 != *wanted {
            material.0 = wanted.clone();
        }
    }
}

/// Point a mesh at the struck or lit material, touching it only on change.
pub(crate) fn swap_material(
    material: &mut MeshMaterial3d<StandardMaterial>,
    struck: bool,
    lit_handle: &Handle<StandardMaterial>,
    struck_handle: &Handle<StandardMaterial>,
) {
    let wanted = if struck { struck_handle } else { lit_handle };
    if material.0 != *wanted {
        material.0 = wanted.clone();
    }
}

/// Pose the rune rotor and set blade, progress and target light visibility.
#[allow(clippy::type_complexity)] // Disjoint mutable visibility queries.
fn sync_rune(
    input: Res<SceneInput>,
    mut rotor: Query<(&RuneVisual, &mut Transform)>,
    mut active: Query<(&RuneActiveLight, &mut Visibility), Without<RuneActivatedLight>>,
    mut progress: Query<
        (&RuneProgressLight, &mut Visibility),
        (Without<RuneActiveLight>, Without<RuneActivatedLight>),
    >,
    mut activated: Query<(&RuneActivatedLight, &mut Visibility), Without<RuneActiveLight>>,
    mut targets: Query<(&RuneTargetLight, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    let Some(scene) = &input.0 else { return };
    let lookup = |index: u32| {
        let rune = scene.runes.get(index as usize);
        if rune.is_none() {
            warn_once!("rune {index} is absent from scene input");
        }
        rune
    };
    for (visual, mut transform) in &mut rotor {
        let Some(rune) = lookup(visual.rune) else {
            continue;
        };
        apply_pose(&mut transform, rune.hub_pose);
        transform.rotate_local_z(rune.angle_rad as f32);
    }
    let visibility = |visible| {
        if visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        }
    };
    for (light, mut state) in &mut active {
        if let Some(rune) = lookup(light.rune) {
            state.set_if_neq(visibility(rune.active_blades[light.id as usize]));
        }
    }
    for (light, mut state) in &mut progress {
        if let Some(rune) = lookup(light.rune) {
            state.set_if_neq(visibility(rune.progress_stages[light.stage as usize]));
        }
    }
    for (light, mut state) in &mut activated {
        if let Some(rune) = lookup(light.rune) {
            state.set_if_neq(visibility(rune.activated_blades[light.id as usize]));
        }
    }
    for (light, mut material) in &mut targets {
        if let Some(rune) = lookup(light.rune) {
            swap_material(
                &mut material,
                rune.hit_flash[light.id as usize],
                &light.lit,
                &light.struck,
            );
        }
    }
}

/// Show full-completion framing and advance the flow arrows from source time.
fn sync_rune_effects(
    input: Res<SceneInput>,
    mut full: Query<(&RuneCompletionDetail, &mut Visibility), Without<RuneFlowArrow>>,
    mut arrows: Query<(&RuneFlowArrow, &mut Transform), Without<RuneCompletionDetail>>,
) {
    let Some(scene) = &input.0 else { return };
    for (light, mut visibility) in &mut full {
        if let Some(rune) = scene.runes.get(light.rune as usize) {
            visibility.set_if_neq(if rune.fully_activated == light.full {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            });
        }
    }
    // Move every row toward the target, wrapping at the end of the arm without
    // blanking rows. One 36 mm row spacing per 100 ms is an app assumption.
    // Source time preserves the position under pause, step and remote snapshots.
    let phase = (scene.source.time_ns % 100_000_000) as f32 / 100_000_000.0;
    for (arrow, mut transform) in &mut arrows {
        let y = 0.159 + (arrow.row as f32 + phase) * 0.036;
        if transform.translation.y != y {
            transform.translation.y = y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(position: [f64; 3]) -> PoseFlu {
        PoseFlu {
            translation_m: position,
            ..PoseFlu::default()
        }
    }
    fn assert_rotation(actual: Quat, expected: Quat) {
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            assert!((actual * axis - expected * axis).length() < 1e-6);
        }
    }

    #[test]
    fn full_framing_and_arrow_flow_follow_source_time_and_pause() {
        let mut app = App::new();
        app.add_plugins(SceneSyncPlugin);
        let full = app
            .world_mut()
            .spawn((
                RuneCompletionDetail {
                    rune: 0,
                    full: true,
                },
                Visibility::Hidden,
            ))
            .id();
        let arrow = app
            .world_mut()
            .spawn((RuneFlowArrow { row: 0 }, Transform::default()))
            .id();
        let mut scene = SceneState {
            runes: vec![RuneAppearance {
                hub_pose: PoseFlu::default(),
                angle_rad: 0.0,
                active_blades: [false; 5],
                activated_blades: [true; 5],
                progress_stages: [false; 5],
                fully_activated: false,
                hit_flash: [false; 5],
            }],
            ..default()
        };
        app.world_mut().resource_mut::<SceneInput>().0 = Some(scene.clone());
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(full).unwrap(),
            Visibility::Hidden
        );
        assert_eq!(
            app.world().get::<Transform>(arrow).unwrap().translation.y,
            0.159
        );
        scene.source.time_ns = 50_000_000;
        scene.runes[0].fully_activated = true;
        app.world_mut().resource_mut::<SceneInput>().0 = Some(scene.clone());
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(full).unwrap(),
            Visibility::Inherited
        );
        assert_eq!(
            app.world().get::<Transform>(arrow).unwrap().translation.y,
            0.159 + 0.5 * 0.036
        );
        app.update();
        assert_eq!(
            app.world().get::<Transform>(arrow).unwrap().translation.y,
            0.159 + 0.5 * 0.036
        );
        scene.source.time_ns = 100_000_000;
        app.world_mut().resource_mut::<SceneInput>().0 = Some(scene.clone());
        app.update();
        assert_eq!(
            app.world().get::<Transform>(arrow).unwrap().translation.y,
            0.159
        );
        scene.runes[0].fully_activated = false;
        scene.runes[0].activated_blades = [false; 5];
        app.world_mut().resource_mut::<SceneInput>().0 = Some(scene);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(full).unwrap(),
            Visibility::Hidden
        );
    }

    #[test]
    fn absent_input_is_passive_and_outposts_follow_their_index() {
        let mut app = App::new();
        app.add_plugins(SceneSyncPlugin);
        let early = app
            .world_mut()
            .spawn((
                OutpostArmor {
                    outpost: 1,
                    face: 2,
                },
                Transform::from_xyz(1., 2., 3.),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Transform>(early).unwrap().translation,
            Vec3::new(1., 2., 3.)
        );
        let armors = [
            pose([3., 0., 1.]),
            pose([3., 0.2, 1.]),
            pose([3., -0.2, 1.]),
        ];
        app.world_mut().resource_mut::<SceneInput>().0 = Some(SceneState {
            outposts: vec![
                OutpostAppearance {
                    disabled: false,
                    origin: pose([9., 9., 9.]),
                    angle_rad: 0.,
                    armors: [pose([9., 9., 9.]); 3],
                    hit_flash: [false; 3],
                },
                OutpostAppearance {
                    disabled: false,
                    origin: pose([4., 0., 0.]),
                    angle_rad: 0.,
                    armors,
                    hit_flash: [false, false, true],
                },
            ],
            ..Default::default()
        });
        app.update();
        // A late entity receives the retained input without a new publication.
        let armor = app
            .world_mut()
            .spawn((
                OutpostArmor {
                    outpost: 1,
                    face: 2,
                },
                Transform::default(),
            ))
            .id();
        let orphan = app
            .world_mut()
            .spawn((
                OutpostArmor {
                    outpost: 7,
                    face: 0,
                },
                Transform::from_xyz(5., 5., 5.),
            ))
            .id();
        let lit = Handle::<StandardMaterial>::Uuid(
            bevy::asset::uuid::Uuid::from_u128(1),
            std::marker::PhantomData,
        );
        let struck = Handle::<StandardMaterial>::Uuid(
            bevy::asset::uuid::Uuid::from_u128(2),
            std::marker::PhantomData,
        );
        let mut bars = Vec::new();
        for face in 0..3 {
            bars.push(
                app.world_mut()
                    .spawn((
                        OutpostArmorLight {
                            outpost: 1,
                            face,
                            lit: lit.clone(),
                            struck: struck.clone(),
                            disabled: Handle::default(),
                        },
                        MeshMaterial3d(lit.clone()),
                    ))
                    .id(),
            );
        }
        for _ in 0..3 {
            app.update();
            for entity in [early, armor] {
                let transform = app.world().get::<Transform>(entity).unwrap();
                assert_eq!(transform.translation, Vec3::new(0.2, 1., -3.));
                assert_rotation(
                    transform.rotation,
                    Quat::from_rotation_y(std::f32::consts::PI),
                );
            }
            assert_eq!(
                app.world().get::<Transform>(orphan).unwrap().translation,
                Vec3::new(5., 5., 5.)
            );
            for (face, bar) in bars.iter().enumerate() {
                let material = app
                    .world()
                    .get::<MeshMaterial3d<StandardMaterial>>(*bar)
                    .unwrap();
                assert_eq!(
                    material.0,
                    if face == 2 {
                        struck.clone()
                    } else {
                        lit.clone()
                    }
                );
            }
        }
        app.world_mut()
            .resource_mut::<SceneInput>()
            .0
            .as_mut()
            .unwrap()
            .outposts[1]
            .disabled = true;
        app.update();
        for bar in &bars {
            assert_eq!(
                app.world()
                    .get::<MeshMaterial3d<StandardMaterial>>(*bar)
                    .unwrap()
                    .0,
                Handle::default()
            );
        }
        app.world_mut()
            .resource_mut::<SceneInput>()
            .0
            .as_mut()
            .unwrap()
            .outposts[1]
            .disabled = false;
        // Clearing the flash restores the lit material.
        app.world_mut()
            .resource_mut::<SceneInput>()
            .0
            .as_mut()
            .unwrap()
            .outposts[1]
            .hit_flash = [false; 3];
        app.update();
        for bar in &bars {
            let material = app
                .world()
                .get::<MeshMaterial3d<StandardMaterial>>(*bar)
                .unwrap();
            assert_eq!(material.0, lit);
        }
    }

    #[test]
    fn rune_rotor_and_lights_follow_only_the_published_appearance() {
        let mut app = App::new();
        app.add_plugins(SceneSyncPlugin);
        let rotor = app
            .world_mut()
            .spawn((
                RuneVisual { rune: 1 },
                Transform::from_scale(Vec3::splat(1.5)),
            ))
            .id();
        let other = app
            .world_mut()
            .spawn((RuneVisual { rune: 0 }, Transform::default()))
            .id();
        let orphan = app
            .world_mut()
            .spawn((RuneVisual { rune: 5 }, Transform::from_xyz(1., 1., 1.)))
            .id();
        let mut lights = Vec::new();
        for id in 0..5 {
            lights.push((
                app.world_mut()
                    .spawn((RuneActiveLight { rune: 1, id }, Visibility::Hidden))
                    .id(),
                app.world_mut()
                    .spawn((RuneActivatedLight { rune: 1, id }, Visibility::Inherited))
                    .id(),
                app.world_mut()
                    .spawn((RuneProgressLight { rune: 1, stage: id }, Visibility::Hidden))
                    .id(),
            ));
        }
        let appearance = RuneAppearance {
            hub_pose: pose([5., 0., 2.]),
            angle_rad: 0.9,
            active_blades: [true, false, true, false, false],
            activated_blades: [false, true, false, false, true],
            fully_activated: false,
            progress_stages: [true, true, true, false, false],
            hit_flash: [false, false, true, false, false],
        };
        let lit = Handle::<StandardMaterial>::Uuid(
            bevy::asset::uuid::Uuid::from_u128(1),
            std::marker::PhantomData,
        );
        let struck = Handle::<StandardMaterial>::Uuid(
            bevy::asset::uuid::Uuid::from_u128(2),
            std::marker::PhantomData,
        );
        let parts: Vec<Entity> = (0..5)
            .map(|id| {
                app.world_mut()
                    .spawn((
                        RuneTargetLight {
                            rune: 1,
                            id,
                            lit: lit.clone(),
                            struck: struck.clone(),
                        },
                        MeshMaterial3d(lit.clone()),
                    ))
                    .id()
            })
            .collect();
        app.world_mut().resource_mut::<SceneInput>().0 = Some(SceneState {
            runes: vec![
                RuneAppearance {
                    hub_pose: pose([1., 0., 0.]),
                    ..appearance.clone()
                },
                appearance.clone(),
            ],
            ..Default::default()
        });
        for _ in 0..3 {
            app.update();
            assert_eq!(
                app.world().get::<Transform>(other).unwrap().translation,
                Vec3::new(0., 0., -1.)
            );
            assert_eq!(
                app.world().get::<Transform>(orphan).unwrap().translation,
                Vec3::new(1., 1., 1.)
            );
            let transform = app.world().get::<Transform>(rotor).unwrap();
            assert_eq!(transform.translation, Vec3::new(0., 2., -5.));
            assert_eq!(transform.scale, Vec3::splat(1.5));
            assert_rotation(transform.rotation, Quat::from_rotation_z(0.9));
            for (id, &(active, activated, progress)) in lights.iter().enumerate() {
                for (entity, visible) in [
                    (active, appearance.active_blades[id]),
                    (activated, appearance.activated_blades[id]),
                    (progress, appearance.progress_stages[id]),
                ] {
                    assert_eq!(
                        *app.world().get::<Visibility>(entity).unwrap(),
                        if visible {
                            Visibility::Inherited
                        } else {
                            Visibility::Hidden
                        }
                    );
                }
            }
            for (id, part) in parts.iter().enumerate() {
                let material = app
                    .world()
                    .get::<MeshMaterial3d<StandardMaterial>>(*part)
                    .unwrap();
                assert_eq!(
                    material.0,
                    if id == 2 { struck.clone() } else { lit.clone() }
                );
            }
        }
    }
}
