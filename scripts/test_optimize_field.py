# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Budget accounting must include every placement, including shared GLBs."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('optimize_field', Path(__file__).with_name('optimize-field.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class BudgetTests(unittest.TestCase):
    def test_shared_meshes_count_per_placement_and_empty_placement_is_one(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'equipment').mkdir()
            def entry(copies):
                return {'visual': 'shared.glb', 'collision': 'shared-collision.glb',
                        'placements_in_source_arena_frame': [{}] * copies}
            (root / 'manifest.json').write_text(json.dumps({'assets': {'floor': entry(0), 'a': entry(2), 'b': entry(2)}}))
            (root / 'equipment/manifest.json').write_text(json.dumps({'assets': {'base': entry(2)}}))
            result = module.placed_counts(root, lambda path: (path.name, None),
                                          lambda name: 10 if name == 'shared.glb' else 3)
            self.assertEqual(result['visual'], 70)
            self.assertEqual(result['collision'], 21)
            self.assertEqual(result['assets']['/floor']['placements'], 1)


if __name__ == '__main__':
    unittest.main()
