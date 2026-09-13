# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
# /// script
# dependencies = ["cairosvg==2.9.0", "pillow==12.1.1"]
# ///
"""Build armor artwork masks from the canonical SVG outlines.

Run with `uv run scripts/generate-armor-atlas.py`. On Homebrew macOS, set
DYLD_FALLBACK_LIBRARY_PATH=/opt/homebrew/lib so CairoSVG can load libcairo.
The flood-fill recipe follows rm-vision-sim's armor3/outpost asset manifests.
No runtime SVG rasterizer or sibling repository is needed.
"""

from collections import deque
import hashlib
import io
import json
from pathlib import Path

import cairosvg
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1] / "assets/armor-atlas"
# Interior seeds select white pattern regions, excluding screw holes, the panel
# outline and the holes in digits. Outpost seeds preserve rm-vision-sim's recipe.
SEEDS = {
    "1": [(362, 152)], "2": [(273, 267)], "3": [(300, 90)],
    "4": [(330, 326)], "5": [(270, 262)],
    "Gs": [(106, 190), (310, 358)],
    "Gb": [(564, 186), (463, 338)],
    "O": [(215, 150), (277, 137), (277, 240)],
    "Bs": [(351, 272)],
    "Bb": [(439, 287)],
    "B3": [(485, 276)], "B4": [(473, 320)], "B5": [(433, 244)],
}


def mask(source, seeds):
    image = Image.open(io.BytesIO(cairosvg.svg2png(bytestring=source))).convert("RGBA")
    width, height = image.size
    alpha = image.getchannel("A").load()
    result = Image.new("RGBA", image.size, (255, 255, 255, 0))
    pixels = result.load()
    visited = set()
    queue = deque(seeds)
    while queue:
        x, y = queue.popleft()
        if (x, y) in visited or alpha[x, y] >= 128:
            continue
        if x in (0, width - 1) or y in (0, height - 1):
            raise ValueError("seed leaks outside the artwork")
        visited.add((x, y))
        pixels[x, y] = (255, 255, 255, 255)
        queue.extend(((x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)))
    return result


def main():
    cell, gutter, columns, rows = 512, 8, 4, 4
    atlas = Image.new("RGBA", (cell * columns, cell * rows), (255, 255, 255, 0))
    preview = Image.new("RGB", (1200, 960), "#20252b")
    labels = ImageDraw.Draw(preview)
    sprites = {}
    for index, (name, seeds) in enumerate(SEEDS.items()):
        source = (ROOT / "sources" / f"{name}.svg").read_bytes()
        image = mask(source, seeds)
        source_size = image.size
        image.thumbnail((cell - 2 * gutter, cell - 2 * gutter), Image.Resampling.LANCZOS)
        x, y = index % columns * cell + gutter, index // columns * cell + gutter
        atlas.paste(image, (x, y))
        sprites[name] = {
            "source": f"sources/{name}.svg", "source_sha256": hashlib.sha256(source).hexdigest(),
            "source_size_px": source_size, "seed_px": seeds,
            "rect_px": [x, y, *image.size],
        }
        image.thumbnail((280, 205), Image.Resampling.LANCZOS)
        px, py = index % columns * 300, index // columns * 240
        preview.paste(image, (px + (300 - image.width) // 2, py + 28), image)
        labels.text((px + 10, py + 8), name, fill="white")
    atlas.save(ROOT / "atlas.png")
    preview.save(ROOT / "preview.png")
    manifest = {
        "schema_version": 1, "image": "atlas.png", "size_px": atlas.size,
        "image_sha256": hashlib.sha256((ROOT / "atlas.png").read_bytes()).hexdigest(),
        "recipe": "SVG rasterization, enclosed transparent region fill at alpha < 128, white RGBA, aspect-preserving Lanczos resize, 8 px gutters",
        "provenance": "Vision2027/assets/armor, copied from DataLabelX; same sources used by rm-vision-sim",
        "qualification": "Artwork placement is a visual fit, not calibrated module geometry.",
        "redistribution": "Source artwork rights not established by conversion.",
        "sprites": sprites,
    }
    (ROOT / "atlas.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
