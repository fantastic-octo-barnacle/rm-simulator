// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Just enough quaternion and matrix arithmetic to place CAD assets without
//! a linear-algebra crate. Quaternions are `[w, x, y, z]`, vectors FLU
//! metres unless stated otherwise.
use rm_simulator_world::Pose;

/// The renderer's axis change from FLU (x forward, y left, z up) to its own
/// right/up/back frame, as a rotation: `x -> -z`, `y -> -x`, `z -> y`.
/// Manifest placements rotate an asset's glTF axes directly, so an asset's
/// world pose carries this change composed on the right (see
/// [`gltf_root_pose`]).
pub const RENDER_BASIS_WXYZ: [f64; 4] = [0.5, -0.5, 0.5, 0.5];

/// Product `a * b` in wxyz order: rotate by `b` first, then by `a`. Both inputs
/// are unit quaternions when they describe rotations.
pub fn quat_multiply(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let [aw, ax, ay, az] = a;
    let [bw, bx, by, bz] = b;
    [
        aw * bw - ax * bx - ay * by - az * bz,
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
    ]
}
/// Inverse of a unit quaternion: the same scalar part with the vector part
/// negated, so `quat_multiply(q, quat_conjugate(q))` is the identity rotation.
pub fn quat_conjugate(q: [f64; 4]) -> [f64; 4] {
    [q[0], -q[1], -q[2], -q[3]]
}
/// `q` scaled to unit length. Dividing the zero quaternion yields `NaN`.
pub fn quat_normalize(q: [f64; 4]) -> [f64; 4] {
    let n = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    q.map(|v| v / n)
}
/// Euclidean length of a wxyz quaternion, one for a unit rotation.
pub fn quat_length(q: [f64; 4]) -> f64 {
    q.iter().map(|v| v * v).sum::<f64>().sqrt()
}
/// Rotation of `angle_rad` about a unit `axis`.
pub fn quat_axis_angle(axis: [f64; 3], angle_rad: f64) -> [f64; 4] {
    let (s, c) = (angle_rad / 2.0).sin_cos();
    [c, axis[0] * s, axis[1] * s, axis[2] * s]
}
/// Rotates the FLU vector `v` by the unit quaternion `q`. A non-unit `q` scales
/// the result by the squared length of `q`.
///
/// ```
/// use rm_simulator_server::math::{quat_axis_angle, rotate};
/// use std::f64::consts::FRAC_PI_2;
///
/// // A quarter turn about up (+z) takes forward (+x) to left (+y).
/// let yaw = quat_axis_angle([0.0, 0.0, 1.0], FRAC_PI_2);
/// let forward = rotate(yaw, [1.0, 0.0, 0.0]);
/// assert!(forward[0].abs() < 1e-12);
/// assert!((forward[1] - 1.0).abs() < 1e-12);
/// assert!(forward[2].abs() < 1e-12);
/// ```
pub fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let [w, x, y, z] = q;
    let u = [x, y, z];
    let uv = cross(u, v);
    let uuv = cross(u, uv);
    let mut out = v;
    for i in 0..3 {
        out[i] += 2.0 * (w * uv[i] + uuv[i]);
    }
    out
}
/// Component-wise sum of two FLU vectors, in metres.
pub fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
/// Right-handed cross product `a x b` in the FLU frame; the result is not
/// normalised.
pub fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
/// Euclidean length of an FLU vector, in the vector's own unit.
pub fn length(v: [f64; 3]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
}

/// World pose of a glTF asset whose manifest placement is the rotation
/// `rotation_wxyz` (applied to the asset's own glTF axes) and the FLU
/// translation `translation_m`. Applying the result as an FLU pose in the
/// renderer reproduces the placed asset.
///
/// ```
/// use rm_simulator_server::math::{gltf_point, gltf_root_pose, quat_axis_angle};
/// use std::f64::consts::FRAC_PI_2;
///
/// // Place an asset at (1, 2, 3) m with a quarter turn about its own +y axis.
/// let root = gltf_root_pose(
///     [1.0, 2.0, 3.0],
///     quat_axis_angle([0.0, 1.0, 0.0], FRAC_PI_2),
/// );
/// // Its own +x then points along FLU -z from the placement.
/// let forward = gltf_point(&root, [1.0, 0.0, 0.0]);
/// assert!((forward[2] - 2.0).abs() < 1e-12);
/// ```
pub fn gltf_root_pose(translation_m: [f64; 3], rotation_wxyz: [f64; 4]) -> Pose {
    Pose {
        translation_m,
        rotation_wxyz: quat_multiply(rotation_wxyz, RENDER_BASIS_WXYZ),
    }
}
/// The placement rotation of a glTF root pose, acting on glTF axes.
pub fn gltf_rotation(root: &Pose) -> [f64; 4] {
    quat_multiply(root.rotation_wxyz, quat_conjugate(RENDER_BASIS_WXYZ))
}
/// A point given in an asset's glTF axes, in world FLU under its root pose.
pub fn gltf_point(root: &Pose, point: [f64; 3]) -> [f64; 3] {
    add(root.translation_m, rotate(gltf_rotation(root), point))
}
/// The world pose of a child frame placed at `translation` with `rotation`
/// inside an asset's glTF axes: the child's own axes stay glTF axes.
pub fn gltf_child(root: &Pose, translation: [f64; 3], rotation_wxyz: [f64; 4]) -> Pose {
    let parent = gltf_rotation(root);
    gltf_root_pose(
        gltf_point(root, translation),
        quat_multiply(parent, rotation_wxyz),
    )
}
/// Column-major 4x4 affine matrix, as glTF stores node transforms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4(pub [f64; 16]);
impl Mat4 {
    /// Identity transform, the neutral element of [`Mat4::mul`].
    pub const IDENTITY: Mat4 = Mat4([
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]);
    /// Wraps 16 column-major values exactly as glTF stores them.
    pub fn from_cols_array(m: &[f64; 16]) -> Self {
        Mat4(*m)
    }
    /// glTF node TRS: scale, then `rotation_xyzw`, then translation.
    ///
    /// ```
    /// use rm_simulator_server::math::Mat4;
    ///
    /// // Uniform scale 2 and a translation, no rotation: (1, 0, 0) m maps to (3, 2, 3) m.
    /// let transform =
    ///     Mat4::from_scale_rotation_translation([2.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0, 2.0, 3.0]);
    /// assert_eq!(transform.transform_point([1.0, 0.0, 0.0]), [3.0, 2.0, 3.0]);
    /// assert_eq!(Mat4::IDENTITY.mul(&transform), transform);
    /// ```
    pub fn from_scale_rotation_translation(
        scale: [f64; 3],
        rotation_xyzw: [f64; 4],
        translation: [f64; 3],
    ) -> Self {
        let q = [
            rotation_xyzw[3],
            rotation_xyzw[0],
            rotation_xyzw[1],
            rotation_xyzw[2],
        ];
        let mut m = [0.0; 16];
        for (column, axis) in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
            .into_iter()
            .enumerate()
        {
            let v = rotate(q, axis);
            for row in 0..3 {
                m[column * 4 + row] = v[row] * scale[column];
            }
        }
        m[12] = translation[0];
        m[13] = translation[1];
        m[14] = translation[2];
        m[15] = 1.0;
        Mat4(m)
    }
    /// Matrix product `self * other`: `other` is applied first.
    pub fn mul(&self, other: &Mat4) -> Mat4 {
        let mut m = [0.0; 16];
        for column in 0..4 {
            for row in 0..4 {
                m[column * 4 + row] = (0..4)
                    .map(|k| self.0[k * 4 + row] * other.0[column * 4 + k])
                    .sum();
            }
        }
        Mat4(m)
    }
    /// Transforms one point, including the translation column. A direction uses
    /// the same matrix without the translation.
    pub fn transform_point(&self, p: [f64; 3]) -> [f64; 3] {
        let m = &self.0;
        std::array::from_fn(|row| {
            m[row] * p[0] + m[4 + row] * p[1] + m[8 + row] * p[2] + m[12 + row]
        })
    }
}

/// Shortest-arc interpolation of unit wxyz quaternions.
pub fn quat_slerp(a: [f64; 4], mut b: [f64; 4], t: f64) -> [f64; 4] {
    let mut dot = a.iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
    if dot < 0. {
        b = b.map(|v| -v);
        dot = -dot;
    }
    if dot > 0.9995 {
        return quat_normalize(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t));
    }
    let theta = dot.clamp(-1., 1.).acos();
    let left = ((1. - t) * theta).sin() / theta.sin();
    let right = (t * theta).sin() / theta.sin();
    std::array::from_fn(|i| a[i] * left + b[i] * right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 1e-9)
    }
    #[test]
    fn render_basis_maps_flu_axes_onto_the_renderer_frame() {
        assert!(close(
            rotate(RENDER_BASIS_WXYZ, [1.0, 0.0, 0.0]),
            [0.0, 0.0, -1.0]
        ));
        assert!(close(
            rotate(RENDER_BASIS_WXYZ, [0.0, 1.0, 0.0]),
            [-1.0, 0.0, 0.0]
        ));
        assert!(close(
            rotate(RENDER_BASIS_WXYZ, [0.0, 0.0, 1.0]),
            [0.0, 1.0, 0.0]
        ));
        assert!((quat_length(RENDER_BASIS_WXYZ) - 1.0).abs() < 1e-12);
    }
    #[test]
    fn quaternions_compose_and_rotate() {
        let yaw = quat_axis_angle([0.0, 0.0, 1.0], std::f64::consts::FRAC_PI_2);
        assert!(close(rotate(yaw, [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
        let twice = quat_multiply(yaw, yaw);
        assert!(close(rotate(twice, [1.0, 0.0, 0.0]), [-1.0, 0.0, 0.0]));
        let back = quat_multiply(twice, quat_conjugate(twice));
        assert!(close(rotate(back, [0.3, 0.2, 0.1]), [0.3, 0.2, 0.1]));
        assert_eq!(quat_normalize([2.0, 0.0, 0.0, 0.0]), [1.0, 0.0, 0.0, 0.0]);
    }
    #[test]
    fn matrices_match_trs_and_compose() {
        let trs = Mat4::from_scale_rotation_translation(
            [2.0, 2.0, 2.0],
            [
                0.0,
                0.0,
                std::f64::consts::FRAC_1_SQRT_2,
                std::f64::consts::FRAC_1_SQRT_2,
            ],
            [1.0, 2.0, 3.0],
        );
        // Scale 2, yaw 90 degrees, then translate: (1, 0, 0) -> (1, 4, 3).
        assert!(close(trs.transform_point([1.0, 0.0, 0.0]), [1.0, 4.0, 3.0]));
        let shift =
            Mat4::from_scale_rotation_translation([1.0; 3], [0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0]);
        assert!(close(
            trs.mul(&shift).transform_point([0.0; 3]),
            [1.0, 2.0, 5.0]
        ));
        assert!(close(
            shift.mul(&trs).transform_point([0.0; 3]),
            [1.0, 2.0, 4.0]
        ));
        assert_eq!(Mat4::IDENTITY.mul(&trs), trs);
    }
    #[test]
    fn gltf_children_follow_the_root_placement() {
        // Root turned a quarter turn about the glTF y axis: glTF +x lands on FLU -z.
        let root = gltf_root_pose(
            [1.0, 2.0, 3.0],
            quat_axis_angle([0.0, 1.0, 0.0], std::f64::consts::FRAC_PI_2),
        );
        assert!(close(gltf_point(&root, [1.0, 0.0, 0.0]), [1.0, 2.0, 2.0]));
        let child = gltf_child(&root, [1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]);
        assert!(close(child.translation_m, [1.0, 2.0, 2.0]));
        assert!(close(gltf_point(&child, [0.0, 0.0, 1.0]), [2.0, 2.0, 2.0]));
        // The blue outpost placement: a half turn about an axis between glTF y
        // and z, which puts the asset's +y up and its +x toward the field centre.
        let axis = quat_normalize([0.0, 0.0, 0.716_301_943, 0.697_790_46]);
        let outpost = gltf_root_pose([3.0, 3.8, 0.19], [0.0, axis[1], axis[2], axis[3]]);
        let forward = gltf_point(&outpost, [1.0, 0.0, 0.0]);
        assert!(close(forward, [2.0, 3.8, 0.19]));
        // Turning the frame -90 degrees about glTF y reads it as an FLU frame.
        let tower = gltf_child(
            &outpost,
            [0.0; 3],
            quat_axis_angle([0.0, 1.0, 0.0], -std::f64::consts::FRAC_PI_2),
        );
        assert!(close(
            rotate(tower.rotation_wxyz, [1.0, 0.0, 0.0]),
            [-1.0, 0.0, 0.0]
        ));
        let up = rotate(tower.rotation_wxyz, [0.0, 0.0, 1.0]);
        assert!(up[2] > 0.999, "{up:?}");
    }
}
