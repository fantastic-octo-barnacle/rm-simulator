// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Reproducible scoring frames fitted at load time from checksum-verified CAD.
//! No edited/exported mesh or baked copy of the field geometry is needed.
use crate::{
    cad_assets::CadAssets,
    collision_mesh::{self, CollisionMesh},
    math::*,
};
use anyhow::{Context, ensure};
use rm_simulator_world::{BaseConfig, Pose};

/// Fits the scoring frames of every base in the CAD package: six armor plates
/// and the dart target plane, in world FLU metres. Returns an empty list when
/// the package carries no base semantics or lacks the reconstructed armor
/// nodes, so an older package contributes no live bases. Errors when a listed
/// plate, the dart carriage or the rail limits are missing, or when a fitted
/// sheet has an implausible size.
pub fn load(cad: &CadAssets) -> anyhow::Result<Vec<BaseConfig>> {
    let Some(semantics) = cad.base.visual_semantics() else {
        return Ok(vec![]);
    };
    // Existing source assemblies are still labelled unclassified by rm-map-tools.
    // These bindings select their optical front sheets, not their support struts.
    // Lower modules use the exporter's explicit LED surface bindings.
    let ids = [
        ("base.reconstructed.armor.0", 2),
        ("base.reconstructed.armor.1", 2),
        ("base.reconstructed.armor.2", 2),
        ("base.unclassified.28", 5),
        ("base.unclassified.22", 4),
        ("base.unclassified.25", 3),
        ("base.dart_target.carriage", 1),
    ];
    if !semantics.nodes.iter().any(|n| n.id == ids[0].0) {
        return Ok(vec![]);
    }
    let path = cad.root.join(&cad.base.file);
    let meshes: Vec<_> = ids
        .iter()
        .map(|(id, primitive)| {
            let node = semantics
                .nodes
                .iter()
                .find(|n| n.id == *id)
                .with_context(|| format!("missing base plate {id}"))?;
            collision_mesh::load_glb_selected(&path, &[(node.node, *primitive)].into())
        })
        .collect::<anyhow::Result<_>>()?;
    let carriage_node = semantics
        .nodes
        .iter()
        .find(|n| n.id == "base.dart_target.carriage")
        .context("missing dart carriage")?
        .node;
    let carriage = collision_mesh::load_glb_selected(&path, &[(carriage_node, 0)].into())?;
    let joint = semantics
        .joints
        .iter()
        .find(|j| j.id == "base.dart_target.slide")
        .context("base dart rail missing")?;
    cad.base
        .placements
        .iter()
        .map(|root| {
            let axis = rotate(
                gltf_rotation(root),
                rotate(
                    [
                        joint.rotation_xyzw[3],
                        joint.rotation_xyzw[0],
                        joint.rotation_xyzw[1],
                        joint.rotation_xyzw[2],
                    ],
                    joint.axis,
                ),
            );
            let mut plates = [Pose::default(); 7];
            for (i, mesh) in meshes.iter().enumerate() {
                let mut mesh = mesh.placed(root);
                if i == 6 {
                    // CAD assigns red to only one dart armor light bar. Reflect
                    // its vertices across the carriage centre along the rail to
                    // fit the paired optical plane, rather than its side wall.
                    let center = gltf_point(root, joint.origin_m);
                    let reflected: Vec<_> = mesh
                        .vertices_m
                        .iter()
                        .map(|p| {
                            let distance = dot(sub(*p, center), axis);
                            std::array::from_fn(|j| p[j] - 2. * distance * axis[j])
                        })
                        .collect();
                    mesh.vertices_m.extend(reflected);
                }
                plates[i] = fit(&mesh, root.translation_m)
                    .with_context(|| format!("fitting base plate {i}"))?;
                if i == 6 {
                    // The optical bars are recessed. Put the scoring/printing
                    // plane on the actual carrier front at the fitted centre.
                    raise_to_front(&carriage.placed(root), &mut plates[i]);
                }
            }
            Ok(BaseConfig {
                team: crate::layout::side_team(root.translation_m[0]),
                plates,
                dart_offsets_m: joint
                    .limits
                    .context("base dart rail limits missing")?
                    .map(|limit| axis.map(|v| v * limit)),
            })
        })
        .collect()
}
/// Dot product of two vectors.
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
/// Componentwise `a - b`.
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
/// Fit a least-squares front plane to the optical sheet. Projected bounds
/// of all sheet vertices supply the scoring centre.
/// The 2 mm outward offset prevents coincident CAD contacts eating scored hits.
fn fit(mesh: &CollisionMesh, origin: [f64; 3]) -> anyhow::Result<Pose> {
    ensure!(!mesh.vertices_m.is_empty(), "empty optical sheet");
    let center: [f64; 3] = std::array::from_fn(|i| {
        mesh.vertices_m.iter().map(|p| p[i]).sum::<f64>() / mesh.vertices_m.len() as f64
    });
    let mut covariance = [[0.; 3]; 3];
    for point in &mesh.vertices_m {
        let delta = sub(*point, center);
        for i in 0..3 {
            for j in 0..3 {
                covariance[i][j] += delta[i] * delta[j];
            }
        }
    }
    let mut normal = smallest_axis(covariance);
    // Armor is exterior to the base; for nearly horizontal top plates use up.
    let outward = [center[0] - origin[0], center[1] - origin[1], 0.3];
    if dot(normal, outward) < 0. {
        normal = normal.map(|v| -v);
    }
    let yaw = normal[1].atan2(normal[0]);
    let pitch = -normal[2].clamp(-1., 1.).asin();
    let rotation = quat_multiply(
        quat_axis_angle([0., 0., 1.], yaw),
        quat_axis_angle([0., 1., 0.], pitch),
    );
    let axes = [
        normal,
        rotate(rotation, [0., 1., 0.]),
        rotate(rotation, [0., 0., 1.]),
    ];
    let bounds: [[f64; 2]; 3] = axes.map(|axis| {
        mesh.vertices_m
            .iter()
            .fold([f64::INFINITY, f64::NEG_INFINITY], |[lo, hi], p| {
                let v = dot(*p, axis);
                [lo.min(v), hi.max(v)]
            })
    });
    ensure!(
        (0.07..0.3).contains(&(bounds[1][1] - bounds[1][0]))
            && (0.025..0.25).contains(&(bounds[2][1] - bounds[2][0])),
        "unexpected base optical sheet size: {bounds:?}"
    );
    let coords = [
        bounds[0][1] + 0.002,
        (bounds[1][0] + bounds[1][1]) * 0.5,
        (bounds[2][0] + bounds[2][1]) * 0.5,
    ];
    Ok(Pose {
        translation_m: std::array::from_fn(|i| (0..3).map(|j| axes[j][i] * coords[j]).sum()),
        rotation_wxyz: rotation,
    })
}

/// Jacobi diagonalization of a symmetric 3x3 covariance matrix. The least
/// variance axis is perpendicular to these thin optical sheets.
fn smallest_axis(mut a: [[f64; 3]; 3]) -> [f64; 3] {
    let mut vectors = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
    for _ in 0..24 {
        let (p, q) = [(0, 1), (0, 2), (1, 2)]
            .into_iter()
            .max_by(|&(p, q), &(r, s)| a[p][q].abs().total_cmp(&a[r][s].abs()))
            .unwrap();
        if a[p][q].abs() < 1e-14 {
            break;
        }
        let angle = 0.5 * (2. * a[p][q]).atan2(a[q][q] - a[p][p]);
        let (s, c) = angle.sin_cos();
        let (pp, qq, pq) = (a[p][p], a[q][q], a[p][q]);
        a[p][p] = c * c * pp - 2. * s * c * pq + s * s * qq;
        a[q][q] = s * s * pp + 2. * s * c * pq + c * c * qq;
        a[p][q] = 0.;
        a[q][p] = 0.;
        for k in 0..3 {
            if k != p && k != q {
                let (kp, kq) = (a[k][p], a[k][q]);
                a[k][p] = c * kp - s * kq;
                a[p][k] = a[k][p];
                a[k][q] = s * kp + c * kq;
                a[q][k] = a[k][q];
            }
            let (vp, vq) = (vectors[k][p], vectors[k][q]);
            vectors[k][p] = c * vp - s * vq;
            vectors[k][q] = s * vp + c * vq;
        }
    }
    let axis = (0..3).min_by(|&i, &j| a[i][i].total_cmp(&a[j][j])).unwrap();
    std::array::from_fn(|i| vectors[i][axis])
}

/// Intersect the fitted centre ray with the carriage's front sheet. Limit the
/// search to nearby geometry so a rail or remote bracket cannot move the face.
fn raise_to_front(mesh: &CollisionMesh, pose: &mut Pose) {
    let inverse = quat_conjugate(pose.rotation_wxyz);
    let vertices: Vec<_> = mesh
        .vertices_m
        .iter()
        .map(|p| rotate(inverse, sub(*p, pose.translation_m)))
        .collect();
    let mut front: f64 = 0.;
    for triangle in &mesh.triangles {
        let [a, b, c] = triangle.map(|i| vertices[i as usize]);
        let det = (b[2] - c[2]) * (a[1] - c[1]) + (c[1] - b[1]) * (a[2] - c[2]);
        if det.abs() < 1e-12 {
            continue;
        }
        let u = ((b[2] - c[2]) * (-c[1]) + (c[1] - b[1]) * (-c[2])) / det;
        let v = ((c[2] - a[2]) * (-c[1]) + (a[1] - c[1]) * (-c[2])) / det;
        if u < 0. || v < 0. || u + v > 1. {
            continue;
        }
        let x = u * a[0] + v * b[0] + (1. - u - v) * c[0];
        if (0.0..0.04).contains(&x) {
            front = front.max(x + 0.002);
        }
    }
    let offset = rotate(pose.rotation_wxyz, [front, 0., 0.]);
    pose.translation_m = add(pose.translation_m, offset);
}
