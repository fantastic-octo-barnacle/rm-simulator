#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Create an isolated V2 terrain preview with surface-projected V1.2 artwork.

Run with rm-map-tools/ocpenv/bin/python. This is a visual composition experiment,
not a replacement simulation preset. Original V2 collision selection is retained.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import shutil
import sys


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--v2', type=Path, required=True)
    parser.add_argument('--v12', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--map-tools', type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or any(args.output.resolve().is_relative_to(p.resolve()) for p in [args.v2, args.v12]):
        parser.error('output must be new and outside both input packages')
    sys.path.insert(0, str(args.map_tools.resolve() / 'python'))
    import numpy as np
    import vtk
    from vtk.util.numpy_support import numpy_to_vtk, numpy_to_vtkIdTypeArray
    from gltf_scene import read_glb, write_glb, mesh_instances, scene_nodes, accessor
    from simplify_package import append_accessor

    manifest = json.loads((args.v2 / 'manifest.json').read_text())
    old = json.loads((args.v12 / 'manifest.json').read_text())
    inputs = {}
    for root, data in [(args.v2, manifest), (args.v12, old)]:
        for name in ['floor', 'arena-static']:
            entry = data['assets'][name]
            path = root / entry['visual']
            actual = digest(path)
            if actual != entry['visual_sha256']:
                raise ValueError(f'checksum mismatch: {path}')
            inputs[str(path.resolve())] = actual

    points, cells, offset = [], [], 0
    for name in ['floor', 'arena-static']:
        d, b = read_glb(args.v2 / manifest['assets'][name]['visual'])
        for _, _, p, t, _ in mesh_instances(d, b):
            points.append(p)
            cells.append(np.column_stack([np.full(len(t), 3), t + offset]))
            offset += len(p)
    vp = vtk.vtkPoints()
    vp.SetData(numpy_to_vtk(np.concatenate(points), deep=True))
    vc = vtk.vtkCellArray()
    all_cells = np.concatenate(cells)
    vc.ImportLegacyFormat(numpy_to_vtkIdTypeArray(all_cells.astype(np.int64).ravel(), deep=True))
    poly = vtk.vtkPolyData()
    poly.SetPoints(vp)
    poly.SetPolys(vc)
    locator = vtk.vtkStaticCellLocator()
    locator.SetDataSet(poly)
    locator.BuildLocator()

    doc, binary = read_glb(args.v12 / old['assets']['arena-static']['visual'])
    source = copy.deepcopy(doc)
    packed = bytearray(binary)
    doc['nodes'] = [{'name': 'V1.2 surface overlays', 'children': []}]
    doc['meshes'] = []
    doc['scenes'] = [{'nodes': [0]}]
    doc['scene'] = 0
    report = {'schema_version': 1, 'preview_only': True, 'inputs': inputs,
              'script_sha256': digest(Path(__file__)), 'grid_edge_m': 0.25,
              'surface_offset_m': 0.002, 'projection_window_m': [0.06, -0.5],
              'selections': [], 'omitted_triangles_without_surface': 0}
    cache = {}

    def project(p):
        key = tuple(p)
        if key not in cache:
            hit, pc = [0.] * 3, [0.] * 3
            t, sub, cell = vtk.mutable(0.), vtk.mutable(0), vtk.mutable(0)
            ok = locator.IntersectWithLine([p[0], p[1], p[2] + 0.06],
                                          [p[0], p[1], p[2] - 0.5],
                                          1e-8, t, hit, pc, sub, cell)
            cache[key] = np.array([p[0], p[1], hit[2] + 0.002]) if ok else None
        return cache[key]

    def subdivide(p, uv):
        lengths = [np.linalg.norm(p[(i + 1) % 3] - p[i]) for i in range(3)]
        i = int(np.argmax(lengths))
        if lengths[i] <= 0.25:
            yield p, uv
            return
        j, k = (i + 1) % 3, (i + 2) % 3
        midpoint, miduv = (p[i] + p[j]) / 2, (uv[i] + uv[j]) / 2
        yield from subdivide(np.array([p[i], midpoint, p[k]]), np.array([uv[i], miduv, uv[k]]))
        yield from subdivide(np.array([midpoint, p[j], p[k]]), np.array([miduv, uv[j], uv[k]]))

    for ni, transform, _ in scene_nodes(source):
        node = source['nodes'][ni]
        if 'mesh' not in node:
            continue
        for pi, prim in enumerate(source['meshes'][node['mesh']]['primitives']):
            material = source['materials'][prim['material']]
            pbr = material.get('pbrMetallicRoughness', {})
            color = pbr.get('baseColorFactor', [1, 1, 1, 1])
            textured = 'baseColorTexture' in pbr
            # Explicit V1.2 painted red/blue materials, plus exported artwork.
            painted = color[0] > 0.9 and max(color[1:3]) < 0.01 or color[2] > 0.3 and max(color[:2]) < 0.01
            if not (textured or painted):
                continue
            positions = accessor(source, binary, prim['attributes']['POSITION']).astype(float)
            positions = positions @ transform[:3, :3].T + transform[:3, 3]
            indices = accessor(source, binary, prim['indices']).reshape(-1, 3)
            uv = accessor(source, binary, prim['attributes']['TEXCOORD_0']) if textured else np.zeros((len(positions), 2))
            out, tex = [], []
            for face in indices:
                p = positions[face]
                normal = np.cross(p[1] - p[0], p[2] - p[0])
                length = np.linalg.norm(normal)
                if length == 0 or abs(normal[2]) / length < 0.98:
                    continue
                for triangle, coords in subdivide(p, uv[face]):
                    projected = [project(v) for v in triangle]
                    if any(v is None for v in projected):
                        report['omitted_triangles_without_surface'] += 1
                        continue
                    if normal[2] < 0:
                        projected = projected[::-1]
                        coords = coords[::-1]
                    out.extend(projected)
                    tex.extend(coords)
            if not out:
                continue
            def add(values, kind, component=5126):
                return append_accessor(doc, packed, np.asarray(values, dtype='<f4' if component == 5126 else '<u4'), kind, component)
            attrs = {'POSITION': add(out, 'VEC3'), 'NORMAL': add([[0, 0, 1]] * len(out), 'VEC3')}
            if textured:
                attrs['TEXCOORD_0'] = add(tex, 'VEC2')
            primitive = {'attributes': attrs, 'indices': add(np.arange(len(out)), 'SCALAR', 5125),
                         'material': prim['material'], 'extras': {'rm': {'layer': 'markings', 'kind': 'artwork' if textured else 'paint'}}}
            doc['nodes'][0]['children'].append(len(doc['nodes']))
            doc['nodes'].append({'name': node.get('name', str(ni)), 'mesh': len(doc['meshes'])})
            doc['meshes'].append({'primitives': [primitive]})
            report['selections'].append({'node': node.get('name'), 'primitive': pi, 'material': material.get('name'), 'triangles': len(out) // 3})

    args.output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(args.v2, args.output)
    doc['buffers'] = [{'byteLength': len(packed)}]
    visual, collision = 'v12-overlays.glb', 'v12-overlays-collision.glb'
    write_glb(args.output / visual, doc, bytes(packed))
    write_glb(args.output / collision, {'asset': {'version': '2.0'}, 'scene': 0,
              'scenes': [{'nodes': [0]}], 'nodes': [{'name': 'Visual-only overlays'}]}, b'')
    manifest.setdefault('static_assets', []).append('v12-overlays')
    manifest['assets']['v12-overlays'] = {'visual': visual, 'visual_sha256': digest(args.output / visual),
        'collision': collision, 'collision_sha256': digest(args.output / collision),
        'collision_method': 'source-tessellation-v1',
        'placements_in_source_arena_frame': [{'translation_m': [0, 0, 0], 'rotation_xyzw': [0, 0, 0, 1]}]}
    manifest['preview_composition'] = 'V2 extraction with projected V1.2 horizontal paint and atlas artwork; equipment remains V2'
    (args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    (args.output / 'v12-overlay-preview.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'output': str(args.output), 'patches': len(report['selections']),
                      'triangles': sum(s['triangles'] for s in report['selections']),
                      'omitted': report['omitted_triangles_without_surface']}, indent=2))


if __name__ == '__main__':
    main()
