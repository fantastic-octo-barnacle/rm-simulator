# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Release input, lockfile and LFS field-staging regression tests."""
import importlib.util
import hashlib
from pathlib import Path
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]


def load(name):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


version = load("release-version")
field = load("stage-release-field")


class ReleaseTests(unittest.TestCase):
    def test_channels(self):
        self.assertEqual(version.release_version("1.2.3", "stable", "9"), "1.2.3")
        for channel in ("alpha", "beta", "rc"):
            self.assertEqual(version.release_version("1.2.3", channel, "2"), f"1.2.3-{channel}.2")

    def test_alpha_release_candidate(self):
        self.assertEqual(version.release_version("0.0.1", "alpha", "rc1"), "0.0.1-alpha.rc1")

    def test_invalid_inputs(self):
        for base in ("v1.2.3", "01.2.3", "1.2", "1.2.3; echo bad", "1.2.3\n"):
            with self.assertRaises(ValueError):
                version.release_version(base, "alpha", "1")
        for channel, number in (("RC", "1"), ("alpha", "0"), ("beta", "01"), ("rc", "$(id)"), ("alpha", "rc0"), ("alpha", "rc01")):
            with self.assertRaises(ValueError):
                version.release_version("1.2.3", channel, number)

    def test_stamp_keeps_third_party_lock_entries(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "app").mkdir()
            (root / "lib").mkdir()
            (root / "Cargo.toml").write_text('[workspace]\nmembers = ["app", "lib"]\n[workspace.package]\nversion = "0.1.0"\n')
            (root / "app/Cargo.toml").write_text('[package]\nname = "app"\nversion.workspace = true\n[dependencies]\nlib = { version = "0.1.0", path = "../lib" }\n[dev-dependencies]\nlib = { path = "../lib", version = "0.1.0" }\n')
            (root / "lib/Cargo.toml").write_text('[package]\nname = "lib"\nversion.workspace = true\n')
            third_party = '[[package]]\nname = "serde"\nversion = "1.0.0"\nsource = "registry+example"\n'
            (root / "Cargo.lock").write_text('version = 4\n[[package]]\nname = "app"\nversion = "0.1.0"\n[[package]]\nname = "lib"\nversion = "0.1.0"\n' + third_party)
            version.stamp(root, "2.0.0-rc.1")
            lock = (root / "Cargo.lock").read_text()
            self.assertIn(third_party, lock)
            self.assertEqual(lock.count('version = "2.0.0-rc.1"'), 2)
            self.assertIn('lib = { path = "../lib", version = "2.0.0-rc.1" }', (root / "app/Cargo.toml").read_text())
            self.assertIn('lib = { version = "2.0.0-rc.1", path = "../lib" }', (root / "app/Cargo.toml").read_text())

    def fixture(self, root):
        files = {}
        for name in ("manifest.json", "equipment/manifest.json", "CAD-NOTICE.md"):
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"test")
            files[name] = hashlib.sha256(b"test").hexdigest()
        return files

    def test_valid_field_stages_and_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            files = self.fixture(source)
            destination = root / "output/field"
            field.stage(source, destination, files)
            self.assertEqual((destination / "CAD-NOTICE.md").read_bytes(), b"test")
            with self.assertRaises(ValueError):
                field.stage(source, destination, files)

    def test_pointer_can_be_checked_but_never_packaged(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            files = self.fixture(root / "source")
            pointer = field.POINTER + f"oid sha256:{files['manifest.json']}\nsize 4\n".encode()
            (root / "source/manifest.json").write_bytes(pointer)
            field.verify(root / "source", files, allow_pointers=True)
            with self.assertRaisesRegex(ValueError, "git lfs pull"):
                field.stage(root / "source", root / "output", files)
            self.assertFalse((root / "output").exists())
            files["manifest.json"] = "0" * 64
            with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
                field.verify(root / "source", files, allow_pointers=True)

    def test_corrupt_or_extra_files_are_rejected_before_staging(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            files = self.fixture(source)
            (source / "manifest.json").write_bytes(b"corrupt")
            with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
                field.stage(source, root / "output", files)
            self.assertFalse((root / "output").exists())
            (source / "extra").write_bytes(b"extra")
            with self.assertRaisesRegex(ValueError, "inventory"):
                field.verify(source, files)

    def test_unsafe_paths_and_symlinks_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary).resolve()
            for name in ("../escape", "/absolute", "field\\escape", "C:bad", ""):
                with self.subTest(name=name), self.assertRaises(ValueError):
                    field.field_file(source, name)
            (source / "target").write_bytes(b"test")
            try:
                (source / "link").symlink_to(source / "target")
            except OSError:
                self.skipTest("symlinks unavailable")
            with self.assertRaises(ValueError):
                field.field_file(source, "link")


if __name__ == "__main__":
    unittest.main()
