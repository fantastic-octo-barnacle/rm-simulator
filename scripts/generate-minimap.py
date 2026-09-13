# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Generate a field tactical map using rm-map-tools' GLB scene traversal.

Run with numpy and Pillow installed. The approved runtime field uses Git LFS.
The imported gltf_scene helpers belong to rm-map-tools, MIT OR Apache-2.0.
"""
import argparse
import hashlib
import json
from pathlib import Path
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFilter


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    local = Path(__file__).resolve().parents[1] / 'local-assets/field'
    versioned = Path(__file__).resolve().parents[1] / 'field'
    default_assets = next((path for path in (local, versioned) if (path / 'manifest.json').is_file()),
                          Path.home() / 'dev/RM/assets/rm2026-field')
    parser.add_argument('--cad-assets', type=Path, default=default_assets)
    parser.add_argument('--map-tools', type=Path, default=Path(__file__).resolve().parents[2] / 'rm-map-tools')
    parser.add_argument('--width', type=int, default=1536)
    args = parser.parse_args()
    if not 256 <= args.width <= 4096:
        parser.error('--width must be between 256 and 4096')
    sys.path.insert(0, str(args.map_tools / 'python'))
    from gltf_scene import local_matrix, mesh_instances, read_glb

    root = args.cad_assets
    manifest_paths = [root / 'manifest.json', root / 'equipment/manifest.json']
    manifests = [(p, json.loads(p.read_text())) for p in manifest_paths]
    floor_z = manifests[0][1].get('floor_top_source_z_m', -1.5304)
    # Same source-arena origin as cad_assets::cad_point_to_flu.
    origin = np.array([0., 1.6243436, floor_z])
    triangles, colors = [], []
    floor_bounds = None
    for path, manifest in manifests:
        for name, asset in manifest['assets'].items():
            if path.parent == root and name not in {'floor', 'arena-static', 'rune', 'outpost', *manifest.get('static_assets', [])}:
                continue
            source = path.parent / asset['visual']
            if digest(source) != asset['visual_sha256']:
                raise ValueError(f'checksum mismatch: {source}')
            doc, binary = read_glb(source)
            placements = asset.get('placements_in_source_arena_frame') or [{}]
            for placement in placements:
                transform = local_matrix({'translation': placement.get('translation_m', [0, 0, 0]),
                                          'rotation': placement.get('rotation_xyzw', [0, 0, 0, 1])})
                for _, _, points, faces, material in mesh_instances(doc, binary):
                    mat = doc.get('materials', [])[material] if material is not None else {}
                    # Decal rectangles are not terrain. This geometry-only map
                    # omits textured artwork instead of painting its transparent area.
                    if mat.get('alphaMode') in {'MASK', 'BLEND'} and 'baseColorTexture' in mat.get('pbrMetallicRoughness', {}):
                        continue
                    points = points @ transform[:3, :3].T + transform[:3, 3] - origin
                    if name == 'floor':
                        bounds = np.array([points[:, :2].min(axis=0), points[:, :2].max(axis=0)])
                        floor_bounds = bounds if floor_bounds is None else np.array([np.minimum(floor_bounds[0], bounds[0]), np.maximum(floor_bounds[1], bounds[1])])
                    tri = points[faces]
                    # Vertical faces and subpixel details add noise in a tactical view.
                    cross = np.cross(tri[:, 1] - tri[:, 0], tri[:, 2] - tri[:, 0])
                    keep = np.abs(cross[:, 2]) > 0.00002
                    tri = tri[keep]
                    rgb = mat.get('pbrMetallicRoughness', {}).get('baseColorFactor', [1, 1, 1, 1])[:3]
                    z = tri[:, :, 2].mean(axis=1)
                    shade = np.clip(np.floor(z / 0.15), 0, 7).astype(np.uint8)
                    palette = np.column_stack([28 + shade * 5, 43 + shade * 6, 55 + shade * 7])
                    if rgb[0] > max(rgb[1], rgb[2]) * 1.6:
                        palette[:] = [153, 61, 68]
                    elif rgb[2] > max(rgb[0], rgb[1]) * 1.6:
                        palette[:] = [40, 118, 162]
                    triangles.append(tri)
                    colors.append(palette)
    if floor_bounds is None:
        raise ValueError('package has no floor bounds')
    # Padding is part of the coordinate metadata, so markers stay registered.
    lower, upper = floor_bounds + np.array([[-0.25, -0.25], [0.25, 0.25]])
    width = args.width
    height = round(width * (upper[1] - lower[1]) / (upper[0] - lower[0]))
    tri, colors = np.concatenate(triangles), np.concatenate(colors)
    points = np.empty_like(tri[:, :, :2])
    points[:, :, 0] = (upper[0] - tri[:, :, 0]) / (upper[0] - lower[0]) * (width - 1)
    points[:, :, 1] = (tri[:, :, 1] - lower[1]) / (upper[1] - lower[1]) * (height - 1)
    image = Image.new('RGB', (width, height), (11, 20, 29))
    draw = ImageDraw.Draw(image)
    # Top-down painter order. Small sloped-face intersections are deliberately
    # approximate; this image is navigation artwork, never collision geometry.
    for i in np.argsort(tri[:, :, 2].mean(axis=1), kind='stable'):
        draw.polygon([tuple(p) for p in points[i]], fill=tuple(map(int, colors[i])))
    image = image.filter(ImageFilter.MedianFilter(3))
    png = root / 'minimap.png'
    image.save(png)
    metadata = {'schema_version': 1, 'image': 'minimap.png', 'image_sha256': digest(png),
                'generator': 'rm-simulator/scripts/generate-minimap.py using rm-map-tools/gltf_scene.py',
                'bounds_flu_m': [*lower.tolist(), *upper.tolist()],
                'orientation': 'x-left-y-down',
                'manifest_sha256': {str(p.relative_to(root)): digest(p) for p in manifest_paths}}
    (root / 'minimap.json').write_text(json.dumps(metadata, indent=2) + '\n')
    print(f'{png}: {width} x {height}; FLU bounds {metadata["bounds_flu_m"]}')


if __name__ == '__main__':
    main()
