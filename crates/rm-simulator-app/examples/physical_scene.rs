// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Optional Bevy adapter for the physical library. No Field, referee, CAD or server API.
//! Advance 16 explicit milliseconds per rendered frame; this is not real-time pacing.
use bevy::prelude::*;
use rm_simulator_physics::{
    ArmorTarget, Caliber, Pose, Shot, TICK_NS, TargetFace, TargetFrames, WorldPhysics,
    motion::RotorMotion,
};
use rm_simulator_render::{
    PoseFlu, RenderingConfig, TeamColor,
    armor::{ArmorAtlas, DiffuserProfile},
    outpost,
    projectile::ProjectileVisualsPlugin,
    sync::*,
};

#[derive(Resource)]
struct Demo {
    physics: WorldPhysics,
    targets: TargetFrames,
    rotor: RotorMotion,
    tick: u64,
}
fn faces(rotor: &RotorMotion, time_ns: u64) -> Vec<TargetFace> {
    rotor
        .armor_poses(time_ns)
        .into_iter()
        .enumerate()
        .map(|(face, pose)| TargetFace {
            target: ArmorTarget::Outpost {
                outpost: 0,
                face: face as u32,
            },
            pose,
        })
        .collect()
}
fn pose(pose: Pose) -> PoseFlu {
    PoseFlu {
        translation_m: pose.translation_m,
        rotation_wxyz: pose.rotation_wxyz,
    }
}
fn main() {
    let rotor = RotorMotion {
        origin: Pose::default(),
        pivot_cad_m: rm_simulator_physics::motion::outpost::PIVOT_CAD_M,
        speed_rad_s: 0.4,
        stopped_at_ns: None,
    };
    let targets = faces(&rotor, 0);
    App::new()
        .add_plugins(DefaultPlugins)
        .init_resource::<ArmorAtlas>()
        .init_resource::<DiffuserProfile>()
        .insert_resource(Demo {
            physics: WorldPhysics::new(&targets, 0.0),
            targets: TargetFrames::new(targets),
            rotor,
            tick: 0,
        })
        .add_plugins((
            SceneSyncPlugin,
            ProjectileVisualsPlugin {
                rendering: RenderingConfig::default(),
            },
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, advance.before(SceneSyncSet))
        .run();
}
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    atlas: Res<ArmorAtlas>,
    diffuser: Res<DiffuserProfile>,
) {
    use rm_simulator_physics::motion::outpost as geometry;
    outpost::spawn(
        &mut commands,
        &mut meshes,
        &mut materials,
        0,
        &outpost::OutpostOptics {
            boxes: geometry::HOUSING_BOXES
                .map(|(offset, size)| (offset.map(|v| v as f32), size.map(|v| v as f32)))
                .to_vec(),
            face_size_m: [
                geometry::FACE_WIDTH_M as f32,
                geometry::FACE_HEIGHT_M as f32,
            ],
            light_span_m: geometry::LIGHT_SPAN_M as f32,
            light_length_m: geometry::LIGHT_LENGTH_M as f32,
            rendering: RenderingConfig::default(),
            emission_profile: diffuser.0.clone(),
        },
        &atlas,
        TeamColor::Red,
    );
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(2.0, 2.0, 3.0).looking_at(Vec3::Y, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.7, -0.5, 0.0)),
    ));
}
fn advance(mut demo: ResMut<Demo>, mut input: ResMut<SceneInput>) {
    let Demo {
        physics,
        targets,
        rotor,
        tick,
    } = &mut *demo;
    for _ in 0..16 {
        let time_ns = *tick * TICK_NS;
        if (*tick).is_multiple_of(200) {
            physics
                .fire(
                    time_ns,
                    Pose::at([-2.0, 0.0, 1.025]),
                    Shot::at_limit(Caliber::Mm17),
                    None,
                )
                .unwrap();
        }
        targets
            .begin(|poses| *poses = faces(rotor, time_ns))
            .unwrap();
        targets
            .update(|poses| *poses = faces(rotor, time_ns + TICK_NS))
            .unwrap();
        // Raw contacts are available here for a consumer's own scoring or sensors.
        physics.step(time_ns, targets).unwrap();
        *tick += 1;
    }
    let time_ns = *tick * TICK_NS;
    input.0 = Some(SceneState {
        source: SourceTime {
            tick: *tick,
            time_ns,
        },
        outposts: vec![OutpostAppearance {
            disabled: false,
            origin: pose(rotor.origin),
            angle_rad: rotor.angle_at(time_ns),
            armors: rotor.armor_poses(time_ns).map(pose),
            hit_flash: [false; 3],
        }],
        projectiles: physics
            .snapshot()
            .iter()
            .map(|p| ProjectileAppearance {
                position_m: p.position_m,
                radius_m: p.caliber.diameter_m() as f32 / 2.0,
            })
            .collect(),
        ..default()
    });
}
