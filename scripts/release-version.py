#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Validate release inputs and stamp workspace manifests and lockfile in CI only."""
import argparse
import os
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]


def release_version(base, channel, number):
    """Return a SemVer version without accepting shell or ref metacharacters."""
    integer = r"(?:0|[1-9][0-9]*)"
    if not re.fullmatch(rf"{integer}\.{integer}\.{integer}", base):
        raise ValueError("version must be MAJOR.MINOR.PATCH without a v prefix")
    if channel not in ("stable", "alpha", "beta", "rc"):
        raise ValueError("unknown release channel")
    if not re.fullmatch(r"(?:rc)?[1-9][0-9]*", number):
        raise ValueError("prerelease identifier must be a positive integer or rc followed by one")
    return base if channel == "stable" else f"{base}-{channel}.{number}"


def stamp(root, version):
    """Update only local workspace versions, preserving locked third-party packages."""
    manifest = root / "Cargo.toml"
    content = manifest.read_text()
    # These patterns deliberately support the repository's literal member list,
    # not arbitrary TOML. Refuse a changed layout instead of rewriting guesses.
    members = re.search(r"(?ms)^members = \[(.*?)\]", content)
    package_version = re.search(r'(\[workspace\.package\]\s+version = ")[^"]+(")', content)
    if members is None or package_version is None:
        raise ValueError("unsupported workspace manifest layout")
    manifests = [root / member / "Cargo.toml" for member in re.findall(r'"([^"]+)"', members[1])]
    names = []
    for path in manifests:
        name = re.search(r'(?m)^name = "([^"]+)"$', path.read_text())
        if name is None:
            raise ValueError(f"missing package name: {path}")
        names.append(name[1])
    content = re.sub(r'(\[workspace\.package\]\s+version = ")[^"]+(")',
                     lambda m: m[1] + version + m[2], content, count=1)
    manifest.write_text(content)
    for path in manifests:
        content = path.read_text()
        for name in names:
            content = re.sub(rf'({re.escape(name)}\s*=\s*\{{[^}}]*?\bversion\s*=\s*")[^"]+(".*)',
                             lambda m: m[1] + version + m[2], content)
        path.write_text(content)
    lock = root / "Cargo.lock"
    content = lock.read_text()
    for name in names:
        content, count = re.subn(rf'(\[\[package\]\]\nname = "{re.escape(name)}"\nversion = ")[^"]+("\n)',
                                lambda m: m[1] + version + m[2], content)
        if count != 1:
            raise ValueError(f"expected exactly one locked workspace package: {name}")
    lock.write_text(content)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stamp", action="store_true")
    args = parser.parse_args()
    version = release_version(os.environ["RELEASE_VERSION"], os.environ["RELEASE_CHANNEL"],
                              os.environ["RELEASE_NUMBER"])
    if args.stamp:
        stamp(ROOT, version)
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a") as stream:
            stream.write(f"version={version}\ntag=v{version}\n")
    print(version)
