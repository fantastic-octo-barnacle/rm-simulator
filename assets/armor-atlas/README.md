<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Armor sprite atlas

`atlas.png` is a transparent white artwork mask. `atlas.json` records
pixel rectangles, original canvas dimensions, source hashes and flood-fill
seeds. `preview.png` shows every entry on a dark background.

| Sprite | Identifier |
|---|---|
| `1`, `2` | Hero, engineer |
| `3`, `4`, `5` | Small infantry armor |
| `B3`, `B4`, `B5` | Large infantry armor |
| `Gs`, `Gb` | Small and large guard/sentry armor |
| `O` | Outpost |
| `Bs`, `Bb` | Small and large base armor |

The unmodified SVGs in `sources/` were copied from
`../Vision/Vision2027/assets/armor`, which copies DataLabelX's armor artwork.
These are the source files referenced by rm-vision-sim's prepared armor masks.
The mask conversion follows its enclosed-transparent-region flood-fill recipe.
Conversion does not establish redistribution rights to the original artwork.

Regenerate from the repository root:

```sh
uv run scripts/generate-armor-atlas.py
```

On Homebrew macOS, prepend `DYLD_FALLBACK_LIBRARY_PATH=/opt/homebrew/lib` so
CairoSVG finds libcairo. The application uses the embedded PNG and manifest;
it needs neither these tools nor a sibling checkout at runtime.

Each source canvas is fitted into a 512 px cell with an 8 px gutter, preserving
aspect ratio. UVs select the image rectangle inside that cell. The mesh uses
the original source aspect and whitespace and centers the canvas on the armor
face at its visible panel height. This is a visual fit, not a calibrated
artwork dimension. Chassis artwork faces outward along local FLU +x and sits
0.5 mm ahead of the front panel. The outpost uses its existing overlay frame.

The app selects `1` for its hero prototype, `3` for infantry, and `O` for
outposts. Other patterns are available through the renderer's `ArmorPattern`.
The external base package uses `Bs` on its dart plate, three upper modules and
three lower modules. `scripts/apply-base-armor-artwork.py` adds the fitted
placements from `docs/base-armor-artwork.json` to the checksummed GLB. Guard and
engineer robots are not added by this artwork change.

The pattern is passive white printing with zero emission. It remains white
during strikes and after defeat; only the light bars flash grey or stop emitting. Their off-state diffusers
remain white plastic.
