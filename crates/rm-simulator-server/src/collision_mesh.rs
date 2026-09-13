// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! CAD triangle loading and indexed ground-height queries. The small GLB reader
//! preserves indexed triangles and node transforms; terrain assembly places
//! them in world FLU metres before building the ground footprint index.
use crate::cad_assets::cad_point_to_flu;
use crate::math::{Mat4, add, gltf_rotation, rotate};
use anyhow::{Context, anyhow, bail, ensure};
use rm_simulator_world::Pose;
use serde::Deserialize;
use std::path::Path;

/// `glTF` read little-endian, the first word of every GLB.
const GLB_MAGIC: u32 = 0x4654_6C67;
/// Chunk kind of the GLB JSON chunk.
const CHUNK_JSON: u32 = 0x4E4F_534A;
/// Chunk kind of the GLB binary buffer chunk.
const CHUNK_BIN: u32 = 0x004E_4942;
/// glTF primitive mode 4, a triangle list. Every other mode is refused.
const MODE_TRIANGLES: u32 = 4;
/// glTF accessor component type 5121, an unsigned byte.
const COMPONENT_U8: u32 = 5121;
/// glTF accessor component type 5123, an unsigned 16-bit integer.
const COMPONENT_U16: u32 = 5123;
/// glTF accessor component type 5125, an unsigned 32-bit integer.
const COMPONENT_U32: u32 = 5125;
/// glTF accessor component type 5126, a 32-bit float.
const COMPONENT_F32: u32 = 5126;

/// Triangle soup with shared vertices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CollisionMesh {
    /// Vertex positions, in world FLU metres once the soup has been converted
    /// with `into_flu` and placed.
    pub vertices_m: Vec<[f64; 3]>,
    /// Triangles as index triples into `vertices_m`.
    pub triangles: Vec<[u32; 3]>,
}
impl CollisionMesh {
    /// Move CAD arena-frame points into world FLU metres, with the floor top
    /// (`floor_top_cad_m` in the arena frame) at height zero.
    pub fn into_flu(mut self, floor_top_cad_m: f64) -> Self {
        for point in &mut self.vertices_m {
            *point = cad_point_to_flu(*point, floor_top_cad_m);
        }
        self
    }
    /// The same triangles (in an asset's glTF axes) placed by the asset's
    /// root pose, in world FLU.
    pub fn placed(&self, root: &Pose) -> Self {
        let rotation = gltf_rotation(root);
        CollisionMesh {
            vertices_m: self
                .vertices_m
                .iter()
                .map(|&point| add(root.translation_m, rotate(rotation, point)))
                .collect(),
            triangles: self.triangles.clone(),
        }
    }
    /// Append another soup, re-basing its indices.
    pub fn append(&mut self, other: CollisionMesh) {
        let base = self.vertices_m.len() as u32;
        self.vertices_m.extend(other.vertices_m);
        self.triangles
            .extend(other.triangles.iter().map(|t| t.map(|i| i + base)));
    }
    /// Height of the highest surface at or below `z_max` over `(x, y)`, from
    /// the triangles whose footprint covers that point. Vertical faces do not
    /// count.
    ///
    /// ```
    /// use rm_simulator_server::collision_mesh::CollisionMesh;
    ///
    /// // Two FLU triangles forming a 2 m square pad with its top at 0.5 m.
    /// let mesh = CollisionMesh {
    ///     vertices_m: vec![
    ///         [0.0, 0.0, 0.5],
    ///         [2.0, 0.0, 0.5],
    ///         [0.0, 2.0, 0.5],
    ///         [2.0, 2.0, 0.5],
    ///     ],
    ///     triangles: vec![[0, 1, 2], [3, 2, 1]],
    /// };
    /// // The pad top is the highest surface at or below the query height.
    /// assert_eq!(mesh.ground_height_below(0.5, 0.5, 1.0), Some(0.5));
    /// // A query below the pad finds nothing.
    /// assert_eq!(mesh.ground_height_below(0.5, 0.5, 0.4), None);
    /// ```
    pub fn ground_height_below(&self, x: f64, y: f64, z_max: f64) -> Option<f64> {
        self.triangles
            .iter()
            .filter_map(|triangle| triangle_height(self, triangle, x, y, z_max))
            .reduce(f64::max)
    }
}

/// Height of `triangle` over `(x, y)`, or `None` when the point lies outside its
/// footprint, the triangle projects to a floor line, or the surface is more than
/// a millimetre above `z_max`.
fn triangle_height(
    mesh: &CollisionMesh,
    triangle: &[u32; 3],
    x: f64,
    y: f64,
    z_max: f64,
) -> Option<f64> {
    let [a, b, c] = triangle.map(|i| mesh.vertices_m[i as usize]);
    let det = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
    if det.abs() < 1e-12 {
        return None;
    }
    let u = ((x - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (y - a[1])) / det;
    let v = ((b[0] - a[0]) * (y - a[1]) - (x - a[0]) * (b[1] - a[1])) / det;
    if u < -1e-9 || v < -1e-9 || u + v > 1.0 + 1e-9 {
        return None;
    }
    let z = a[2] + u * (b[2] - a[2]) + v * (c[2] - a[2]);
    // Preserve the spawn query's millimetre of height slack.
    (z <= z_max + 1e-3).then_some(z)
}

/// Immutable ground geometry with a bounding-volume tree over triangle footprints.
/// Build after placement and assembly. Exposing only shared mesh access prevents
/// edits from invalidating the index; physics still receives the original triangles.
#[derive(Debug)]
pub struct GroundMesh {
    mesh: CollisionMesh,
    root: GroundNode,
}

/// Bounding-volume tree over triangle footprints: a leaf holds up to eight
/// indices, a branch splits the rest along the wider floor axis.
#[derive(Debug)]
enum GroundNode {
    Leaf {
        bounds: [f64; 4],
        triangles: Vec<usize>,
    },
    Branch {
        bounds: [f64; 4],
        children: Box<[GroundNode; 2]>,
    },
}

impl GroundNode {
    /// Builds the tree over `triangles`, recursing until a leaf holds at most
    /// eight.
    fn build(mesh: &CollisionMesh, triangles: &mut [usize]) -> Self {
        let mut bounds = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for &index in triangles.iter() {
            let footprint = triangle_bounds(mesh, index);
            for axis in 0..2 {
                bounds[axis] = bounds[axis].min(footprint[axis]);
                bounds[axis + 2] = bounds[axis + 2].max(footprint[axis + 2]);
            }
        }
        if triangles.len() <= 8 {
            return Self::Leaf {
                bounds,
                triangles: triangles.to_vec(),
            };
        }
        let axis = usize::from(bounds[3] - bounds[1] > bounds[2] - bounds[0]);
        let middle = triangles.len() / 2;
        triangles.select_nth_unstable_by(middle, |&a, &b| {
            let a = triangle_bounds(mesh, a);
            let b = triangle_bounds(mesh, b);
            (a[axis] + a[axis + 2]).total_cmp(&(b[axis] + b[axis + 2]))
        });
        let (left, right) = triangles.split_at_mut(middle);
        Self::Branch {
            bounds,
            children: Box::new([Self::build(mesh, left), Self::build(mesh, right)]),
        }
    }

    /// Calls `visit` with every triangle index whose footprint can contain
    /// `(x, y)`. Prunes any node whose bounds miss the point, so distant
    /// triangles are never tested.
    fn visit(&self, x: f64, y: f64, visit: &mut impl FnMut(usize)) {
        let bounds = match self {
            Self::Leaf { bounds, .. } | Self::Branch { bounds, .. } => bounds,
        };
        if x < bounds[0] || y < bounds[1] || x > bounds[2] || y > bounds[3] {
            return;
        }
        match self {
            Self::Leaf { triangles, .. } => triangles.iter().for_each(|&index| visit(index)),
            Self::Branch { children, .. } => {
                for child in children.iter() {
                    child.visit(x, y, visit);
                }
            }
        }
    }
}

/// Floor-plane bounds of one triangle, widened to cover the barycentric test's
/// edge tolerance.
fn triangle_bounds(mesh: &CollisionMesh, index: usize) -> [f64; 4] {
    let points = mesh.triangles[index].map(|i| mesh.vertices_m[i as usize]);
    let mut bounds = [0.0; 4];
    for axis in 0..2 {
        let min = points.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min);
        let max = points
            .iter()
            .map(|p| p[axis])
            .fold(f64::NEG_INFINITY, f64::max);
        // The barycentric test admits weights down to -1e-9, including a
        // weight of -2e-9 for the first vertex. Expand the footprint to match.
        let slack = 4e-9 * (max - min) + 16.0 * f64::EPSILON * min.abs().max(max.abs()).max(1.0);
        bounds[axis] = min - slack;
        bounds[axis + 2] = max + slack;
    }
    bounds
}

impl From<CollisionMesh> for GroundMesh {
    fn from(mesh: CollisionMesh) -> Self {
        let mut triangles: Vec<_> = (0..mesh.triangles.len()).collect();
        let root = GroundNode::build(&mesh, &mut triangles);
        Self { mesh, root }
    }
}

impl std::ops::Deref for GroundMesh {
    type Target = CollisionMesh;
    fn deref(&self) -> &Self::Target {
        &self.mesh
    }
}

impl GroundMesh {
    /// Highest surface below the point, using the same triangle test and
    /// tolerances as CollisionMesh, with unrelated footprints pruned.
    pub fn ground_height_below(&self, x: f64, y: f64, z_max: f64) -> Option<f64> {
        let mut best: Option<f64> = None;
        self.root.visit(x, y, &mut |index| {
            if let Some(z) = triangle_height(&self.mesh, &self.mesh.triangles[index], x, y, z_max) {
                best = Some(best.map_or(z, |best| best.max(z)));
            }
        });
        best
    }
}

/// The subset of the glTF JSON this reader needs.
#[derive(Deserialize)]
struct Gltf {
    #[serde(default)]
    scene: Option<usize>,
    #[serde(default)]
    scenes: Vec<Scene>,
    #[serde(default)]
    nodes: Vec<Node>,
    #[serde(default)]
    meshes: Vec<Mesh>,
    #[serde(default)]
    accessors: Vec<Accessor>,
    #[serde(default, rename = "bufferViews")]
    buffer_views: Vec<BufferView>,
    #[serde(default, rename = "extensionsRequired")]
    extensions_required: Vec<String>,
}
/// A glTF scene: the root nodes the reader starts from.
#[derive(Deserialize)]
struct Scene {
    #[serde(default)]
    nodes: Vec<usize>,
}
/// A glTF node: its name, mesh, children and either a matrix or a TRS
/// transform.
#[derive(Deserialize, Default)]
struct Node {
    #[serde(default)]
    name: String,
    mesh: Option<usize>,
    #[serde(default)]
    children: Vec<usize>,
    matrix: Option<[f64; 16]>,
    translation: Option<[f64; 3]>,
    rotation: Option<[f64; 4]>,
    scale: Option<[f64; 3]>,
}
/// A glTF mesh, which holds one or more primitives.
#[derive(Deserialize)]
struct Mesh {
    primitives: Vec<Primitive>,
}
/// One glTF primitive: its attribute accessors, optional index accessor and
/// mode.
#[derive(Deserialize)]
struct Primitive {
    attributes: std::collections::BTreeMap<String, usize>,
    indices: Option<usize>,
    #[serde(default = "default_mode")]
    mode: u32,
}
/// glTF's default primitive mode, a triangle list, used when a primitive omits
/// `mode`.
fn default_mode() -> u32 {
    MODE_TRIANGLES
}
/// A glTF accessor: where its elements live and what they contain.
#[derive(Deserialize)]
struct Accessor {
    #[serde(rename = "bufferView")]
    buffer_view: Option<usize>,
    #[serde(default, rename = "byteOffset")]
    byte_offset: usize,
    #[serde(rename = "componentType")]
    component_type: u32,
    count: usize,
    #[serde(rename = "type")]
    kind: String,
}
/// A glTF buffer view: a byte range of the embedded buffer with an optional
/// stride.
#[derive(Deserialize)]
struct BufferView {
    buffer: usize,
    #[serde(default, rename = "byteOffset")]
    byte_offset: usize,
    #[serde(rename = "byteLength")]
    byte_length: usize,
    #[serde(rename = "byteStride")]
    byte_stride: Option<usize>,
}

/// One mesh node of a GLB, named after the node (or its nearest named
/// ancestor), in the file's own frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CollisionPart {
    /// The node's name, or its nearest named ancestor's when the node itself is
    /// unnamed.
    pub name: String,
    /// The node's triangles with every ancestor transform applied, still in the
    /// file's own frame.
    pub mesh: CollisionMesh,
}

/// Read a GLB collision proxy as one soup; points stay in the file's own frame.
pub fn load_glb(path: &Path) -> anyhow::Result<CollisionMesh> {
    let mut mesh = CollisionMesh::default();
    for part in load_glb_parts(path)? {
        mesh.append(part.mesh);
    }
    Ok(mesh)
}

/// Read a GLB collision proxy part by part (one per mesh node, in document
/// order); points stay in the file's own frame.
pub fn load_glb_parts(path: &Path) -> anyhow::Result<Vec<CollisionPart>> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading collision proxy {}", path.display()))?;
    parse_glb(&bytes).with_context(|| format!("parsing collision proxy {}", path.display()))
}

/// Reads the little-endian u32 at `offset`, or an error when the file ends
/// first.
fn u32_at(bytes: &[u8], offset: usize) -> anyhow::Result<u32> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| anyhow!("truncated at byte {offset}"))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Parses a GLB held in memory, keeping one part per mesh node.
fn parse_glb(bytes: &[u8]) -> anyhow::Result<Vec<CollisionPart>> {
    parse_glb_selected(bytes, None)
}
/// Load precisely the selected (node index, primitive index) pairs, retaining
/// all ancestor transforms. Indices come from the checksum-verified sidecar.
pub fn load_glb_selected(
    path: &Path,
    selected: &std::collections::BTreeSet<(usize, usize)>,
) -> anyhow::Result<CollisionMesh> {
    let bytes = std::fs::read(path)?;
    let mut mesh = CollisionMesh::default();
    for part in parse_glb_selected(&bytes, Some(selected))? {
        mesh.append(part.mesh);
    }
    Ok(mesh)
}
/// Parses a GLB keeping only the selected (node, primitive) pairs.
fn parse_glb_selected(
    bytes: &[u8],
    selected: Option<&std::collections::BTreeSet<(usize, usize)>>,
) -> anyhow::Result<Vec<CollisionPart>> {
    parse_glb_filtered(bytes, selected, &[], None)
}
/// Load selected collision triangles, optionally excluding joint subtrees or
/// retaining only one joint subtree. Ancestor transforms are preserved.
pub fn load_glb_filtered(
    path: &Path,
    selected: &std::collections::BTreeSet<(usize, usize)>,
    excluded: &[usize],
    subtree: Option<usize>,
) -> anyhow::Result<CollisionMesh> {
    let mut mesh = CollisionMesh::default();
    for part in parse_glb_filtered(&std::fs::read(path)?, Some(selected), excluded, subtree)? {
        mesh.append(part.mesh);
    }
    Ok(mesh)
}
/// Parses a GLB under the selection, exclusion and subtree filters. Walks the
/// scene depth-first in document order and transforms each node's vertices by
/// its whole ancestor chain. Errors on a bad GLB magic or version, a truncated
/// chunk, a missing JSON chunk, a file that requires glTF extensions, a missing
/// scene, a node tree deeper than 64 levels, and any primitive failure or
/// out-of-range index.
fn parse_glb_filtered(
    bytes: &[u8],
    selected: Option<&std::collections::BTreeSet<(usize, usize)>>,
    excluded: &[usize],
    subtree: Option<usize>,
) -> anyhow::Result<Vec<CollisionPart>> {
    ensure!(u32_at(bytes, 0)? == GLB_MAGIC, "not a binary glTF file");
    ensure!(u32_at(bytes, 4)? == 2, "unsupported GLB version");
    let mut offset = 12;
    let mut json: Option<&[u8]> = None;
    let mut bin: Option<&[u8]> = None;
    while offset + 8 <= bytes.len() {
        let length = u32_at(bytes, offset)? as usize;
        let kind = u32_at(bytes, offset + 4)?;
        let data = bytes
            .get(offset + 8..offset + 8 + length)
            .ok_or_else(|| anyhow!("chunk runs past the end of the file"))?;
        match kind {
            CHUNK_JSON => json = Some(data),
            CHUNK_BIN => bin = Some(data),
            _ => {}
        }
        offset += 8 + length;
    }
    let json = json.ok_or_else(|| anyhow!("GLB lacks a JSON chunk"))?;
    let gltf: Gltf = serde_json::from_slice(json).context("glTF JSON")?;
    ensure!(
        gltf.extensions_required.is_empty(),
        "glTF requires extensions {:?}",
        gltf.extensions_required
    );
    let bin = bin.unwrap_or(&[]);
    let mut parts = Vec::new();
    let scene = gltf
        .scenes
        .get(gltf.scene.unwrap_or(0))
        .ok_or_else(|| anyhow!("glTF has no scene"))?;
    // Depth-first in document order (the stack pops from the back).
    let mut stack: Vec<(usize, Mat4, usize, String, bool)> = scene
        .nodes
        .iter()
        .rev()
        .map(|&node| (node, Mat4::IDENTITY, 0, String::new(), subtree.is_none()))
        .collect();
    while let Some((index, parent, depth, inherited, included)) = stack.pop() {
        if excluded.contains(&index) {
            continue;
        }
        let included = included || subtree == Some(index);
        ensure!(depth < 64, "node tree is too deep or cyclic");
        let node = gltf
            .nodes
            .get(index)
            .ok_or_else(|| anyhow!("node {index} is out of range"))?;
        let name = if node.name.is_empty() {
            inherited
        } else {
            node.name.clone()
        };
        let local = match node.matrix {
            Some(matrix) => Mat4::from_cols_array(&matrix),
            None => Mat4::from_scale_rotation_translation(
                node.scale.unwrap_or([1.0; 3]),
                node.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]),
                node.translation.unwrap_or([0.0; 3]),
            ),
        };
        let world = parent.mul(&local);
        if let Some(mesh_index) = node.mesh {
            let primitives = &gltf
                .meshes
                .get(mesh_index)
                .ok_or_else(|| anyhow!("mesh {mesh_index} is out of range"))?
                .primitives;
            let mut mesh = CollisionMesh::default();
            for (primitive_index, primitive) in primitives.iter().enumerate() {
                if !included || selected.is_some_and(|s| !s.contains(&(index, primitive_index))) {
                    continue;
                }
                append_primitive(&mut mesh, &gltf, bin, primitive, &world)?;
            }
            parts.push(CollisionPart {
                name: name.clone(),
                mesh,
            });
        }
        stack.extend(
            node.children
                .iter()
                .rev()
                .map(|&child| (child, world, depth + 1, name.clone(), included)),
        );
    }
    Ok(parts)
}

/// Transforms one primitive's positions by `world` and appends its triangles to
/// `mesh`, re-basing the indices. Without an index accessor the positions are
/// read as a triangle list in order. Refuses a mode that is not a triangle list,
/// a missing POSITION, a position accessor that is not a float VEC3, a
/// non-scalar index accessor, an unsupported index component type, a
/// non-finite vertex, an index count that is not a multiple of three and any
/// index past the vertex count.
fn append_primitive(
    mesh: &mut CollisionMesh,
    gltf: &Gltf,
    bin: &[u8],
    primitive: &Primitive,
    world: &Mat4,
) -> anyhow::Result<()> {
    ensure!(
        primitive.mode == MODE_TRIANGLES,
        "primitive mode {} is not a triangle list",
        primitive.mode
    );
    let position = *primitive
        .attributes
        .get("POSITION")
        .ok_or_else(|| anyhow!("primitive lacks POSITION"))?;
    let positions = read_accessor(gltf, bin, position)?;
    ensure!(
        positions.kind == "VEC3" && positions.component_type == COMPONENT_F32,
        "POSITION must be float VEC3"
    );
    let base = mesh.vertices_m.len() as u32;
    for index in 0..positions.count {
        let element = positions.element(index)?;
        let point = world.transform_point([
            f32_le(&element[0..4]),
            f32_le(&element[4..8]),
            f32_le(&element[8..12]),
        ]);
        ensure!(point.iter().all(|v| v.is_finite()), "non-finite vertex");
        mesh.vertices_m.push(point);
    }
    let indices: Vec<u32> = match primitive.indices {
        Some(accessor) => {
            let indices = read_accessor(gltf, bin, accessor)?;
            ensure!(indices.kind == "SCALAR", "indices must be scalars");
            (0..indices.count)
                .map(|i| {
                    let element = indices.element(i)?;
                    Ok(match indices.component_type {
                        COMPONENT_U8 => element[0] as u32,
                        COMPONENT_U16 => u16::from_le_bytes([element[0], element[1]]) as u32,
                        COMPONENT_U32 => {
                            u32::from_le_bytes([element[0], element[1], element[2], element[3]])
                        }
                        other => bail!("unsupported index component type {other}"),
                    })
                })
                .collect::<anyhow::Result<_>>()?
        }
        None => (0..positions.count as u32).collect(),
    };
    ensure!(
        indices.len().is_multiple_of(3),
        "index count is not a multiple of three"
    );
    for triangle in indices.as_chunks::<3>().0 {
        ensure!(
            triangle.iter().all(|&i| (i as usize) < positions.count),
            "index out of range"
        );
        mesh.triangles.push(triangle.map(|i| base + i));
    }
    Ok(())
}

/// Reads a little-endian f32 as f64. glTF stores vertex positions as f32; the
/// mesh keeps f64.
fn f32_le(bytes: &[u8]) -> f64 {
    f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64
}

/// A resolved accessor: the bytes from its first element, the stride between
/// elements, the width of one element and what it declares.
struct AccessorData<'a> {
    data: &'a [u8],
    stride: usize,
    size: usize,
    count: usize,
    component_type: u32,
    kind: String,
}
impl AccessorData<'_> {
    /// Bytes of element `index`, or an error when it runs past the buffer view.
    fn element(&self, index: usize) -> anyhow::Result<&[u8]> {
        let start = index * self.stride;
        self.data
            .get(start..start + self.size)
            .ok_or_else(|| anyhow!("accessor element {index} runs past its buffer view"))
    }
}

/// Resolves one accessor against the embedded binary chunk, which must be
/// buffer 0. Errors on an out-of-range accessor or buffer view, a missing buffer
/// view, an unsupported component type, an unsupported accessor type, a stride
/// smaller than the element and any range that runs past its buffer view.
fn read_accessor<'a>(gltf: &Gltf, bin: &'a [u8], index: usize) -> anyhow::Result<AccessorData<'a>> {
    let accessor = gltf
        .accessors
        .get(index)
        .ok_or_else(|| anyhow!("accessor {index} is out of range"))?;
    let view_index = accessor
        .buffer_view
        .ok_or_else(|| anyhow!("accessor {index} has no buffer view"))?;
    let view = gltf
        .buffer_views
        .get(view_index)
        .ok_or_else(|| anyhow!("buffer view {view_index} is out of range"))?;
    ensure!(
        view.buffer == 0,
        "only the embedded binary buffer is supported"
    );
    let component = match accessor.component_type {
        COMPONENT_U8 => 1,
        COMPONENT_U16 => 2,
        COMPONENT_U32 | COMPONENT_F32 => 4,
        other => bail!("unsupported component type {other}"),
    };
    let components = match accessor.kind.as_str() {
        "SCALAR" => 1,
        "VEC3" => 3,
        other => bail!("unsupported accessor type {other}"),
    };
    let size = component * components;
    let stride = view.byte_stride.unwrap_or(size);
    ensure!(
        stride >= size,
        "buffer view stride is smaller than its elements"
    );
    let view_data = bin
        .get(view.byte_offset..view.byte_offset + view.byte_length)
        .ok_or_else(|| anyhow!("buffer view {view_index} runs past the binary chunk"))?;
    let data = view_data
        .get(accessor.byte_offset..)
        .ok_or_else(|| anyhow!("accessor {index} starts past its buffer view"))?;
    ensure!(
        accessor.count == 0 || (accessor.count - 1) * stride + size <= data.len(),
        "accessor {index} runs past its buffer view"
    );
    Ok(AccessorData {
        data,
        stride,
        size,
        count: accessor.count,
        component_type: accessor.component_type,
        kind: accessor.kind.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GLB with one right triangle in the xy plane and one raised copy of it
    /// under a translated node (so node transforms are exercised).
    fn sample_glb() -> Vec<u8> {
        let mut bin = Vec::new();
        for value in [0.0f32, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0, 0.0] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0u16, 1, 2] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
        bin.extend_from_slice(&[0, 0]);
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0,1]}}],
            "nodes":[{{"mesh":0,"name":"flat"}},{{"name":"raised","children":[2]}},
            {{"mesh":0,"translation":[0,0,0.5]}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1}}]}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},
            {{"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}}],
            "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},
            {{"buffer":0,"byteOffset":36,"byteLength":6}}],
            "buffers":[{{"byteLength":{}}}]}}"#,
            bin.len()
        );
        let mut json = json.into_bytes();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let mut glb = Vec::new();
        glb.extend_from_slice(&GLB_MAGIC.to_le_bytes());
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(&CHUNK_JSON.to_le_bytes());
        glb.extend_from_slice(&json);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(&CHUNK_BIN.to_le_bytes());
        glb.extend_from_slice(&bin);
        glb
    }

    #[test]
    fn joint_subtree_filter_preserves_triangles_and_ancestor_placement() {
        let bytes = sample_glb();
        let fixed = parse_glb_filtered(&bytes, None, &[1], None).unwrap();
        let moving = parse_glb_filtered(&bytes, None, &[], Some(1)).unwrap();
        let triangles =
            |parts: &[CollisionPart]| parts.iter().map(|p| p.mesh.triangles.len()).sum::<usize>();
        assert_eq!(triangles(&fixed), 1);
        assert_eq!(triangles(&moving), 1);
        assert_eq!(triangles(&parse_glb(&bytes).unwrap()), 2);
        let vertices: Vec<_> = moving.iter().flat_map(|p| &p.mesh.vertices_m).collect();
        assert_eq!(vertices.len(), 3);
        assert!(vertices.iter().all(|v| v[2] == 0.5));
    }

    fn layered_grid() -> CollisionMesh {
        let mut mesh = CollisionMesh::default();
        for x in -16..16 {
            for y in -16..16 {
                for z in [0.0, 0.65] {
                    let x = f64::from(x);
                    let y = f64::from(y);
                    let base = mesh.vertices_m.len() as u32;
                    mesh.vertices_m.extend([
                        [x, y, z],
                        [x + 1.0, y, z + 0.1],
                        [x, y + 1.0, z],
                        [x + 1.0, y + 1.0, z + 0.1],
                    ]);
                    mesh.triangles
                        .extend([[base, base + 1, base + 2], [base + 3, base + 2, base + 1]]);
                }
            }
        }
        mesh
    }

    #[test]
    fn ground_index_matches_full_scan_at_edges_slopes_and_stacked_surfaces() {
        let ground = GroundMesh::from(layered_grid());
        for i in -34..=34 {
            for j in -34..=34 {
                let x = f64::from(i) * 0.5;
                let y = f64::from(j) * 0.5;
                for z in [-0.1, 0.0, 0.049, 0.1, 0.649, 0.7, 1.0] {
                    assert_eq!(
                        ground.ground_height_below(x, y, z),
                        ground.mesh.ground_height_below(x, y, z),
                        "{x}, {y}, {z}"
                    );
                }
            }
        }
        assert_eq!(ground.ground_height_below(0.5, 0.25, 0.5), Some(0.05));
        assert_eq!(ground.ground_height_below(0.5, 0.25, 1.0), Some(0.7));
    }

    #[test]
    fn ground_index_keeps_edge_tolerance_and_skips_vertical_or_degenerate_faces() {
        let mesh = CollisionMesh {
            vertices_m: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            triangles: vec![[0, 1, 2], [0, 1, 3], [0, 0, 0]],
        };
        let ground = GroundMesh::from(mesh);
        for (x, y) in [
            (-0.5e-9, 0.5),
            (1.0 + 1.5e-9, -0.75e-9),
            (0.5, 0.5 + 0.5e-9),
            (-1e-6, 0.5),
        ] {
            assert_eq!(
                ground.ground_height_below(x, y, 0.0),
                ground.mesh.ground_height_below(x, y, 0.0)
            );
        }
        assert_eq!(ground.ground_height_below(0.25, 0.25, -0.001), Some(0.0));
        assert_eq!(ground.ground_height_below(0.25, 0.25, -0.0011), None);
        assert_eq!(
            GroundMesh::from(CollisionMesh::default()).ground_height_below(0.0, 0.0, 1.0),
            None
        );
    }

    #[test]
    fn ground_index_prunes_distant_triangles_without_changing_geometry() {
        let mesh = layered_grid();
        let original = mesh.clone();
        let ground = GroundMesh::from(mesh);
        assert_eq!(*ground, original);
        let mut tested = 0;
        ground.root.visit(0.25, 0.25, &mut |_| tested += 1);
        assert!(tested > 0);
        assert!(tested < ground.triangles.len() / 20, "{tested} candidates");
        let mut outside = 0;
        ground.root.visit(100.0, 100.0, &mut |_| outside += 1);
        assert_eq!(outside, 0);
    }

    #[test]
    fn glb_triangles_follow_node_transforms_and_report_ground_height() {
        let parts = parse_glb(&sample_glb()).unwrap();
        // Mesh nodes take their own name or the nearest named ancestor's.
        assert_eq!(
            parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["flat", "raised"]
        );
        assert_eq!(parts[1].mesh.vertices_m[1], [2.0, 0.0, 0.5]);
        let mut mesh = CollisionMesh::default();
        for part in parts {
            mesh.append(part.mesh);
        }
        assert_eq!(mesh.vertices_m.len(), 6);
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [3, 4, 5]]);
        assert_eq!(mesh.vertices_m[4], [2.0, 0.0, 0.5]);
        // The raised copy is the ground under a point above it; the flat one
        // is found when the search starts below the raised copy.
        assert_eq!(mesh.ground_height_below(0.5, 0.5, 1.0), Some(0.5));
        assert_eq!(mesh.ground_height_below(0.5, 0.5, 0.2), Some(0.0));
        assert_eq!(mesh.ground_height_below(1.5, 1.5, 1.0), None);
        // An identity root pose reads glTF x as FLU -y and glTF -z as FLU
        // forward; this one sits 1 m forward.
        let placed = mesh.placed(&Pose::at([1.0, 0.0, 0.0]));
        assert_eq!(placed.vertices_m[1], [1.0, -2.0, 0.0]);
        assert_eq!(placed.vertices_m[5], [0.5, 0.0, 2.0]);
        let mut merged = CollisionMesh::default();
        merged.append(mesh.clone());
        merged.append(mesh);
        assert_eq!(merged.triangles[2], [6, 7, 8]);
        assert_eq!(merged.vertices_m.len(), 12);
    }
    #[test]
    fn malformed_glb_files_are_rejected() {
        assert!(parse_glb(b"not a glb").is_err());
        let mut glb = sample_glb();
        // Point an index past the vertex count.
        let position = glb.len() - 4;
        glb[position..position + 2].copy_from_slice(&9u16.to_le_bytes());
        assert!(parse_glb(&glb).is_err());
        let truncated = &sample_glb()[..40];
        assert!(parse_glb(truncated).is_err());
    }
    #[test]
    fn into_flu_moves_the_cad_floor_top_to_zero() {
        let mesh = CollisionMesh {
            vertices_m: vec![[
                1.0,
                crate::cad_assets::CENTER_CAD_Y_M,
                crate::cad_assets::FLOOR_TOP_CAD_M,
            ]],
            triangles: Vec::new(),
        }
        .into_flu(crate::cad_assets::FLOOR_TOP_CAD_M);
        assert!(
            mesh.vertices_m[0]
                .iter()
                .zip(&[1.0, 0.0, 0.0])
                .all(|(a, b)| (a - b).abs() < 1e-9)
        );
    }
}
