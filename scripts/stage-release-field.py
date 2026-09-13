#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Verify the versioned Git LFS field and stage it for release without networking."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import tempfile

ROOT = Path(__file__).resolve().parents[1]
POINTER = b"version https://git-lfs.github.com/spec/v1\n"
REQUIRED = {"manifest.json", "equipment/manifest.json", "CAD-NOTICE.md"}


def field_file(source, name):
    """Accept only regular files inside the field, with no traversal or symlinks."""
    relative = PurePosixPath(name)
    if (not relative.parts or relative.is_absolute() or ".." in relative.parts
            or "\\" in name or ":" in name):
        raise ValueError(f"unsafe field file: {name}")
    path = source / relative
    if path.resolve() != path.absolute() or not path.is_file():
        raise ValueError(f"missing or symlinked field file: {name}")
    return path


def verify(source, files, allow_pointers=False):
    """Verify every pinned file; cheap CI may verify LFS object IDs without downloads."""
    source = source.resolve()
    if not REQUIRED.issubset(files):
        raise ValueError("field inventory is missing required manifests or CAD notice")
    actual = {path.relative_to(source).as_posix() for path in source.rglob("*") if path.is_file()}
    if actual != set(files):
        raise ValueError("field files differ from the pinned inventory")
    for name, expected in files.items():
        data = field_file(source, name).read_bytes()
        if data.startswith(POINTER):
            if not allow_pointers:
                raise ValueError(f"{name} is an LFS pointer; run git lfs pull --include='field/**'")
            match = re.fullmatch(POINTER + rb"oid sha256:([0-9a-f]{64})\nsize [0-9]+\n", data)
            digest = match[1].decode() if match else None
        else:
            digest = hashlib.sha256(data).hexdigest()
        if digest != expected:
            raise ValueError(f"field SHA-256 mismatch: {name}")


def stage(source, destination, files):
    """Verify before copying; do not replace an existing destination."""
    if destination.exists() or destination.is_symlink():
        raise ValueError(f"{destination} already exists; remove it explicitly before staging")
    verify(source, files)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=destination.parent) as temporary:
        output = Path(temporary) / "field"
        for name in files:
            target = output / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / name, target)
        # Catch changes between the validation and copy before exposing the stage.
        verify(output, files)
        output.rename(destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, default=ROOT / "dist/field")
    parser.add_argument("--verify-only", action="store_true")
    parser.add_argument("--allow-lfs-pointers", action="store_true", help="only with --verify-only")
    parser.add_argument("--update-checksums", action="store_true", help="record an intentional field update")
    args = parser.parse_args()
    if args.allow_lfs_pointers and not args.verify_only:
        parser.error("--allow-lfs-pointers requires --verify-only")
    if args.update_checksums and (args.verify_only or args.allow_lfs_pointers):
        parser.error("--update-checksums cannot be combined with verification flags")
    contract_path = ROOT / "scripts/release-field.json"
    contract = json.loads(contract_path.read_text())
    source = ROOT / contract["directory"]
    if args.update_checksums:
        files = {}
        for path in sorted(source.rglob("*")):
            if path.is_file():
                name = path.relative_to(source).as_posix()
                data = field_file(source.resolve(), name).read_bytes()
                if data.startswith(POINTER):
                    raise ValueError("hydrate the LFS field before recording checksums")
                files[name] = hashlib.sha256(data).hexdigest()
        verify(source, files)
        contract["files"] = files
        contract_path.write_text(json.dumps(contract, indent=2) + "\n")
    elif args.verify_only:
        verify(source, contract["files"], args.allow_lfs_pointers)
    else:
        stage(source, args.destination, contract["files"])
    print(f"Verified {len(contract['files'])} field files from {contract['directory']}")


if __name__ == "__main__":
    main()
