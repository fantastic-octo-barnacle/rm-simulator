<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Field detail

`scripts/optimize-field.py` creates a simplified package from an immutable,
checksummed field package. It calls rm-map-tools' simplifier with the settings in
`scripts/field-detail-standard.json` by default; pass
`scripts/field-detail-coarse.json` for the coarser visual candidate. See
[field package](field-package.md) for the collision contract and
[semantic assets](semantic-assets.md) for the sidecar bindings.

Run it with the rm-map-tools Python environment, which must provide
`meshoptimizer==0.2.30a0`. The output path must be new and outside the input:

```sh
../rm-map-tools/ocpenv/bin/python scripts/optimize-field.py \
  --map-tools ../rm-map-tools \
  --input /path/to/checksummed-field \
  --output local-assets/field-candidate
```

For a coarse candidate, add `--settings scripts/field-detail-coarse.json`.
The command regenerates the minimap, checks the configured visual and collision
triangle budgets, and writes `field-detail-build.json` with input hashes,
settings, dependency versions and per-asset results. A budget failure retains
the candidate for inspection and exits with a failure status. Sampled deviation
checks compare against the input package; they do not establish maximum error
against the source STEP or prove every clearance.

Inspect the candidate with the server's `inspect_assets` example and compare
its ground against the input before installing it. To install a reviewed
candidate, build the inspector and run `scripts/install-field-detail.py --help`.
The installer verifies the minimap and package, stages a copy and leaves a
named backup beside the destination for rollback. A collision-budget overrun
requires `--allow-collision-budget-overrun`.

Runtime level of detail switching and spatial chunking are separate from this
package build. GPU occlusion culling is available in Graphics settings and is
off by default.
