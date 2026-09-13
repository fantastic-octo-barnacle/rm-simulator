// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Conversions between the world's FLU poses, the renderer's `PoseFlu` and
//! Bevy transforms.
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use rm_simulator_render::{PoseFlu, apply_pose};
use rm_simulator_world::Pose;

/// The renderer's pose as the world's `Pose`. Both carry FLU metres and a wxyz
/// rotation, so no axis or sign changes here.
pub fn to_pose(pose: PoseFlu) -> Pose {
    Pose {
        translation_m: pose.translation_m,
        rotation_wxyz: pose.rotation_wxyz,
    }
}
/// The world's `Pose` as the renderer's `PoseFlu`, again without converting axes.
pub fn pose_flu(value: Pose) -> PoseFlu {
    PoseFlu {
        translation_m: value.translation_m,
        rotation_wxyz: value.rotation_wxyz,
    }
}
/// A world root pose as the renderer's transform.
pub fn transform_of(pose: &Pose) -> Transform {
    let mut transform = Transform::IDENTITY;
    apply_pose(&mut transform, pose_flu(*pose));
    transform
}
/// A world wxyz quaternion as Bevy's xyzw `DQuat`. The world orders the scalar
/// component first and Bevy orders it last.
pub fn dquat(rotation_wxyz: [f64; 4]) -> DQuat {
    let [w, x, y, z] = rotation_wxyz;
    DQuat::from_xyzw(x, y, z, w)
}
/// A Bevy `DQuat` as the world's wxyz quaternion array, the inverse of `dquat`.
pub fn wxyz(q: DQuat) -> [f64; 4] {
    [q.w, q.x, q.y, q.z]
}
/// The two stages of a gimbal bolted to the body, from the body pose and the
/// aim the pilot holds in the world: the yaw stage turns about the body's
/// own up axis and the pitch stage about the yaw stage's left axis, by the
/// angles that point the barrel (+x) along the aim. A two-axis mount cannot
/// roll, so on a tilted chassis the barrel rolls with the body while its
/// direction stays where the pilot points it.
pub fn gimbal_poses(body: PoseFlu, aim: PoseFlu) -> (PoseFlu, PoseFlu) {
    let body_rotation = dquat(body.rotation_wxyz);
    let direction = body_rotation.inverse() * (dquat(aim.rotation_wxyz) * DVec3::X);
    let yaw = direction.y.atan2(direction.x);
    let pitch = direction.z.clamp(-1.0, 1.0).asin();
    let yaw_stage = body_rotation * DQuat::from_rotation_z(yaw);
    let turret = yaw_stage * DQuat::from_rotation_y(-pitch);
    (
        PoseFlu {
            translation_m: aim.translation_m,
            rotation_wxyz: wxyz(yaw_stage.normalize()),
        },
        PoseFlu {
            translation_m: aim.translation_m,
            rotation_wxyz: wxyz(turret.normalize()),
        },
    )
}
