// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Checksum-bound rm-map-tools articulation data. Names are display labels;
//! stable semantic IDs and node indices bind geometry and motion.
use crate::cad_assets::verify_sha256;
use anyhow::{Context, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

/// One exported articulation joint: a semantic ID over a glTF origin frame and
/// the node whose subtree it moves. Semantic IDs and node indices bind the
/// geometry; display names are ignored.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Joint {
    /// Stable semantic ID, unique among the file's joints and never shared
    /// with a node binding ID.
    pub id: String,
    /// Joint type from the JSON `type` key: `continuous`, `revolute` or
    /// `prismatic`. Any other value fails validation.
    #[serde(rename = "type")]
    pub kind: String,
    /// Semantic ID of the node binding the joint hangs from.
    pub parent: String,
    /// Semantic IDs of the node bindings that move with the joint, in the
    /// exporter's order.
    pub children: Vec<String>,
    /// Joint origin in the asset's glTF frame, in metres.
    pub origin_m: [f64; 3],
    /// Rest rotation of the joint frame relative to its parent, xyzw. Omitted
    /// means the identity; a present value must be unit length.
    #[serde(default = "identity")]
    pub rotation_xyzw: [f64; 4],
    /// Unit axis of motion in the joint frame: the rotation axis of a
    /// `revolute` or `continuous` joint, the sliding direction of a
    /// `prismatic` one.
    pub axis: [f64; 3],
    /// Travel limits in the joint frame as `[low, high]`, radians for a
    /// revolute joint and metres for a prismatic one. Both bounds include zero
    /// so the exported rest pose is reachable. `None` for a continuous joint,
    /// which has no stops.
    pub limits: Option<[f64; 2]>,
    /// glTF node index of the joint's origin frame in the selected scene. Its
    /// only child is the motion node, and its exported transform must match
    /// `origin_m` and `rotation_xyzw`.
    pub origin_node: usize,
    /// glTF node index of the node carrying the moving geometry. Its rest
    /// transform must be the identity; the renderer rotates it for rune and
    /// outpost motion.
    pub motion_node: usize,
    /// Exporter note on how the joint's fitted travel was verified, for
    /// example `illustrative_travel`. Informational; validation ignores it.
    pub verification: Option<String>,
}
/// Serde default for a joint whose export omits `rotation_xyzw`.
fn identity() -> [f64; 4] {
    [0., 0., 0., 1.]
}
/// A semantic ID bound to one glTF node in the selected scene.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct NodeBinding {
    /// Stable semantic ID, unique within the file and never reused as a joint
    /// ID.
    pub id: String,
    /// glTF node index in the selected scene.
    pub node: usize,
    /// The node's `extras.rm` object as exported. Validation requires it to
    /// equal this binding's metadata and to carry the same ID.
    pub metadata: Value,
}
/// Everything one checksummed GLB contributes: its node bindings, joints,
/// surface records and collision selection. The scene index and the two
/// primitive sets are filled by validation and skipped by deserialisation.
///
/// ```
/// use rm_simulator_server::semantics::FileSemantics;
///
/// // A file record as it appears in articulation.json.
/// let file: FileSemantics = serde_json::from_str(
///     r#"{
///         "file": "rune.glb",
///         "sha256": "00",
///         "nodes": [{"id": "assembly", "node": 0, "metadata": {"id": "assembly"}}],
///         "joints": [{
///             "id": "spin", "type": "continuous", "parent": "assembly",
///             "children": ["rotor"], "origin_m": [0.0, 0.0, 0.0],
///             "axis": [0.0, 0.0, 1.0], "origin_node": 1, "motion_node": 2
///         }]
///     }"#,
/// )
/// .unwrap();
/// assert_eq!(file.joints[0].kind, "continuous");
/// assert_eq!(file.joints[0].rotation_xyzw, [0.0, 0.0, 0.0, 1.0]);
/// // The scene index and the primitive sets come from validation, not JSON.
/// assert_eq!(file.scene, 0);
/// assert!(file.fixed_primitives.is_empty());
/// ```
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct FileSemantics {
    /// Path of the GLB relative to the asset root, for example
    /// `equipment/base.glb`. It must stay inside the root and match `sha256`
    /// before the file is parsed.
    pub file: String,
    /// Index of the glTF scene the bindings are validated against, taken from
    /// the GLB's own `scene` and `0` when it declares none. Filled by
    /// validation.
    #[serde(skip)]
    pub scene: usize,
    /// Expected SHA-256 of the file, in either case. The file is read only
    /// after it matches.
    pub sha256: String,
    /// Semantic node bindings, one per ID.
    pub nodes: Vec<NodeBinding>,
    /// Articulation joints, one per ID.
    pub joints: Vec<Joint>,
    /// Exported surface bindings; absent means empty. No consumer reads them
    /// yet, but validation still rejects duplicate IDs or metadata that
    /// differs from the GLB.
    #[serde(default)]
    pub surfaces: Vec<SurfaceBinding>,
    /// Primitives the exporter removed from the collision GLB. Their indices
    /// are positions in the pre-filter mesh, so applying them to the compacted
    /// collision mesh would remove unrelated triangles.
    #[serde(default)]
    pub collision_exclusions: Vec<Exclusion>,
    /// Exporter notes about bindings still to be produced, such as LED and
    /// detector surfaces. Informational; any content is accepted.
    #[serde(default)]
    pub pending: Vec<String>,
    /// Mesh primitives outside any moving joint subtree, in selected scene.
    #[serde(skip)]
    pub fixed_primitives: BTreeSet<(usize, usize)>,
    /// Mesh primitives `(node, primitive)` that carry collision, from the live
    /// metadata of the selected scene. Primitives marked `collision: false`
    /// are absent.
    #[serde(skip)]
    pub collision_primitives: BTreeSet<(usize, usize)>,
}
/// A named visual surface bound to one mesh primitive, such as an LED or an
/// armor face. Validation requires the primitive's `extras.rm` metadata to
/// match and to carry the same surface ID.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SurfaceBinding {
    /// Stable surface ID, unique within the file.
    pub id: String,
    /// glTF node index holding the primitive, in the selected scene.
    pub node: usize,
    /// None when the exporter removed this surface from the collision GLB.
    pub primitive: Option<usize>,
    /// The primitive's `extras.rm` object as exported.
    pub metadata: Value,
}
/// One primitive the exporter removed from the collision GLB. The indices are
/// pre-filter positions, so applying them to the compacted collision mesh
/// would remove unrelated triangles.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Exclusion {
    /// glTF node index in the pre-filter mesh.
    pub node: usize,
    /// Mesh primitive index inside that node, when the record names one.
    pub primitive: Option<usize>,
}
/// The semantic files of one named asset, keyed by file kind. A kind present
/// here must match the manifest entry for that asset.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct AssetSemantics {
    /// One entry per exported file kind, `visual` or `collision`.
    pub files: BTreeMap<String, FileSemantics>,
}
/// A manifest reference to a semantic sidecar: the file to read and the
/// checksum it must have before any of it is trusted.
#[derive(Deserialize)]
pub struct Descriptor {
    /// Sidecar path relative to the directory the descriptor is resolved
    /// against.
    pub file: String,
    /// Expected SHA-256 of the sidecar; it is parsed only after this matches.
    pub sha256: String,
    /// Sidecar schema version. Only version 1 loads.
    pub schema_version: u32,
}
/// The sidecar JSON as rm-map-tools writes it: its coordinate conventions and
/// one `AssetSemantics` per named asset.
#[derive(Deserialize)]
struct Sidecar {
    /// Sidecar schema version; only 1 is accepted.
    schema_version: u32,
    /// Declared units by quantity, checked as metres and radians.
    units: BTreeMap<String, String>,
    /// Frame the node and joint data is expressed in, checked against the
    /// exporter's selected-scene string.
    frame: String,
    /// Quaternion component order, checked as `xyzw`.
    quaternion_order: String,
    /// One entry per named asset.
    assets: BTreeMap<String, AssetSemantics>,
}
/// Resolve a package-relative asset path, refusing anything that could leave
/// the package root. Every component must be a normal name, so an empty path,
/// an absolute path, `.` and `..` are all rejected.
///
/// ```
/// use rm_simulator_server::semantics::safe_path;
/// use std::path::Path;
///
/// let root = Path::new("/package");
/// assert_eq!(
///     safe_path(root, "equipment/rune.glb").unwrap(),
///     root.join("equipment/rune.glb")
/// );
/// assert!(safe_path(root, "../outside.glb").is_err());
/// assert!(safe_path(root, "/etc/passwd").is_err());
/// assert!(safe_path(root, "").is_err());
/// ```
pub fn safe_path(root: &Path, file: &str) -> anyhow::Result<PathBuf> {
    ensure!(
        !file.is_empty()
            && Path::new(file)
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
        "unsafe asset path {file:?}"
    );
    Ok(root.join(file))
}
/// Read a verified sidecar and validate every GLB it binds.
///
/// The sidecar must be schema version 1, match `descriptor.sha256`, and
/// declare metres, radians, `xyzw` and the exporter's selected-scene frame.
/// Each bound GLB is then read, checksummed and checked against its own
/// bindings. Returns the assets keyed by name.
pub fn load(
    root: &Path,
    descriptor: &Descriptor,
) -> anyhow::Result<BTreeMap<String, AssetSemantics>> {
    ensure!(
        descriptor.schema_version == 1,
        "unsupported articulation descriptor"
    );
    let path = safe_path(root, &descriptor.file)?;
    verify_sha256(&path, &descriptor.sha256)?;
    let mut data: Sidecar = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        data.schema_version == 1
            && data.units.get("length").map(String::as_str) == Some("metres")
            && data.units.get("angle").map(String::as_str) == Some("radians")
            && data.quaternion_order == "xyzw"
            && data.frame == "GLB selected scene coordinates before external instance placement",
        "unsupported articulation coordinates or schema"
    );
    for (asset, semantics) in &mut data.assets {
        for file in semantics.files.values_mut() {
            let path = safe_path(root, &file.file)?;
            verify_sha256(&path, &file.sha256)?;
            let bytes = std::fs::read(&path)?;
            ensure!(
                bytes.len() >= 20
                    && &bytes[..4] == b"glTF"
                    && bytes[4..8] == 2u32.to_le_bytes()
                    && bytes[16..20] == *b"JSON",
                "invalid semantic GLB"
            );
            let len = u32::from_le_bytes(bytes[12..16].try_into()?) as usize;
            let doc: Value =
                serde_json::from_slice(bytes.get(20..20 + len).context("truncated GLB JSON")?)?;
            validate_file(file, &doc)
                .with_context(|| format!("semantic asset {asset}/{}", file.file))?;
        }
    }
    Ok(data.assets)
}
/// Check one file's bindings against its parsed glTF document, then record the
/// selected scene and the collision and fixed primitive sets on `file`.
///
/// Every semantic ID must be unique, every node and motion index in range,
/// every joint frame finite and unit length, and every binding must agree with
/// the node and primitive metadata in the GLB. The scene traversal also
/// rejects cycles, multiple parents and bindings outside the selected scene.
fn validate_file(file: &mut FileSemantics, doc: &Value) -> anyhow::Result<()> {
    let nodes = doc["nodes"].as_array().context("missing nodes")?;
    let mut ids = BTreeMap::new();
    for binding in &file.nodes {
        ensure!(
            ids.insert(binding.id.clone(), binding.node).is_none(),
            "duplicate semantic ID"
        );
        let node = nodes
            .get(binding.node)
            .context("semantic node out of range")?;
        ensure!(
            node["extras"]["rm"] == binding.metadata
                && binding.metadata["id"].as_str() == Some(&binding.id),
            "node metadata differs from sidecar"
        );
    }
    let mut surface_ids = BTreeSet::new();
    for surface in &file.surfaces {
        ensure!(surface_ids.insert(&surface.id), "duplicate surface ID");
        let node = nodes
            .get(surface.node)
            .context("surface node out of range")?;
        if let Some(primitive) = surface.primitive {
            let mesh = node["mesh"].as_u64().context("surface node has no mesh")? as usize;
            ensure!(
                doc["meshes"][mesh]["primitives"][primitive]["extras"]["rm"] == surface.metadata
                    && surface.metadata["id"].as_str() == Some(&surface.id),
                "surface metadata differs from sidecar"
            );
        }
    }
    let mut motions = BTreeSet::new();
    let mut joint_ids = BTreeSet::new();
    for joint in &file.joints {
        ensure!(
            !ids.contains_key(&joint.id)
                && joint_ids.insert(&joint.id)
                && motions.insert(joint.motion_node),
            "duplicate joint"
        );
        ensure!(
            ["continuous", "revolute", "prismatic"].contains(&joint.kind.as_str()),
            "unsupported joint type"
        );
        ensure!(
            joint
                .origin_m
                .iter()
                .chain(&joint.axis)
                .chain(&joint.rotation_xyzw)
                .all(|v| v.is_finite()),
            "nonfinite joint frame"
        );
        ensure!(
            (joint.axis.iter().map(|v| v * v).sum::<f64>() - 1.).abs() < 1e-6
                && (joint.rotation_xyzw.iter().map(|v| v * v).sum::<f64>() - 1.).abs() < 1e-6,
            "joint axis or rotation is not unit length"
        );
        if let Some([lo, hi]) = joint.limits {
            ensure!(
                lo.is_finite() && hi.is_finite() && lo <= 0. && hi >= 0.,
                "invalid joint limits/rest value"
            );
        }
        ensure!(
            ids.contains_key(&joint.parent)
                && !joint.children.is_empty()
                && joint.children.iter().all(|id| ids.contains_key(id)),
            "dangling joint binding"
        );
        let motion = nodes
            .get(joint.motion_node)
            .context("missing motion node")?;
        let origin = nodes
            .get(joint.origin_node)
            .context("missing origin node")?;
        ensure!(
            motion["extras"]["rm"]["id"].as_str() == Some(&joint.id),
            "motion ID differs from sidecar"
        );
        ensure!(
            motion
                .get("scale")
                .is_none_or(|v| *v == serde_json::json!([1, 1, 1]))
                && motion.get("matrix").is_none()
                && motion
                    .get("translation")
                    .is_none_or(|v| *v == serde_json::json!([0, 0, 0]))
                && motion
                    .get("rotation")
                    .is_none_or(|v| *v == serde_json::json!([0, 0, 0, 1])),
            "motion node must have zero rest transform"
        );
        let children = origin["children"]
            .as_array()
            .context("joint origin has no motion child")?;
        ensure!(
            children == &vec![serde_json::json!(joint.motion_node)],
            "joint origin/motion hierarchy differs"
        );
        ensure!(
            nodes[ids[&joint.parent]]["children"]
                .as_array()
                .is_some_and(|c| c.contains(&serde_json::json!(joint.origin_node))),
            "joint parent binding differs"
        );
        ensure!(
            motion["children"].as_array().map(Vec::len) == Some(joint.children.len()),
            "joint child count differs"
        );
        // The exporter stores the origin's matrix relative to the parent; the
        // sidecar frame is asset-wide. Validate it through the scene traversal below.
        ensure!(
            motion["children"]
                .as_array()
                .context("motion children missing")?
                .iter()
                .all(|n| joint
                    .children
                    .iter()
                    .any(|id| n.as_u64() == Some(ids[id] as u64))),
            "motion child binding differs"
        );
    }
    let scene = doc["scene"].as_u64().unwrap_or(0) as usize;
    file.scene = scene;
    let roots = doc["scenes"][scene]["nodes"]
        .as_array()
        .context("missing selected scene")?;
    let mut stack: Vec<_> = roots
        .iter()
        .map(|n| {
            Ok((
                n.as_u64().context("bad scene node")? as usize,
                false,
                crate::math::Mat4::IDENTITY,
            ))
        })
        .collect::<anyhow::Result<_>>()?;
    let mut visited = BTreeSet::new();
    while let Some((index, moving, parent)) = stack.pop() {
        ensure!(
            visited.insert(index),
            "cyclic or multiply parented scene node"
        );
        let node = nodes.get(index).context("scene node out of range")?;
        let local = if let Some(matrix) = node.get("matrix") {
            crate::math::Mat4(serde_json::from_value(matrix.clone())?)
        } else {
            crate::math::Mat4::from_scale_rotation_translation(
                serde_json::from_value(
                    node.get("scale")
                        .cloned()
                        .unwrap_or(serde_json::json!([1, 1, 1])),
                )?,
                serde_json::from_value(
                    node.get("rotation")
                        .cloned()
                        .unwrap_or(serde_json::json!([0, 0, 0, 1])),
                )?,
                serde_json::from_value(
                    node.get("translation")
                        .cloned()
                        .unwrap_or(serde_json::json!([0, 0, 0])),
                )?,
            )
        };
        let world = parent.mul(&local);
        for joint in file.joints.iter().filter(|j| j.origin_node == index) {
            let expected = crate::math::Mat4::from_scale_rotation_translation(
                [1.; 3],
                joint.rotation_xyzw,
                joint.origin_m,
            );
            ensure!(
                world
                    .0
                    .iter()
                    .zip(expected.0)
                    .all(|(a, b)| (a - b).abs() < 1e-6),
                "joint frame differs from exported scene"
            );
        }
        let moving = moving || motions.contains(&index);
        if let Some(mesh) = node["mesh"].as_u64() {
            let primitives = doc["meshes"][mesh as usize]["primitives"]
                .as_array()
                .context("missing mesh primitives")?;
            for (primitive, data) in primitives.iter().enumerate() {
                // collision_exclusions contains pre-filter indices: never use
                // those against the rewritten collision GLB. Read live metadata.
                if node["extras"]["rm"]["collision"] != false
                    && data["extras"]["rm"]["collision"] != false
                {
                    file.collision_primitives.insert((index, primitive));
                    if !moving {
                        file.fixed_primitives.insert((index, primitive));
                    }
                }
            }
        }
        if let Some(children) = node["children"].as_array() {
            for child in children {
                stack.push((child.as_u64().context("bad child")? as usize, moving, world));
            }
        }
    }
    ensure!(
        file.nodes.iter().all(|n| visited.contains(&n.node))
            && file.joints.iter().all(|j| visited.contains(&j.motion_node)),
        "binding outside selected scene"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> (FileSemantics, Value) {
        let nodes = vec![
            json!({"id":"assembly", "node":0,"metadata":{"id":"assembly","roles":["assembly"]}}),
            json!({"id":"rotor", "node":3,"metadata":{"id":"rotor","roles":["rotor"]}}),
            json!({"id":"fixed.logo", "node":4,"metadata":{"id":"fixed.logo","roles":["decoration"]}}),
        ];
        let joint = json!({"id":"spin", "type":"continuous", "parent":"assembly", "children":["rotor"], "origin_m":[1.,2.,3.], "axis":[0.,1.,0.], "origin_node":1,"motion_node":2});
        let file: FileSemantics = serde_json::from_value(
            json!({"file":"test.glb","sha256":"unused", "nodes":nodes,"joints":[joint]}),
        )
        .unwrap();
        let doc = json!({"scene":0,"scenes":[{"nodes":[0]}],"nodes":[
            {"name":"completely renamed root","children":[1,4],"extras":{"rm":file.nodes[0].metadata}},
            {"translation":[1.,2.,3.],"children":[2]},
            {"extras":{"rm":{"id":"spin"}},"children":[3]},
            {"name":"unrelated display label","mesh":0,"extras":{"rm":file.nodes[1].metadata}},
            {"mesh":1,"extras":{"rm":file.nodes[2].metadata}}
        ],"meshes":[{"primitives":[{}]}, {"primitives":[{}, {"extras":{"rm":{"collision":false}}}]}]});
        (file, doc)
    }
    #[test]
    fn semantic_selection_uses_ids_and_hierarchy_and_retains_stationary_decoration() {
        let (mut file, doc) = fixture();
        validate_file(&mut file, &doc).unwrap();
        assert_eq!(file.fixed_primitives, BTreeSet::from([(4, 0)]));
        assert_eq!(file.collision_primitives, BTreeSet::from([(3, 0), (4, 0)]));
        // Historical exclusions refer to pre-filter indices and must not
        // accidentally remove a different retained primitive after compaction.
        let (mut file, doc) = fixture();
        file.collision_exclusions.push(Exclusion {
            node: 4,
            primitive: Some(0),
        });
        validate_file(&mut file, &doc).unwrap();
        assert!(file.fixed_primitives.contains(&(4, 0)));
    }
    #[test]
    fn rejects_mismatched_frames_ids_cycles_and_nonzero_rest() {
        for bad in 0..6 {
            let (mut file, mut doc) = fixture();
            match bad {
                0 => file.joints[0].origin_m[0] = 2.,
                1 => file.joints[0].axis = [0., 2., 0.],
                2 => doc["nodes"][2]["extras"]["rm"]["id"] = json!("wrong"),
                3 => doc["nodes"][4]["children"] = json!([0]),
                4 => doc["nodes"][2]["translation"] = json!([1, 0, 0]),
                _ => file.joints[0].children = vec!["missing".into()],
            }
            assert!(validate_file(&mut file, &doc).is_err(), "case {bad}");
        }
    }
    #[test]
    fn sidecar_checksum_and_units_are_required() {
        use sha2::{Digest, Sha256};
        let root = std::env::temp_dir().join(format!("rm-semantic-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let value = json!({"schema_version":1,"units":{"length":"mm","angle":"radians"},"frame":"GLB selected scene coordinates before external instance placement","quaternion_order":"xyzw","assets":{}}).to_string();
        std::fs::write(root.join("articulation.json"), &value).unwrap();
        let mut descriptor = Descriptor {
            file: "articulation.json".into(),
            sha256: format!("{:x}", Sha256::digest(value.as_bytes())),
            schema_version: 1,
        };
        assert!(
            load(&root, &descriptor)
                .unwrap_err()
                .to_string()
                .contains("coordinates")
        );
        descriptor.sha256 = "bad".into();
        assert!(
            load(&root, &descriptor)
                .unwrap_err()
                .to_string()
                .contains("checksum")
        );
        assert!(safe_path(&root, "../outside").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
