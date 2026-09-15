// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Player input: the gimbal camera, driving the chassis (or flying), the gun
//! and mouse capture. Every action reaches the world as a protocol command.
use bevy::math::{DQuat, DVec3};
use bevy::{
    input::mouse::AccumulatedMouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use rm_simulator_render::{FLOOR_Y_M, apply_pose, flu_position, pose_from_transform};
use rm_simulator_server::protocol::Command;
use rm_simulator_world::{ChassisCommand, Pose, Shot, chassis::yaw_of};

use crate::bindings::InputAction;
use crate::frames::{gimbal_poses, pose_flu, to_pose};
use crate::hud::HudState;
use crate::session::Session;

/// Muzzle offset from the gun pivot along the view line. Driving, the pivot is
/// the turret and this is the barrel length; flying, the pivot is the eye and
/// the muzzle also sits below the view line.
///
/// It is the physics barrel length the host launches from, published once by
/// `rm_simulator_physics::chassis`, so the muzzle a player sees cannot drift
/// from the point their ball actually leaves.
pub const MUZZLE_FORWARD_M: f32 = rm_simulator_world::chassis::MUZZLE_FORWARD_M as f32;
/// Muzzle offset below the view line in metres while flying. Driving levels the
/// muzzle with the turret, since the gun pivot already sits on the barrel axis.
pub const MUZZLE_DROP_M: f32 = 0.05;
/// Third-person camera: behind and above the turret, offset over the right
/// shoulder so the chassis does not sit on the crosshair.
const THIRD_PERSON_BACK_M: f32 = 2.4;
const THIRD_PERSON_UP_M: f32 = 0.9;
const THIRD_PERSON_RIGHT_M: f32 = 0.45;
/// Commanded chassis speeds (the wheels are motor-limited to about 5.4 m/s).
const DRIVE_SPEED_M_S: f64 = 2.0;
const DRIVE_FAST_M_S: f64 = 5.0;
/// Spin-mode yaw rate and how hard the chassis heading follows the gimbal.
const SPIN_RAD_S: f64 = 6.0;
const FOLLOW_GAIN_PER_S: f64 = 6.0;
const MAX_FOLLOW_RAD_S: f64 = 8.0;
/// Gimbal pitch range while driving, as (lowest, highest) radians. The auto-aim
/// assist solves inside this same range, so an assist solution is always a pitch
/// the player's own aim could have reached.
pub(crate) const DRIVE_PITCH_RAD: (f32, f32) = (-0.52, 0.79);

/// The local client's view: eye and gun pivot positions, view angles and mouse
/// capture. Every role has one; a client with no chassis only ever flies it.
#[derive(Resource)]
pub struct Player {
    /// Eye position in Bevy coordinates.
    pub position: Vec3,
    /// Gun pivot in Bevy coordinates: the eye when flying, the turret when driving.
    pub muzzle_origin: Vec3,
    /// Muzzle offset below the view line in metres; zero while driving and
    /// `MUZZLE_DROP_M` while flying.
    pub muzzle_drop_m: f32,
    /// View yaw in radians, counter-clockwise about up from world +x. The same
    /// angle travels as the chassis command's `aim_yaw_rad`.
    pub yaw_rad: f32,
    /// View pitch in radians above the horizon. `Player::at` clamps it to
    /// 1.5 rad; driving clamps it to the gun's elevation limits every frame.
    pub pitch_rad: f32,
    /// Mouse capture is held, so relative mouse motion aims and the trigger can
    /// fire. A blocking panel always drops it.
    pub captured: bool,
}
impl Player {
    /// A player whose eye and gun pivot are at `eye`, looking along `yaw_rad`.
    pub fn at(eye: Vec3, yaw_rad: f32, pitch_rad: f32) -> Self {
        Self {
            position: eye,
            muzzle_origin: eye,
            muzzle_drop_m: MUZZLE_DROP_M,
            yaw_rad,
            pitch_rad: pitch_rad.clamp(-1.5, 1.5),
            captured: false,
        }
    }
}
/// Marker for the single gameplay camera, the one the pilot's eye or the free
/// camera drives.
#[derive(Component)]
pub struct PlayerCamera;
/// What the pilot is commanding, as opposed to the chassis command derived
/// from it. Aim, the body-frame wish and the follow yaw rate all move with the
/// gimbal and the chassis heading every frame; these values change only when
/// the pilot acts or the robot is placed, so a change here is a transition.
#[derive(Clone, Copy, PartialEq)]
struct DriveIntent {
    forward: f64,
    left: f64,
    fast: bool,
    spinning: bool,
    blocked: bool,
    placement_revision: u64,
}
/// The driven chassis, present unless flying or spectating.
#[derive(Resource)]
pub struct Drive {
    /// Id of the chassis this client drives; every chassis command names it.
    pub chassis_id: u32,
    /// Camera trails behind and above the turret instead of sitting at the eye.
    pub third_person: bool,
    /// Spin mode, toggled by the `Spin` action: the chassis turns at a fixed
    /// rate instead of following the aim.
    pub spinning: bool,
    intent: Option<DriveIntent>,
}
impl Drive {
    /// A drive for `chassis_id`: spin off, third person set by `third_person`,
    /// and no command sent yet.
    pub fn new(chassis_id: u32, third_person: bool) -> Self {
        Self {
            chassis_id,
            third_person,
            spinning: false,
            intent: None,
        }
    }
}
/// The weapon this client fires: the projectile and the cadence the host allows.
#[derive(Resource)]
pub struct Gun {
    /// Projectile and launch speed selected by the pilot within host limits.
    pub shot: Shot,
    /// Shortest simulation-time gap between shots, in nanoseconds.
    pub interval_ns: u64,
    /// World time at which the next shot may leave.
    pub(crate) next_shot_ns: u64,
    /// The trigger was pressed while the mouse was captured and is still held.
    trigger_held: bool,
}
impl Gun {
    /// A gun for `shot`: ready at world time zero and firing no faster than one
    /// round per `interval_ns`.
    pub fn new(shot: Shot, interval_ns: u64) -> Self {
        Self {
            shot,
            interval_ns,
            next_shot_ns: 0,
            trigger_held: false,
        }
    }
}

/// Gimbal rotation in Bevy coordinates: yaw about up, then pitch.
fn view_rotation(player: &Player) -> Quat {
    Quat::from_euler(EulerRot::YXZ, player.yaw_rad, player.pitch_rad, 0.0)
}
/// Gun pivot pose: the eye when flying, the turret when driving, facing the view line.
pub fn gun_transform(player: &Player) -> Transform {
    Transform::from_translation(player.muzzle_origin).with_rotation(view_rotation(player))
}
/// Muzzle pose in world FLU: ahead of (and, flying, below) the gun pivot.
fn muzzle_pose(player: &Player) -> Pose {
    let muzzle = gun_transform(player).mul_transform(Transform::from_xyz(
        0.0,
        -player.muzzle_drop_m,
        -MUZZLE_FORWARD_M,
    ));
    to_pose(pose_from_transform(&muzzle))
}

fn wrap_angle(angle: f64) -> f64 {
    (angle + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
}
/// Chassis command from the keys and the gimbal: WASD drive in the gimbal's
/// yaw frame, the chassis heading follows the gimbal (or spins with `R`),
/// Ctrl is fast, and the gun aims where the gimbal looks.
pub fn drive_command(
    wish_forward: f64,
    wish_left: f64,
    fast: bool,
    spinning: bool,
    gimbal_yaw_rad: f64,
    gimbal_pitch_rad: f64,
    chassis_yaw_rad: f64,
) -> ChassisCommand {
    let wish = DVec3::new(wish_forward, wish_left, 0.0).normalize_or_zero()
        * if fast {
            DRIVE_FAST_M_S
        } else {
            DRIVE_SPEED_M_S
        };
    // Wish in the gimbal frame -> world -> chassis body frame.
    let body = DQuat::from_rotation_z(gimbal_yaw_rad - chassis_yaw_rad) * wish;
    let yaw_rate_rad_s = if spinning {
        SPIN_RAD_S
    } else {
        (FOLLOW_GAIN_PER_S * wrap_angle(gimbal_yaw_rad - chassis_yaw_rad))
            .clamp(-MAX_FOLLOW_RAD_S, MAX_FOLLOW_RAD_S)
    };
    ChassisCommand {
        forward_m_s: body.x,
        left_m_s: body.y,
        yaw_rate_rad_s,
        aim_yaw_rad: gimbal_yaw_rad,
        aim_pitch_rad: gimbal_pitch_rad,
    }
}

/// Read the drive keys and hand the chassis a new command when it changes.
pub fn drive_chassis(
    ui: Res<HudState>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    player: Res<Player>,
    mut drive: ResMut<Drive>,
    mut session: ResMut<Session>,
) {
    if !ui.blocks_input()
        && ui
            .controls
            .just_pressed(InputAction::Spin, &keys, buttons.as_deref())
    {
        drive.spinning = !drive.spinning;
    }
    let Some(chassis) = session.presented_chassis() else {
        return;
    };
    let axis = |positive, negative| {
        f64::from(u8::from(ui.controls.pressed(
            positive,
            &keys,
            buttons.as_deref(),
        ))) - f64::from(u8::from(ui.controls.pressed(
            negative,
            &keys,
            buttons.as_deref(),
        )))
    };
    let blocked = ui.blocks_input();
    let intent = DriveIntent {
        forward: if blocked {
            0.
        } else {
            axis(InputAction::Forward, InputAction::Backward)
        },
        left: if blocked {
            0.
        } else {
            axis(InputAction::Left, InputAction::Right)
        },
        fast: ui
            .controls
            .pressed(InputAction::Fast, &keys, buttons.as_deref()),
        spinning: drive.spinning && !blocked,
        blocked,
        placement_revision: chassis.placement_revision,
    };
    let mut command = drive_command(
        intent.forward,
        intent.left,
        intent.fast,
        intent.spinning,
        f64::from(player.yaw_rad),
        f64::from(player.pitch_rad),
        yaw_of(chassis.pose),
    );
    if blocked {
        command.yaw_rate_rad_s = 0.;
    }
    let transition = drive.intent != Some(intent);
    drive.intent = Some(intent);
    session.drive(drive.chassis_id, command, transition);
}

/// Mouse angles are commanded aim. Keep them separate from the returned host pose.
fn driving_cradle(
    chassis: &rm_simulator_world::ChassisSnapshot,
    _player: &Player,
    _local_aim: bool,
) -> rm_simulator_render::PoseFlu {
    // Match the rendered two-axis gimbal, including the roll inherited from
    // its chassis-mounted yaw stage. The stabilized aim alone has no roll.
    gimbal_poses(pose_flu(chassis.pose), pose_flu(chassis.turret)).1
}

/// Sample mouse aim and camera mode before assist and chassis input. Mouse
/// motion applies only while capture is held and no panel blocks input.
#[allow(clippy::too_many_arguments)]
pub fn sample_drive_aim(
    ui: Res<HudState>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    motion: Res<AccumulatedMouseMotion>,
    mut drive: ResMut<Drive>,
    mut player: ResMut<Player>,
) {
    if !ui.blocks_input()
        && ui
            .controls
            .just_pressed(InputAction::Camera, &keys, buttons.as_deref())
    {
        drive.third_person = !drive.third_person;
    }
    if player.captured && !ui.blocks_input() {
        player.yaw_rad -= motion.delta.x * ui.sensitivity;
        player.pitch_rad -= motion.delta.y * ui.controls.vertical_sensitivity(ui.sensitivity);
    }
    player.pitch_rad = player.pitch_rad.clamp(DRIVE_PITCH_RAD.0, DRIVE_PITCH_RAD.1);
}

/// Place the camera from the accepted predicted motor pose after this frame's
/// mouse/assist sample and prediction exchange. Never substitute commanded aim
/// for physical barrel orientation.
pub fn drive_camera(
    session: Res<Session>,
    drive: Res<Drive>,
    mut player: ResMut<Player>,
    mut camera: Single<&mut Transform, With<PlayerCamera>>,
) {
    let Some(chassis) = session.presented_chassis() else {
        return;
    };
    let pivot = flu_position(chassis.turret.translation_m);
    player.muzzle_origin = pivot;
    player.muzzle_drop_m = 0.0;
    let cradle = driving_cradle(chassis, &player, session.local_aim);
    let mut mount = Transform::default();
    apply_pose(&mut mount, cradle);
    let rotation = mount.rotation;
    let heading = Quat::from_rotation_y(rotation.to_euler(EulerRot::YXZ).0);
    if drive.third_person {
        let mut eye = pivot
            + rotation * Vec3::Z * THIRD_PERSON_BACK_M
            + heading * Vec3::X * THIRD_PERSON_RIGHT_M
            + Vec3::Y * THIRD_PERSON_UP_M;
        // Keep the trailing camera above the chassis when looking up.
        eye.y = eye.y.max(pivot.y);
        let eye = session.corrected_eye(eye);
        player.position = eye;
        **camera = Transform::from_translation(eye).with_rotation(rotation);
    } else {
        // The eye rides the camera block on the pitch stage, so it rolls
        // with the gimbal on a tilted chassis while looking along the aim.
        player.position = mount.translation
            + mount.rotation
                * rm_simulator_render::chassis::camera_eye(
                    chassis.config.turret_half_m.map(|v| v as f32),
                );
        player.position = session.corrected_eye(player.position);
        **camera = Transform::from_translation(player.position).with_rotation(mount.rotation);
    }
}

/// Move the free camera with the fly keys when no chassis is driven, at 3 m/s or
/// 8 m/s while `Fast` is held, and keep its eye above the floor. Motion follows
/// app frame time, not the 1 ms simulation tick. Mouse motion aims only while
/// capture is held and no panel blocks input.
pub fn fly_camera(
    ui: Res<HudState>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    motion: Res<AccumulatedMouseMotion>,
    mut player: ResMut<Player>,
    mut camera: Single<&mut Transform, With<PlayerCamera>>,
) {
    if player.captured && !ui.blocks_input() {
        player.yaw_rad -= motion.delta.x * ui.sensitivity;
        player.pitch_rad = (player.pitch_rad
            - motion.delta.y * ui.controls.vertical_sensitivity(ui.sensitivity))
        .clamp(-1.5, 1.5);
    }
    let rotation = view_rotation(&player);
    let forward = Vec3::new(-player.yaw_rad.sin(), 0.0, -player.yaw_rad.cos());
    let right = Vec3::new(forward.z, 0.0, -forward.x);
    let mut wish = Vec3::ZERO;
    for (key, direction) in [
        (InputAction::Forward, forward),
        (InputAction::Backward, -forward),
        (InputAction::Right, -right),
        (InputAction::Left, right),
        (InputAction::Up, Vec3::Y),
        (InputAction::Down, Vec3::NEG_Y),
    ] {
        if !ui.blocks_input() && ui.controls.pressed(key, &keys, buttons.as_deref()) {
            wish += direction;
        }
    }
    let speed_m_s = if ui
        .controls
        .pressed(InputAction::Fast, &keys, buttons.as_deref())
    {
        8.0
    } else {
        3.0
    };
    player.position += wish.normalize_or_zero() * speed_m_s * time.delta_secs();
    player.position.y = player.position.y.max(FLOOR_Y_M + 0.15);
    player.muzzle_origin = player.position;
    player.muzzle_drop_m = MUZZLE_DROP_M;
    **camera = Transform::from_translation(player.position).with_rotation(rotation);
}

/// Shots leave at the gun's rate while the trigger is held with the mouse captured.
pub fn fire_gun(
    ui: Res<HudState>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    player: Res<Player>,
    mut gun: ResMut<Gun>,
    mut session: ResMut<Session>,
    mut assist: Option<ResMut<crate::auto_aim::AutoAim>>,
) {
    let empty_keys = ButtonInput::default();
    let keys = keys.as_deref().unwrap_or(&empty_keys);
    if ui
        .controls
        .just_pressed(InputAction::Fire, keys, Some(&buttons))
    {
        gun.trigger_held = player.captured && !ui.blocks_input();
    } else if !ui.controls.pressed(InputAction::Fire, keys, Some(&buttons)) {
        gun.trigger_held = false;
    }
    if ui.blocks_input() || !player.captured {
        gun.trigger_held = false;
    }
    let now_ns = session.presentation_time_ns();
    let auto_fire = assist.as_ref().is_some_and(|assist| assist.fire_ready)
        && player.captured
        && !ui.blocks_input();
    if (gun.trigger_held || auto_fire) && !session.paused && now_ns >= gun.next_shot_ns {
        gun.next_shot_ns = now_ns + gun.interval_ns;
        let mut timing = session.fire_timing();
        timing.observed_muzzle_pose = session.presented_chassis().map(|chassis| {
            let turret = driving_cradle(chassis, &player, session.local_aim);
            let mut transform = Transform::default();
            apply_pose(&mut transform, turret);
            to_pose(pose_from_transform(
                &transform.mul_transform(Transform::from_xyz(0., 0., -MUZZLE_FORWARD_M)),
            ))
        });
        let command = session.chassis_id.map_or_else(
            || Command::SpawnProjectile {
                muzzle: session.weapon.spread.apply(muzzle_pose(&player), 0, now_ns),
                shot: session
                    .weapon
                    .sample_shot(0, now_ns, session.weapon_limits.max_speed_m_s),
            },
            |shooter| Command::Fire {
                shooter,
                timing: Some(timing),
            },
        );
        if auto_fire && let Some(assist) = assist.as_mut() {
            assist.shot_requested(session.fire_time_ns());
        }
        session.apply(command);
    }
}

/// Left click captures the mouse; modal menus release it.
pub fn mouse_capture(
    ui: Res<HudState>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut player: ResMut<Player>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if ui.blocks_input() {
        player.captured = false;
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
        return;
    }
    if buttons.just_pressed(MouseButton::Left) {
        player.captured = true;
    } else {
        return;
    }
    cursor.grab_mode = if player.captured {
        CursorGrabMode::Locked
    } else {
        CursorGrabMode::None
    };
    cursor.visible = !player.captured;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_cradle_follows_tilt_and_inversion_without_changing_aim() {
        let session = crate::session::test_session(true);
        let mut chassis = session.own_chassis().unwrap().clone();
        let player = Player::at(Vec3::ZERO, 0., 0.);
        for roll in [0.4, 2.4, std::f64::consts::PI] {
            let body = DQuat::from_rotation_x(roll) * DQuat::from_rotation_y(0.25);
            chassis.pose.rotation_wxyz = crate::frames::wxyz(body);
            chassis.turret.rotation_wxyz = [1., 0., 0., 0.];
            let cradle = driving_cradle(&chassis, &player, false);
            let rotation = crate::frames::dquat(cradle.rotation_wxyz);
            assert!((rotation * DVec3::X - DVec3::X).length() < 1e-9);
            // With the barrel facing forward, the camera's up is the body's
            // up projected perpendicular to that barrel, including when inverted.
            let body_up = body * DVec3::Z;
            let expected_up = DVec3::new(0., body_up.y, body_up.z).normalize();
            assert!((rotation * DVec3::Z - expected_up).length() < 1e-9);
            assert_eq!(cradle.translation_m, chassis.turret.translation_m);
        }
    }

    #[test]
    fn commanded_mouse_aim_does_not_move_host_driven_view_before_confirmation() {
        let mut session = crate::session::test_session(true);
        session.ready();
        let mut player = Player::at(Vec3::ZERO, 0.0, 0.0);
        let before = driving_cradle(session.own_chassis().unwrap(), &player, false);
        player.yaw_rad = 0.7;
        player.pitch_rad = 0.3;
        assert_eq!(
            driving_cradle(session.own_chassis().unwrap(), &player, false),
            before
        );
        assert_eq!(
            driving_cradle(session.own_chassis().unwrap(), &player, true),
            before
        );
        let id = session.chassis_id.unwrap();
        session.apply(Command::Chassis {
            chassis: id,
            command: ChassisCommand {
                aim_yaw_rad: 0.7,
                aim_pitch_rad: 0.3,
                ..Default::default()
            },
        });
        session.apply(Command::Step { ticks: 1 });
        crate::session::wait_for_session(&mut session, Session::commands_confirmed);
        let after = driving_cradle(session.own_chassis().unwrap(), &player, false);
        assert_ne!(after, before);
        let barrel = crate::frames::dquat(after.rotation_wxyz) * DVec3::X;
        let aim =
            crate::frames::dquat(session.own_chassis().unwrap().turret.rotation_wxyz) * DVec3::X;
        assert!((barrel - aim).length() < 1e-9);
    }

    use rm_simulator_server::cad_assets::CadAssets;
    use rm_simulator_server::layout::{
        add_terrain, chassis_placement, default_spawn, load_terrain,
    };
    use rm_simulator_world::{ChassisConfig, Field, Team};

    #[test]
    fn opening_a_panel_stops_drive_spin_and_latches_the_trigger_off() {
        let session = crate::session::test_session(false);
        let mut drive = Drive::new(session.chassis_id.unwrap(), false);
        drive.spinning = true;
        let mut player = Player::at(Vec3::ZERO, 0., 0.);
        player.captured = true;
        let mut app = App::new();
        app.insert_resource(session)
            .insert_resource(drive)
            .insert_resource(player)
            .insert_resource(HudState {
                settings: true,
                ..default()
            })
            .insert_resource(Gun::new(
                Shot::at_limit(rm_simulator_world::Caliber::Mm17),
                100_000_000,
            ))
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_systems(Update, (drive_chassis, fire_gun).chain());
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        // A blocked panel is a transition, so the stop frame leaves at once.
        let command = app
            .world()
            .resource::<Session>()
            .last_input_frame
            .expect("a drive frame was sent")
            .command;
        assert_eq!(command.forward_m_s, 0.);
        assert_eq!(command.left_m_s, 0.);
        assert_eq!(command.yaw_rate_rad_s, 0.);
        assert!(!app.world().resource::<Gun>().trigger_held);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.world_mut().resource_mut::<HudState>().settings = false;
        app.update();
        assert!(
            !app.world().resource::<Gun>().trigger_held,
            "closing a panel must require a new trigger press"
        );
    }

    #[test]
    fn modal_releases_cursor_and_close_click_cannot_capture_or_exit() {
        let mut app = App::new();
        let mut player = Player::at(Vec3::ZERO, 0., 0.);
        player.captured = true;
        app.insert_resource(player)
            .insert_resource(HudState {
                settings: true,
                ..default()
            })
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_message::<AppExit>()
            .add_systems(Update, mouse_capture);
        let cursor = app
            .world_mut()
            .spawn((
                PrimaryWindow,
                CursorOptions {
                    grab_mode: CursorGrabMode::Locked,
                    visible: false,
                    ..default()
                },
            ))
            .id();
        app.update();
        assert!(!app.world().resource::<Player>().captured);
        assert!(app.world().get::<CursorOptions>(cursor).unwrap().visible);
        app.world_mut().resource_mut::<HudState>().close();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        assert!(!app.world().resource::<Player>().captured);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .reset_all();
        app.world_mut().resource_mut::<HudState>().consumed = false;
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        assert!(app.world().resource::<Player>().captured);
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 1e-4)
    }
    fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
        let [w, x, y, z] = q.map(|v| v as f32);
        let out = Quat::from_xyzw(x, y, z, w) * Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32);
        [out.x as f64, out.y as f64, out.z as f64]
    }
    #[test]
    fn muzzle_sits_ahead_of_and_below_the_eye_along_the_view_line() {
        // Looking FLU left (+y).
        let player = Player::at(
            flu_position([1.0, 2.0, 1.5]),
            std::f32::consts::FRAC_PI_2,
            0.0,
        );
        let muzzle = muzzle_pose(&player);
        assert!(close(
            muzzle.translation_m,
            [
                1.0,
                2.0 + MUZZLE_FORWARD_M as f64,
                1.5 - MUZZLE_DROP_M as f64
            ]
        ));
        assert!(close(
            rotate(muzzle.rotation_wxyz, [1.0, 0.0, 0.0]),
            [0.0, 1.0, 0.0]
        ));
    }
    #[test]
    fn drive_commands_are_relative_to_the_aim_and_turn_the_chassis_after_it() {
        // Aiming left (+y) with the chassis facing forward: W drives the body
        // sideways to its left, and the heading is pulled toward the aim.
        let command = drive_command(
            1.0,
            0.0,
            false,
            false,
            std::f64::consts::FRAC_PI_2,
            0.2,
            0.0,
        );
        assert!(
            command.forward_m_s.abs() < 1e-9 && (command.left_m_s - DRIVE_SPEED_M_S).abs() < 1e-9
        );
        assert!(command.yaw_rate_rad_s > 0.0 && command.yaw_rate_rad_s <= MAX_FOLLOW_RAD_S);
        // The gun aims where the gimbal looks.
        assert_eq!(
            (command.aim_yaw_rad, command.aim_pitch_rad),
            (std::f64::consts::FRAC_PI_2, 0.2)
        );
        // Aligned: W is straight ahead, Ctrl is fast, diagonals are normalised.
        let fast = drive_command(1.0, 1.0, true, false, 0.3, 0.0, 0.3);
        let speed = (fast.forward_m_s.powi(2) + fast.left_m_s.powi(2)).sqrt();
        assert!((speed - DRIVE_FAST_M_S).abs() < 1e-9 && fast.yaw_rate_rad_s.abs() < 1e-9);
        // Spinning keeps the yaw rate fixed whatever the aim does.
        assert_eq!(
            drive_command(0.0, 0.0, false, true, 2.0, 0.0, -1.0).yaw_rate_rad_s,
            SPIN_RAD_S
        );
        // The heading error wraps: from +3.1 rad to -3.1 rad is a short
        // counter-clockwise turn, not a long clockwise one.
        let wrapped = drive_command(0.0, 0.0, false, false, -3.1, 0.0, 3.1);
        assert!(wrapped.yaw_rate_rad_s > 0.0 && wrapped.yaw_rate_rad_s < 1.0);
    }
    /// The field packages installed on this machine: the default one (the
    /// V1.2.0 arena with its flat slab) and the earlier V2.0.0 extraction
    /// with its crowned slab. Whichever are missing are skipped.
    fn installed_cad_packages() -> Vec<(String, CadAssets)> {
        let home = std::env::var_os("HOME").unwrap_or_default();
        [
            rm_simulator_server::cad_assets::default_cad_assets(),
            std::path::PathBuf::from(home).join("dev/RM/assets/rm2026-extracted"),
        ]
        .into_iter()
        .filter_map(|root| match rm_simulator_server::cad_assets::load(&root) {
            Ok(cad) => Some((root.display().to_string(), cad)),
            Err(error) => {
                eprintln!(
                    "CAD assets at {} not usable ({error:#}); skipping",
                    root.display()
                );
                None
            }
        })
        .collect()
    }
    /// Drives the default chassis over the hex-marked plateau on the centre
    /// line (x ≈ -8.5..-6.3 m) in every installed field package; skipped
    /// when none is present. Both releases model it as a truncated pyramid
    /// with 17° ramps and a 0.15 m top, which the chassis crosses.
    #[test]
    fn chassis_crosses_the_cad_centre_plateau_when_the_assets_are_present() {
        for (root, cad) in installed_cad_packages() {
            eprintln!("centre plateau in {root}");
            chassis_crosses_the_centre_plateau(&cad);
        }
    }
    fn chassis_crosses_the_centre_plateau(cad: &CadAssets) {
        let terrain = load_terrain(cad).unwrap();
        let config = ChassisConfig::default();
        let (spawn, yaw_deg) = default_spawn(Team::Blue);
        let start = chassis_placement(
            config.clone(),
            rm_simulator_world::RobotKind::Infantry,
            Some(&terrain),
            Team::Blue,
            spawn,
            yaw_deg,
        );
        // A flat slab puts the spawn on height zero; the crowned V2.0.0 slab
        // sits about 0.11 m below the reference along the centre line.
        let ground = start.spawn.translation_m[2] - config.rest_height_m() - 0.01;
        assert!((-0.13..0.01).contains(&ground), "{ground}");
        let mut field = Field::new(&rm_simulator_world::FieldConfig {
            floor_height_m: rm_simulator_server::layout::CATCH_FLOOR_M,
            chassis: vec![start],
            ..Default::default()
        })
        .unwrap();
        add_terrain(&mut field, &terrain).unwrap();
        field.step(300).unwrap();
        let mut highest = f64::MIN;
        for _ in 0..40 {
            // Hold the heading the way the app's aim-follow does; the omni
            // rollers otherwise let the ridge sides push the body sideways.
            let chassis = field.snapshot().chassis.remove(0);
            field
                .command_chassis(
                    0,
                    drive_command(1.0, 0.0, false, false, 0.0, 0.0, yaw_of(chassis.pose)),
                )
                .unwrap();
            field.step(100).unwrap();
            let chassis = field.snapshot().chassis.remove(0);
            highest = highest.max(chassis.pose.translation_m[2]);
            let up = rotate(chassis.pose.rotation_wxyz, [0.0, 0.0, 1.0]);
            assert!(up[2] > 0.8, "chassis tipped: {up:?} at {:?}", chassis.pose);
        }
        let chassis = field.snapshot().chassis.remove(0);
        let [x, y, z] = chassis.pose.translation_m;
        assert!(chassis.wheels.iter().all(|w| w.contact.is_some()));
        // Started at -9 m; the plateau spans -8.5..-6.3 m and costs some of
        // the commanded 2 m/s. On the flat V1.2.0 slab its top is at +0.15 m;
        // the crowned V2.0.0 slab puts it at about +0.04 m.
        let crest = if ground > -0.05 { ground + 0.15 } else { 0.04 };
        assert!(x > -6.0 && y.abs() < 0.5, "{x} {y}");
        assert!(highest > crest + config.rest_height_m() - 0.03, "{highest}");
        assert!(z < highest - 0.05, "{z} {highest}");
    }
    /// The 起伏路段 undulating road near the red-side wall (x ≈ 6.3..8.7,
    /// y ≈ 5.4..7.5 m): a strip of waves about 70 mm high on the 0.2 m deck.
    /// The V2.0.0 STEP models it; the default package grafts those solids
    /// in, since the V1.2.0 STEP has a flat deck there. Checked in every
    /// installed field package through the ground the chassis rides on.
    #[test]
    fn cad_undulating_road_is_in_the_ground_when_the_assets_are_present() {
        for (root, cad) in installed_cad_packages() {
            eprintln!("undulating road in {root}");
            let terrain = load_terrain(&cad).unwrap();
            let heights: Vec<f64> = (0..23)
                .map(|i| {
                    let x = 6.45 + 0.1 * f64::from(i);
                    terrain.ground.ground_height_below(x, 6.4, 1.0).unwrap()
                })
                .collect();
            let (lowest, highest) = heights
                .iter()
                .fold((f64::MAX, f64::MIN), |(lo, hi), &h| (lo.min(h), hi.max(h)));
            // Waves, not a flat deck, and never above the 0.27 m crests; the
            // crowned V2.0.0 slab carries its road about 0.2 m lower.
            assert!((0.04..0.12).contains(&(highest - lowest)), "{heights:?}");
            assert!(highest < 0.28 && lowest > -0.2, "{heights:?}");
            let flat_slab = terrain.ground.ground_height_below(-9.0, 0.0, 1.0).unwrap() > -0.05;
            if flat_slab {
                assert!(lowest > 0.18 && highest > 0.24, "{heights:?}");
            }
        }
    }
    /// The covered passage under the blue-side base highland deck (about
    /// x = -12.5, y = 5.5 m): 0.65 m of clearance, so the chassis drives
    /// through underneath and also rests on the deck above. Checked in every
    /// installed field package.
    #[test]
    fn chassis_passes_under_and_rides_on_the_cad_highland_deck_when_present() {
        for (root, cad) in installed_cad_packages() {
            eprintln!("highland deck in {root}");
            chassis_passes_under_and_rides_on_the_highland_deck(&cad);
        }
    }
    fn chassis_passes_under_and_rides_on_the_highland_deck(cad: &CadAssets) {
        let terrain = load_terrain(cad).unwrap();
        let config = ChassisConfig::default();
        let field_with = |spawn: [f64; 3]| {
            let placement = chassis_placement(
                config.clone(),
                rm_simulator_world::RobotKind::Infantry,
                Some(&terrain),
                Team::Blue,
                spawn,
                0.0,
            );
            let mut field = Field::new(&rm_simulator_world::FieldConfig {
                floor_height_m: rm_simulator_server::layout::CATCH_FLOOR_M,
                chassis: vec![placement.clone()],
                ..Default::default()
            })
            .unwrap();
            add_terrain(&mut field, &terrain).unwrap();
            (field, placement.spawn.translation_m[2])
        };
        // Under the deck: the ground is the slab (height zero when flat, the
        // crowned V2.0.0 slab near -0.26 m) and the deck underside is 0.65 m
        // above it; drive 1 m forward beneath it.
        let (mut field, start_z) = field_with([-12.8, 5.5, 0.0]);
        let ground_z = start_z - config.rest_height_m();
        assert!((-0.28..0.02).contains(&ground_z), "{ground_z}");
        field.step(300).unwrap();
        for _ in 0..25 {
            let chassis = field.snapshot().chassis.remove(0);
            field
                .command_chassis(
                    0,
                    drive_command(1.0, 0.0, false, false, 0.0, 0.0, yaw_of(chassis.pose)),
                )
                .unwrap();
            field.step(100).unwrap();
        }
        let chassis = field.snapshot().chassis.remove(0);
        let [x, _, z] = chassis.pose.translation_m;
        assert!(x > -12.3, "{x}");
        assert!((z - start_z).abs() < 0.03, "{z} vs {start_z}");
        assert!(chassis.wheels.iter().all(|w| w.contact.is_some()));
        // On the deck top (0.9 m above the flat slab, about 0.46 m on the
        // crowned one): set down from above and stay there.
        let (mut field, deck_z) = field_with([-12.5, 5.5, 1.5]);
        assert!(deck_z > 0.55, "{deck_z}");
        field.step(1_000).unwrap();
        let chassis = field.snapshot().chassis.remove(0);
        // The deck top is not perfectly level, so the body may settle by up
        // to the suspension travel below the placement height.
        let drop_m = deck_z - chassis.pose.translation_m[2];
        assert!(
            drop_m > 0.0 && drop_m < config.suspension_rest_m + 0.01,
            "{:?} vs {deck_z}",
            chassis.pose
        );
        assert!(chassis.wheels.iter().all(|w| w.contact.is_some()));
    }
}
