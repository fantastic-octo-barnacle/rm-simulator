<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Scene discrepancies against the rule manual

Updated 2026-09-11. The confirmed defects below are fixed. Optional effects have
an explicit decision, and unconfirmed material reports remain limited to the
surfaces actually inspected.

Reference: RoboMaster 2026 University Championship Rule Manual V2.1.0,
2026-07-17, printed pages 61 and 109-110. Figure dimensions and colours are
visual readings where the text supplies no numerical requirement.

This audit used `local-assets/field`, combining optimized field geometry with
semantic equipment. Package contents and counts are specific to that capture;
see the [README](../README.md#field-assets) for current discovery order.
The audit installed its corrected package there; `local-assets/field.before-scene-reference-audit`
retains the previous package. No CAD geometry is committed to either repository.

## Field and materials

| Issue | Resolution and evidence |
|---|---|
| Bumpy road covers ROBOMASTER lettering | Fixed in map-tools package composition. Two duplicate V1 wordmarks overlap the grafted V2 roads. Remove both complete words, including the letters outside the bumps, while retaining the other wordmarks and the road placement. This is a composition decision informed by Figure 4-36, not a new field dimension. `python/compose_road_markings.py` selects 30 exact source nodes and applies the same omission to visual and collision scenes. Each loses 11,508 triangles. All other nodes, binary geometry and materials are unchanged. Matched overhead captures show both roads clear of lettering. |
| Rune artwork stays coloured independently of activation | Fixed in the renderer. Semantic joint binding now preserves the rune face optics configuration and reaches the material conversion. Painted red/blue rune artwork becomes neutral black; overlays supply the state-dependent lights. Both faces are dark in the Inactive captures. |
| Centre emblem has the wrong team colour | Fixed for semantic and legacy assets. The app supplies each face's team from the placed CAD pose. The old 40 mm radial classifier also missed the actual R, which reaches 50.93 mm. Its new 52 mm limit excludes the 53 mm black disk. The R sits 0.1 mm behind that disk in the source; a 0.3 mm outward rendering offset makes it visible. Semantic fixed caps remain fixed; legacy caps still counter-rotate. |
| Other components appear coloured where the manual shows black | The inspected rune artwork and neutral equipment had two identified causes: the bypass above and blue/purple scene lamps. Ambient, key and fill lamps are now neutral white. No general red/blue material replacement was applied to field scenery. This does not certify every CAD component against every manual illustration; no additional named surface discrepancy was established in this pass. |
| Opposite-face lights show through arm openings | Found during the state captures and fixed. Light strips now have front-facing geometry instead of solid cuboids. The red face no longer shows blue strips through the open arm channels, and vice versa. |

The removed source nodes are `source_13267048_BREP_161_1` through
`source_13267062_BREP_175_1`, and `source_13267137_BREP_188_1` through
`source_13267151_BREP_202_1`. The composer requires both known grafts, unique
marking nodes, thin sheets and overlapping word/road bounds. It checks input
hashes and writes updated output hashes and a composition report.

Identified material cases in `rune.glb`:

| Surface | Source identifier | Treatment |
|---|---|---|
| Painted arm/target artwork | `face_0/blade_*`, `face_1/blade_*`; source `mat_15`, `mat_18`, `mat_19` | Dark while unlit, following Figures 5-20 and 5-21. |
| Fixed R artwork | `rune.fixed.face.0.logo`, `rune.fixed.face.1.logo`; `mat_15` primitive | Team colour, following Figure 5-20. The adjacent `mat_14` black disk is preserved. |
| Stand and hub housing | `static`, `face_*/hub`; neutral source materials | Neutral illumination removes the purple cast. Mechanical geometry is retained. |

## Rune appearance and activation states

| State or effect | Resolution / decision | Reference |
|---|---|---|
| Inactive | Both faces' arm and target lights are dark. The centre emblem remains visible, matching the illustrated device. Existing world tests retain rotation and ignore inactive hits. | Section 5.5.2.1 |
| Small Rune activating | Available targets keep concentric rings and tapered spokes. Completed arms use a thin circular target outline and lit arm shafts. Five progress captures per face distinguish these from the final pattern. | Figure 5-20 |
| Flowing arrows | All nine arrow rows stay lit and move continuously toward the target, wrapping by one row every 100 ms. The speed and spacing are rendering assumptions; published simulation time drives the motion. Pause freezes the effect; stepping and remote snapshots select the same phase. | Section 5.5.2.1 |
| Big Rune activating | Progress now grows along both arm outlines from the hub, in five equal path lengths, instead of filling the central shaft. This approximates the illustrated shape. A first hit removes that target's available pattern; the other remains available through the existing one-second bonus window. Completed-arm outlines remain suppressed until full activation. No scoring or window timing changed. | Figure 5-21 |
| Available target artwork | Retained the 270/150/70 mm outer ring diameters, 20 mm ring widths and 35 mm tapered tabs read from the figure. Corrected the optical offset to local +Z, 1.5 mm outward, and aligned centres to the CAD's 698.5 mm orbit. Front-facing strips prevent rear lights showing through. | Figure 5-22 |
| Fully activated | Added a separate final pattern: small centre rings and outer target framing replace the activation-stage circular outlines; arm shafts stay lit. | Figure 5-23 |
| Activated blinking | Keep the documented three 2 Hz blinks as the default app assumption. `--rune-flash-hz 0` gives steady illumination for reference captures. Big Rune progress and final framing now share the blink mask. | Figure 5-23; `scene::rune_lit` |
| Transition blackout | Keep zero added visual delay. The allowed interval is up to 200 ms; this pass does not impose a mandatory delay or change hit acceptance. | Note below Figure 5-22 |
| Failure, reset and timeout | Regression checks cover wrong-target and 2.5-second timeout clearing, deactivation masks, completion masks and blink propagation. Existing world/referee tests cover reset, the 20-second activation-window expiry and the one-second Big Rune bonus boundary. Static captures inspect the resulting appearances; they do not claim to replay every transition visually. | Sections 5.5.2 and 5.5.2.1 |

## Validation and captures

`just verify` passes formatting, Clippy with warnings denied, all workspace
tests, dependency boundaries and cargo-deny: 200 Rust tests passed. All 79
map-tools Python tests pass, including the new composition checks,
and the corrected package loads and renders with field collision enabled.

Local images live under `local-assets/scene-reference-audit/`. The 28 semantic
fixtures cover both faces in Inactive, all five Small Rune steps, full Small
Rune activation, all five Big Rune progress levels, the first-hit appearance,
and full Big Rune activation. Representative legacy captures live in `legacy/`.
`roads-before.png` and `roads-after.png` use the same overhead camera. Captures
contain external CAD artwork and stay outside version control.

The fixture script uses the real app and TCP client with synthetic appearance
snapshots based on a paused host's CAD poses. These are reproducible visual
fixtures, separate from tests of world and referee transitions:

```sh
just server --listen 127.0.0.1:17770 --http none --start-paused --no-field-collision --no-chassis
# In another terminal, after building the app:
cargo build -p rm-simulator-app --locked
python3 scripts/capture-rune-reference.py
# Optional legacy comparison:
python3 scripts/capture-rune-reference.py --cad-assets "$HOME/dev/RM/assets/rm2026-field" --output local-assets/scene-reference-audit/legacy --states inactive,small-2,full
```

To reproduce the map composition from the retained source package, run from
`rm-map-tools`, choosing an output directory that does not exist:

```sh
ocpenv/bin/python python/compose_road_markings.py ../rm-simulator/local-assets/field.before-scene-reference-audit out/scene-reference-field-new
PYTHONPATH=python ocpenv/bin/python -m unittest test_compose_road_markings
```
