<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Licensing, provenance and third-party notices

## This repository

`rm-simulator` is dual-licensed under your choice of either

- the [MIT License](LICENSE-MIT), or
- the [Apache License, Version 2.0](LICENSE-APACHE).

`Cargo.toml` declares the workspace license as `MIT OR Apache-2.0`, so every
crate in it is dual-licensed on those terms.

Source files begin with their SPDX short-form identifier and the copyright line
`Copyright (c) 2026 hxyulin <hxyulin@proton.me>`. That covers the Rust, Python,
TOML, YAML, Markdown, HTML, CSS and shell files. Files that cannot hold a
comment, such as JSON manifests and compiled artifacts, and generated data such
as the recorded measurements and PNG masks, inherit the same terms from
`Cargo.toml` and this notice. The third-party artwork under
`assets/armor-atlas/sources/` is deliberately unmarked; see below.

Third-party components keep their own terms. They are listed below, and the
provenance of copied code is recorded so that no notice is lost.

## Provenance of reused code

Rune rules, outpost geometry, rune and outpost light visuals, lighting, pose
conversion, and the scene synchronization pattern were copied from the sibling
`rm-vision-sim` repository and adapted for a first-person application. Both
repositories share the same copyright holder, so this code is distributed here
under the repository's `MIT OR Apache-2.0` terms and keeps this provenance
notice. The two repositories share no crate or wire contract.

The fitted outpost geometry and rune pose helpers now in
`crates/rm-simulator-physics/src/motion.rs` were moved from the world crate.
They carry the same `rm-vision-sim` provenance described above.

The RMUC 2026 field geometry is loaded at run time from exported glTF files.
The approved runtime package is stored in `field/` through Git LFS. Its geometry,
textures and source data remain the property of DJI / RoboMaster under their
original terms, not this workspace's software license. `field/CAD-NOTICE.md`
retains the rm-map-tools upstream notice verbatim; the repository it names there
is the exporter that produced this package, not this simulator.
The original archive URL and checksum are retained as import provenance in
`scripts/release-field.json`. The CAD scene plugin follows the `cad_scene`
module of `rm-vision-sim`.

The offline `scripts/generate-minimap.py` tool imports GLB scene traversal from
`rm-map-tools/python/gltf_scene.py`, licensed MIT OR Apache-2.0. The generated
map is derived from DJI / RoboMaster CAD and stays in the LFS field
package alongside its source manifests. No rm-map-tools code is vendored here.

Dependencies retain their own licenses.

## Mozilla Public License 2.0 components

The competitor UI uses Bevy Feathers (MIT OR Apache-2.0) and Bevy Flair
(MIT OR Apache-2.0). Bevy Flair depends on the following MPL-2.0 components.
They are used **unmodified**: no patch, fork, vendored copy or `[patch]`/
`[replace]` override applies to any of them, and this repository contains none
of their source.

| Component | Version | Upstream source | Immutable source archive |
|---|---|---|---|
| cssparser | 0.37.0 | [servo/rust-cssparser](https://github.com/servo/rust-cssparser) | [cssparser-0.37.0.crate](https://static.crates.io/crates/cssparser/cssparser-0.37.0.crate) |
| cssparser-color | 0.5.0 | [servo/rust-cssparser](https://github.com/servo/rust-cssparser) | [cssparser-color-0.5.0.crate](https://static.crates.io/crates/cssparser-color/cssparser-color-0.5.0.crate) |
| cssparser-macros | 0.7.1 | [servo/rust-cssparser](https://github.com/servo/rust-cssparser) | [cssparser-macros-0.7.1.crate](https://static.crates.io/crates/cssparser-macros/cssparser-macros-0.7.1.crate) |
| dtoa-short | 0.3.5 | [upsuper/dtoa-short](https://github.com/upsuper/dtoa-short) | [dtoa-short-0.3.5.crate](https://static.crates.io/crates/dtoa-short/dtoa-short-0.3.5.crate) |
| selectors | 0.38.0 | [servo/stylo](https://github.com/servo/stylo) | [selectors-0.38.0.crate](https://static.crates.io/crates/selectors/selectors-0.38.0.crate) |

`Cargo.lock` records the exact version of every component above.

The MPL-2.0 is a file-level copyleft license. It does not reach this
repository's own `MIT OR Apache-2.0` code, which merely links against the
unmodified components.

### Source Code Form offer (MPL-2.0 section 3.2)

The Source Code Form of every component in the table above is the published
crate archive named in the rightmost column. Those URLs are immutable and
version-pinned. Each archive holds the complete corresponding crate source and
`Cargo.toml`. Four of the five also ship an MPL-2.0 `LICENSE` file; `selectors`
0.38.0 declares `license = "MPL-2.0"` in its `Cargo.toml` without shipping a
copy. The same source is available from any crates.io mirror or with
`cargo download`/`cargo vendor` at the version `Cargo.lock` pins.

If you distribute a binary built from this repository, including the packaged
Steam build produced by `scripts/package-steam.py`, include this notice and
`LICENSES/MPL-2.0.txt`. A recipient who cannot reach those URLs may request the
Source Code Form from <hxyulin@proton.me> at no charge beyond the cost of
distribution.

The full license text is vendored at [LICENSES/MPL-2.0.txt](LICENSES/MPL-2.0.txt),
taken from Mozilla's published copy. It matches the `LICENSE` file shipped by
cssparser, cssparser-color, cssparser-macros and dtoa-short apart from one
trailing space that this repository's whitespace hooks strip. `deny.toml`
allowlists `MPL-2.0` only for the five components above, and
`scripts/check-mpl-compliance.py` fails `just verify` if a resolved dependency,
that allowlist and this notice ever disagree.

## Referenced third-party designs and artwork

The armor artwork masks under `assets/` were generated by `rm-vision-sim`
from user-supplied SVG sources; each `asset.json` records the source hash and
recipe. Their redistribution rights were not established by that conversion.

The canonical SVGs in `assets/armor-atlas/sources` were copied without changes
from `Vision2027/assets/armor`, whose source is DataLabelX. They are the artwork
sources used by rm-vision-sim's prepared masks. The atlas follows its recorded
flood-fill conversion recipe; `atlas.json` retains source hashes, seeds and
qualification. Source artwork rights remain with their original owners, so these
SVGs carry no SPDX header and assert no copyright for this repository.

The procedural robot equipment in `rm-simulator-render/src/equipment.rs` and
`chassis.rs` was hand-built using user-supplied RoboMaster module STEP files and
DJI user-guide drawings as dimensional references. No CAD tessellation or manual
illustrations are included. Source titles, dimensions and approximation choices
are recorded in `docs/robot-equipment.md`. DJI retains rights in its source
product designs and documents; these references do not establish redistribution
rights for the source files.

The UI layout references the July 2026 student edition of the RoboMaster 2026
competitor client interface manual, main interface and panels 1, 3, 5 and 6.
The UI is implemented in code; no screenshots, logos or illustrations from the
manual are embedded or redistributed.

The Valve Steamworks SDK under `vendor/steamworks-sdk/` is downloaded separately
and is not part of this repository or its license. See `docs/steam.md`.
