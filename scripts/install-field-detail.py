#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Install a reviewed derivative, retaining the previous package for rollback."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--candidate', required=True, type=Path)
    parser.add_argument('--destination', required=True, type=Path)
    parser.add_argument('--backup', required=True, type=Path)
    parser.add_argument('--inspector', required=True, type=Path)
    parser.add_argument('--allow-collision-budget-overrun', action='store_true')
    args = parser.parse_args()
    candidate, destination, backup = (p.resolve() for p in (args.candidate, args.destination, args.backup))
    if not destination.is_dir() or backup.exists() or backup.parent != destination.parent:
        parser.error('destination must exist; backup must be new and beside it')
    if candidate == destination or candidate.is_relative_to(destination):
        parser.error('candidate must be external to destination')
    build = json.loads((candidate / 'field-detail-build.json').read_text())
    if build['after']['visual'] >= build['settings']['budgets']['visual']:
        parser.error('visual triangle budget failed')
    if not build['budget_passed'] and not args.allow_collision_budget_overrun:
        parser.error('collision budget failed; explicit overrun acceptance required')
    minimap = json.loads((candidate / 'minimap.json').read_text())
    for path, expected in minimap['manifest_sha256'].items():
        if hashlib.sha256((candidate / path).read_bytes()).hexdigest() != expected:
            parser.error('stale minimap')
    subprocess.run([str(args.inspector.resolve()), str(candidate)], check=True)
    with tempfile.TemporaryDirectory(prefix='field-install-', dir=destination.parent) as temporary:
        stage = Path(temporary) / 'package'
        shutil.copytree(candidate, stage)
        destination.rename(backup)
        try:
            stage.rename(destination)
        except BaseException:
            backup.rename(destination)
            raise
    print(json.dumps({'installed': str(destination), 'rollback_package': str(backup)}))


if __name__ == '__main__':
    main()
