// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! World snapshots into the renderer's `SceneState`: the static CAD
//! instances, the rune and outpost light overlays, projectiles, hit
//! flashes and the chassis visuals. The renderer is passive; this is the
//! only place that decides what it shows.
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use rm_simulator_render::{
    PoseFlu, RenderingConfig, TeamColor,
    armor::{ArmorAtlas, ArmorPattern, DiffuserProfile},
    cad::{CadInstance, CadJoint, CadRole, CadRuneFace, JointDriver},
    camera_appearance,
    chassis::ArmorOptics,
    lighting, outpost, rune,
    sync::{
        ArmorAppearance, ChassisAppearance, OutpostAppearance, ProjectileAppearance,
        RuneAppearance, SceneInput, SceneState, SourceTime, WheelAppearance,
    },
};
use rm_simulator_server::{
    cad_assets::CadAssets,
    layout::{outpost_team, rune_team},
};
use rm_simulator_world::{
    ArmorHit, ArmorTarget, ChassisSnapshot, FieldSnapshot, RefereeSnapshot, RuneKind, RuneState,
    SMALL_ARMOR_HOUSING_HALF_M, Team, outpost as outpost_rules,
};
use std::collections::HashMap;

use crate::controls::{Drive, MUZZLE_FORWARD_M, PlayerCamera};
use crate::frames::{dquat, gimbal_poses, pose_flu, transform_of, wxyz};

use crate::session::{FlashSettings, Session};

/// Axis-aligned arena footprint in world FLU metres: the low corner then the
/// high corner. It bounds the floor and the arena-spanning scenery, and
/// `setup_stadium` sizes the venue from it.
#[derive(Resource)]
pub struct ArenaBounds(pub [[f64; 3]; 2]);

/// Rendering profile the camera, scenery and rule overlays are drawn with.
/// The app inserts the lit field profile at startup and keeps it across joins.
#[derive(Resource)]
pub struct Appearance(pub RenderingConfig);
/// Build the ordered CAD instances of a whole field: the floor and arena shell,
/// the single rune, every outpost, base and tech core, then each extra static
/// asset. The floor and arena shell take a grey scenery override; base and tech
/// core placements take the side's team colour.
///
/// `rune_faces` registers the rune asset's named face nodes so the rune angle
/// turns them; false leaves the CAD rune static, as `--no-rune` does.
/// `base_plates` hides the base asset's imported armor-plate and unclassified
/// meshes, so pass true when live bases draw those plates from rule state.
///
/// An asset that exports visual semantics gets a `CadRole::Semantic` role
/// instead, binding its joints to the rune angle, the outpost rotor and the
/// base gate, dart target and dart door mechanisms. Instance order is the
/// renderer's spawn order and must stay stable across joins.
pub fn cad_instances(cad: &CadAssets, rune_faces: bool, base_plates: bool) -> Vec<CadInstance> {
    let arena = transform_of(&cad.arena_pose());
    let mut instances = vec![
        CadInstance {
            team: None,
            file: cad.floor.file.clone(),
            transform: arena,
            role: CadRole::Static {
                base_color: Some(Color::srgb(0.24, 0.25, 0.27)),
            },
        },
        CadInstance {
            team: None,
            file: cad.arena_static.file.clone(),
            transform: arena,
            role: CadRole::Static {
                base_color: Some(Color::srgb(0.60, 0.61, 0.63)),
            },
        },
        CadInstance {
            team: None,
            file: cad.rune.file.clone(),
            transform: transform_of(&cad.rune.placements[0]),
            role: CadRole::Rune {
                faces: if rune_faces {
                    (0..2)
                        .map(|face| CadRuneFace {
                            node: format!("face_{face}"),
                            rune: face as u32,
                            color: team_color(rune_team(
                                &rm_simulator_server::layout::cad_rune_hub(
                                    &cad.rune.placements[0],
                                    face,
                                ),
                            )),
                            spin: if face == 0 { -1.0 } else { 1.0 },
                        })
                        .collect()
                } else {
                    Vec::new()
                },
            },
        },
    ];
    for (index, placement) in cad.outpost.placements.iter().enumerate() {
        instances.push(CadInstance {
            team: Some(team_color(outpost_team(placement))),
            file: cad.outpost.file.clone(),
            transform: transform_of(placement),
            role: CadRole::Outpost {
                outpost: index as u32,
            },
        });
    }
    for asset in [&cad.base, &cad.tech_core]
        .into_iter()
        .chain(&cad.static_assets)
    {
        for placement in &asset.placements {
            instances.push(CadInstance {
                team: (asset.file == cad.base.file || asset.file == cad.tech_core.file).then(
                    || {
                        team_color(rm_simulator_server::layout::side_team(
                            placement.translation_m[0],
                        ))
                    },
                ),
                file: asset.file.clone(),
                transform: transform_of(placement),
                role: CadRole::Static { base_color: None },
            });
        }
    }
    for instance in &mut instances {
        let asset = [
            &cad.floor,
            &cad.arena_static,
            &cad.rune,
            &cad.outpost,
            &cad.base,
            &cad.tech_core,
        ]
        .into_iter()
        .chain(&cad.static_assets)
        .find(|asset| asset.file == instance.file)
        .expect("CAD instance asset");
        if let Some(semantics) = asset.visual_semantics() {
            let joints = semantics
                .joints
                .iter()
                .map(|joint| {
                    let driver = match &instance.role {
                        CadRole::Rune { faces } => faces
                            .iter()
                            .find(|face| joint.id == format!("rune.face.{}.spin", face.rune))
                            .map(|face| JointDriver::Rune {
                                index: face.rune,
                                sign: face.spin,
                            }),
                        CadRole::Outpost { outpost } if joint.id == "outpost.spin" => {
                            Some(JointDriver::Outpost { index: *outpost })
                        }
                        _ => {
                            let team = rm_simulator_server::layout::side_team(-f64::from(
                                instance.transform.translation.z,
                            ))
                            .index();
                            rm_simulator_server::layout::mechanism_joint(joint).map(|kind| {
                                match kind {
                                    rm_simulator_world::referee::Mechanism::Base => {
                                        JointDriver::BaseOpening { team }
                                    }
                                    rm_simulator_world::referee::Mechanism::DartTarget => {
                                        JointDriver::DartTarget
                                    }
                                    rm_simulator_world::referee::Mechanism::DartDoor => {
                                        JointDriver::DartDoor { team }
                                    }
                                }
                            })
                        }
                    };
                    CadJoint {
                        id: joint.id.clone(),
                        axis: Vec3::from_array(joint.axis.map(|v| v as f32)),
                        prismatic: joint.kind == "prismatic",
                        limits: joint.limits.map(|v| v.map(|x| x as f32)),
                        driver,
                    }
                })
                .collect();
            // Optical dimensions are not exported yet. Keep the existing rule
            // overlay, replacing only explicitly classified outpost armor modules.
            let mut hidden_ids: Vec<String> = if matches!(instance.role, CadRole::Outpost { .. }) {
                semantics
                    .nodes
                    .iter()
                    .filter(|node| {
                        node.metadata["roles"]
                            .as_array()
                            .is_some_and(|roles| roles.iter().any(|role| role == "armor_module"))
                    })
                    .map(|node| node.id.clone())
                    .collect()
            } else {
                Vec::new()
            };
            if base_plates && asset.file == cad.base.file {
                hidden_ids.extend((0..3).map(|i| format!("base.reconstructed.armor.{i}")));
                hidden_ids.extend((21..=29).map(|i| format!("base.unclassified.{i}")));
            }
            let rune_faces = match &instance.role {
                CadRole::Rune { faces } => faces.clone(),
                _ => Vec::new(),
            };
            instance.role = CadRole::Semantic {
                scene: semantics.scene,
                joints,
                hidden_ids,
                rune_faces,
            };
        }
    }
    instances
}

/// Keep one camera through loading and gameplay so its render target stays stable.
pub fn setup_camera(
    mut commands: Commands,
    appearance: Res<Appearance>,
    mut images: ResMut<Assets<Image>>,
) {
    let rendering = appearance.0;
    let mut camera = commands.spawn((
        PlayerCamera,
        Camera3d::default(),
        lighting::skybox(&mut images),
        IsDefaultUiCamera,
        Projection::Perspective(PerspectiveProjection {
            fov: 70_f32.to_radians(),
            near: 0.01,
            far: 300.0,
            ..default()
        }),
        Transform::from_xyz(0.0, 1.0, 0.0),
    ));
    camera_appearance(&mut camera, rendering);
    lighting::spawn_lighting(&mut commands);
}

/// Spawn the venue stadium around the arena footprint, once. The two
/// `ArenaBounds` corners are world FLU metres and convert to Bevy's
/// right/up/back frame here, at the renderer boundary. A later join finds the
/// stadium already present and spawns nothing.
pub fn setup_stadium(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    bounds: Res<ArenaBounds>,
    stadium: Query<Entity, With<lighting::Stadium>>,
) {
    if stadium.is_empty() {
        let [low, high] = bounds.0;
        lighting::spawn_stadium(
            &mut commands,
            &mut meshes,
            &mut materials,
            rm_simulator_render::flu_position(low),
            rm_simulator_render::flu_position(high),
        );
    }
}

/// Spawn the rune and outpost overlays, the crosshair
/// and the HUD text.
pub fn setup(
    mut commands: Commands,
    atlas: Res<ArmorAtlas>,
    diffuser: Res<DiffuserProfile>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    appearance: Res<Appearance>,
    session: Res<Session>,
) {
    let rendering = appearance.0;
    let snapshot = &session.snapshot;
    for (index, rune) in snapshot.runes.iter().enumerate() {
        let team = referee_team(snapshot, |r| &r.rune_teams, index)
            .unwrap_or_else(|| rune_team(&rune.hub_pose));
        rune::spawn_rune(
            &mut commands,
            &mut meshes,
            &mut materials,
            index as u32,
            rendering,
            team_color(team),
        );
    }
    if !snapshot.outposts.is_empty() || !snapshot.bases.is_empty() {
        let optics = outpost::OutpostOptics {
            emission_profile: diffuser.0.clone(),
            boxes: outpost_rules::HOUSING_BOXES
                .iter()
                .map(|(p, s)| (p.map(|x| x as f32), s.map(|x| x as f32)))
                .collect(),
            face_size_m: [
                outpost_rules::FACE_WIDTH_M as f32,
                outpost_rules::FACE_HEIGHT_M as f32,
            ],
            light_span_m: outpost_rules::LIGHT_SPAN_M as f32,
            light_length_m: outpost_rules::LIGHT_LENGTH_M as f32,
            rendering,
        };
        for (id, base) in snapshot.bases.iter().enumerate() {
            outpost::spawn_base(
                &mut commands,
                &mut meshes,
                &mut materials,
                id as u32,
                &optics,
                &atlas,
                team_color(base.config.team),
            );
        }
        for (id, outpost) in snapshot.outposts.iter().enumerate() {
            let team = referee_team(snapshot, |r| &r.outpost_teams, id)
                .unwrap_or_else(|| outpost_team(&outpost.origin));
            outpost::spawn(
                &mut commands,
                &mut meshes,
                &mut materials,
                id as u32,
                &optics,
                &atlas,
                team_color(team),
            );
        }
    }
    crate::hud::spawn_hud(&mut commands);
}

/// Wheel visual pose: hub in the world, local +y along the axle, spun about it.
fn wheel_pose(
    chassis: &ChassisSnapshot,
    hub_m: [f64; 2],
    wheel: &rm_simulator_world::WheelSnapshot,
) -> PoseFlu {
    let axle_yaw = if chassis.config.mecanum {
        0.0
    } else {
        hub_m[1].atan2(hub_m[0]) - std::f64::consts::FRAC_PI_2
    };
    let rotation = dquat(chassis.pose.rotation_wxyz)
        * DQuat::from_rotation_z(axle_yaw)
        * DQuat::from_rotation_y(wheel.spin_rad);
    // The ray starts at a suspension anchor. Draw a grounded wheel one radius
    // above its contact instead of leaving it hovering at the unloaded anchor.
    let centre = wheel.contact.map_or(wheel.hub_m, |contact| {
        (DVec3::from_array(contact.point_m)
            + dquat(chassis.pose.rotation_wxyz) * DVec3::Z * chassis.config.wheel_radius_m)
            .to_array()
    });
    PoseFlu {
        translation_m: centre,
        rotation_wxyz: wxyz(rotation.normalize()),
    }
}
/// The small armor module geometry the renderer draws on every chassis,
/// from the rules: the same module as the outpost's middle armor.
pub fn armor_optics() -> ArmorOptics {
    ArmorOptics {
        housing_half_m: SMALL_ARMOR_HOUSING_HALF_M.map(|v| v as f32),
        face_size_m: [
            outpost_rules::FACE_WIDTH_M as f32,
            outpost_rules::FACE_HEIGHT_M as f32,
        ],
        light_span_m: outpost_rules::LIGHT_SPAN_M as f32,
        light_length_m: outpost_rules::LIGHT_LENGTH_M as f32,
    }
}
/// The number painted on a chassis: the pilot's robot when the roster names
/// it, else 1 for a mecanum Hero and 3 for any other (a bot, or a chassis the
/// roster has not described yet).
fn armor_pattern(
    chassis: &ChassisSnapshot,
    robot: Option<rm_simulator_server::protocol::Robot>,
) -> ArmorPattern {
    use rm_simulator_server::protocol::Robot;
    match robot {
        Some(Robot::Hero) => ArmorPattern::One,
        Some(Robot::Infantry3) => ArmorPattern::Three,
        Some(Robot::Infantry4) => ArmorPattern::Four,
        None if chassis.config.mecanum => ArmorPattern::One,
        None => ArmorPattern::Three,
    }
}
/// One chassis for the renderer, using the actual host or predicted motor pose. `flash`
/// lists the armor plates flashing after a strike; `robot` is what the roster
/// says its pilot drives.
fn chassis_appearance(
    chassis: &ChassisSnapshot,
    flash: &[bool],
    robot: Option<rm_simulator_server::protocol::Robot>,
) -> ChassisAppearance {
    let config = &chassis.config;
    let aim = pose_flu(chassis.turret);
    let (yaw_stage, turret) = gimbal_poses(pose_flu(chassis.pose), aim);
    ChassisAppearance {
        armor_pattern: armor_pattern(chassis, robot),
        mecanum: config.mecanum,
        hp_fraction: if chassis.defeated { 0.0 } else { 1.0 },
        id: chassis.id,
        team: team_color(chassis.team),
        pose: pose_flu(chassis.pose),
        body_half_m: config.body_half_m.map(|v| v as f32),
        wheels: chassis
            .wheels
            .iter()
            .zip(&config.wheel_hubs_m)
            .map(|(wheel, hub)| WheelAppearance {
                pose: wheel_pose(chassis, *hub, wheel),
                radius_m: config.wheel_radius_m as f32,
                width_m: config.wheel_width_m as f32,
            })
            .collect(),
        armor: config
            .armor_faces()
            .iter()
            .enumerate()
            .map(|(plate, face)| ArmorAppearance {
                local: pose_flu(*face),
                hit_flash: flash.get(plate).copied().unwrap_or(false),
            })
            .collect(),
        yaw_stage,
        turret,
        turret_half_m: config.turret_half_m.map(|v| v as f32),
        pivot_above_body_m: (config.turret_center_m[2] - config.body_half_m[2]) as f32,
        barrel_length_m: MUZZLE_FORWARD_M,
        defeated: chassis.defeated,
    }
}

/// Fill the renderer's `SceneInput` from the session once a frame; the passive
/// renderer applies that caller-owned state unchanged.
///
/// The source is the session's visual snapshot, which already merges locally
/// predicted projectiles. While the session is live and unpaused, rune blade,
/// outpost, base plate and dart target poses are evaluated at the time a shot
/// fired now would use, so the drawn scene matches the aim. A pending
/// screenshot or a pause uses the snapshot's own time instead.
///
/// The driven chassis uses its predicted pose while live; every other chassis
/// is sampled from the remote pose history at `remote_view_time_ns`. A pending
/// screenshot or a pause falls every chassis back to the snapshot. An HP
/// fraction from the referee's robot record overrides the default full bar, and
/// detected armor hits keep the grey strike flash for `hit_flash_ns`, an app
/// setting rather than a rule constant.
pub fn publish_scene(
    mut session: ResMut<Session>,
    drive: Option<Res<Drive>>,
    mut input: ResMut<SceneInput>,
    capture: Option<Res<crate::screenshot::ScreenshotRequest>>,
) {
    if capture.is_none() {
        let now = session.time.now();
        let duration_ns = session.flash.hit_flash_ns;
        session.hit_feedback.present(now, duration_ns);
    }
    let visual = session.visual_snapshot();
    let snapshot = &visual;
    let live_presentation = capture.is_none();
    // Runes, outposts and the dart target move as functions of time, so they
    // are drawn where they will be when a shot taken now leaves the gun.
    let time_ns = if live_presentation && !session.paused {
        session.fire_time_ns()
    } else {
        snapshot.time_ns
    };
    let hits: Vec<_> = if live_presentation {
        session
            .hit_feedback
            .visible(session.time.now(), session.flash.hit_flash_ns)
            .collect()
    } else {
        flashing(snapshot, session.flash.hit_flash_ns).collect()
    };
    let mut scene = scene_state_with_hits(snapshot, session.flash, time_ns, &hits);
    let own = drive.map(|drive| drive.chassis_id);
    let mut armor_flash: HashMap<u32, [bool; rm_simulator_world::chassis::ARMOR_COUNT]> =
        HashMap::new();
    for hit in &hits {
        if let ArmorTarget::Chassis { chassis, plate } = hit.target
            && let Some(flag) = armor_flash
                .entry(chassis)
                .or_default()
                .get_mut(plate as usize)
        {
            *flag = true;
        }
    }
    scene.chassis = snapshot
        .chassis
        .iter()
        .map(|chassis| {
            let mine = Some(chassis.id) == own;
            let flash = armor_flash.get(&chassis.id).map_or(&[][..], |f| &f[..]);
            let interpolated = if !mine && live_presentation && !session.paused {
                session
                    .remote_history
                    .sample(chassis.id, session.remote_view_time_ns)
            } else {
                None
            };
            let robot = session
                .roster
                .iter()
                .find(|p| p.chassis == Some(chassis.id))
                .and_then(|p| p.robot);
            let mut appearance = chassis_appearance(
                if mine && live_presentation {
                    session.presented_chassis().unwrap_or(chassis)
                } else {
                    interpolated.as_ref().unwrap_or(chassis)
                },
                flash,
                robot,
            );
            if let Some(robot) = snapshot
                .referee
                .as_ref()
                .and_then(|r| r.robots.iter().find(|r| r.id == chassis.id))
            {
                appearance.hp_fraction = robot.hp as f32 / robot.max_hp.max(1) as f32;
            }
            appearance
        })
        .collect();
    input.0 = Some(scene);
}

/// Detected strikes still within the flash window.
fn flashing(snapshot: &FieldSnapshot, hit_flash_ns: u64) -> impl Iterator<Item = &ArmorHit> {
    snapshot.hits.iter().filter(move |hit| {
        hit.detected && snapshot.time_ns.saturating_sub(hit.time_ns) < hit_flash_ns
    })
}

/// Blink an activated rune: `flashes` on/off cycles at `flash_hz`, then lit.
/// A zero rate never blinks; a zero count blinks until the state changes.
fn rune_lit(since_ns: u64, now_ns: u64, flash_hz: f64, flashes: u32) -> bool {
    if flash_hz <= 0.0 {
        return true;
    }
    let phase = now_ns.saturating_sub(since_ns) as f64 * 1e-9 * flash_hz;
    if flashes > 0 && phase >= f64::from(flashes) {
        return true;
    }
    phase.fract() < 0.5
}

/// Owner of field object `index` according to the referee, if there is one.
fn referee_team(
    snapshot: &FieldSnapshot,
    owners: impl Fn(&RefereeSnapshot) -> &Vec<Team>,
    index: usize,
) -> Option<Team> {
    snapshot
        .referee
        .as_ref()
        .and_then(|referee| owners(referee).get(index).copied())
}

fn team_color(team: Team) -> TeamColor {
    match team {
        Team::Red => TeamColor::Red,
        Team::Blue => TeamColor::Blue,
    }
}

#[cfg(test)]
fn scene_state(snapshot: &FieldSnapshot, flash: FlashSettings) -> SceneState {
    scene_state_at(snapshot, flash, snapshot.time_ns)
}

#[cfg(test)]
fn scene_state_at(
    snapshot: &FieldSnapshot,
    flash: FlashSettings,
    presentation_ns: u64,
) -> SceneState {
    let hits: Vec<_> = flashing(snapshot, flash.hit_flash_ns).collect();
    scene_state_with_hits(snapshot, flash, presentation_ns, &hits)
}

fn scene_state_with_hits(
    snapshot: &FieldSnapshot,
    flash: FlashSettings,
    presentation_ns: u64,
    hits: &[&ArmorHit],
) -> SceneState {
    let mut rune_flash: HashMap<u32, [bool; 5]> = HashMap::new();
    let mut outpost_flash: HashMap<u32, [bool; outpost::FACE_COUNT]> = HashMap::new();
    for hit in hits {
        match hit.target {
            ArmorTarget::Rune { rune, blade } => {
                if let Some(flag) = rune_flash.entry(rune).or_default().get_mut(blade as usize) {
                    *flag = true;
                }
            }
            ArmorTarget::Outpost { outpost, face } => {
                if let Some(flag) = outpost_flash
                    .entry(outpost)
                    .or_default()
                    .get_mut(face as usize)
                {
                    *flag = true;
                }
            }
            // Chassis flashes are gathered with the chassis in `publish_scene`.
            ArmorTarget::Chassis { .. } | ArmorTarget::Base { .. } => {}
        }
    }
    SceneState {
        bases: snapshot
            .bases
            .iter()
            .enumerate()
            .map(|(index, base)| rm_simulator_render::sync::BaseAppearance {
                disabled: base.hp == 0,
                plates: std::array::from_fn(|plate| pose_flu(base.pose(plate, presentation_ns))),
                hit_flash: std::array::from_fn(|plate| {
                    hits.iter().any(|hit| {
                        hit.target
                            == ArmorTarget::Base {
                                base: index as u32,
                                plate: plate as u32,
                            }
                    })
                }),
            })
            .collect(),
        dart_target_fraction: rm_simulator_world::referee::dart_target_fraction(presentation_ns)
            as f32,
        base_open: snapshot
            .referee
            .as_ref()
            .map_or([false; 2], |r| r.base_open),
        dart_door_open: snapshot
            .referee
            .as_ref()
            .map_or([true; 2], |r| r.dart_door_open),
        source: SourceTime {
            tick: snapshot.tick,
            time_ns: snapshot.time_ns,
        },
        runes: snapshot
            .runes
            .iter()
            .enumerate()
            .map(|(index, rune)| {
                let big = rune.kind == RuneKind::Big;
                let activated = rune.state == RuneState::Activated;
                // An activated rune blinks its arms (a visual aid, see
                // `--rune-flash-hz`); the rule text shows them lit.
                let lit = !activated
                    || rune_lit(
                        rune.state_since_ns,
                        presentation_ns,
                        flash.rune_flash_hz,
                        flash.rune_flashes,
                    );
                RuneAppearance {
                    hub_pose: pose_flu(rune.hub_pose),
                    angle_rad: rune.presentation_angle_rad(presentation_ns),
                    active_blades: rune.active_blades,
                    // Big Rune shows hit blades only once the whole rune is activated.
                    activated_blades: rune.activated.map(|hit| hit && lit && (!big || activated)),
                    fully_activated: activated && lit,
                    progress_stages: std::array::from_fn(|stage| {
                        big && lit && (stage as u32) < rune.completed_groups
                    }),
                    hit_flash: rune_flash.get(&(index as u32)).copied().unwrap_or_default(),
                }
            })
            .collect(),
        outposts: snapshot
            .outposts
            .iter()
            .enumerate()
            .map(|(index, outpost)| {
                let outpost = outpost.presentation_at(presentation_ns);
                OutpostAppearance {
                    disabled: outpost.destroyed,
                    origin: pose_flu(outpost.origin),
                    angle_rad: outpost.angle_rad,
                    armors: std::array::from_fn(|face| {
                        pose_flu(
                            outpost
                                .armors
                                .iter()
                                .find(|armor| armor.id == face as u32)
                                .expect("outpost snapshot has three faces")
                                .pose,
                        )
                    }),
                    hit_flash: outpost_flash
                        .get(&(index as u32))
                        .copied()
                        .unwrap_or_default(),
                }
            })
            .collect(),
        projectiles: snapshot
            .projectiles
            .iter()
            .map(|projectile| ProjectileAppearance {
                position_m: projectile.position_m,
                radius_m: (projectile.caliber.diameter_m() / 2.0) as f32,
            })
            .collect(),
        chassis: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;
    use rm_simulator_world::{Caliber, ChassisConfig, Field, Pose, Shot, tick_ns};

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 1e-4)
    }
    fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
        let [w, x, y, z] = q.map(|v| v as f32);
        let out = Quat::from_xyzw(x, y, z, w) * Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32);
        [out.x as f64, out.y as f64, out.z as f64]
    }
    #[test]
    fn presentation_reconstructs_motion_without_changing_source_or_authority() {
        let mut field = Field::new(&rm_simulator_world::FieldConfig::default()).unwrap();
        let checkpoint = field.snapshot();
        let flash = FlashSettings {
            hit_flash_ns: 50_000_000,
            rune_flash_hz: 2.0,
            rune_flashes: 3,
        };
        let at = 123_000_000 / tick_ns() * tick_ns();
        let presented = scene_state_at(&checkpoint, flash, at);
        field.step(at / tick_ns()).unwrap();
        let actual = scene_state(&field.snapshot(), flash);
        assert!((presented.runes[0].angle_rad - actual.runes[0].angle_rad).abs() < 1e-12);
        assert_eq!(
            presented.outposts[0].angle_rad,
            actual.outposts[0].angle_rad
        );
        assert_eq!(presented.outposts[0].armors, actual.outposts[0].armors);
        assert_eq!(presented.source.tick, checkpoint.tick);
        assert_eq!(presented.source.time_ns, checkpoint.time_ns);
        let exact = scene_state_at(&checkpoint, flash, checkpoint.time_ns);
        assert_eq!(
            exact.outposts[0].angle_rad,
            checkpoint.outposts[0].angle_rad
        );
    }

    #[test]
    fn rune_state_masks_clear_on_failure_deactivation_and_big_completion_blinks() {
        let mut field = Field::new(&rm_simulator_world::FieldConfig::default()).unwrap();
        let flash = FlashSettings {
            hit_flash_ns: 50_000_000,
            rune_flash_hz: 2.0,
            rune_flashes: 3,
        };
        let view = |field: &Field| scene_state(&field.snapshot(), flash).runes[0].clone();
        field.rune_mut(0).unwrap().hit(0, 0).unwrap();
        assert!(view(&field).activated_blades[0]);
        field.rune_mut(0).unwrap().hit(0, 0).unwrap();
        assert_eq!(view(&field).activated_blades, [false; 5]);
        field.rune_mut(0).unwrap().hit(0, 0).unwrap();
        let deactivate_at = 2_501_000_000_u64.div_ceil(tick_ns()) * tick_ns();
        field.step(deactivate_at / tick_ns()).unwrap();
        assert_eq!(view(&field).activated_blades, [false; 5]);
        field
            .rune_mut(0)
            .unwrap()
            .deactivate(deactivate_at)
            .unwrap();
        assert_eq!(view(&field).active_blades, [false; 5]);
        assert_eq!(view(&field).progress_stages, [false; 5]);
        assert!(!view(&field).fully_activated);
        let mut snapshot = field.snapshot();
        let rune = &mut snapshot.runes[0];
        rune.kind = RuneKind::Big;
        rune.state = RuneState::Activated;
        rune.activated = [true; 5];
        rune.completed_groups = 5;
        rune.state_since_ns = snapshot.time_ns - 300_000_000;
        let dark = scene_state(&snapshot, flash);
        assert_eq!(dark.runes[0].activated_blades, [false; 5]);
        assert_eq!(dark.runes[0].progress_stages, [false; 5]);
        assert!(!dark.runes[0].fully_activated);
        snapshot.time_ns += 300_000_000;
        let lit = scene_state(&snapshot, flash);
        assert_eq!(lit.runes[0].progress_stages, [true; 5]);
        assert!(lit.runes[0].fully_activated);
    }

    #[test]
    fn additional_static_assets_are_drawn_once_per_placement() {
        let asset = |file: &str, count: usize| rm_simulator_server::cad_assets::CadAsset {
            semantics: None,
            file: file.to_owned(),
            placements: vec![rm_simulator_world::Pose::at([1.0, 2.0, 3.0]); count],
            collision: None,
            source_tessellated_collision: false,
        };
        let cad = CadAssets {
            root: std::path::PathBuf::new(),
            floor_top_cad_m: -1.6,
            collision_solids: true,
            floor: asset("floor", 0),
            arena_static: asset("arena", 0),
            rune: asset("rune", 1),
            outpost: asset("outpost", 2),
            base: asset("base", 2),
            tech_core: asset("tech-core", 2),
            static_assets: vec![
                asset("centre", 1),
                asset("dart", 2),
                asset("resource", 2),
                asset("footing", 2),
            ],
        };
        let instances = cad_instances(&cad, true, false);
        assert_eq!(instances.len(), 16);
        for asset in &cad.static_assets {
            let drawn: Vec<_> = instances.iter().filter(|i| i.file == asset.file).collect();
            assert_eq!(drawn.len(), asset.placements.len());
            for (instance, placement) in drawn.iter().zip(&asset.placements) {
                assert_eq!(instance.transform, transform_of(placement));
                assert!(matches!(
                    instance.role,
                    CadRole::Static { base_color: None }
                ));
            }
        }
    }
    #[test]
    fn activated_runes_blink_at_the_configured_rate() {
        // 2 Hz: lit for the first quarter second of each half second.
        assert!(rune_lit(1_000_000_000, 1_000_000_000, 2.0, 3));
        assert!(rune_lit(1_000_000_000, 1_249_000_000, 2.0, 3));
        assert!(!rune_lit(1_000_000_000, 1_250_000_000, 2.0, 3));
        assert!(!rune_lit(1_000_000_000, 1_499_000_000, 2.0, 3));
        assert!(rune_lit(1_000_000_000, 1_500_000_000, 2.0, 3));
        // Three flashes take 1.5 s; the dark half of the fourth cycle stays lit.
        assert!(!rune_lit(1_000_000_000, 2_300_000_000, 2.0, 3));
        assert!(rune_lit(1_000_000_000, 2_800_000_000, 2.0, 3));
        assert!(rune_lit(1_000_000_000, 9_800_000_000, 2.0, 3));
        // A zero count blinks forever; a zero rate disables the blink;
        // a clock behind the activation stays lit.
        assert!(!rune_lit(1_000_000_000, 9_800_000_000, 2.0, 0));
        assert!(rune_lit(1_000_000_000, 1_300_000_000, 0.0, 3));
        assert!(rune_lit(5_000_000_000, 1_000_000_000, 2.0, 3));
        // The scene adaptation applies it to the activated arms only.
        let mut field = Field::new(&rm_simulator_world::FieldConfig::default()).unwrap();
        loop {
            field.step(100).unwrap();
            let rune = field.snapshot().runes[0].clone();
            if rune.state == RuneState::Activated {
                break;
            }
            let blade = rune.active_blade.unwrap();
            let now = field.time_ns();
            field.rune_mut(0).unwrap().hit(now, blade).unwrap();
        }
        let snapshot = field.snapshot();
        let since = snapshot.runes[0].state_since_ns;
        let flash = |hz| FlashSettings {
            hit_flash_ns: 50_000_000,
            rune_flash_hz: hz,
            rune_flashes: 3,
        };
        let lit = scene_state(&snapshot, flash(0.0));
        assert_eq!(lit.runes[0].activated_blades, [true; 5]);
        // Step into the dark half of a 2 Hz blink: the first dark moment at
        // least one step past the snapshot, wrapped forward by blink periods.
        let period = 500_000_000;
        let mut dark_at = since + 300_000_000;
        dark_at += (snapshot.time_ns.saturating_sub(dark_at) / period + 1) * period;
        field
            .step((dark_at - snapshot.time_ns) / rm_simulator_world::tick_ns())
            .unwrap();
        let dark = scene_state(&field.snapshot(), flash(2.0));
        assert_eq!(dark.runes[0].activated_blades, [false; 5]);
        let still_lit = scene_state(&field.snapshot(), flash(0.0));
        assert_eq!(still_lit.runes[0].activated_blades, [true; 5]);
    }
    #[test]
    fn hit_flash_window_follows_the_setting() {
        let (config, muzzle) = {
            let config = rm_simulator_world::FieldConfig {
                runes: Vec::new(),
                outposts: vec![rm_simulator_world::OutpostConfig {
                    pivot_cad_m: None,
                    origin: Pose::at([4.0, 0.0, 0.0]),
                    speed_rad_s: 0.0,
                }],
                ..Default::default()
            };
            let field = Field::new(&config).unwrap();
            let face = field.snapshot().outposts[0].armors[0].pose;
            let muzzle = Pose::at([
                face.translation_m[0] - 1.5,
                face.translation_m[1],
                face.translation_m[2] + 0.005,
            ]);
            (config, muzzle)
        };
        let mut field = Field::new(&config).unwrap();
        field
            .fire(muzzle, Shot::at_limit(Caliber::Mm17), None)
            .unwrap();
        field.step(300_000_000 / tick_ns()).unwrap();
        let snapshot = field.snapshot();
        let hit = snapshot.hits.iter().find(|hit| hit.detected).unwrap();
        let age_ns = snapshot.time_ns - hit.time_ns;
        assert!(age_ns > 50_000_000, "{age_ns}");
        let short = FlashSettings {
            hit_flash_ns: 50_000_000,
            rune_flash_hz: 0.0,
            rune_flashes: 3,
        };
        let long = FlashSettings {
            hit_flash_ns: age_ns + 1,
            rune_flash_hz: 0.0,
            rune_flashes: 3,
        };
        assert_eq!(
            scene_state(&snapshot, short).outposts[0].hit_flash,
            [false; 3]
        );
        assert!(scene_state(&snapshot, long).outposts[0].hit_flash[0]);
    }
    /// On a chassis rolled 20° the yaw stage stays bolted to the body and
    /// the pitch stage still points the barrel along the world aim.
    #[test]
    fn gimbal_stages_follow_the_body_and_point_the_barrel_at_the_aim() {
        let roll = DQuat::from_rotation_x(20_f64.to_radians());
        let body = PoseFlu {
            translation_m: [0.0, 0.0, 0.2],
            rotation_wxyz: wxyz(roll),
        };
        let aim_rotation = DQuat::from_rotation_z(0.7) * DQuat::from_rotation_y(-0.3);
        let aim = PoseFlu {
            translation_m: [0.0, 0.0, 0.4],
            rotation_wxyz: wxyz(aim_rotation),
        };
        let (stage, turret) = gimbal_poses(body, aim);
        let stage_up = rotate(stage.rotation_wxyz, [0.0, 0.0, 1.0]);
        let body_up = roll * DVec3::Z;
        assert!(close(stage_up, body_up.to_array()), "{stage_up:?}");
        let barrel = rotate(turret.rotation_wxyz, [1.0, 0.0, 0.0]);
        let wanted = aim_rotation * DVec3::X;
        assert!(close(barrel, wanted.to_array()), "{barrel:?} vs {wanted:?}");
        // The pitch axis is the yaw stage's left axis: the turret's left
        // stays in the stage's plane.
        let turret_left = rotate(turret.rotation_wxyz, [0.0, 1.0, 0.0]);
        let stage_left = rotate(stage.rotation_wxyz, [0.0, 1.0, 0.0]);
        assert!(
            close(turret_left, stage_left),
            "{turret_left:?} vs {stage_left:?}"
        );
    }
    #[test]
    fn wheel_visuals_put_the_axle_on_the_hub_radius_and_the_turret_above_the_body() {
        let config = ChassisConfig::default();
        let yaw = std::f64::consts::FRAC_PI_2;
        let mut field = Field::new(&rm_simulator_world::FieldConfig {
            chassis: vec![rm_simulator_world::ChassisPlacement {
                config: config.clone(),
                spawn: Pose::yawed([1.0, 0.0, config.rest_height_m()], yaw),
                team: Team::Blue,
                kind: rm_simulator_world::RobotKind::Infantry,
                performance: None,
            }],
            ..Default::default()
        })
        .unwrap();
        field.step(1).unwrap();
        let chassis = field.snapshot().chassis.remove(0);
        for (wheel, hub) in chassis.wheels.iter().zip(&config.wheel_hubs_m) {
            let pose = wheel_pose(&chassis, *hub, wheel);
            let axle = rotate(pose.rotation_wxyz, [0.0, 1.0, 0.0]);
            // The axle is the radial direction of the hub, turned with the body.
            let radius = (hub[0].powi(2) + hub[1].powi(2)).sqrt();
            let expected = [-hub[1] / radius, hub[0] / radius, 0.0];
            assert!(close(axle, expected), "{axle:?} vs {expected:?}");
        }
        let pivot = chassis.turret.translation_m;
        assert!(
            (pivot[2] - chassis.pose.translation_m[2] - config.turret_center_m[2]).abs() < 1e-6
        );
        assert!((pivot[0] - 1.0).abs() < 1e-3 && pivot[1].abs() < 1e-3);
        // Another player's turret is drawn pointing where the world holds
        // its aim, on a gimbal that turns with the body.
        let appearance = chassis_appearance(&chassis, &[false, true], None);
        assert_eq!(appearance.turret.translation_m, pivot);
        assert_eq!(appearance.team, TeamColor::Blue);
        assert_eq!(appearance.yaw_stage.translation_m, pivot);
        let barrel = rotate(appearance.turret.rotation_wxyz, [1.0, 0.0, 0.0]);
        let aim = rotate(chassis.turret.rotation_wxyz, [1.0, 0.0, 0.0]);
        assert!(close(barrel, aim), "{barrel:?} vs {aim:?}");
        let stage_up = rotate(appearance.yaw_stage.rotation_wxyz, [0.0, 0.0, 1.0]);
        let body_up = rotate(chassis.pose.rotation_wxyz, [0.0, 0.0, 1.0]);
        assert!(close(stage_up, body_up), "{stage_up:?} vs {body_up:?}");
        assert_eq!(appearance.armor.len(), 4);
        assert!(!appearance.armor[0].hit_flash && appearance.armor[1].hit_flash);
        assert!(!appearance.defeated);
        assert!((appearance.pivot_above_body_m - 0.15).abs() < 1e-6);
    }
}
