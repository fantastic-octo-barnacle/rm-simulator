<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Semantic assets

The asset-contract guide: rm-map-tools' semantic sidecar, the collision and
simplification contracts, the recorded geometry counts, and each build's
provenance and checksums. [Field package](field-package.md) owns package
composition, discovery, deployment and terrain; [field detail](field-detail.md)
records reproducible simplification and installation. The
[documentation index](README.md) lists every guide.

The simulator loads rm-map-tools' `articulation.json` alongside the checksummed
visual and collision GLBs. A declared but invalid sidecar stops loading: the
loader checks hashes, schema, units, coordinate conventions, unique IDs,
hierarchy, joint frames and zero rest transforms, and it rejects unsupported
gameplay joint axes/frames rather than silently placing scoring targets
incorrectly. Packages without semantic data retain the legacy loader. The local
package lives in `local-assets/field` and is ignored by Git; `--cad-assets`
overrides discovery, whose full order (`field/` beside an installed `bin/`
directory, the build checkout's `local-assets/field`, the checkout's LFS
`field/`, then `~/dev/RM/assets/rm2026-field`) is in
[field package](field-package.md). New exports may declare the
`source-tessellation-v1` collision contract, also described there.

A development installation can place built executables at `local-assets/bin/rm-simulator` and
`local-assets/bin/rm-simulator-server`. They find the package at `../field` relative to their `bin` directory, so a
worktree build can be installed without depending on its source checkout. Run:

```sh
./local-assets/bin/rm-simulator --fly
```

Build and run from the repository:

```sh
cargo run -p rm-simulator-app -- --fly
cargo run -p rm-simulator-server --example inspect_assets -- local-assets/field
```

To compose a fresh package into an unused directory:

```sh
python3 scripts/build-semantic-assets.py \
  --optimized ../rm-map-tools/out/detail-preview-runtime \
  --semantic ../assets/rm2026-reference \
  --out local-assets/field
```

The compositor verifies input hashes and records them in `build-provenance.json`.
It uses optimized scenery and the reference's semantic rune, outpost, base,
tech-core and
dart station. The gate selection uses exact triangle ranges, so the compositor
keeps its pinned source geometry during composition. It does not transplant ranges
onto a different tessellation. The subsequent mesh simplifier operates within
already bound primitives, so it can reduce those meshes without reapplying the
original triangle selectors.

## Geometry counts and contracts

The earlier native-texture build contained 878,139 placed visual triangles,
including artwork quads, compared with 1,172,056 in the preceding package.
Arena backing simplification is enabled, and the base visual uses a 4 mm metric
and 12 mm sampled limit after its lettering is extracted. Textures and alpha
masks are embedded in the GLBs; `texture-atlas.json` is a provenance report and
is not needed by the renderer. Source semantic anchors and joint frames remain.

In that build, selected artwork was removed from collider sources before a new
simplification pass. The inspector reports 575,589 ground, 43,780 fixture and
209,460 equipment triangles, totaling 828,829 versus the preceding 927,028.
Visual-only quads do not become colliders. Collision uses a 4 mm metric and
8 mm sampled limit; its settings remain independent of visual simplification. The `meshopt-boundary-simplification-v1` contract continues to require
checksummed files, matching border declarations and finite error metadata.
Checks sample 4,096 triangles per direction; these are not certified maximum
errors against STEP. The coarser backing geometry remains visible up close.

Reproduce the candidate with rm-map-tools' native-texture build script. It
records source inputs, exporter settings, counts and validation. The native-
texture deployment preserves local package additions and regenerates the minimap
against the new manifest hashes. No frame-time improvement has been measured for
this package.

## What comes from the export

- Checksummed semantic IDs, node and primitive bindings, joint origins, axes,
  joint types, limits, and the selected glTF scene.
- Rune and outpost joint motion. The renderer rotates the exported motion node;
  the exporter supplies its pivot parent and the children's rebased rest poses.
  Display names can change without breaking the bindings.
- Stationary geometry selected by joint hierarchy. Fixed rune shaft caps and
  logos remain stationary through their exported parenting, with no envelope
  detection or counterrotation in the semantic path.
- Outpost gameplay pivot and rune gameplay hub positions. Rule direction and
  team association still belong to the simulator.
- Explicit collision exclusions read from live node/primitive metadata. The
  sidecar's historical exclusion indices refer to geometry before filtering;
  they are never applied again to the compacted collision mesh.

## Remaining fitted data

LED surfaces and target detector frames/dimensions are still pending in the
reference. Existing procedural rule optics remain in use, and explicitly tagged
outpost armor modules are hidden beneath those overlays. Semantic CAD retains
its source materials; the legacy color and cap geometry heuristics do not run.
Rune detector-front offset and detector/orbit dimensions remain fitted constants.

Base shield slides and the dart gate follow referee overrides, including their
collision meshes. Base dart targets sweep between the exported rail limits with
a four-second cosine cycle. This is illustrative motion, not a rulebook target
mode. The world computes position from simulation time; clients apply that same
position, so pause and single stepping hold both geometry and collision together.
Tech-core arm frames are loaded but remain at rest: the reference still marks
shared hardware as needing rigid-link partitioning before arm animation.

## Full joint refresh, 2026-09-11

The reference package supplies all 14 joint definitions: two rune joints, one
outpost joint, one dart gate, four base joints and six tech-core arm frames.
That composition preserved the installed arena and resource additions.
The triangle counts above describe the earlier native-texture deployment, not
this refreshed package.

The exact executed command, input manifest hashes, reference build record,
simplifier source hashes and Python dependency versions are in
[semantic-assets-build.json](semantic-assets-build.json). The complete simplifier
configuration is [semantic-assets-simplify.json](semantic-assets-simplify.json).
The compositor embeds full upstream manifests and build/deployment records in
`build-provenance.json`, including original exporter settings and source hashes.
No STEP tessellation is performed by this refresh.

To reproduce after installation, from the simulator repository root, use unused
output directories as shown. The retained backup is the scenery input.
`field-all-joints` is retained as the unsimplified intermediate; reuse it and
skip composition, or move it aside before rebuilding it:

```sh
python3 scripts/build-semantic-assets.py \
  --optimized local-assets/field.before-all-joints \
  --semantic ../assets/rm2026-reference \
  --out local-assets/field-all-joints
../rm-map-tools/ocpenv/bin/python ../rm-map-tools/python/simplify_package.py docs/semantic-assets-simplify.json
../rm-map-tools/ocpenv/bin/python scripts/generate-minimap.py --cad-assets local-assets/field-all-joints-simplified
cargo run -p rm-simulator-server --example inspect_assets -- local-assets/field-all-joints-simplified
```

The updated simplifier refines disconnected components that would otherwise
vanish. This build uses one pass from the unsimplified semantic reference,
with separate visual and collision settings. It does not repeatedly simplify
the already optimized scenery. The experimental recursive benchmark outputs
are not used, since their source bindings predate the new joints.

Keep the source package and reference version pinned to the recorded hashes.
Changing either creates a new asset build, even with identical settings.
`mesh-simplification.json` in the output records per-primitive results and sampled
source deviations; those samples are not certified geometric error bounds.

The semantic reference build has 1,192,800 placed visual triangles versus
6,486,053 before simplification, an 81.6% reduction. The installed
standard-detail package carries 497,886. The preceding installed package had
781,913, so this richer semantic reference is still heavier overall than either
installed package. Manifest
counts include all declared instances; the inspector separately reports static
physics triangles, excluding mechanism subtrees managed as moving colliders.
All 14 visual and collision joint records are identical before and after the
simplifier. The minimap was regenerated for the final manifest hashes.

Installation retains `local-assets/field.before-all-joints` and the prior binaries
with `.before-all-joints` suffixes. Both binaries were rebuilt with
`cargo build --workspace --bins` and installed into `local-assets/bin`.
The output also contains `asset-refresh.json` and `asset-refresh-simplify.json`
as local copies of the checked-in recipe and settings.

## Base armor artwork

The seven plate placements use the atlas's `Bs` sprite as passive white printing.
The dart decal is parented to `base.dart_target.carriage`; the lower decals stay
on the reconstructed modules and are occluded by the closed shields. The source
housing geometry and all collider files remain unchanged. The addition costs
14 visual triangles per base, 28 for the two placed bases.

[base-armor-artwork.json](base-armor-artwork.json) pins the input GLB hash and
records each measured face center and basis, plus the visual canvas fit and
0.6 mm offset above its face. The script embeds the existing atlas, updates the
visual manifest and articulation hashes, and writes a local provenance record.
Run after simplification so face fits refer to the checked geometry:

```sh
../rm-map-tools/ocpenv/bin/python scripts/apply-base-armor-artwork.py \
  --input local-assets/field.before-base-artwork \
  --output local-assets/field-base-artwork-rebuild
../rm-map-tools/ocpenv/bin/python scripts/generate-minimap.py --cad-assets local-assets/field-base-artwork-rebuild
cargo run -p rm-simulator-server --example inspect_assets -- local-assets/field-base-artwork-rebuild
```

The artwork-rebuild candidate package's `base-armor-artwork.json` records the
exact invocation, script and helper hashes, atlas hash, placement recipe and
output hash. It lives in the candidate at `local-assets/field-base-artwork`, not
in the installed runtime package, whose base entry carries no artwork record. The
pre-artwork package is retained at `local-assets/field.before-base-artwork`.
