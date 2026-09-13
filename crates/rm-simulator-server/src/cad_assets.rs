// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Extracted RMUC2026 CAD manifest: asset files, checksums and placements.
//! Placements are converted from the CAD's Z-up arena frame into world FLU
//! metres, with the playing floor at height zero.
use crate::math::{gltf_root_pose, quat_length, quat_normalize};
use anyhow::{Context, anyhow, ensure};
use rm_simulator_world::Pose;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Prefer a package beside installed binaries, then the source checkout's package.
pub fn default_cad_assets() -> PathBuf {
    if let Some(local) = std::env::current_exe()
        .ok()
        .and_then(|executable| bundled_cad_assets(&executable))
    {
        return local;
    }
    let local = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local-assets/field");
    if local.join("manifest.json").is_file() {
        return local;
    }
    let versioned = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../field");
    if versioned.join("manifest.json").is_file() {
        return versioned;
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join("dev/RM/assets/rm2026-field")
}

/// The `field` package beside an installed binary: `Some(../field)` when the
/// executable sits in a directory named `bin` and that package has a manifest.
/// Returns `None` otherwise, so a build-tree binary falls through to the other
/// search paths.
fn bundled_cad_assets(executable: &Path) -> Option<PathBuf> {
    let bin = executable.parent()?;
    if bin.file_name()? != "bin" {
        return None;
    }
    let field = bin.parent()?.join("field");
    field.join("manifest.json").is_file().then_some(field)
}

/// Top of the playing floor in the CAD arena frame for packages whose
/// manifest records no `floor_top_source_z_m`: the V2.0.0 extraction, whose
/// slab is crowned and was measured at the small level pads near
/// (±12.8, ±3.0) m (about 0.11 m higher than its centre line and 0.32 m
/// higher than its side walls). Packages built by rm-map-tools from the
/// V1.2.0 arena have a flat slab and record its top in the manifest.
pub const FLOOR_TOP_CAD_M: f64 = -1.5304;
/// Field centre along the CAD arena y axis; the x extent is already centred.
pub const CENTER_CAD_Y_M: f64 = 1.624_343_6;

/// A CAD arena-frame point in world FLU metres, with the playing floor top
/// (`floor_top_cad_m` in the arena frame) at height zero.
///
/// ```
/// use rm_simulator_server::cad_assets::{CENTER_CAD_Y_M, cad_point_to_flu};
///
/// let floor_top_cad_m = -1.5;
/// // The floor top at the field centre is the FLU origin.
/// assert_eq!(
///     cad_point_to_flu([0.0, CENTER_CAD_Y_M, floor_top_cad_m], floor_top_cad_m),
///     [0.0, 0.0, 0.0]
/// );
/// // CAD x, y and z are already FLU forward, left and up.
/// let point = cad_point_to_flu([3.0, 1.0, -1.0], -1.5);
/// assert_eq!(point[0], 3.0);
/// assert_eq!(point[1], 1.0 - CENTER_CAD_Y_M);
/// assert_eq!(point[2], 0.5);
/// ```
pub fn cad_point_to_flu(point: [f64; 3], floor_top_cad_m: f64) -> [f64; 3] {
    [
        point[0],
        point[1] - CENTER_CAD_Y_M,
        point[2] - floor_top_cad_m,
    ]
}

/// A parsed `manifest.json`. Schema version 1 and metres are required before
/// any asset is read.
#[derive(Deserialize)]
struct Manifest {
    /// Manifest schema version; only 1 is accepted.
    schema_version: u32,
    /// Declared length unit; only `metres` is accepted.
    units: String,
    /// Top of the playing floor in the arena frame; absent in the V2.0.0
    /// extraction, which uses [`FLOOR_TOP_CAD_M`].
    #[serde(default)]
    floor_top_source_z_m: Option<f64>,
    /// Every collision node is a closed tessellation of one CAD solid that
    /// matches the visual; absent in the V2.0.0 extraction, whose proxy
    /// carries lattices, recesses and undersides the app must simplify.
    #[serde(default)]
    collision_solids: bool,
    /// Additional placed scenery, separate from the animated equipment.
    #[serde(default)]
    static_assets: Vec<String>,
    /// Reference to the package's semantic sidecar, when it has one.
    #[serde(default)]
    articulation: Option<crate::semantics::Descriptor>,
    /// One entry per named asset.
    assets: std::collections::BTreeMap<String, ManifestAsset>,
}
/// One named asset in a manifest: the files to verify and where they sit.
#[derive(Deserialize)]
struct ManifestAsset {
    /// Manifest reference to this asset's sidecar binding. It must agree with
    /// the sidecar whenever either is present.
    #[serde(default)]
    semantics: Option<serde_json::Value>,
    /// Visual GLB path relative to the manifest's directory.
    visual: String,
    /// Expected SHA-256 of the visual GLB.
    visual_sha256: String,
    /// Collision GLB path, when the package ships one.
    #[serde(default)]
    collision: Option<String>,
    /// Expected SHA-256 of the collision GLB.
    #[serde(default)]
    collision_sha256: Option<String>,
    /// Declared collision contract: `source-tessellation-v1` or a meshopt
    /// simplification method.
    #[serde(default)]
    collision_method: Option<String>,
    /// Producer error records for a simplified pair, keyed `visual` and
    /// `collision`.
    #[serde(default)]
    mesh_simplification: Option<std::collections::BTreeMap<String, MeshSimplification>>,
    /// Placements of the asset's canonical frame in the CAD arena frame.
    #[serde(default)]
    placements_in_source_arena_frame: Vec<ManifestPlacement>,
}
/// The simplifier's error record for one file. A record is accepted only when
/// its measured errors stay inside its declared limits.
#[derive(Deserialize)]
struct MeshSimplification {
    /// Simplification method that produced the file.
    method: String,
    /// Declared geometric error limit in millimetres; must be finite and
    /// positive.
    error_limit_mm: f64,
    /// Estimated error in millimetres; must lie between zero and
    /// `error_limit_mm`.
    estimated_error_mm: f64,
    /// Declared limit on sampled deviation, in millimetres; must be finite and
    /// positive.
    sampled_deviation_limit_mm: f64,
    /// Largest sampled deviation in millimetres; must lie between zero and
    /// `sampled_deviation_limit_mm`.
    max_sampled_deviation_mm: f64,
    /// Whether the simplifier kept open mesh borders locked.
    open_borders_locked: bool,
    /// Triangles sampled per direction when borders are not locked, then
    /// required to be in `1024..=16384`.
    #[serde(default)]
    deviation_triangles_per_direction: Option<u32>,
    /// Welding applied to positions; only `exact` is accepted.
    position_welding: String,
}

/// Whether the manifest proves a collision file is usable, and why not when
/// the metadata is malformed.
///
/// A declared `source-tessellation-v1` contract is accepted. A meshopt
/// simplification is accepted only when both the visual and collision records
/// pair their method with the matching border lock, stay inside their error
/// limits and use exact position welding, and the collision record carries the
/// method the entry declares. An entry with no method, or one this version
/// does not know, returns `false`: a legacy proxy is never inferred to be
/// usable.
fn verified_collision(entry: &ManifestAsset) -> anyhow::Result<bool> {
    match entry.collision_method.as_deref() {
        Some("source-tessellation-v1") => Ok(true),
        Some(method @ ("meshopt-simplification-v1" | "meshopt-boundary-simplification-v1")) => {
            let records = entry
                .mesh_simplification
                .as_ref()
                .ok_or_else(|| anyhow!("simplified collision requires producer error metadata"))?;
            for kind in ["visual", "collision"] {
                let record = records
                    .get(kind)
                    .ok_or_else(|| anyhow!("missing {kind} simplification metadata"))?;
                ensure!(
                    matches!(
                        (record.method.as_str(), record.open_borders_locked),
                        ("meshopt-simplification-v1", true)
                            | ("meshopt-boundary-simplification-v1", false)
                    ) && (kind != "collision" || record.method == method)
                        && (record.open_borders_locked
                            || record
                                .deviation_triangles_per_direction
                                .is_some_and(|count| (1024..=16384).contains(&count)))
                        && record.error_limit_mm.is_finite()
                        && record.error_limit_mm > 0.0
                        && record.estimated_error_mm.is_finite()
                        && record.estimated_error_mm >= 0.0
                        && record.estimated_error_mm <= record.error_limit_mm + 0.0001
                        && record.sampled_deviation_limit_mm.is_finite()
                        && record.sampled_deviation_limit_mm > 0.0
                        && record.max_sampled_deviation_mm.is_finite()
                        && record.max_sampled_deviation_mm >= 0.0
                        && record.max_sampled_deviation_mm <= record.sampled_deviation_limit_mm
                        && record.position_welding == "exact",
                    "invalid {kind} simplification contract"
                );
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}
/// One placement of an asset's canonical frame in the CAD arena frame.
#[derive(Deserialize)]
struct ManifestPlacement {
    /// Translation in the arena frame, in metres. Must be finite.
    translation_m: [f64; 3],
    /// Rotation applied to the asset's glTF axes, xyzw. Must be finite and
    /// unit length to within 1e-4.
    rotation_xyzw: [f64; 4],
}

/// One verified visual file with its placements in world FLU.
#[derive(Clone, Debug, PartialEq)]
pub struct CadAsset {
    /// Path relative to the asset root, as an asset server expects.
    pub file: String,
    /// Checked sidecar bindings for this asset, or `None` for a legacy asset
    /// with no articulation data.
    pub semantics: Option<crate::semantics::AssetSemantics>,
    /// Root pose of each placement (see [`gltf_root_pose`]): a renderer
    /// applying it as an FLU pose reproduces the placed asset.
    pub placements: Vec<Pose>,
    /// Verified collision proxy (`*-collision.glb`), when the manifest lists one.
    pub collision: Option<PathBuf>,
    /// Approved source tessellation or error-limited simplification contract.
    /// Never inferred from legacy proxies. The field name is retained for callers.
    pub source_tessellated_collision: bool,
}
/// One verified CAD package: where it was loaded from, the floor height it
/// records, and the named assets the layout needs.
#[derive(Clone, Debug, PartialEq)]
pub struct CadAssets {
    /// Package root. Every manifest path is resolved inside it, and every
    /// listed file is read only after its checksum matches.
    pub root: PathBuf,
    /// Top of the playing floor in the CAD arena frame; height zero in FLU.
    pub floor_top_cad_m: f64,
    /// Legacy package metadata; not permission to use an old collision proxy.
    pub collision_solids: bool,
    /// The arena floor slab, with no placements.
    pub floor: CadAsset,
    /// The stationary arena scenery, with no placements.
    pub arena_static: CadAsset,
    /// The rune assembly, with one placement.
    pub rune: CadAsset,
    /// The outpost tower, with one placement per team.
    pub outpost: CadAsset,
    /// The base, with one placement per team.
    pub base: CadAsset,
    /// The tech core, with one placement per team.
    pub tech_core: CadAsset,
    /// Additional scenery with verified collision proxies and rigid placements.
    pub static_assets: Vec<CadAsset>,
}

impl CadAsset {
    /// Sidecar bindings for the visual GLB, or `None` for a legacy asset.
    pub fn visual_semantics(&self) -> Option<&crate::semantics::FileSemantics> {
        self.semantics.as_ref()?.files.get("visual")
    }
    /// Sidecar bindings for the GLB the physics reads: the collision file when
    /// the asset declares a source tessellation, otherwise the visual file.
    pub fn physics_semantics(&self) -> Option<&crate::semantics::FileSemantics> {
        self.semantics
            .as_ref()?
            .files
            .get(if self.source_tessellated_collision {
                "collision"
            } else {
                "visual"
            })
    }
    /// Use a verified collider pipeline when declared; legacy assets keep visual geometry.
    pub fn physics_file(&self, root: &Path) -> PathBuf {
        if self.source_tessellated_collision
            && let Some(path) = &self.collision
        {
            return path.clone();
        }
        root.join(&self.file)
    }
}

impl CadAssets {
    /// Root pose of this package's arena frame; see [`arena_pose`].
    pub fn arena_pose(&self) -> Pose {
        arena_pose(self.floor_top_cad_m)
    }
}

/// Root pose of the CAD arena frame (whose axes are already FLU): the field
/// centred on the origin with the floor top (`floor_top_cad_m` in the arena
/// frame) at height zero.
///
/// ```
/// use rm_simulator_server::cad_assets::{FLOOR_TOP_CAD_M, arena_pose, cad_point_to_flu};
/// use rm_simulator_server::math::gltf_point;
///
/// // A CAD point under the arena root pose is the same as converting it.
/// let arena = arena_pose(FLOOR_TOP_CAD_M);
/// let point = [3.0, 1.0, -1.0];
/// let world = gltf_point(&arena, point);
/// let converted = cad_point_to_flu(point, FLOOR_TOP_CAD_M);
/// assert!(world.iter().zip(converted).all(|(a, b)| (a - b).abs() < 1e-12));
/// ```
pub fn arena_pose(floor_top_cad_m: f64) -> Pose {
    gltf_root_pose(
        cad_point_to_flu([0.0; 3], floor_top_cad_m),
        [1.0, 0.0, 0.0, 0.0],
    )
}

/// Read and verify the manifests below `root`. Every listed visual file must exist
/// and match its recorded SHA-256.
pub fn load(root: &Path) -> anyhow::Result<CadAssets> {
    let arena = read_manifest(&root.join("manifest.json"))?;
    let equipment = read_manifest(&root.join("equipment/manifest.json"))?;
    let arena_semantics = arena
        .articulation
        .as_ref()
        .map(|d| crate::semantics::load(root, d))
        .transpose()?
        .unwrap_or_default();
    let equipment_semantics = equipment
        .articulation
        .as_ref()
        .map(|d| crate::semantics::load(&root.join("equipment"), d))
        .transpose()?
        .unwrap_or_default();
    let floor_top_cad_m = arena.floor_top_source_z_m.unwrap_or(FLOOR_TOP_CAD_M);
    ensure!(
        (-3.0..=0.0).contains(&floor_top_cad_m),
        "manifest floor_top_source_z_m {floor_top_cad_m} is outside the arena"
    );
    let asset = |manifest: &Manifest, name: &str, prefix: &str, expected: usize| {
        let entry = manifest
            .assets
            .get(name)
            .ok_or_else(|| anyhow!("manifest lacks asset {name:?}"))?;
        let source_tessellated_collision = verified_collision(entry)?;
        let semantics = if prefix.is_empty() {
            arena_semantics.get(name)
        } else {
            equipment_semantics.get(name)
        }
        .cloned();
        ensure!(
            entry.semantics.is_some() == semantics.is_some(),
            "{name}: manifest/sidecar semantic binding differs"
        );
        if let Some(binding) = &entry.semantics {
            let descriptor = manifest
                .articulation
                .as_ref()
                .ok_or_else(|| anyhow!("missing articulation descriptor"))?;
            ensure!(
                binding["asset"].as_str() == Some(name)
                    && binding["file"].as_str() == Some(descriptor.file.as_str()),
                "{name}: invalid manifest semantic reference"
            );
        }
        if let Some(data) = &semantics {
            for (kind, binding) in &data.files {
                let (file, hash) = match kind.as_str() {
                    "visual" => (
                        Some(entry.visual.as_str()),
                        Some(entry.visual_sha256.as_str()),
                    ),
                    "collision" => (
                        entry.collision.as_deref(),
                        entry.collision_sha256.as_deref(),
                    ),
                    _ => anyhow::bail!("unknown semantic file kind {kind}"),
                };
                ensure!(
                    file == Some(binding.file.as_str()) && hash == Some(binding.sha256.as_str()),
                    "{name}: sidecar file differs from manifest"
                );
            }
            ensure!(
                data.files.contains_key("visual"),
                "missing visual semantics"
            );
            if source_tessellated_collision {
                ensure!(
                    data.files.contains_key("collision"),
                    "missing source collision semantics"
                );
            }
            // Gameplay supports the known rule-driven wheels. Reject incompatible
            // frames rather than silently misaligning scoring with the visual joint.
            let required: &[(&str, [f64; 3])] = match name {
                "rune" => &[
                    ("rune.face.0.spin", [0., 0., 1.]),
                    ("rune.face.1.spin", [0., 0., 1.]),
                ],
                "outpost" => &[("outpost.spin", [0., 1., 0.])],
                _ => &[],
            };
            for (id, axis) in required {
                for binding in data.files.values() {
                    let joint = binding
                        .joints
                        .iter()
                        .find(|j| j.id == *id)
                        .ok_or_else(|| anyhow!("{name}: missing rule joint {id}"))?;
                    ensure!(
                        joint.kind == "continuous"
                            && joint.axis == *axis
                            && joint.rotation_xyzw == [0., 0., 0., 1.]
                            && joint.limits.is_none(),
                        "{id}: unsupported rule joint frame"
                    );
                    let visual_joint = data.files["visual"]
                        .joints
                        .iter()
                        .find(|j| j.id == *id)
                        .ok_or_else(|| anyhow!("missing visual joint"))?;
                    ensure!(
                        joint.origin_m == visual_joint.origin_m,
                        "{id}: collision and visual pivots differ"
                    );
                }
            }
        }
        let file = format!("{prefix}{}", entry.visual);
        verify_sha256(&root.join(&file), &entry.visual_sha256)?;
        let collision = entry
            .collision
            .as_ref()
            .map(|collision| {
                let path = root.join(format!("{prefix}{collision}"));
                let expected = entry
                    .collision_sha256
                    .as_deref()
                    .ok_or_else(|| anyhow!("{name} lists a collision proxy without a checksum"))?;
                verify_sha256(&path, expected)?;
                Ok::<_, anyhow::Error>(path)
            })
            .transpose()?;
        ensure!(
            entry.placements_in_source_arena_frame.len() == expected,
            "{name} has {} placements, expected {expected}",
            entry.placements_in_source_arena_frame.len()
        );
        let placements = entry
            .placements_in_source_arena_frame
            .iter()
            .map(|placement| placement_pose(placement, floor_top_cad_m))
            .collect::<anyhow::Result<Vec<_>>>()?;
        ensure!(
            !source_tessellated_collision || collision.is_some(),
            "{name} declares source tessellation without a collision file"
        );
        Ok::<_, anyhow::Error>(CadAsset {
            file,
            semantics,
            placements,
            collision,
            source_tessellated_collision,
        })
    };
    let mut static_assets = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    for name in &arena.static_assets {
        ensure!(
            ![
                "floor",
                "arena-static",
                "rune",
                "outpost",
                "base",
                "tech-core"
            ]
            .contains(&name.as_str())
                && names.insert(name),
            "duplicate or reserved static asset {name:?}"
        );
        let entry = arena
            .assets
            .get(name)
            .ok_or_else(|| anyhow!("manifest lacks static asset {name:?}"))?;
        let count = entry.placements_in_source_arena_frame.len();
        ensure!(count > 0, "static asset {name:?} has no placements");
        let scenery = asset(&arena, name, "", count)?;
        static_assets.push(scenery);
    }
    Ok(CadAssets {
        static_assets,
        root: root.to_path_buf(),
        floor_top_cad_m,
        collision_solids: arena.collision_solids,
        floor: asset(&arena, "floor", "", 0)?,
        arena_static: asset(&arena, "arena-static", "", 0)?,
        rune: asset(&arena, "rune", "", 1)?,
        outpost: asset(&arena, "outpost", "", 2)?,
        base: asset(&equipment, "base", "equipment/", 2)?,
        tech_core: asset(&equipment, "tech-core", "equipment/", 2)?,
    })
}

/// Parse a manifest file and check the schema version and units that every
/// asset read depends on.
fn read_manifest(path: &Path) -> anyhow::Result<Manifest> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading CAD manifest {}", path.display()))?;
    let manifest: Manifest =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    ensure!(
        manifest.schema_version == 1,
        "{}: unsupported schema version {}",
        path.display(),
        manifest.schema_version
    );
    ensure!(
        manifest.units == "metres",
        "{}: units are {:?}, expected metres",
        path.display(),
        manifest.units
    );
    Ok(manifest)
}

/// Check a file against the checksum its manifest records. The file is read
/// whole and hashed; a missing or unreadable file is an error, and so is any
/// mismatch, with both digests in the message.
pub(crate) fn verify_sha256(path: &Path, expected: &str) -> anyhow::Result<()> {
    let bytes =
        std::fs::read(path).with_context(|| format!("reading CAD asset {}", path.display()))?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    ensure!(
        actual.eq_ignore_ascii_case(expected),
        "{} does not match the manifest checksum (expected {expected}, found {actual})",
        path.display()
    );
    Ok(())
}

/// A manifest placement (asset canonical frame into the CAD arena frame) as a
/// world root pose, including the arena's own placement.
fn placement_pose(placement: &ManifestPlacement, floor_top_cad_m: f64) -> anyhow::Result<Pose> {
    let t = placement.translation_m;
    let [x, y, z, w] = placement.rotation_xyzw;
    ensure!(
        t.iter().chain(&[x, y, z, w]).all(|v| v.is_finite()),
        "placement contains a non-finite value"
    );
    let rotation = [w, x, y, z];
    ensure!(
        (quat_length(rotation) - 1.0).abs() < 1e-4,
        "placement rotation is not unit length"
    );
    Ok(gltf_root_pose(
        cad_point_to_flu(t, floor_top_cad_m),
        quat_normalize(rotation),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::gltf_point;

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 1e-3)
    }
    #[test]
    fn installed_binaries_find_their_package_without_the_build_checkout() {
        let root = std::env::temp_dir().join(format!("rm-bundled-assets-{}", std::process::id()));
        std::fs::create_dir_all(root.join("field")).unwrap();
        std::fs::write(root.join("field/manifest.json"), "{}").unwrap();
        for binary in ["rm-simulator", "rm-simulator-server"] {
            assert_eq!(
                bundled_cad_assets(&root.join("bin").join(binary)),
                Some(root.join("field"))
            );
        }
        assert_eq!(bundled_cad_assets(&root.join("debug/rm-simulator")), None);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(bundled_cad_assets(&root.join("bin/rm-simulator")), None);
    }

    #[test]
    fn arena_frame_puts_the_floor_at_zero_and_maps_cad_axes_to_flu() {
        let arena = arena_pose(FLOOR_TOP_CAD_M);
        // The floor top at the field centre lands on the origin.
        let floor = gltf_point(&arena, [0.0, CENTER_CAD_Y_M, FLOOR_TOP_CAD_M]);
        assert!(close(floor, [0.0; 3]));
        // The point conversion agrees with the root pose.
        let point = [3.0, 1.0, -1.0];
        assert!(close(
            cad_point_to_flu(point, FLOOR_TOP_CAD_M),
            gltf_point(&arena, point)
        ));
        // A flat V1.2.0 slab records its own top; the same CAD point then sits
        // higher in FLU by the difference.
        let flat = -1.641_344;
        assert!(close(
            cad_point_to_flu(point, flat),
            gltf_point(&arena_pose(flat), point)
        ));
        assert!((cad_point_to_flu(point, flat)[2] - (-1.0 - flat)).abs() < 1e-6);
        // CAD x/y/z are FLU forward/left/up.
        for axis in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            assert!(close(
                gltf_point(&arena, axis),
                cad_point_to_flu(axis, FLOOR_TOP_CAD_M)
            ));
        }
    }
    #[test]
    fn only_explicit_source_tessellation_selects_a_collision_file() {
        let root = Path::new("/assets");
        let mut asset = CadAsset {
            semantics: None,
            file: "visual.glb".into(),
            placements: Vec::new(),
            collision: Some(root.join("collision.glb")),
            source_tessellated_collision: false,
        };
        assert_eq!(asset.physics_file(root), root.join("visual.glb"));
        asset.source_tessellated_collision = true;
        assert_eq!(asset.physics_file(root), root.join("collision.glb"));
    }

    #[test]
    fn placements_reproduce_measured_field_positions() {
        // Blue-side outpost from the manifest.
        let placement = ManifestPlacement {
            translation_m: [3.091651401311, 5.491744379119, -1.339703205233],
            rotation_xyzw: [0.0, 0.7163019434246493, 0.6977904598416853, 0.0],
        };
        let pose = placement_pose(&placement, FLOOR_TOP_CAD_M).unwrap();
        assert!(close(pose.translation_m, [3.0917, 3.8674, 0.1907]));
        // The asset's x axis (tower forward) points toward the field centre,
        // and its y axis is up.
        let forward = gltf_point(&pose, [1.0, 0.0, 0.0]);
        assert!(close(forward, [2.0917, 3.8674, 0.1907]));
        let up = gltf_point(&pose, [0.0, 1.0, 0.0]);
        assert!(up[2] - pose.translation_m[2] > 0.999, "{up:?}");
        // The root pose is a unit rotation.
        assert!((quat_length(pose.rotation_wxyz) - 1.0).abs() < 1e-9);
        assert!(
            placement_pose(
                &ManifestPlacement {
                    translation_m: [0.0, 0.0, f64::NAN],
                    rotation_xyzw: [0.0, 0.0, 0.0, 1.0],
                },
                FLOOR_TOP_CAD_M
            )
            .is_err()
        );
        assert!(
            placement_pose(
                &ManifestPlacement {
                    translation_m: [0.0; 3],
                    rotation_xyzw: [0.0, 0.0, 0.0, 2.0],
                },
                FLOOR_TOP_CAD_M
            )
            .is_err()
        );
    }
    #[test]
    fn simplified_colliders_require_valid_error_and_border_metadata() {
        let record = serde_json::json!({
            "method": "meshopt-simplification-v1", "error_limit_mm": 1.0,
            "estimated_error_mm": 0.5, "open_borders_locked": true,
            "sampled_deviation_limit_mm": 3.0, "max_sampled_deviation_mm": 1.0,
            "position_welding": "exact"
        });
        let mut entry = serde_json::json!({
            "visual": "a.glb", "visual_sha256": "unused",
            "collision_method": "meshopt-simplification-v1",
            "mesh_simplification": {"visual": record, "collision": record}
        });
        let parse = |value: &serde_json::Value| {
            serde_json::from_value::<ManifestAsset>(value.clone()).unwrap()
        };
        assert!(verified_collision(&parse(&entry)).unwrap());
        entry["mesh_simplification"]["collision"]["estimated_error_mm"] = serde_json::json!(2.0);
        assert!(verified_collision(&parse(&entry)).is_err());
        entry["mesh_simplification"]["collision"]["estimated_error_mm"] = serde_json::json!(0.5);
        entry["mesh_simplification"]["collision"]["open_borders_locked"] = serde_json::json!(false);
        assert!(verified_collision(&parse(&entry)).is_err());
        entry.as_object_mut().unwrap().remove("mesh_simplification");
        assert!(verified_collision(&parse(&entry)).is_err());
    }

    #[test]
    fn unlocked_borders_require_the_explicit_boundary_contract() {
        let locked = serde_json::json!({
            "method": "meshopt-simplification-v1", "error_limit_mm": 2.0,
            "estimated_error_mm": 1.0, "open_borders_locked": true,
            "sampled_deviation_limit_mm": 8.0, "max_sampled_deviation_mm": 3.0,
            "position_welding": "exact"
        });
        let mut unlocked = locked.clone();
        unlocked["method"] = "meshopt-boundary-simplification-v1".into();
        unlocked["open_borders_locked"] = false.into();
        unlocked["deviation_triangles_per_direction"] = 4096.into();
        let mut entry = serde_json::json!({
            "visual": "a.glb", "visual_sha256": "unused",
            "collision_method": "meshopt-boundary-simplification-v1",
            "mesh_simplification": {"visual": locked, "collision": unlocked}
        });
        let check = |value: &serde_json::Value| {
            verified_collision(&serde_json::from_value::<ManifestAsset>(value.clone()).unwrap())
        };
        assert!(check(&entry).unwrap());
        entry["mesh_simplification"]["visual"] = unlocked;
        assert!(check(&entry).unwrap());
        entry["collision_method"] = "meshopt-simplification-v1".into();
        assert!(check(&entry).is_err());
        entry["collision_method"] = "meshopt-boundary-simplification-v1".into();
        entry["mesh_simplification"]["collision"]["open_borders_locked"] = true.into();
        assert!(check(&entry).is_err());
        entry["mesh_simplification"]["collision"]["open_borders_locked"] = false.into();
        entry["mesh_simplification"]["collision"]["max_sampled_deviation_mm"] = 9.0.into();
        assert!(check(&entry).is_err());
        entry["mesh_simplification"]["collision"]["max_sampled_deviation_mm"] = 3.0.into();
        entry["mesh_simplification"]["collision"]["deviation_triangles_per_direction"] = 96.into();
        assert!(check(&entry).is_err());
    }

    #[test]
    fn additional_scenery_is_verified_and_legacy_packages_still_load() {
        let dir = std::env::temp_dir().join(format!("rm-static-assets-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("equipment")).unwrap();
        std::fs::write(dir.join("a.glb"), b"hello").unwrap();
        std::fs::write(dir.join("equipment/a.glb"), b"hello").unwrap();
        let entry = |count: usize| {
            serde_json::json!({
                "visual": "a.glb",
                "visual_sha256": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
                "collision": "a.glb",
                "collision_sha256": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
                "placements_in_source_arena_frame": vec![serde_json::json!({
                    "translation_m": [1.0, 2.0, 3.0], "rotation_xyzw": [0.0, 0.0, 0.0, 1.0]
                }); count]
            })
        };
        let mut manifest = serde_json::json!({"schema_version": 1, "units": "metres", "assets": {
            "floor": entry(0), "arena-static": entry(0), "rune": entry(1), "outpost": entry(2),
            "base": entry(2), "tech-core": entry(2), "centre-platform": entry(1)
        }});
        let save = |manifest: &serde_json::Value| {
            std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
        };
        std::fs::write(dir.join("equipment/manifest.json"), manifest.to_string()).unwrap();
        save(&manifest);
        assert!(load(&dir).unwrap().static_assets.is_empty());
        manifest["static_assets"] = serde_json::json!(["centre-platform"]);
        save(&manifest);
        let cad = load(&dir).unwrap();
        assert_eq!(cad.static_assets.len(), 1);
        assert!(close(
            cad.static_assets[0].placements[0].translation_m,
            cad_point_to_flu([1.0, 2.0, 3.0], FLOOR_TOP_CAD_M)
        ));
        manifest["assets"]["centre-platform"]["collision_method"] =
            serde_json::json!("source-tessellation-v1");
        save(&manifest);
        assert!(load(&dir).unwrap().static_assets[0].source_tessellated_collision);
        manifest["assets"]["centre-platform"]
            .as_object_mut()
            .unwrap()
            .remove("collision");
        save(&manifest);
        assert!(load(&dir).is_err());
        manifest["assets"]["centre-platform"]["collision"] = serde_json::json!("a.glb");
        manifest["assets"]["centre-platform"]["collision_sha256"] = serde_json::json!("bad");
        save(&manifest);
        assert!(load(&dir).is_err());
        manifest["assets"]["centre-platform"] = entry(0);
        save(&manifest);
        assert!(load(&dir).is_err());
        manifest["assets"]["centre-platform"] = entry(1);
        for names in [
            serde_json::json!(["missing"]),
            serde_json::json!(["rune"]),
            serde_json::json!(["centre-platform", "centre-platform"]),
        ] {
            manifest["static_assets"] = names;
            save(&manifest);
            assert!(load(&dir).is_err());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn checksum_and_manifest_validation_reject_bad_input() {
        let dir = std::env::temp_dir().join(format!("rm-simulator-cad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.glb");
        std::fs::write(&file, b"hello").unwrap();
        assert!(
            verify_sha256(
                &file,
                "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
            )
            .is_ok()
        );
        assert!(verify_sha256(&file, "00").is_err());
        assert!(verify_sha256(&dir.join("missing.glb"), "00").is_err());
        let manifest = dir.join("manifest.json");
        std::fs::write(
            &manifest,
            r#"{"schema_version":2,"units":"metres","assets":{}}"#,
        )
        .unwrap();
        assert!(read_manifest(&manifest).is_err());
        std::fs::write(
            &manifest,
            r#"{"schema_version":1,"units":"mm","assets":{}}"#,
        )
        .unwrap();
        assert!(read_manifest(&manifest).is_err());
        std::fs::write(
            &manifest,
            r#"{"schema_version":1,"units":"metres","assets":{}}"#,
        )
        .unwrap();
        let parsed = read_manifest(&manifest).unwrap();
        assert_eq!(parsed.floor_top_source_z_m, None);
        assert!(!parsed.collision_solids);
        std::fs::write(
            &manifest,
            r#"{"schema_version":1,"units":"metres","floor_top_source_z_m":-1.641344,"collision_solids":true,"assets":{}}"#,
        )
        .unwrap();
        let parsed = read_manifest(&manifest).unwrap();
        assert_eq!(parsed.floor_top_source_z_m, Some(-1.641_344));
        assert!(parsed.collision_solids);
        assert!(load(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
