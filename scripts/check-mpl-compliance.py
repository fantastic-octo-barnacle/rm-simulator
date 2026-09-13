#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Assert that every MPL-2.0 dependency is licensed, noticed and unmodified.

`rm-simulator` is `MIT OR Apache-2.0`, but Bevy Flair drags in a handful of
MPL-2.0 crates. The MPL is file-level copyleft: linking against unmodified
covered software is fine, but section 3.2 requires that whoever receives a
binary is told that the covered files are MPL-2.0 and how to obtain their
Source Code Form. That obligation lives in `NOTICE.md`; this script keeps the
notice, `deny.toml` and the resolved graph from drifting apart.

It fails when

* a resolved dependency requires MPL-2.0 and `deny.toml` does not allow it for
  that exact crate, so `cargo deny` would reject the build;
* `NOTICE.md` does not name the covered crate at its locked version, or does
  not link the immutable source archive the MPL-2.0 source offer depends on;
* `LICENSES/MPL-2.0.txt` is missing or is not the real MPL-2.0 text, or it
  disagrees with the `LICENSE` file a covered crate actually ships;
* a `[patch]` or `[replace]` entry in any manifest rewrites a covered crate,
  which would mean we ship Modified Covered Software and must publish it;
* a covered crate's source is vendored into this repository;
* `deny.toml` or `NOTICE.md` still lists a covered crate that no longer
  resolves.

Run it directly, or through `just mpl`.
"""
from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NOTICE = ROOT / "NOTICE.md"
DENY = ROOT / "deny.toml"
MPL_TEXT = ROOT / "LICENSES" / "MPL-2.0.txt"

MPL_ID = "MPL-2.0"

# `| name | 1.2.3 | repo | archive |` rows of the NOTICE.md components table.
NOTICE_ROW = re.compile(
    r"^\|\s*(?P<name>[A-Za-z0-9_.-]+)\s*\|\s*(?P<version>[0-9][^|]*?)\s*\|"
    r"(?P<rest>.*?)\|\s*$",
    re.MULTILINE,
)
# An immutable, version-pinned source archive on the crates.io static CDN.
ARCHIVE_URL = re.compile(
    r"https://static\.crates\.io/crates/(?P<name>[A-Za-z0-9_.-]+)/"
    r"(?P=name)-(?P<version>[0-9][^/)\s]*)\.crate"
)
# The MPL-2.0 text is recognisable by its title and both exhibits.
MPL_MARKERS = (
    "Mozilla Public License Version 2.0",
    "3. Responsibilities",
    "Exhibit A - Source Code Form License Notice",
    'Exhibit B - "Incompatible With Secondary Licenses" Notice',
)


def license_tokens(expression: str) -> set[str]:
    """Return the SPDX license identifiers in an expression.

    Parentheses, the ``AND``/``OR`` operators and the ``WITH`` operator (and
    the exception that follows it) are not identifiers.

    >>> sorted(license_tokens("MIT OR Apache-2.0"))
    ['Apache-2.0', 'MIT']
    >>> sorted(license_tokens("(MIT OR Apache-2.0) AND Unicode-3.0"))
    ['Apache-2.0', 'MIT', 'Unicode-3.0']
    >>> sorted(license_tokens("GPL-2.0-only WITH Classpath-exception-2.0"))
    ['Classpath-exception-2.0', 'GPL-2.0-only']
    >>> license_tokens("")
    set()
    """
    return set(re.findall(r"[A-Za-z0-9.+-]+", expression)) - {"AND", "OR", "WITH"}


def requires_mpl(expression: str) -> bool:
    """Whether a license expression names MPL-2.0 at all.

    >>> requires_mpl("MPL-2.0")
    True
    >>> requires_mpl("MIT OR MPL-2.0")
    True
    >>> requires_mpl("MIT OR Apache-2.0")
    False
    """
    return MPL_ID in license_tokens(expression)


def cargo_packages() -> list[dict]:
    """Every package Cargo resolves for the workspace with all features on."""
    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1",
             "--all-features", "--locked"],
            cwd=ROOT, text=True,
        )
    )
    return metadata["packages"]


def deny_mpl_allowlist(text: str) -> dict[str, set[str]]:
    """Map crate name to allowed licenses from `[[licenses.exceptions]]`.

    >>> deny_mpl_allowlist('[licenses]\\nallow = ["MIT"]\\n')
    {}
    >>> sorted(deny_mpl_allowlist(
    ...     '[[licenses.exceptions]]\\nname = "cssparser"\\nallow = ["MPL-2.0"]\\n'
    ... ))
    ['cssparser']
    """
    allowed: dict[str, set[str]] = {}
    for block in text.split("[[licenses.exceptions]]")[1:]:
        name = re.search(r'^name\s*=\s*"([^"]+)"', block, re.MULTILINE)
        allow = re.search(r"^allow\s*=\s*\[(.*?)\]", block, re.MULTILINE | re.DOTALL)
        if name and allow:
            allowed[name.group(1)] = set(re.findall(r'"([^"]+)"', allow.group(1)))
    return allowed


def notice_rows(text: str) -> dict[str, str]:
    """Map crate name to the version documented in the NOTICE.md table.

    >>> notice_rows("| Component | Version | Source | Archive |\\n"
    ...             "|---|---|---|---|\\n"
    ...             "| selectors | 0.38.0 | [x](y) | [z](w) |\\n")
    {'selectors': '0.38.0'}
    """
    return {m.group("name"): m.group("version") for m in NOTICE_ROW.finditer(text)}


def patched_crates(manifests: list[Path]) -> set[str]:
    """Crate names rewritten by a `[patch]` or `[replace]` manifest entry."""
    patched: set[str] = set()
    for manifest in manifests:
        text = manifest.read_text(encoding="utf-8")
        for section in re.finditer(
            r"^\[(?:patch\.[^\]]+|replace)\]\s*$(.*?)(?=^\[|\Z)",
            text, re.MULTILINE | re.DOTALL,
        ):
            patched |= set(
                re.findall(r'^"?([A-Za-z0-9_.-]+)"?\s*=', section.group(1), re.MULTILINE)
            )
    return patched


def workspace_manifests() -> list[Path]:
    """The root manifest and every crate manifest in the workspace."""
    return [ROOT / "Cargo.toml", *sorted(ROOT.glob("crates/*/Cargo.toml"))]


def vendored_source(crate: str, version: str) -> Path | None:
    """A directory that vendors the covered crate's source, if one exists."""
    for candidate in (
        ROOT / "vendor" / crate,
        ROOT / "vendor" / f"{crate}-{version}",
        ROOT / "third_party" / crate,
        ROOT / crate,
    ):
        if (candidate / "Cargo.toml").is_file():
            return candidate
    return None


def normalized_license(text: str) -> list[str]:
    """Comparable form of a license text: line endings and trailing space removed.

    >>> normalized_license("a  \\r\\nb\\n")
    ['a', 'b', '']
    """
    return [line.rstrip() for line in text.replace("\r\n", "\n").split("\n")]


def shipped_license_texts(packages: list[dict]) -> dict[str, Path]:
    """Map each covered crate that ships one to its MPL-2.0 `LICENSE` file.

    Only crates whose source Cargo already unpacked are considered, so this
    reports what it can rather than failing on a machine with no registry.

    >>> shipped_license_texts([{"name": "x", "license": "MIT", "manifest_path": "/nope"}])
    {}
    """
    found: dict[str, Path] = {}
    for package in packages:
        if not requires_mpl(package.get("license") or ""):
            continue
        manifest = Path(package.get("manifest_path") or "")
        if not manifest.is_file():
            continue
        candidates = sorted(manifest.parent.glob("LICEN[CS]E*"))
        candidates += sorted(manifest.parent.glob("COPYING*"))
        for candidate in candidates:
            if not candidate.is_file():
                continue
            body = candidate.read_text(encoding="utf-8", errors="replace")
            if "Mozilla Public License Version 2.0" in body:
                found[package["name"]] = candidate
                break
    return found


def check(packages: list[dict]) -> list[str]:
    """Return one human-readable failure per compliance problem found."""
    failures: list[str] = []
    covered = {
        package["name"]: package["version"]
        for package in packages
        if requires_mpl(package.get("license") or "")
    }

    allowlist = deny_mpl_allowlist(DENY.read_text(encoding="utf-8"))
    notice = NOTICE.read_text(encoding="utf-8")
    rows = notice_rows(notice)
    archives = {(m.group("name"), m.group("version")) for m in ARCHIVE_URL.finditer(notice)}
    patched = patched_crates(workspace_manifests())

    if not MPL_TEXT.is_file():
        failures.append(f"{MPL_TEXT.relative_to(ROOT)} is missing")
    else:
        text = MPL_TEXT.read_text(encoding="utf-8")
        failures += [
            f"{MPL_TEXT.relative_to(ROOT)} does not contain {marker!r}"
            for marker in MPL_MARKERS
            if marker not in text
        ]
        own = normalized_license(text)
        for name, path in sorted(shipped_license_texts(packages).items()):
            if normalized_license(path.read_text(encoding="utf-8", errors="replace")) != own:
                failures.append(
                    f"{MPL_TEXT.relative_to(ROOT)} differs from the MPL-2.0 text "
                    f"{name} ships at {path}"
                )

    if not covered:
        failures.append(
            "no MPL-2.0 dependency is resolved; the NOTICE.md components "
            "section and the deny.toml exceptions should be removed"
        )

    for name, version in sorted(covered.items()):
        if MPL_ID not in allowlist.get(name, set()):
            failures.append(
                f"deny.toml does not allow {MPL_ID} for {name} {version} "
                f"(add a [[licenses.exceptions]] block)"
            )
        if rows.get(name) != version:
            failures.append(
                f"NOTICE.md documents {name} as {rows.get(name)!r}, "
                f"but Cargo resolves {version}"
            )
        if (name, version) not in archives:
            failures.append(
                f"NOTICE.md does not link the source archive "
                f"https://static.crates.io/crates/{name}/{name}-{version}.crate"
            )
        if name in patched:
            failures.append(
                f"{name} is rewritten by a [patch]/[replace] entry; publish the "
                f"modified source and say so in NOTICE.md"
            )
        site = vendored_source(name, version)
        if site is not None:
            failures.append(
                f"{site.relative_to(ROOT)} vendors {name}; delete it, or treat "
                f"the copy as Modified Covered Software in NOTICE.md"
            )

    failures += [
        f"deny.toml allows {MPL_ID} for {name}, which is no longer resolved"
        for name, allow in sorted(allowlist.items())
        if MPL_ID in allow and name not in covered
    ]
    failures += [
        f"NOTICE.md documents {name}, which no longer resolves as MPL-2.0"
        for name in sorted(rows)
        if name in allowlist and name not in covered
    ]
    return failures


def main() -> int:
    """Report the compliance failures, or the number of covered crates."""
    packages = cargo_packages()
    failures = check(packages)
    if failures:
        print("MPL-2.0 compliance check failed:")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print(
        f"MPL-2.0 compliance check passed for {len(covered_names(packages))} "
        f"covered dependencies."
    )
    return 0


def covered_names(packages: list[dict]) -> list[str]:
    """The names of the resolved packages that require MPL-2.0.

    >>> covered_names([{"name": "a", "license": "MPL-2.0"},
    ...                {"name": "b", "license": "MIT"}])
    ['a']
    """
    return [
        package["name"]
        for package in packages
        if requires_mpl(package.get("license") or "")
    ]


if __name__ == "__main__":
    sys.exit(main())
