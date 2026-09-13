<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Reproducible field detail

`scripts/optimize-field.py` builds a new package from an immutable, checksummed
checkpoint through `rm-map-tools/python/simplify_package.py`. The producer's
protection-policy support was added in commit `f16ee8f`. It is also integrated
in the sibling repository's main branch. No CAD or generated mesh is committed
in this simulator repository.

The standard-detail build recorded on 12 September 2026 contains 497,886 placed visual triangles and
312,749 actual collision triangles, including articulated mechanisms. The
previous checkpoint contained 1,056,054 collision triangles. The 500k visual
target is met; the 100k collision target is not. The conservative manifest count
also counts inactive mesh primitives, so it differs from the real loader count.

Keep the previous checkpoint for rebuilding and rollback. On this development
machine it is `local-assets/field.before-standard-detail-20260912`.

```sh
../rm-map-tools/ocpenv/bin/python scripts/optimize-field.py \
  --map-tools ../rm-map-tools \
  --input local-assets/field.before-standard-detail-20260912 \
  --output ../assets/rm2026-standard-rebuilt
cargo run --release -p rm-simulator-server --example compare_ground -- \
  local-assets/field.before-standard-detail-20260912 ../assets/rm2026-standard-rebuilt
```

Use the producer's Python environment with `meshoptimizer==0.2.30a0`.
`field-detail-build.json` records all dependency versions, script and producer
hashes, input manifests, simplification parameters and upstream error records.
Measurements refer to the input checkpoint; they do not claim error relative to
the original STEP. The minimap is regenerated with the new manifest fingerprints.
Budget failure leaves a candidate available for inspection and returns a failing
exit status. It never replaces the installed package by itself.

The settings preserve arena, floor, centre-platform and outpost-footing files
byte-for-byte. This keeps painted sheets, ramp geometry and deck clearance.
Selected detector-front and dart-station support primitives are also preserved.
Mechanical dart carriage geometry can simplify while retaining joint metadata.
The standard build uses sampled deviation checks; those do not prove collision
equivalence or exhaustive clearance.

The deployment check sampled 24,219 ground rays with no surface-presence losses.
Maximum height difference allowing 5 mm of ray-ceiling slack was 37.2 mm in
simplified equipment. A raw 150 mm difference at a ray ceiling was traced to a
3.656 mm surface shift across that ceiling; the report preserves both metrics.
Default-view screenshots were compared for markings, silhouettes and optics.
Further robot driving checks and target-hardware performance measurements remain
useful before a production release.

To install a reviewed candidate, build the existing package inspector example
and use `scripts/install-field-detail.py --help`. It validates the candidate and
minimap, stages a copy, and keeps a named backup beside the destination. Installing
a package over the collision budget requires `--allow-collision-budget-overrun`.
If the final rename fails, the original package is restored. To roll back, move
the current package aside and restore the named backup to `local-assets/field`.

That build supplies one standard-detail package. Runtime level of detail selection and spatial
chunking remain separate work; optional GPU occlusion culling is available in
Graphics settings and remains off by default until measured.

## Coarser visuals with the original map composition

The `field-detail-coarse.json` settings retain the V1.2 base and grafted V2
roads. Visual simplification uses a 30 mm meshoptimizer metric and a 35 mm
sampled acceptance limit against the retained checkpoint. Terrain protections,
optical primitive protections and the standard collision settings remain.
This candidate has 355,661 placed visual triangles, 28.6% below standard detail.
These sampled tolerances are not certified maximum errors against STEP.

```sh
../rm-map-tools/ocpenv/bin/python scripts/optimize-field.py \
  --map-tools ../rm-map-tools \
  --input local-assets/field.before-standard-detail-20260912 \
  --settings scripts/field-detail-coarse.json \
  --output local-assets/field-coarse
just run --cad-assets /Users/hxyulin/dev/RM/rm-simulator/local-assets/field-coarse
```

The absolute launch path above is for this development checkout and also works
from its other worktrees. Build output must be a new directory. The generator
still reports the existing collision-budget failure and retains the candidate.
An experimental 60 mm collision metric lost an equipment surface in the ground
probe and was rejected. The final candidate uses the standard collision settings.
All collider GLBs are byte-identical to standard detail; 24,219 ground probes
showed no height or surface-presence changes. The final candidate loaded and
rendered successfully. The installed package stays unchanged during review.

## V2 terrain with V1.2 overlays: visual preview

Build a separate experimental package without replacing the installed map:

```sh
../rm-map-tools/ocpenv/bin/python scripts/preview-v2-overlays.py \
  --map-tools ../rm-map-tools --v2 ../assets/rm2026-extracted \
  --v12 local-assets/field --output local-assets/v2-v12-preview
../rm-map-tools/ocpenv/bin/python scripts/generate-minimap.py \
  --cad-assets local-assets/v2-v12-preview --map-tools ../rm-map-tools
just run --cad-assets local-assets/v2-v12-preview
```

The preview retains the V2 extraction's terrain, equipment and collision
selection. It adds a separate `v12-overlays.glb` visual asset with an empty
collider. Horizontal atlas artwork and the V1.2 red/blue paint materials are
selected from the installed V1.2-derived arena. Their triangles are subdivided
to at most 25 cm edges, projected vertically onto V2 terrain and offset 2 mm.
The projection searches from 6 cm above to 50 cm below the original artwork.
Triangles with missing surface hits are omitted and counted in
`v12-overlay-preview.json`, alongside source hashes and exact selections.
This is a preview heuristic, not a reviewed cross-release correspondence.

V2 structures retain their unpainted appearance. Vertical lettering and
equipment artwork are not transferred by this horizontal overlay experiment.
The legacy V2 extraction is heavier than the optimized standard package; use
this candidate to judge composition before further simplification. It does not
change the default package or claim the standard triangle budgets.
