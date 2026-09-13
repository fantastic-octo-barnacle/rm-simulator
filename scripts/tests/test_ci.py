# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""CI validation receipt trust boundaries and generated-cache pruning tests."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]


def load(name):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


validation = load("validated-commit")
cache = load("prune-build-cache")


class ValidationTests(unittest.TestCase):
    def setUp(self):
        self.run = {
            "id": 10, "head_sha": "a" * 40, "status": "completed",
            "conclusion": "success", "event": "workflow_dispatch",
            "repository": {"full_name": "owner/repo"},
            "path": ".github/workflows/full-check.yml", "html_url": "https://example.test/run/10",
        }

    def check(self, run):
        return validation.successful_run([run], "owner/repo", "a" * 40, 11)

    def test_only_exact_source_success_from_trusted_workflow_qualifies(self):
        self.assertEqual(self.check(self.run), self.run)
        for key, value in [
            ("head_sha", "b" * 40), ("status", "in_progress"),
            ("conclusion", "failure"), ("conclusion", "cancelled"),
            ("conclusion", "skipped"), ("event", "pull_request"),
            ("repository", {"full_name": "fork/repo"}),
            ("path", ".github/workflows/ci.yaml"), ("id", 11),
        ]:
            with self.subTest(key=key, value=value):
                candidate = copy.deepcopy(self.run)
                candidate[key] = value
                self.assertIsNone(self.check(candidate))

    def test_successful_release_including_dry_run_qualifies(self):
        self.run["path"] = ".github/workflows/release.yml"
        self.assertEqual(self.check(self.run), self.run)

    def test_api_errors_require_validation(self):
        with patch.object(validation.subprocess, "run") as request:
            request.return_value.returncode = 1
            self.assertIsNone(validation.lookup("owner/repo", "a" * 40, 11))

    def test_release_history_is_checked_when_manual_history_is_empty(self):
        self.run["path"] = ".github/workflows/release.yml"
        with patch.object(validation.subprocess, "run") as request:
            request.side_effect = [
                validation.subprocess.CompletedProcess([], 0, json.dumps({"workflow_runs": []})),
                validation.subprocess.CompletedProcess([], 0, json.dumps({"workflow_runs": [self.run]})),
            ]
            self.assertEqual(validation.lookup("owner/repo", "a" * 40, 11), self.run)

    def test_force_bypasses_history_and_outputs_false(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "output"
            with patch.dict(validation.os.environ, {
                "FORCE_VALIDATION": "true", "GITHUB_OUTPUT": str(output),
                "GITHUB_STEP_SUMMARY": "",
            }), patch.object(validation, "lookup") as lookup:
                validation.main()
            lookup.assert_not_called()
            self.assertEqual(output.read_text(), "reused=false\n")


class CacheTests(unittest.TestCase):
    def test_pruning_keeps_installed_libraries_and_rust_artifacts(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary)
            native = target / "release/build/game-networking-sockets-sys-abc/out"
            removable = [native / "GNS/vcpkg/.git/objects/pack/data", native / "GNS/vcpkg/downloads/archive"]
            retained = [native / "vcpkg/installed/lib/gns.lib", native / "build/src/gns.lib",
                        target / "release/deps/library.rlib", native / "GNS/vcpkg/vcpkg.exe"]
            for path in removable + retained:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"data")
            self.assertEqual(cache.prune(target), 8)
            self.assertTrue(all(path.exists() for path in removable))
            self.assertEqual(cache.prune(target, apply=True), 8)
            self.assertTrue(all(path.exists() for path in retained))
            self.assertTrue(all(not path.exists() for path in removable))

    def test_pruning_does_not_follow_an_external_directory_link(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "target"
            native = target / "release/build/game-networking-sockets-sys-abc/out/GNS/vcpkg"
            native.mkdir(parents=True)
            external = root / "external"
            external.mkdir()
            (external / "keep").write_text("keep")
            try:
                (native / "downloads").symlink_to(external, target_is_directory=True)
            except OSError:
                self.skipTest("directory symlinks unavailable")
            self.assertEqual(cache.prune(target, apply=True), 0)
            self.assertTrue((external / "keep").exists())


if __name__ == "__main__":
    unittest.main()
