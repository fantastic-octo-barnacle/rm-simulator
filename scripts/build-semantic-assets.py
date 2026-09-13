#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Compose optimized scenery with checksum-pinned semantic equipment; CAD stays local."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--optimized', type=Path, required=True)
    parser.add_argument('--semantic', type=Path, required=True)
    parser.add_argument('--out', type=Path, default=Path(__file__).resolve().parents[1] / 'local-assets/field')
    args = parser.parse_args()
    if args.out.exists():
        parser.error('output exists; choose a new directory to preserve the existing package')
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=args.out.parent) as temporary:
        stage = Path(temporary) / 'field'
        shutil.copytree(args.optimized, stage)
        provenance = {
            'schema_version': 2,
            'command': [sys.executable, *sys.argv],
            'working_directory': str(Path.cwd()),
            'script_sha256': digest(Path(__file__)),
            'optimized': str(args.optimized.resolve()),
            'semantic': str(args.semantic.resolve()),
            'operation': 'compose existing exports without tessellation or simplification',
            'inputs': {},
            'upstream_records': {},
        }
        # Preserve the export recipe chain before replacing the composition record.
        for label, root in [('optimized', args.optimized), ('semantic', args.semantic)]:
            provenance['upstream_records'][label] = {
                name: {'sha256': digest(root / name), 'record': json.loads((root / name).read_text())}
                for name in ('build-provenance.json', 'deployment.json', 'reference.json')
                if (root / name).exists()
            }
        for scope in ('', 'equipment'):
            target, source = stage / scope, args.semantic / scope
            manifest = json.loads((target / 'manifest.json').read_text())
            reference = json.loads((source / 'manifest.json').read_text())
            descriptor = reference['articulation']
            sidecar_path = source / descriptor['file']
            if digest(sidecar_path) != descriptor['sha256']:
                raise ValueError('semantic sidecar checksum mismatch')
            sidecar = json.loads(sidecar_path.read_text())
            for name in sidecar['assets']:
                entry = reference['assets'][name]
                for kind in ('visual', 'collision'):
                    if kind in entry:
                        path = source / entry[kind]
                        if digest(path) != entry[kind + '_sha256']:
                            raise ValueError(f'{name}/{kind}: checksum mismatch')
                        shutil.copy2(path, target / entry[kind])
                manifest['assets'][name] = entry
            shutil.copy2(sidecar_path, target / descriptor['file'])
            manifest['articulation'] = descriptor
            # A composed package has a different equipment manifest from the reference.
            manifest.pop('reference_equipment', None)
            (target / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
            provenance['inputs'][scope or 'field'] = {
                'optimized_manifest_sha256': digest(args.optimized / scope / 'manifest.json'),
                'semantic_manifest_sha256': digest(source / 'manifest.json'),
                'articulation_sha256': descriptor['sha256'],
                'semantic_assets': list(sidecar['assets']),
                'joints': {
                    name: [joint['id'] for joint in asset['files']['visual']['joints']]
                    for name, asset in sidecar['assets'].items()
                },
                # Full input manifests retain export_policy, simplification and
                # texture settings, including per-asset overrides and source hashes.
                'optimized_manifest': json.loads((args.optimized / scope / 'manifest.json').read_text()),
                'semantic_manifest': reference,
            }
            for entry in manifest['assets'].values():
                for kind in ('visual', 'collision'):
                    if kind in entry and digest(target / entry[kind]) != entry[kind + '_sha256']:
                        raise ValueError(f'composed {kind} checksum mismatch')
        (stage / 'build-provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
        stage.rename(args.out)
    print(args.out.resolve())


if __name__ == '__main__':
    main()
