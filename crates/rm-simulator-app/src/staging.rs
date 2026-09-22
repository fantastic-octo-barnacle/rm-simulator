// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Connected deployment over the real field. The host owns validation and creates
//! the robot only after confirmation; this screen only selects a position.
use crate::{
    args::Args,
    controls::{Drive, Gun, PlayerCamera},
    loading::{LeaveRequest, Screen},
    session::Session,
};
use bevy::{
    camera::ScalingMode,
    feathers::{controls::FeathersButton, theme::ThemedText},
    prelude::*,
    ui_widgets::{Activate, ActivateOnPress},
};
use rm_simulator_render::{flu_position, flu_vector, sync::SceneSyncSet};
use rm_simulator_server::{
    deployment,
    protocol::{Chassis, Command, Robot, Role},
};
use rm_simulator_world::Team;

/// The requested defaults, retained while the connected staging screen is open.
#[derive(Resource)]
pub struct StagingConfig(pub Args);

/// Installs the connected setup screen and its scene presentation.
pub struct StagingPlugin;
impl Plugin for StagingPlugin {
    fn build(&self, app: &mut App) {
        app.init_gizmo_group::<DeploymentGizmos>()
            .add_systems(
                OnEnter(Screen::Staging),
                (enter, spawn_zones, spawn_side_highlights),
            )
            .add_systems(OnExit(Screen::Staging), exit)
            .add_systems(
                Update,
                (
                    poll,
                    pick_side,
                    input,
                    camera,
                    ui,
                    publish,
                    publish_preview,
                    outlines,
                    show_zones,
                    show_side_highlights,
                    hover_caption,
                )
                    .chain()
                    .before(SceneSyncSet)
                    .run_if(in_state(Screen::Staging)),
            );
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Side,
    Robot,
    Position,
}
#[derive(Clone, Copy)]
enum Action {
    Team(Team),
    Robot(Robot),
    Chassis(Chassis),
    Next,
    Back,
    Deploy,
    Watch,
    Leave,
}
#[derive(Resource)]
struct Stage {
    step: Step,
    team: Team,
    robot: Robot,
    chassis: Chassis,
    position_m: Option<[f64; 2]>,
    pending: Option<u64>,
    status: String,
    actions: Vec<Action>,
}
#[derive(Component)]
struct StageUi;
#[derive(Component)]
struct StageOwned;
#[derive(Component)]
struct Zone(Team);
#[derive(Component)]
struct SideHighlight(Team);
#[derive(Component)]
struct SideHint;
#[derive(Resource, Default)]
struct SideHover(Option<Team>);
#[derive(Default, Reflect, bevy::gizmos::config::GizmoConfigGroup)]
struct DeploymentGizmos;

fn enter(
    mut commands: Commands,
    config: Res<StagingConfig>,
    mut hud: Query<&mut Node, With<crate::hud::HudRoot>>,
) {
    commands.insert_resource(SideHover::default());
    commands.insert_resource(Stage {
        step: Step::Side,
        team: config.0.team.into(),
        robot: config.0.robot,
        chassis: if config.0.chassis.config(config.0.robot).is_ok() {
            config.0.chassis
        } else {
            Chassis::Auto
        },
        position_m: None,
        pending: None,
        status: String::new(),
        actions: vec![],
    });
    for mut node in &mut hud {
        node.display = Display::None;
    }
}
fn exit(
    mut commands: Commands,
    roots: Query<Entity, With<StageOwned>>,
    mut cameras: Query<&mut Projection, With<PlayerCamera>>,
    mut hud: Query<&mut Node, With<crate::hud::HudRoot>>,
) {
    for root in &roots {
        commands.entity(root).despawn();
    }
    commands.remove_resource::<Stage>();
    commands.remove_resource::<SideHover>();
    for mut projection in &mut cameras {
        *projection = Projection::Perspective(PerspectiveProjection {
            fov: 70.0_f32.to_radians(),
            near: 0.01,
            far: 300.0,
            ..default()
        });
    }
    for mut node in &mut hud {
        node.display = Display::Flex;
    }
}
fn poll(
    mut commands: Commands,
    mut session: ResMut<Session>,
    mut stage: ResMut<Stage>,
    config: Res<StagingConfig>,
    mut next: ResMut<NextState<Screen>>,
) {
    if let Err(error) = session.poll() {
        commands.insert_resource(LeaveRequest(Some(error)));
        return;
    }
    if session.disconnected() {
        commands.insert_resource(LeaveRequest(Some("Disconnected from host".into())));
        return;
    }
    if let Some(previous) = stage.pending {
        if session.commands_confirmed()
            && let Some(id) = session.chassis_id
            && session.own_chassis().is_some()
        {
            if let Some(choice) = config.0.performance {
                session.apply(Command::SetPerformance {
                    chassis: id,
                    performance: choice.performance(),
                });
            }
            let chassis = session.own_chassis().expect("confirmed chassis");
            commands.insert_resource(crate::controls::Player::at(
                flu_position(chassis.turret.translation_m),
                rm_simulator_world::chassis::yaw_of(chassis.pose) as f32,
                0.0,
            ));
            remember(&config.0, &stage, false);
            commands.insert_resource(Drive::new(id, config.0.third_person));
            commands.insert_resource(Gun::new(session.weapon.shot, session.weapon.interval_ns));
            next.set(Screen::InMatch);
        } else if session.commands_confirmed() && session.rejection_count > previous {
            stage.pending = None;
            stage.status = session
                .last_rejection
                .clone()
                .unwrap_or_else(|| "Deployment refused. Choose another position.".into());
        }
    }
}
fn choose_robot(stage: &mut Stage, robot: Robot) {
    stage.robot = robot;
    if stage.chassis.config(robot).is_err() {
        stage.chassis = Chassis::Auto;
    }
    stage.position_m = None;
}
#[allow(clippy::too_many_arguments)]
fn input(
    mut commands: Commands,
    mut stage: ResMut<Stage>,
    mut session: ResMut<Session>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<PlayerCamera>>,
    mut next: ResMut<NextState<Screen>>,
) {
    if stage.pending.is_some() {
        if keys.just_pressed(KeyCode::Escape)
            || stage
                .actions
                .iter()
                .any(|action| matches!(action, Action::Leave))
        {
            commands.insert_resource(LeaveRequest(None));
        }
        return;
    }
    let mut actions = if stage.actions.is_empty() {
        Vec::new()
    } else {
        std::mem::take(&mut stage.actions)
    };
    if keys.just_pressed(KeyCode::Escape) {
        actions.push(Action::Back);
    }
    for action in actions {
        stage.status.clear();
        match action {
            Action::Team(team) => {
                stage.team = team;
                stage.position_m = None;
                stage.step = Step::Robot;
            }
            Action::Robot(robot) => choose_robot(&mut stage, robot),
            Action::Chassis(chassis) => stage.chassis = chassis,
            Action::Next => {
                stage.step = Step::Position;
                stage.position_m = None;
            }
            Action::Back => {
                stage.step = match stage.step {
                    Step::Side => {
                        commands.insert_resource(LeaveRequest(None));
                        return;
                    }
                    Step::Robot => Step::Side,
                    Step::Position => Step::Robot,
                }
            }
            Action::Watch => {
                next.set(Screen::InMatch);
                return;
            }
            Action::Leave => {
                commands.insert_resource(LeaveRequest(None));
                return;
            }
            Action::Deploy => {
                if let Some(position_m) = stage.position_m {
                    let before = session.rejection_count;
                    session.apply(Command::Deploy {
                        team: stage.team,
                        robot: stage.robot,
                        chassis: stage.chassis,
                        position_m,
                    });
                    stage.pending = Some(before);
                    stage.status = "Waiting for host…".into();
                    return;
                }
            }
        }
    }
    if stage.step != Step::Position {
        return;
    }
    if stage.robot == Robot::Drone {
        let (slot, _) = rm_simulator_server::layout::drone_slot(stage.team, 0);
        if stage.position_m.is_none() {
            stage.position_m = Some([slot[0], slot[1]]);
        }
        return;
    }
    let mut point = None;
    if mouse.just_pressed(MouseButton::Left)
        && let Ok(window) = windows.single()
        && let Some(cursor) = window.cursor_position()
        && cursor.x < window.width() - 340.0
        && cursor.y > 88.0
        && let Ok((camera, transform)) = cameras.single()
        && let Ok(ray) = camera.viewport_to_world(transform, cursor)
        && let Some(distance) = ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
    {
        let p = flu_vector(ray.get_point(distance));
        point = Some([p[0], p[1]]);
    }
    let dx = i32::from(keys.just_pressed(KeyCode::ArrowUp))
        - i32::from(keys.just_pressed(KeyCode::ArrowDown));
    let dy = i32::from(keys.just_pressed(KeyCode::ArrowLeft))
        - i32::from(keys.just_pressed(KeyCode::ArrowRight));
    if dx != 0 || dy != 0 {
        let mut p = stage
            .position_m
            .unwrap_or(deployment::for_team(stage.team, [10.4, 0.0]));
        p[0] += f64::from(dx) * 0.1;
        p[1] += f64::from(dy) * 0.1;
        point = Some(p);
    }
    if let Some(p) = point {
        if deployment::contains(stage.team, p) {
            stage.position_m = Some(p);
            stage.status.clear();
        } else {
            stage.status = "Choose a point inside the outlined ring, outside the base.".into();
        }
    }
    if keys.just_pressed(KeyCode::Enter) && stage.position_m.is_some() {
        stage.actions.push(Action::Deploy);
    }
}
fn camera(
    stage: Res<Stage>,
    windows: Query<&Window>,
    bounds: Res<crate::scene::ArenaBounds>,
    mut cameras: Query<(&mut Transform, &mut Projection), With<PlayerCamera>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let Ok((mut transform, mut projection)) = cameras.single_mut() else {
        return;
    };
    let close = stage.step != Step::Side;
    let center = if close && stage.robot == Robot::Drone {
        let (p, _) = rm_simulator_server::layout::drone_slot(stage.team, 0);
        [p[0], p[1]]
    } else if close {
        deployment::for_team(stage.team, deployment::RED_BASE_M)
    } else {
        [0.0, 0.0]
    };
    let target = flu_position([center[0], center[1], 0.0]);
    let (offset, up, height) = if stage.step == Step::Position {
        (Vec3::Y * 20.0, Vec3::NEG_Z, 6.5)
    } else if stage.step == Step::Robot {
        (Vec3::new(6.0, 8.0, 0.0), Vec3::Y, 6.5)
    } else {
        {
            let [low, high] = bounds.0;
            let height = ((high[0] - low[0]) as f32 + 2.0) * window.height()
                / (window.width() - 340.0).max(160.0);
            // Look across the short axis: the arena's long edge stays horizontal.
            (Vec3::new(24.0, 28.0, 0.0), Vec3::Y, height.max(18.0))
        }
    };
    let mut view = Transform::from_translation(target + offset).looking_at(target, up);
    // Place the field in the available area to the left of the form.
    let aspect = window.width() / window.height().max(1.0);
    view.translation +=
        view.rotation * Vec3::X * (height * aspect * 170.0 / window.width().max(1.0));
    *transform = view;
    *projection = Projection::Orthographic(OrthographicProjection {
        scaling_mode: ScalingMode::FixedVertical {
            viewport_height: height,
        },
        near: 0.01,
        far: 300.0,
        ..OrthographicProjection::default_3d()
    });
}
fn label(commands: &mut Commands, parent: Entity, text: impl Into<String>, size: f32) {
    commands.spawn((
        ChildOf(parent),
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(size),
            ..default()
        },
        TextColor(Color::srgb(0.88, 0.92, 0.96)),
    ));
}
fn button(commands: &mut Commands, parent: Entity, caption: impl Into<String>, action: Action) {
    let caption = caption.into();
    let accessible = caption.clone();
    commands
        .spawn_scene(bsn! {
            @FeathersButton { @caption: bsn! { Text(caption) ThemedText } }
            ActivateOnPress
            AccessibleLabel(accessible)
            Node { min_height: px(34), flex_shrink: 0.0 }
            on(move |_: On<Activate>, mut stage: ResMut<Stage>| { stage.actions.push(action); })
        })
        .insert(ChildOf(parent));
}
fn ui(
    mut commands: Commands,
    stage: Res<Stage>,
    session: Res<Session>,
    roots: Query<Entity, With<StageUi>>,
) {
    if !stage.is_changed() {
        return;
    }
    for root in &roots {
        commands.entity(root).despawn();
    }
    let header = commands
        .spawn((
            StageUi,
            StageOwned,
            Node {
                position_type: PositionType::Absolute,
                left: px(24),
                top: px(22),
                padding: UiRect::all(px(14)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.035, 0.05, 0.075, 0.94)),
        ))
        .id();
    label(&mut commands, header, "DEPLOYMENT  /  CONNECTED", 22.0);
    let panel = commands
        .spawn((
            StageUi,
            StageOwned,
            bevy::ui_widgets::ScrollArea,
            Node {
                position_type: PositionType::Absolute,
                right: px(16),
                top: px(16),
                bottom: px(16),
                width: px(308),
                padding: UiRect::all(px(18)),
                flex_direction: FlexDirection::Column,
                row_gap: px(9),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            BackgroundColor(Color::srgba(0.035, 0.05, 0.075, 0.97)),
        ))
        .id();
    let phase = match stage.step {
        Step::Side => "01  CHOOSE SIDE",
        Step::Robot => "02  CHOOSE ROBOT",
        Step::Position => "03  START POSITION",
    };
    label(&mut commands, panel, phase, 22.0);
    if session.role == Role::Referee {
        label(&mut commands, panel, "Referee / no robot", 16.0);
        button(&mut commands, panel, "Enter as referee", Action::Watch);
    } else {
        match stage.step {
            Step::Side => {
                label(
                    &mut commands,
                    panel,
                    "Hover over either half of the arena, then click to choose your side.",
                    16.0,
                );
                commands.spawn((
                    ChildOf(panel),
                    SideHint,
                    Node {
                        min_height: px(48),
                        flex_shrink: 0.0,
                        ..default()
                    },
                    Text::new("CHOOSE A HALF"),
                    TextFont {
                        font_size: FontSize::Px(20.0),
                        ..default()
                    },
                    TextColor(Color::WHITE),
                ));
                label(
                    &mut commands,
                    panel,
                    "Keyboard: 1 for the left half, 2 for the right half.",
                    14.0,
                );
                button(&mut commands, panel, "Watch as spectator", Action::Watch);
            }
            Step::Robot => {
                label(
                    &mut commands,
                    panel,
                    format!("{} side", stage.team.name()),
                    16.0,
                );
                for robot in Robot::ALL {
                    button(
                        &mut commands,
                        panel,
                        format!(
                            "{} {}",
                            if robot == stage.robot { "[x]" } else { "[ ]" },
                            robot.name()
                        ),
                        Action::Robot(robot),
                    );
                }
                label(&mut commands, panel, "Drivetrain", 14.0);
                for &chassis in stage.robot.chassis_choices() {
                    button(
                        &mut commands,
                        panel,
                        format!(
                            "{} {}",
                            if stage.chassis.resolved(stage.robot) == chassis {
                                "[x]"
                            } else {
                                "[ ]"
                            },
                            chassis.name()
                        ),
                        Action::Chassis(chassis),
                    );
                }
                button(
                    &mut commands,
                    panel,
                    "Choose start position →",
                    Action::Next,
                );
            }
            Step::Position => {
                label(
                    &mut commands,
                    panel,
                    format!(
                        "{} / {}\n{}",
                        stage.team.name(),
                        stage.robot.name(),
                        stage.chassis.resolved(stage.robot).name()
                    ),
                    18.0,
                );
                label(
                    &mut commands,
                    panel,
                    if stage.robot == Robot::Drone {
                        "Drone uses its designated aerial starting pad and flight boundary."
                    } else {
                        "Click anywhere in the outlined ring around your base. The base itself is excluded.\n\nArrow keys adjust by 0.1 m. Enter deploys."
                    },
                    16.0,
                );
                if let Some(p) = stage.position_m {
                    label(
                        &mut commands,
                        panel,
                        format!("START  {:.2}, {:.2} m", p[0], p[1]),
                        16.0,
                    );
                    if stage.pending.is_none() {
                        button(&mut commands, panel, "Deploy robot →", Action::Deploy);
                    }
                } else {
                    label(&mut commands, panel, "Select your starting position", 16.0);
                }
            }
        }
    }
    if !stage.status.is_empty() {
        label(&mut commands, panel, &stage.status, 15.0);
    }
    if stage.pending.is_none() {
        button(&mut commands, panel, "← Back", Action::Back);
    }
    button(&mut commands, panel, "Leave lobby", Action::Leave);
}
// A separate system identity keeps the gameplay schedule's ordering unambiguous.
fn publish(
    session: ResMut<Session>,
    drive: Option<Res<Drive>>,
    input: ResMut<rm_simulator_render::sync::SceneInput>,
    capture: Option<Res<crate::screenshot::ScreenshotRequest>>,
) {
    crate::scene::publish_scene(session, drive, input, capture);
}
fn publish_preview(
    mut input: ResMut<rm_simulator_render::sync::SceneInput>,
    stage: Res<Stage>,
    session: Res<Session>,
) {
    if stage.step != Step::Side
        && let Some(point) = stage.position_m.or_else(|| {
            (stage.step == Step::Robot).then(|| {
                if stage.robot == Robot::Drone {
                    let (p, _) = rm_simulator_server::layout::drone_slot(stage.team, 0);
                    [p[0], p[1]]
                } else {
                    deployment::for_team(stage.team, [10.4, 0.0])
                }
            })
        })
        && let Some(ground_m) = session.deployment_ground_height(point)
        && let Some(scene) = &mut input.0
    {
        scene.chassis.push(preview(&stage, point, ground_m));
    }
}

/// A drawing-only pose built from the same dimensions as the chosen chassis.
/// It never creates a second physics world or enters a protocol command.
fn preview(
    stage: &Stage,
    point: [f64; 2],
    ground_m: f64,
) -> rm_simulator_render::sync::ChassisAppearance {
    use rm_simulator_world::{ChassisSnapshot, Pose, WheelSnapshot};
    let config = stage
        .chassis
        .config(stage.robot)
        .expect("supported chassis");
    let yaw = if stage.team == Team::Red {
        std::f64::consts::PI
    } else {
        0.0
    };
    let pose = Pose::yawed(
        [
            point[0],
            point[1],
            if stage.robot == Robot::Drone {
                rm_simulator_server::layout::drone_slot(stage.team, 0).0[2]
            } else {
                ground_m + config.rest_height_m() + 0.03
            },
        ],
        yaw,
    );
    let transform = |local: [f64; 3]| {
        let rotated =
            crate::frames::dquat(pose.rotation_wxyz) * bevy::math::DVec3::from_array(local);
        (bevy::math::DVec3::from_array(pose.translation_m) + rotated).to_array()
    };
    let snapshot = ChassisSnapshot {
        // This id is visual only, and cannot be sent back to the host.
        id: u32::MAX,
        placement_revision: 0,
        team: stage.team,
        pose,
        turret: Pose::yawed(transform(config.turret_center_m), yaw),
        velocity_m_s: [0.0; 3],
        angular_velocity_rad_s: [0.0; 3],
        command: default(),
        jump_held: false,
        held_aim_rad: [yaw, 0.0],
        gimbal_velocity_rad_s: [0.0; 2],
        defeated: false,
        wheels: config
            .wheel_hubs_m
            .iter()
            .map(|hub| WheelSnapshot {
                hub_m: transform([hub[0], hub[1], -config.hub_drop_m]),
                ..default()
            })
            .collect(),
        config,
    };
    crate::scene::chassis_appearance(&snapshot, &[], Some(stage.robot))
}

fn remember(args: &Args, stage: &Stage, spectate: bool) {
    if cfg!(test) {
        return;
    }
    if let Some(path) = crate::title::remembered_path() {
        let mut fields =
            crate::title::TitleFields::initial(args, crate::title::load_remembered(&path));
        fields.set_seat(if spectate {
            crate::title::Seat::Spectator { slot: 0 }
        } else {
            crate::title::Seat::Pilot {
                team: stage.team,
                robot: stage.robot,
            }
        });
        fields.chassis = stage.chassis;
        if let Err(error) = crate::title::save_remembered(&path, &fields) {
            warn!("saving deployment preferences: {error}");
        }
    }
}

fn outlines(stage: Res<Stage>, mut gizmos: Gizmos<DeploymentGizmos>) {
    let teams: &[Team] = if stage.step == Step::Position {
        std::slice::from_ref(&stage.team)
    } else {
        &[Team::Red, Team::Blue]
    };
    for &team in teams {
        let color = if team == Team::Red {
            Color::srgb(1.0, 0.25, 0.24)
        } else {
            Color::srgb(0.2, 0.6, 1.0)
        };
        for boundary in [&deployment::RED_OUTER_M, &deployment::RED_INNER_M] {
            let points: Vec<Vec3> = boundary
                .iter()
                .map(|&p| {
                    let p = deployment::for_team(team, p);
                    flu_position([p[0], p[1], 0.35])
                })
                .collect();
            gizmos.linestrip(points.iter().copied().chain(points.first().copied()), color);
        }
    }
    if let Some(p) = stage.position_m {
        let center = flu_position([p[0], p[1], 0.4]);
        gizmos.circle(
            Isometry3d::new(center, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            0.3,
            Color::srgb(0.5, 1.0, 0.75),
        );
        gizmos.line(center + Vec3::Y * 0.1, center + Vec3::Y * 1.2, Color::WHITE);
    }
}

fn spawn_zones(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut gizmos: ResMut<bevy::gizmos::config::GizmoConfigStore>,
    session: Res<Session>,
) {
    let (config, _) = gizmos.config_mut::<DeploymentGizmos>();
    config.line.width = 3.0;
    config.depth_bias = -0.01;
    for team in [Team::Red, Team::Blue] {
        let vertices = deployment::RED_OUTER_M
            .into_iter()
            .chain(deployment::RED_INNER_M)
            .map(|p| {
                let p = deployment::for_team(team, p);
                session
                    .deployment_ground_height(p)
                    .map(|z| flu_position([p[0], p[1], z + 0.04]).to_array())
            })
            .collect::<Option<Vec<_>>>();
        let Some(vertices) = vertices else {
            continue;
        };
        let indices = (0..6_u32)
            .flat_map(|i| {
                let next = (i + 1) % 6;
                [i, next, i + 6, next, next + 6, i + 6]
            })
            .collect();
        let mesh = Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vertices)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 12])
        .with_inserted_indices(bevy::mesh::Indices::U32(indices));
        let color = if team == Team::Red {
            Color::srgba(1.0, 0.18, 0.12, 0.22)
        } else {
            Color::srgba(0.1, 0.5, 1.0, 0.22)
        };
        commands.spawn((
            Zone(team),
            StageOwned,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: color,
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                cull_mode: None,
                ..default()
            })),
            Transform::default(),
        ));
    }
}
fn show_zones(stage: Res<Stage>, mut zones: Query<(&Zone, &mut Visibility)>) {
    for (zone, mut visible) in &mut zones {
        *visible =
            if stage.step == Step::Side || (zone.0 == stage.team && stage.robot != Robot::Drone) {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
    }
}

/// World-plane picking is independent of the decorative scenery's height.
fn team_at(point: [f64; 2], bounds: [[f64; 3]; 2]) -> Option<Team> {
    if !point.into_iter().all(f64::is_finite)
        || (0..2).any(|axis| point[axis] < bounds[0][axis] || point[axis] > bounds[1][axis])
    {
        return None;
    }
    Some(rm_simulator_server::layout::side_team(point[0]))
}
#[allow(clippy::too_many_arguments)]
fn pick_side(
    mut stage: ResMut<Stage>,
    mut hover: ResMut<SideHover>,
    session: Res<Session>,
    bounds: Res<crate::scene::ArenaBounds>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<PlayerCamera>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let active = stage.step == Step::Side && session.role != Role::Referee;
    let selected = if active {
        windows.single().ok().and_then(|window| {
            let cursor = window.cursor_position()?;
            if cursor.x >= window.width() - 340.0 || cursor.y <= 88.0 {
                return None;
            }
            let (camera, transform) = cameras.single().ok()?;
            let ray = camera.viewport_to_world(transform, cursor).ok()?;
            let distance = ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))?;
            let p = flu_vector(ray.get_point(distance));
            team_at([p[0], p[1]], bounds.0)
        })
    } else {
        None
    };
    if hover.0 != selected {
        hover.0 = selected;
    }
    if active {
        let choice = if mouse.just_pressed(MouseButton::Left) {
            selected
        } else if keys.just_pressed(KeyCode::Digit1) {
            Some(Team::Blue)
        } else if keys.just_pressed(KeyCode::Digit2) {
            Some(Team::Red)
        } else {
            None
        };
        if let Some(team) = choice {
            stage.actions.push(Action::Team(team));
        }
    }
}
fn hover_caption(
    hover: Res<SideHover>,
    mut hints: Query<(&mut Text, &mut TextColor), With<SideHint>>,
) {
    for (mut text, mut color) in &mut hints {
        let (caption, tint) = match hover.0 {
            Some(Team::Red) => ("RED SIDE / CLICK TO JOIN", Color::srgb(1.0, 0.4, 0.35)),
            Some(Team::Blue) => ("BLUE SIDE / CLICK TO JOIN", Color::srgb(0.3, 0.65, 1.0)),
            None => ("CHOOSE A HALF", Color::srgb(0.6, 0.7, 0.8)),
        };
        if text.0 != caption {
            text.0 = caption.into();
            color.0 = tint;
        }
    }
}
fn spawn_side_highlights(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    bounds: Res<crate::scene::ArenaBounds>,
    session: Res<Session>,
) {
    let [low, high] = bounds.0;
    for team in [Team::Red, Team::Blue] {
        let (min_x, max_x) = if team == Team::Red {
            (0.0, high[0])
        } else {
            (low[0], 0.0)
        };
        // Sample the verified floor so the tint follows decks and sloping slabs.
        let columns = 24_u32;
        let rows = 24_u32;
        let mut vertices = Vec::new();
        let mut valid = Vec::new();
        for x in 0..=columns {
            for y in 0..=rows {
                let p = [
                    min_x + (max_x - min_x) * f64::from(x) / f64::from(columns),
                    low[1] + (high[1] - low[1]) * f64::from(y) / f64::from(rows),
                ];
                let height = session.deployment_ground_height(p);
                valid.push(height.is_some());
                // Missing vertices are never referenced by a triangle.
                vertices.push(height.map_or([0.0; 3], |z| {
                    flu_position([p[0], p[1], z + 0.025]).to_array()
                }));
            }
        }
        let mut indices = Vec::new();
        for x in 0..columns {
            for y in 0..rows {
                let a = x * (rows + 1) + y;
                let b = a + rows + 1;
                for triangle in [[a, b, a + 1], [b, b + 1, a + 1]] {
                    if triangle.iter().all(|&i| valid[i as usize]) {
                        indices.extend(triangle);
                    }
                }
            }
        }
        let mesh = Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vertices)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; valid.len()])
        .with_inserted_indices(bevy::mesh::Indices::U32(indices));
        let color = if team == Team::Red {
            Color::srgba(1.0, 0.12, 0.08, 0.24)
        } else {
            Color::srgba(0.05, 0.4, 1.0, 0.28)
        };
        commands.spawn((
            StageOwned,
            SideHighlight(team),
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: color,
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                cull_mode: None,
                ..default()
            })),
            Transform::default(),
            Visibility::Hidden,
            bevy::light::NotShadowCaster,
            bevy::light::NotShadowReceiver,
        ));
    }
}
fn show_side_highlights(
    stage: Res<Stage>,
    hover: Res<SideHover>,
    mut highlights: Query<(&SideHighlight, &mut Visibility)>,
) {
    for (half, mut visibility) in &mut highlights {
        let desired = if stage.step == Step::Side && hover.0 == Some(half.0) {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if *visibility != desired {
            *visibility = desired;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arena_picking_distinguishes_halves_and_rejects_background() {
        let bounds = [[-14.0, -7.5, 0.0], [14.0, 7.5, 1.0]];
        assert_eq!(team_at([-8.0, 3.0], bounds), Some(Team::Blue));
        assert_eq!(team_at([8.0, -3.0], bounds), Some(Team::Red));
        assert_eq!(team_at([15.0, 0.0], bounds), None);
        assert_eq!(team_at([0.0, 8.0], bounds), None);
        assert_eq!(team_at([f64::NAN, 0.0], bounds), None);
    }

    #[test]
    fn confirmed_deployment_hands_off_to_driving_on_the_same_session() {
        use clap::Parser;
        let args = Args::try_parse_from([
            "rm-simulator",
            "--staging",
            "--start-paused",
            "--no-field-collision",
            "--no-rune",
            "--no-referee",
        ])
        .unwrap();
        let mut opened = Session::open(
            &args,
            &crate::session::test_cad_assets(),
            [9.0, 0.0, 1.0],
            180.0,
            |_, _| {},
        )
        .unwrap();
        opened.session.ready();
        assert_eq!(opened.session.role, Role::Spectator);
        assert!(opened.session.chassis_id.is_none());
        let client = opened.session.client_id;
        let before = opened.session.rejection_count;
        opened.session.apply(Command::Deploy {
            team: Team::Blue,
            robot: Robot::Infantry4,
            chassis: Chassis::Balance,
            position_m: [-10.4, 0.0],
        });
        let mut app = App::new();
        app.insert_resource(opened.session)
            .insert_resource(StagingConfig(args))
            .insert_resource(Stage {
                step: Step::Position,
                team: Team::Blue,
                robot: Robot::Infantry4,
                chassis: Chassis::Balance,
                position_m: Some([-10.4, 0.0]),
                pending: Some(before),
                status: String::new(),
                actions: vec![],
            })
            .init_resource::<NextState<Screen>>()
            .add_systems(Update, poll);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !app.world().contains_resource::<Drive>() {
            assert!(
                std::time::Instant::now() < deadline,
                "deployment handoff timed out"
            );
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let session = app.world().resource::<Session>();
        assert_eq!(session.client_id, client);
        assert_eq!(session.team, Team::Blue);
        assert_eq!(session.role, Role::Pilot);
        assert!(session.own_chassis().unwrap().config.balance_assist);
        assert_eq!(session.snapshot.chassis.len(), 1);
        assert_eq!(
            app.world().resource::<crate::controls::Player>().yaw_rad,
            0.0
        );
        assert!(matches!(
            app.world().resource::<NextState<Screen>>(),
            NextState::Pending(Screen::InMatch)
        ));
    }

    #[test]
    fn changing_robot_clears_position_and_resolves_unsupported_drivetrain() {
        let mut stage = Stage {
            step: Step::Robot,
            team: Team::Red,
            robot: Robot::Infantry3,
            chassis: Chassis::Balance,
            position_m: Some([10.4, 0.0]),
            pending: None,
            status: String::new(),
            actions: vec![],
        };
        choose_robot(&mut stage, Robot::Hero);
        assert_eq!(stage.chassis.resolved(stage.robot), Chassis::Mecanum);
        assert_eq!(stage.position_m, None);
    }
}
