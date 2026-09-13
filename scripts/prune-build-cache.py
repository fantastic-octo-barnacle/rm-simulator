#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Measure or remove rebuildable vcpkg downloads and Git history from GNS output.

The locked game-networking-sockets-sys 0.3.0 build script recreates out/GNS
before invoking vcpkg on Windows. Completed builds need the installed libraries,
not the clone's Git history or downloaded archives. Keep all installed files,
CMake output, Cargo fingerprints and Rust artifacts. Never modify registry source.
"""
import argparse
import os
from pathlib import Path
import shutil


def candidates(target):
    """Find only generated vcpkg Git/download directories, without following links."""
    target = target.resolve()
    for root in (target / "release" / "build").glob("game-networking-sockets-sys-*/out/GNS/vcpkg"):
        for name in (".git", "downloads"):
            path = root / name
            if path.is_dir() and not path.is_symlink() and path.resolve().is_relative_to(target):
                yield path


def prune(target, apply=False):
    """Report bytes eligible for removal, optionally delete them, and return bytes."""
    total = 0
    for path in candidates(target):
        size = sum(file.stat().st_size for file in path.rglob("*")
                   if file.is_file() and not file.is_symlink())
        total += size
        print(f"{'Remove' if apply else 'Would remove'} {size / 1024**2:.1f} MiB: {path}")
        if apply:
            # Windows Git object files can be read-only.
            def retry(function, name, error):
                os.chmod(name, 0o700)
                function(name)
            shutil.rmtree(path, onerror=retry)
    print(f"Rebuildable cache data: {total / 1024**2:.1f} MiB")
    return total


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target-dir", type=Path, default=Path(os.environ.get("CARGO_TARGET_DIR", "target")))
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    prune(args.target_dir, args.apply)


if __name__ == "__main__":
    main()
