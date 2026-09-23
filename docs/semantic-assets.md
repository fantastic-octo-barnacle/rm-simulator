<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Semantic assets

The simulator loads rm-map-tools' `articulation.json` alongside checksummed
visual and collision GLBs. The loader verifies file hashes, schema, units,
coordinate conventions, unique IDs, hierarchy, joint frames and zero rest
transforms. A declared but invalid sidecar stops loading. Packages without a
sidecar retain the legacy loader. See [field package](field-package.md) for
package discovery, composition, collision contracts and deployment.

The checked-in `field/` package contains the runtime manifests and their
referenced sidecars. Build reports excluded from the runtime package are listed
in `scripts/release-field.json`. The upstream package and its checksum are the
record of those reports.

## Compose a package

`scripts/build-semantic-assets.py` composes optimized scenery with a semantic
reference package into a new directory. It verifies source hashes, copies the
reference's semantic assets and joint bindings, and records its inputs in the
output's `build-provenance.json`. It performs no STEP tessellation or mesh
simplification. Pass explicit, existing input packages and an unused output path:

```sh
python3 scripts/build-semantic-assets.py \
  --optimized /path/to/optimized-field \
  --semantic /path/to/semantic-reference \
  --out local-assets/field-candidate
cargo run -p rm-simulator-server --example inspect_assets -- local-assets/field-candidate
```

The compositor preserves the selected gate source geometry because its triangle
ranges are tied to that tessellation. A later simplification pass may reduce
meshes within their already bound primitives. Keep source versions and manifest
hashes with any measured comparison.

## What the sidecar supplies

- Semantic IDs, node and primitive bindings, joint origins, axes, types, limits
  and the selected glTF scene.
- Rune and outpost motion nodes with pivots and rebased child rest poses.
- Stationary geometry selected by joint hierarchy, including fixed rune caps
  and logos.
- Outpost gameplay pivot and rune gameplay hub positions. Rule direction and
  team assignment remain in the simulator.
- Collision exclusions from live node and primitive metadata. Historical
  pre-filter indices are not applied to a compacted collision mesh again.

The reference does not supply all LED surfaces or detector frames. Procedural
optics and fitted detector dimensions remain in use. Base shield slides and
dart targets follow referee overrides, including their collision meshes. Tech
core arm frames load at rest until their shared hardware is partitioned into
rigid links for animation.

## Base armor artwork

`scripts/apply-base-armor-artwork.py` adds the atlas's `Bs` sprite as passive
printing on checksum-pinned base faces in a new package. Its default placement
recipe is `docs/base-armor-artwork.json`; the script verifies the source GLB and
atlas hashes. It changes visual assets and manifest hashes, leaving collider
files unchanged. Run `--help` for its input, output and map-tools options, then
inspect the candidate before installing it.
