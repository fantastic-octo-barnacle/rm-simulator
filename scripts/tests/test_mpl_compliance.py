# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""MPL-2.0 notice checking, exercised against the real repository state."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "check-mpl-compliance.py"
spec = importlib.util.spec_from_file_location("check_mpl_compliance", SCRIPT)
mpl = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mpl)


class LicenseExpressionTests(unittest.TestCase):
    def test_tokens_drop_operators(self):
        self.assertEqual(mpl.license_tokens("MIT OR Apache-2.0"), {"MIT", "Apache-2.0"})
        self.assertEqual(mpl.license_tokens("(MIT) AND (Apache-2.0)"), {"MIT", "Apache-2.0"})
        self.assertEqual(mpl.license_tokens(""), set())

    def test_requires_mpl_only_for_mpl(self):
        self.assertTrue(mpl.requires_mpl("MPL-2.0"))
        self.assertTrue(mpl.requires_mpl("MIT OR MPL-2.0"))
        self.assertFalse(mpl.requires_mpl("MIT OR Apache-2.0"))
        # `MPL-2.0-exception` is not the identifier MPL-2.0.
        self.assertFalse(mpl.requires_mpl("MIT"))


class NoticeParsingTests(unittest.TestCase):
    def test_only_data_rows_survive(self):
        text = (
            "| Component | Version | Upstream source | Immutable source archive |\n"
            "|---|---|---|---|\n"
            "| selectors | 0.38.0 | [stylo](https://example) "
            "| [selectors-0.38.0.crate](https://static.crates.io/crates/"
            "selectors/selectors-0.38.0.crate) |\n"
        )
        self.assertEqual(mpl.notice_rows(text), {"selectors": "0.38.0"})
        archives = {(m.group("name"), m.group("version"))
                    for m in mpl.ARCHIVE_URL.finditer(text)}
        self.assertEqual(archives, {("selectors", "0.38.0")})

    def test_deny_allowlist_reads_exception_blocks(self):
        text = (
            '[licenses]\nallow = ["MIT"]\n\n'
            '[[licenses.exceptions]]\nname = "cssparser"\nallow = ["MPL-2.0"]\n\n'
            '[[licenses.exceptions]]\nname = "other"\nallow = ["MIT"]\n'
        )
        self.assertEqual(
            mpl.deny_mpl_allowlist(text),
            {"cssparser": {"MPL-2.0"}, "other": {"MIT"}},
        )

    def test_patch_detection(self):
        with tempfile.TemporaryDirectory() as temp:
            manifest = Path(temp) / "Cargo.toml"
            manifest.write_text(
                "[patch.crates-io]\nselectors = { path = \"vendor/selectors\" }\n"
            )
            self.assertEqual(mpl.patched_crates([manifest]), {"selectors"})

    def test_normalized_license_ignores_line_endings_and_padding(self):
        self.assertEqual(
            mpl.normalized_license("a  \r\nb\n"),
            mpl.normalized_license("a\nb\n"),
        )

    def test_no_such_manifest_yields_no_shipped_text(self):
        self.assertEqual(
            mpl.shipped_license_texts(
                [{"name": "x", "license": "MPL-2.0", "manifest_path": "/nonexistent/Cargo.toml"}]
            ),
            {},
        )


class RepositoryStateTests(unittest.TestCase):
    """The checked-in notice, allowlist and lockfile must agree."""

    def test_repository_is_compliant(self):
        packages = mpl.cargo_packages()
        self.assertEqual(mpl.check(packages), [])

    def test_every_covered_crate_is_reported(self):
        covered = mpl.covered_names(mpl.cargo_packages())
        self.assertEqual(sorted(covered), sorted(set(covered)))
        self.assertIn("cssparser", covered)
        self.assertIn("selectors", covered)

    def test_vendored_text_matches_the_components_own_copy(self):
        found = mpl.shipped_license_texts(mpl.cargo_packages())
        self.assertIn("cssparser", found, "registry source is needed for this check")
        own = mpl.normalized_license(mpl.MPL_TEXT.read_text(encoding="utf-8"))
        for name, path in found.items():
            body = path.read_text(encoding="utf-8", errors="replace")
            self.assertEqual(mpl.normalized_license(body), own, name)

    def test_missing_archive_link_is_detected(self):
        packages = mpl.cargo_packages()
        original = mpl.NOTICE
        with tempfile.TemporaryDirectory() as temp:
            stripped = Path(temp) / "NOTICE.md"
            text = original.read_text(encoding="utf-8")
            stripped.write_text(
                mpl.ARCHIVE_URL.sub("https://example.invalid/removed", text),
                encoding="utf-8",
            )
            mpl.NOTICE = stripped
            try:
                failures = mpl.check(packages)
            finally:
                mpl.NOTICE = original
        self.assertTrue(any("source archive" in failure for failure in failures))


if __name__ == "__main__":
    unittest.main()
