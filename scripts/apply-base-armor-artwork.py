#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Place the existing armor atlas on checksum-pinned base faces in a new package.

Uses rm-map-tools' MIT/Apache-2.0 gltf_scene helpers. Only visual decal quads are
added; source geometry, collision files and joint records remain unchanged.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile

import numpy as np


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    repo = Path(__file__).resolve().parents[1]
    parser.add_argument('--config', type=Path, default=repo / 'docs/base-armor-artwork.json')
    parser.add_argument('--map-tools', type=Path, default=repo.parent / 'rm-map-tools')
    args = parser.parse_args()
    if args.output.exists():
        parser.error('output exists; use a new directory')
    sys.path.insert(0, str(args.map_tools / 'python'))
    from gltf_scene import read_glb, write_glb, scene_nodes

    recipe = json.loads(args.config.read_text())
    source = args.input / 'equipment/base.glb'
    if digest(source) != recipe['input_visual_sha256']:
        raise ValueError('base differs from the measured placement source')
    atlas_root = repo / 'assets/armor-atlas'
    atlas = json.loads((atlas_root / 'atlas.json').read_text())
    if digest(atlas_root / 'atlas.png') != atlas['image_sha256']:
        raise ValueError('atlas checksum mismatch')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=args.output.parent) as temporary:
        stage = Path(temporary) / 'field'
        shutil.copytree(args.input, stage)
        equipment = stage / 'equipment'
        manifest = json.loads((equipment / 'manifest.json').read_text())
        descriptor = manifest['articulation']
        sidecar_path = equipment / descriptor['file']
        if digest(sidecar_path) != descriptor['sha256']:
            raise ValueError('sidecar checksum mismatch')
        sidecar = json.loads(sidecar_path.read_text())
        binding = sidecar['assets']['base']['files']['visual']
        doc, binary = read_glb(source)
        binary = bytearray(binary)
        parents = {doc['nodes'][i].get('extras', {}).get('rm', {}).get('id'): (i, matrix)
                   for i, matrix, _ in scene_nodes(doc)}

        def view(data):
            binary.extend(b'\0' * (-len(binary) % 4))
            result = len(doc['bufferViews'])
            doc['bufferViews'].append({'buffer': 0, 'byteOffset': len(binary), 'byteLength': len(data)})
            binary.extend(data)
            return result

        def accessor(values, kind, component):
            values = np.asarray(values, dtype='<u4' if component == 5125 else '<f4')
            record = {'bufferView': view(values.tobytes()), 'componentType': component,
                      'count': len(values), 'type': kind}
            if kind == 'VEC3':
                record.update(min=values.min(0).tolist(), max=values.max(0).tolist())
            result = len(doc['accessors'])
            doc['accessors'].append(record)
            return result

        image = len(doc.setdefault('images', []))
        doc['images'].append({'name': 'armor-atlas', 'bufferView': view((atlas_root / 'atlas.png').read_bytes()),
                              'mimeType': 'image/png'})
        sampler = len(doc.setdefault('samplers', []))
        doc['samplers'].append({'magFilter': 9729, 'minFilter': 9729, 'wrapS': 33071, 'wrapT': 33071})
        texture = len(doc.setdefault('textures', []))
        doc['textures'].append({'source': image, 'sampler': sampler})
        material = len(doc['materials'])
        doc['materials'].append({'name': 'base_passive_white_armor_print', 'alphaMode': 'BLEND',
                                'emissiveFactor': [0, 0, 0],
                                'pbrMetallicRoughness': {'baseColorFactor': [1, 1, 1, 1],
                                    'baseColorTexture': {'index': texture},
                                    'metallicFactor': 0, 'roughnessFactor': 0.8}})
        for placement in recipe['placements']:
            parent, matrix = parents[placement['parent_id']]
            sprite = atlas['sprites'][placement['sprite']]
            height = placement['canvas_height_m']
            width = height * sprite['source_size_px'][0] / sprite['source_size_px'][1]
            right, up = np.array(placement['right']), np.array(placement['up'])
            normal = np.cross(right, up)
            normal /= np.linalg.norm(normal)
            center = np.array(placement['center_m']) + normal * recipe['surface_offset_m']
            points = np.array([center + right * x * width / 2 + up * y * height / 2
                               for x, y in [(-1, -1), (1, -1), (1, 1), (-1, 1)]])
            inverse = np.linalg.inv(matrix)
            points = points @ inverse[:3, :3].T + inverse[:3, 3]
            local_normal = matrix[:3, :3].T @ normal
            local_normal /= np.linalg.norm(local_normal)
            x, y, w, h = sprite['rect_px']
            aw, ah = atlas['size_px']
            uv = [[x / aw, (y + h) / ah], [(x + w) / aw, (y + h) / ah],
                  [(x + w) / aw, y / ah], [x / aw, y / ah]]
            metadata = {'id': placement['id'], 'roles': ['armor_artwork'], 'layer': 'markings',
                        'collision': False, 'artwork': {'sprite': placement['sprite'], 'emissive': False}}
            primitive = {'attributes': {'POSITION': accessor(points, 'VEC3', 5126),
                                       'NORMAL': accessor([local_normal] * 4, 'VEC3', 5126),
                                       'TEXCOORD_0': accessor(uv, 'VEC2', 5126)},
                         'indices': accessor([0, 1, 2, 0, 2, 3], 'SCALAR', 5125),
                         'material': material, 'extras': {'rm': metadata}}
            mesh = len(doc['meshes'])
            doc['meshes'].append({'name': placement['id'], 'primitives': [primitive]})
            node = len(doc['nodes'])
            doc['nodes'].append({'name': placement['id'], 'mesh': mesh, 'extras': {'rm': metadata}})
            doc['nodes'][parent].setdefault('children', []).append(node)
            binding['nodes'].append({'id': placement['id'], 'node': node, 'metadata': metadata})
            binding['collision_exclusions'].append({'node': node})
        doc['buffers'][0]['byteLength'] = len(binary)
        write_glb(equipment / 'base.glb', doc, binary)
        entry = manifest['assets']['base']
        entry['visual_sha256'] = binding['sha256'] = digest(equipment / 'base.glb')
        entry['triangles'] += 2 * len(recipe['placements'])
        entry['nodes'] = len(doc['nodes'])
        entry['armor_artwork'] = {'recipe_sha256': digest(args.config), 'added_triangles': 2 * len(recipe['placements']),
                                  'atlas_sha256': atlas['image_sha256']}
        sidecar_path.write_text(json.dumps(sidecar, indent=2) + '\n')
        descriptor['sha256'] = digest(sidecar_path)
        (equipment / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        record = {'command': [sys.executable, *sys.argv], 'script_sha256': digest(Path(__file__)),
                  'recipe': recipe, 'recipe_sha256': digest(args.config),
                  'atlas_sha256': atlas['image_sha256'], 'output_visual_sha256': entry['visual_sha256'],
                  'gltf_scene_sha256': digest(args.map_tools / 'python/gltf_scene.py')}
        (stage / 'base-armor-artwork.json').write_text(json.dumps(record, indent=2) + '\n')
        stage.rename(args.output)
    print(args.output.resolve())


if __name__ == '__main__':
    main()
