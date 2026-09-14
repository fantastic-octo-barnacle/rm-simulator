<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Field package

Composition, deployment, collision contracts and loading of the RMUC 2026 field
package. The [project README](../README.md) carries the user-facing summary;
the [documentation index](README.md) lists every guide.

## Composition

The default package is built by the sibling `rm-map-tools` project
(`python/export_field_package.py`) from DJI's RMUC2026 V1.2.0 STEP, the
release that still carries the arena's face colours. The floor slab and the
385 placed arena solids are tessellated part by part with their STEP colours
(dark grey slab top, dark plate tops, beige plate sides, white lettering, red
and blue markings), one glTF node per solid, and the manifest records the
slab top as `floor_top_source_z_m` so the simulator puts it at height zero.
The rune and the outposts are carried over from the earlier V2.0.0 extraction
(`~/dev/RM/assets/rm2026-extracted`); the bases and tech cores come from the
V1.2.0 equipment package. Both releases share
the arena frame, and the equipment placements land on the V1.2.0 plates. The
older extraction still loads with `--cad-assets`; its manifest has no slab
height, so its crowned slab is referenced to the level pads near
(±12.8, ±3.0) m.

The exported runtime package is versioned in `field/` through Git LFS with its
upstream ownership notice retained. The loader discovers it in this order:
`--cad-assets` when supplied, `field/` beside an installed `bin/` directory,
the build checkout's `local-assets/field`, the checkout's LFS `field/`, then
`~/dev/RM/assets/rm2026-field`. The selected directory must contain
`manifest.json`, the `*.glb` visuals and `*-collision.glb` proxies it lists,
and `equipment/manifest.json` with `base.glb` and `tech-core.glb`. Every file
is checked against the SHA-256 recorded in its manifest during loading. The
manifest's `floor_top_source_z_m` is the slab top in the arena frame; when
absent the V2.0.0 pad height applies. Keep original STEP files and
intermediate exports outside Git; only the approved runtime package belongs
in LFS.

The package's `static_assets` list adds ten entries: the centre platform, both
dart stations, both resource-zone structures, both outpost footings and six
energy units, with
source colours and triangle-mesh collisions. The existing rune and outpost
assets retain their animation hierarchy. Older packages without this list
still load.

## Textures and semantic assets

Text and paint can be exported as native glTF texture patches. Their PNGs,
UVs and alpha-mask materials are embedded in the GLBs; no texture sidecar loader
is needed. Textured CAD materials retain their exported appearance. Package composition and
previous native-texture measurements are documented in [semantic assets](semantic-assets.md).
The later standard-detail build is recorded in [field detail](field-detail.md);
use loader counts and manifest hashes when comparing a different installed package.

The simulator supports rm-map-tools semantic packages: exported joint frames,
stable IDs and fixed geometry drive animation and collision selection. See
[Semantic assets](semantic-assets.md) for package composition, running this
version, geometry counts and remaining fitted detector data.

## Deployment

To assemble the complete simulator package from the field and element exports,
run the standard-library deployment utility in `rm-map-tools`:

```sh
python3 ../rm-map-tools/python/deploy_field.py --replace
```

Its defaults read `~/dev/RM/assets/rm2026-field` and
`~/dev/RM/assets/rm2026-field-elements`, verify their checksums and matching
arena, and install at `~/dev/RM/assets/rm2026-field`. It builds in a temporary
directory first and retains the previous installation as a dated backup.
Use `--field`, `--elements` and `--out` for other locations. No CAD import or
re-tessellation is needed. Restart the app and server after deployment.

## Collision contracts

Static collision loads each asset's declared, checksummed collision geometry,
falling back to visual triangles for packages without a supported collision
contract. Reproducible approximate meshes are permitted to reduce CPU and memory
cost, provided ramps, clearance, traversable openings and scoring behavior are
verified. Rune and outpost moving targets remain rule-driven bodies. CAD source
materials and semantic joints remain intact.

New exports may declare `collision_method: "source-tessellation-v1"` per asset.
The simulator then uses that asset's checksummed collision GLB, tessellated from
the same source faces with separate tolerances. Unmarked legacy proxies remain
ignored and those assets use their visual geometry. This does not change
rule-driven scoring shapes or joint motion.

Ground-height queries for chassis spawning use an index of triangle footprints
built during terrain loading. The index retains the original triangles and
height tolerances, including stacked surfaces. Wheel suspension continues to
use Rapier's collision queries.

## Terrain

The V1.2.0 floor slab is flat at height zero and the
terrain plates (the centre-line plateaus at x ≈ ±6.3..8.5 m, the 起伏路段
undulating roads near the side walls, ramps, the central highland, covered
passages) stand on it. The V2.0.0 slab is crowned
instead: about 0.11 m below the reference height along the centre line,
falling to about 0.32 m below at the side walls, with height zero at its
small level pads. Invisible walls, 2 m high, surround the loaded floor bounds
to keep robots inside the arena. The CAD floor remains the driving surface;
there is no invisible catch plane beneath a loaded field.

Both CAD releases put a hex-marked truncated pyramid on the centre line at
x ≈ ±6.3..8.5 m: a 1.5 × 1.0 m flat top 0.15 m above the floor with 17°
ramps on all four sides, which the chassis crosses (the default spawn faces
the red-side one). The 起伏路段 undulating road, a 2.4 × 2.1 m strip of
waves about 70 mm high on the 0.2 m deck near each side wall (x ≈ 6.3..8.7,
y ≈ 5.4..7.5 m and its point mirror), exists only in the V2.0.0 STEP; the
default package carries the two V2.0.0 solids grafted onto its flat deck in
V1.2.0's plate colours (listed under `grafted` in the manifest), so both
maps have it. The covered passages under the base highland decks (around
x = ±12.5, y = ±5.5 m) have 0.65 m of clearance in both.

## Placement frames

Placements come from the manifests' `placements_in_source_arena_frame`. The CAD
arena frame is forward/left/up in metres; the simulator centres the field on
the origin and puts the top of the playing floor at height zero. Rune hubs are
derived from the `face_0`/`face_1` pivots inside `rune.glb` (targets on the
blade front plane, 698.5 mm orbit) and outpost bases from the outpost
placements, including the CAD's slight lean, so the emissive rune targets and
outpost armor overlays coincide with the imported geometry.

## Arena colours

The arena's colours are the CAD's. The V1.2.0 STEP styles every arena face
(white for most surfaces, dark grey slab and plate tops, beige plate sides,
red and blue 1 mm marking sheets), and the package keeps them as glTF
materials. The V2.0.0 STEP styles every arena solid in the exporter's
unpainted cream and its markings are flush cream solids; the simulator
recolours only that yellow-tinted cream (floor dark, arena light grey), so
painted materials, white line markings included, are left alone.

The default field is enclosed by a dark stadium with neutral lighting. Equipment
paint follows team ownership: red occupies +x and blue occupies -x. Destroying
an outpost opens its team's base shields. Friendly-fire rules are unchanged.

Scene reference fixes and reproducible rune-state captures were audited against
the rule manual. Rune arrows follow simulation
time, including pause and step. Neutral lamps preserve the imported field paint;
only rune optics receive the state-dependent material treatment.

## Loading, splash and the collision view

A splash screen shows stage progress and the current
loading activity. Verification, terrain loading, physics construction and
connection setup run in the background. Visual CAD instances load across
frames before local gameplay starts. The bar measures stages and completed
instances, not elapsed time. Press `Esc` to cancel a join and return to the
title screen. GPU uploads
and individual scene instantiations can still briefly stall the window. The
startup log reports triangle counts; the F3 debug panel shows the geometry held by the physics.
Press C to cycle Off / Overlay / Only. The green wireframe is built once from
client-loaded, verified collision assets and reused. It makes no debug requests
to the server. Unknown or mismatched packages remain unavailable.
Amber chassis, armor, projectile and articulated scenery outlines use the client's
predicted and interpolated content. Moving CAD wire meshes are built once in the background; all collider transforms
then follow the same scene poses as the visuals every frame. No physics world is
created and gameplay poses are never replaced. This is a client inspection view,
not a capture of current server physics.
Scenery stays visible until the fixed wireframe is ready.
The panel offers Off / Overlay / Only radio buttons, a visual mesh wireframe
checkbox and a persistent rendering statistics overlay with FPS, frame time,
visible mesh instance counts and render pass CPU times. Visual wireframe is
disabled when the GPU lacks support.
Screenshot mode waits for a requested wireframe and reports an error if it is
unavailable or the build fails. Capture renders into a dedicated image at the
window’s physical resolution. It waits for GPU pipeline compilation and
three settled frames, retries blank readbacks up to three captures, and fails
if capture has not finished within two minutes of gameplay readiness.
GPU mesh uploads still happen through Bevy.
