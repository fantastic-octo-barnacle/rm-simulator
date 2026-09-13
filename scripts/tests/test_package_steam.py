# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
import importlib.util
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("package_steam", Path(__file__).parents[1] / "package-steam.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class SteamPackageTests(unittest.TestCase):
    def test_production_never_defaults_to_spacewar(self):
        for value in (None, 0, -1, 480, 2**32):
            with self.assertRaises(ValueError):
                MODULE.validate_app_id(value, False)
        MODULE.validate_app_id(480, True)
        MODULE.validate_app_id(12345, False)

    def test_mismatched_sdk_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            sdk = Path(directory)
            api = sdk / "public/steam/steam_api.json"
            api.parent.mkdir(parents=True)
            api.write_text('{"version": "old"}')
            expected = MODULE.digest(api)
            MODULE.validate_sdk(sdk, expected)
            api.write_text('{"version": "new"}')
            with self.assertRaises(ValueError):
                MODULE.validate_sdk(sdk, expected)

    def test_runtime_follows_native_target(self):
        self.assertEqual(MODULE.runtime_relative("aarch64-apple-darwin"), Path("osx/libsteam_api.dylib"))
        self.assertEqual(MODULE.runtime_relative("x86_64-pc-windows-msvc"), Path("win64/steam_api64.dll"))
        self.assertEqual(MODULE.runtime_relative("aarch64-unknown-linux-gnu"), Path("linuxarm64/libsteam_api.so"))
        with self.assertRaises(ValueError):
            MODULE.runtime_relative("wasm32-unknown-unknown")

    def test_license_artifacts_exist_and_cover_mpl(self):
        required = {"NOTICE.md", "LICENSE-MIT", "LICENSE-APACHE", "LICENSES/MPL-2.0.txt"}
        shipped = {artifact.as_posix() for artifact in MODULE.LICENSE_ARTIFACTS}
        self.assertEqual(shipped, required)
        for artifact in MODULE.LICENSE_ARTIFACTS:
            self.assertTrue((MODULE.ROOT / artifact).is_file(), artifact)

    def test_notice_still_carries_the_mpl_source_offer(self):
        notice = (MODULE.ROOT / "NOTICE.md").read_text()
        self.assertIn("MPL-2.0 section 3.2", notice)
        for crate in ("cssparser", "cssparser-color", "cssparser-macros",
                      "dtoa-short", "selectors"):
            self.assertIn(crate, notice)


if __name__ == "__main__":
    unittest.main()
