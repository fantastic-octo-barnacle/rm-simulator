// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Static, CAD-free robot visual fixture. `cargo run -p rm-simulator-render
//! --example robots -- [output.png] [healthy|hit|defeated]`. HP is staged here.
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, save_to_disk},
};
use rm_simulator_render::{
    chassis::{ArmorOptics, ChassisVisualsPlugin, RobotModel},
    sync::*,
    *,
};

#[derive(Resource)]
struct Capture {
    path: Option<String>,
    frames: u32,
    target: Handle<Image>,
}
fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Robot equipment study".into(),
                resolution: (1440, 1000).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins((
            SceneSyncPlugin,
            ChassisVisualsPlugin {
                rendering: RenderingConfig::field(),
                armor: ArmorOptics {
                    housing_half_m: [0.0095, 0.0705, 0.0675],
                    face_size_m: [0.128, 0.113],
                    light_span_m: 0.130,
                    light_length_m: 0.056,
                },
            },
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, capture)
        .run();
}
fn pose(x: f64, y: f64, z: f64, yaw: f64) -> PoseFlu {
    PoseFlu {
        translation_m: [x, y, z],
        rotation_wxyz: [(yaw / 2.0).cos(), 0.0, 0.0, (yaw / 2.0).sin()],
    }
}
fn robot(model: RobotModel, y: f64, effect: &str) -> ChassisAppearance {
    let hero = model == RobotModel::Hero;
    let mecanum = matches!(model, RobotModel::Hero | RobotModel::Engineer);
    let balance = model == RobotModel::Balance;
    let (hx, hy, hz, radius, width, pivot, turret) = match model {
        RobotModel::Hero => (0.33, 0.28, 0.06, 0.1015, 0.065, 0.25, [0.09, 0.09, 0.075]),
        RobotModel::Engineer => (0.36, 0.29, 0.08, 0.1015, 0.065, 0.36, [0.13, 0.13, 0.10]),
        RobotModel::Drone => (0.40, 0.40, 0.065, 0.0, 0.0, -0.14, [0.035; 3]),
        RobotModel::Balance => (0.18, 0.24, 0.09, 0.10, 0.045, 0.20, [0.06; 3]),
        _ => (0.26, 0.26, 0.05, 0.0765, 0.04, 0.20, [0.06; 3]),
    };
    let z = if model == RobotModel::Drone {
        0.4
    } else {
        radius + if balance { 0.18 } else { 0.05 } + 0.025
    };
    let hubs = if model == RobotModel::Engineer {
        0.24
    } else {
        0.22
    };
    let armor = [
        (hx + 0.02, 0.0),
        (0.0, hy + 0.02),
        (-hx - 0.02, 0.0),
        (0.0, -hy - 0.02),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (x, y))| {
        let q = Quat::from_rotation_z(i as f32 * std::f32::consts::FRAC_PI_2)
            * Quat::from_rotation_y(-15_f32.to_radians());
        ArmorAppearance {
            local: PoseFlu {
                translation_m: [x, y, 0.015],
                rotation_wxyz: [q.w as f64, q.x as f64, q.y as f64, q.z as f64],
            },
            hit_flash: effect == "hit" && i == 0,
        }
    })
    .collect();
    ChassisAppearance {
        model,
        armor_pattern: match model {
            RobotModel::Hero => armor::ArmorPattern::One,
            RobotModel::Engineer => armor::ArmorPattern::Two,
            RobotModel::Sentry => armor::ArmorPattern::GuardSmall,
            _ => armor::ArmorPattern::Three,
        },
        id: model as u32,
        mecanum,
        hp_fraction: if hero { 0.6 } else { 1.0 },
        team: if hero {
            TeamColor::Red
        } else {
            TeamColor::Blue
        },
        pose: pose(0.0, y, z, 0.0),
        body_half_m: [hx as f32, hy as f32, hz as f32],
        wheels: (if model == RobotModel::Drone {
            vec![]
        } else if balance {
            vec![[0.0, 0.24], [0.0, -0.24]]
        } else if model == RobotModel::Engineer {
            vec![
                [hubs, 0.255],
                [-hubs, 0.255],
                [-hubs, -0.255],
                [hubs, -0.255],
            ]
        } else if mecanum {
            vec![[hubs, hubs], [-hubs, hubs], [-hubs, -hubs], [hubs, -hubs]]
        } else {
            vec![[0.245, 0.0], [0.0, 0.245], [-0.245, 0.0], [0.0, -0.245]]
        })
        .into_iter()
        .map(|[x, dy]| WheelAppearance {
            pose: pose(
                x,
                y + dy,
                radius,
                if mecanum {
                    0.0
                } else {
                    dy.atan2(x) - std::f64::consts::FRAC_PI_2
                },
            ),
            radius_m: radius as f32,
            width_m: width,
        })
        .collect(),
        armor,
        yaw_stage: pose(0.0, y, z + pivot, 0.0),
        turret: pose(0.0, y, z + pivot, 0.0),
        turret_half_m: turret,
        pivot_above_body_m: (pivot - hz) as f32,
        barrel_length_m: 0.35,
        defeated: effect == "defeated",
    }
}
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut scene: ResMut<SceneInput>,
) {
    let args: Vec<_> = std::env::args().collect();
    let effect = args.get(2).map_or("healthy", String::as_str);
    let all = args.get(4).is_some_and(|v| v == "all");
    let focused = args.get(4).and_then(|v| match v.as_str() {
        "omni" => Some(RobotModel::Omni),
        "sentry" => Some(RobotModel::Sentry),
        "balance" => Some(RobotModel::Balance),
        "engineer" => Some(RobotModel::Engineer),
        "drone" => Some(RobotModel::Drone),
        _ => None,
    });
    scene.0 = Some(SceneState {
        chassis: if let Some(model) = focused {
            vec![robot(model, 0.0, effect)]
        } else if all {
            [
                RobotModel::Omni,
                RobotModel::Sentry,
                RobotModel::Balance,
                RobotModel::Engineer,
                RobotModel::Hero,
            ]
            .into_iter()
            .enumerate()
            .map(|(i, model)| robot(model, -2.2 + i as f64 * 1.1, effect))
            .collect()
        } else {
            vec![
                robot(RobotModel::Omni, 0.48, effect),
                robot(RobotModel::Hero, -0.48, effect),
            ]
        },
        ..default()
    });
    let image = images.add(Image::new_target_texture(
        1440,
        1000,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    let mut camera = commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(
            if all {
                1.1
            } else if focused.is_some() {
                0.85
            } else {
                1.45
            },
            if all {
                2.1
            } else if focused.is_some() {
                0.80
            } else {
                1.05
            },
            if args.get(3).is_some_and(|v| v == "rear") {
                1.8
            } else {
                if all {
                    -4.5
                } else if focused.is_some() {
                    -1.1
                } else {
                    -1.8
                }
            },
        )
        .looking_at(Vec3::new(0.0, 0.25, 0.0), Vec3::Y),
    ));
    camera_appearance(&mut camera, RenderingConfig::field());
    if args.get(1).is_some() {
        camera.insert(bevy::camera::RenderTarget::Image(image.clone().into()));
    }
    let camera_id = camera.id();
    commands.insert_resource(Capture {
        path: args.get(1).cloned(),
        frames: 0,
        target: image,
    });
    commands.spawn((
        DirectionalLight {
            illuminance: 8500.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(2.0, 5.0, -3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 5000.0,
            ..default()
        },
        Transform::from_xyz(-3.0, 2.0, 1.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(200.0, 0.02, 200.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.22, 0.25, 0.30),
            perceptual_roughness: 0.85,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.015, 0.0),
    ));
    commands.spawn((UiTargetCamera(camera_id), Text::new(if let Some(model)=focused { format!("{model:?} / procedural model") } else if all { "OMNI INFANTRY     SENTRY     BALANCE INFANTRY     ENGINEER     HERO\nProcedural approximations. No STEP or URDF meshes.".into() } else { format!("RED: HERO / MECANUM     BLUE: INFANTRY / OMNI\nAM02 armor  /  LI01 HP bar  /  FI02 RFID  /  SM01 / SM11  /  VT03\nStatic visual study: {effect}. Hero HP staged at 60%.") }),TextFont {font_size:22.0.into(),..default()},Node {position_type:PositionType::Absolute,left:Val::Px(35.0),bottom:Val::Px(28.0),..default()}));
}
fn capture(mut commands: Commands, mut capture: ResMut<Capture>) {
    capture.frames += 1;
    if capture.frames == 300
        && let Some(path) = capture.path.take()
    {
        commands
            .spawn(Screenshot::image(capture.target.clone()))
            .observe(save_to_disk(path))
            .observe(
                |_: On<bevy::render::view::screenshot::ScreenshotCaptured>,
                 mut exit: MessageWriter<AppExit>| {
                    exit.write(AppExit::Success);
                },
            );
    }
}
