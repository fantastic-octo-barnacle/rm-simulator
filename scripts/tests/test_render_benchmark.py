# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Sweep expansion and subprocess failure reporting, without a GPU."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "benchmark-render.py"
spec = importlib.util.spec_from_file_location("benchmark_render", SCRIPT)
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


class RenderBenchmarkTests(unittest.TestCase):
    def test_axes_repetitions_and_seed(self):
        config = {"base": {"graphics": {"preset": "High"}}, "sweep": {
            "graphics.preset": ["High", "Ultra"], "geometry": ["normal", "sparse"]},
            "repeats": 3, "seed": 5}
        first = bench.expand(config)
        self.assertEqual(first, bench.expand(config))
        self.assertEqual(len(first), 12)
        self.assertEqual(len({(index, repeat) for index, repeat, _ in first}), 12)
        self.assertEqual(config["base"], {"graphics": {"preset": "High"}})
        self.assertEqual({case["graphics"]["preset"] for _, _, case in first}, {"High", "Ultra"})

    def test_reject_empty_axis_and_unknown_config(self):
        for config in ({"sweep": {"geometry": []}}, {"repeat": 2}, {"repeats": 0}):
            with self.assertRaises(ValueError):
                bench.expand(config)

    def test_failed_process_remains_in_comparison(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / "fake-bench"
            binary.write_text("#!/bin/sh\nexit 7\n")
            binary.chmod(0o755)
            config = root / "config.json"
            config.write_text(json.dumps({"repeats": 1}))
            output = root / "results"
            result = subprocess.run([sys.executable, str(SCRIPT), str(config),
                "--binary", str(binary), "--cad-assets", str(root), "--output", str(output),
                "--cooldown", "0", "--cpu-only"], capture_output=True, text=True, check=False)
            self.assertEqual(result.returncode, 1, result.stderr)
            report = json.loads((output / "sweep.json").read_text())
            self.assertEqual(report["runs"][0]["exit_code"], 7)
            self.assertFalse(report["runs"][0]["valid"])
            self.assertIn("False", (output / "comparison.csv").read_text())
