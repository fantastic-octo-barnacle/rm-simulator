// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Hand-built referee equipment approximations. Dimensions and drawing references
//! are recorded in `docs/robot-equipment.md`; no source CAD is embedded here.
use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology, prelude::*};

/// Surface finish for one generated equipment part.
#[derive(Clone, Copy)]
pub(crate) enum Finish {
    /// Painted housing shell.
    Shell,
    /// Bright metal fitting.
    Metal,
    /// Dark recess or opening.
    Black,
    /// Dark glass lens.
    Lens,
    /// LI01 HP segment, with a caller-owned fill level.
    Hp(usize),
    /// Powered status window.
    Status,
}
/// One mesh piece of an equipment module, in the module's local frame.
pub(crate) struct Part {
    /// Triangulated geometry, already in module coordinates.
    pub mesh: Mesh,
    /// Placement of the mesh in the module's local frame.
    pub transform: Transform,
    /// Which shared material the piece uses.
    pub finish: Finish,
}
/// A box part at a module-local position.
fn block(size: Vec3, position: Vec3, finish: Finish) -> Part {
    Part {
        mesh: Cuboid::from_size(size).into(),
        transform: Transform::from_translation(position),
        finish,
    }
}

/// Eight-sided rectangular housing, chamfered in its XY face. +Z is the back.
/// Flat faces keep the silhouette legible without tessellated screws or fillets.
pub(crate) fn housing(width: f32, height: f32, depth: f32, bevel: f32) -> Mesh {
    let x = width / 2.0;
    let y = height / 2.0;
    let b = bevel.min(x).min(y);
    let ring = [
        (-x + b, -y),
        (x - b, -y),
        (x, -y + b),
        (x, y - b),
        (x - b, y),
        (-x + b, y),
        (-x, y - b),
        (-x, -y + b),
    ];
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut triangle = |a: Vec3, b: Vec3, c: Vec3| {
        let n = (b - a).cross(c - a).normalize();
        positions.extend([a.to_array(), b.to_array(), c.to_array()]);
        normals.extend([n.to_array(); 3]);
    };
    for i in 0..8 {
        let (ax, ay) = ring[i];
        let (bx, by) = ring[(i + 1) % 8];
        let a = Vec3::new(ax, ay, -depth / 2.0);
        let b = Vec3::new(bx, by, -depth / 2.0);
        let c = Vec3::new(ax, ay, depth / 2.0);
        let d = Vec3::new(bx, by, depth / 2.0);
        triangle(Vec3::new(0.0, 0.0, -depth / 2.0), b, a);
        triangle(Vec3::new(0.0, 0.0, depth / 2.0), c, d);
        triangle(a, b, d);
        triangle(a, d, c);
    }
    let uv = vec![[0.0, 0.0]; positions.len()];
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
}

/// LI01 main housing and two support legs. User guide installation drawing,
/// p.4: 285 × 50 × 84.75 mm including supports. LED segmentation is an app choice.
pub(crate) fn light_bar() -> Vec<Part> {
    let mut parts = vec![Part {
        mesh: housing(0.285, 0.040, 0.050, 0.009),
        transform: Transform::from_xyz(0.0, 0.065, 0.0),
        finish: Finish::Shell,
    }];
    for x in [-0.05, 0.05] {
        parts.push(block(
            Vec3::new(0.012, 0.045, 0.018),
            Vec3::new(x, 0.0225, 0.010),
            Finish::Metal,
        ));
    }
    for i in 0..10 {
        parts.push(block(
            Vec3::new(0.024, 0.014, 0.003),
            Vec3::new((i as f32 - 4.5) * 0.026, 0.064, -0.026),
            Finish::Hp(i),
        ));
    }
    for x in [-0.108, 0.108] {
        parts.push(block(
            Vec3::new(0.042, 0.003, 0.016),
            Vec3::new(x, 0.086, 0.0),
            Finish::Status,
        ));
    }
    parts
}

/// FI02 plate, logo/detection face downward. Guide p.4 dimension drawing:
/// 121.67 × 105.67 × 17.80 mm. The indicators occupy all four edges.
pub(crate) fn interaction() -> Vec<Part> {
    let mut parts = vec![Part {
        mesh: housing(0.12167, 0.10567, 0.0178, 0.006),
        transform: Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
        finish: Finish::Shell,
    }];
    parts.push(block(
        Vec3::new(0.087, 0.002, 0.073),
        Vec3::new(0.0, -0.010, 0.0),
        Finish::Black,
    ));
    for side in [-1.0, 1.0] {
        parts.push(block(
            Vec3::new(0.035, 0.004, 0.002),
            Vec3::new(0.0, 0.0, side * 0.053),
            Finish::Status,
        ));
        parts.push(block(
            Vec3::new(0.002, 0.004, 0.035),
            Vec3::new(side * 0.061, 0.0, 0.0),
            Finish::Status,
        ));
    }
    parts
}

/// SM01/SM11 open muzzle shroud. Guide pp.4–5 gives 109.81/119.70 mm
/// lengths and 41.20/73.80 mm heights. Widths are approximate drawing readings.
/// The front opening remains empty, with the barrel terminating at its rear.
pub(crate) fn speed_monitor(hero: bool) -> Vec<Part> {
    let (length, height, bore) = if hero {
        (0.1197, 0.0738, 0.044)
    } else {
        (0.10981, 0.0412, 0.022)
    };
    let width = bore + 0.020;
    let mut parts = Vec::new();
    for side in [-1.0, 1.0] {
        parts.push(block(
            Vec3::new(0.009, height, length),
            Vec3::new(side * (width / 2.0 - 0.0045), 0.0, length / 2.0),
            Finish::Shell,
        ));
        parts.push(block(
            Vec3::new(width - 0.018, (height - bore) / 2.0, length),
            Vec3::new(0.0, side * (height + bore) / 4.0, length / 2.0),
            Finish::Shell,
        ));
        parts.push(block(
            Vec3::new(0.002, 0.006, length * 0.68),
            Vec3::new(side * (width / 2.0 + 0.001), 0.0, length * 0.49),
            Finish::Status,
        ));
        for z in [0.025, 0.076] {
            parts.push(block(
                Vec3::new(0.003, 0.010, 0.007),
                Vec3::new(side * (bore / 2.0 + 0.001), 0.0, z),
                Finish::Black,
            ));
        }
    }
    parts
}

/// Flattened VTM visual variant, 50 mm wide, 28 mm high and 80 mm deep.
/// Artist-adjusted proportions, not a dimensionally faithful VT03 case.
pub(crate) const CAMERA_LENS_M: Vec3 = Vec3::new(0.0, 0.0, -0.045);
pub(crate) fn camera() -> Vec<Part> {
    let mut parts = vec![Part {
        mesh: housing(0.050, 0.028, 0.080, 0.005),
        transform: Transform::default(),
        finish: Finish::Shell,
    }];
    parts.push(block(
        Vec3::new(0.027, 0.022, 0.003),
        Vec3::new(0.0, 0.0, -0.041),
        Finish::Black,
    ));
    parts.push(Part {
        mesh: Cylinder::new(0.009, 0.004).mesh().resolution(12).build(),
        transform: Transform::from_xyz(0.0, 0.0, -0.043)
            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        finish: Finish::Lens,
    });
    // Short mounting foot lands on the cradle's front edge.
    parts.push(block(
        Vec3::new(0.026, 0.012, 0.036),
        Vec3::new(0.0, -0.021, 0.024),
        Finish::Metal,
    ));
    // Cooling slots run across the top of the longer, shallow case.
    for i in 0..6 {
        parts.push(block(
            Vec3::new(0.033, 0.001, 0.003),
            Vec3::new(0.0, 0.0145, (i as f32 - 2.5) * 0.008),
            Finish::Black,
        ));
    }
    for side in [-1.0, 1.0] {
        parts.push(block(
            Vec3::new(0.007, 0.005, 0.043),
            Vec3::new(side * 0.027, -0.014, 0.004),
            Finish::Metal,
        ));
        parts.push(block(
            Vec3::new(0.004, 0.004, 0.020),
            Vec3::new(side * 0.010, -0.006, 0.049),
            Finish::Black,
        ));
    }
    parts.push(block(
        Vec3::new(0.003, 0.003, 0.002),
        Vec3::new(0.019, 0.0, -0.041),
        Finish::Status,
    ));
    parts
}
