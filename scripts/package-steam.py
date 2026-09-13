#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Stage a native Steam-enabled executable with its matching runtime for testing.

This is development/runtime staging, not a portable distribution. The field
assets and existing GNS native dependencies are not copied into this package. The locked
steamworks-sys crate supplies the default SDK. An explicit --sdk must match its
API checksum; a newer SDK is not assumed to match older generated bindings.

The license texts and NOTICE.md are copied in beside the executable. A binary
that embeds the MPL-2.0 CSS crates has to carry that notice and the source
offer in NOTICE.md, so the package ships them rather than relying on the
recipient finding the repository.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SYS_VERSION = "0.13.0"

# Obligations of the binary distribution, not documentation. MPL-2.0 section
# 3.2 requires telling recipients how to obtain the covered Source Code Form.
LICENSE_ARTIFACTS = (
    Path("NOTICE.md"),
    Path("LICENSE-MIT"),
    Path("LICENSE-APACHE"),
    Path("LICENSES/MPL-2.0.txt"),
)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate_sdk(sdk, expected):
    api = sdk / "public/steam/steam_api.json"
    if not api.is_file() or digest(api) != expected:
        raise ValueError(
            "SDK API does not match steamworks-sys " + SYS_VERSION
            + ". Omit --sdk to use the locked crate's matching SDK. "
            "The external SDK 1.65 is not compatible with these bindings."
        )


def validate_app_id(app_id, development):
    if app_id is None or not 0 < app_id <= 0xFFFFFFFF:
        raise ValueError("--app-id must be a nonzero 32-bit integer")
    if app_id == 480 and not development:
        raise ValueError("App ID 480 requires --development; supply your own App ID for distribution")


def runtime_relative(target):
    if target in ("aarch64-apple-darwin", "x86_64-apple-darwin"):
        return Path("osx/libsteam_api.dylib")
    if target in ("x86_64-pc-windows-msvc", "x86_64-pc-windows-gnu"):
        return Path("win64/steam_api64.dll")
    if target == "x86_64-unknown-linux-gnu":
        return Path("linux64/libsteam_api.so")
    if target == "aarch64-unknown-linux-gnu":
        return Path("linuxarm64/libsteam_api.so")
    raise ValueError(f"unsupported native Steam target: {target}")


def native_dependencies(executable, target):
    if "darwin" in target:
        output = subprocess.check_output(["otool", "-L", str(executable)], text=True)
        return [line.strip().split(" (compatibility", 1)[0] for line in output.splitlines()[1:]]
    if "linux" in target:
        output = subprocess.check_output(["ldd", str(executable)], text=True)
        return [line.strip() for line in output.splitlines()]
    return ["Inspect native DLL prerequisites with dumpbin /dependents before distribution."]


def command_json(command, env):
    return json.loads(subprocess.check_output(command, cwd=ROOT, env=env, text=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app-id", type=int)
    parser.add_argument("--development", action="store_true", help="allow Spacewar App ID 480")
    parser.add_argument("--sdk", type=Path, help="optional SDK with the exact locked API description")
    parser.add_argument("--output", type=Path, default=ROOT / "dist/steam")
    parser.add_argument("--profile", choices=("dev", "release"), default="release")
    parser.add_argument("--check-sdk-only", action="store_true")
    args = parser.parse_args()
    env = os.environ.copy()
    # Avoid accidentally inheriting a different user's SDK override during
    # reproducible packaging. The explicit --sdk option is validated below.
    env.pop("STEAM_SDK_LOCATION", None)
    expected = (ROOT / "scripts/steam-sdk-api.sha256").read_text().split()[0]
    if args.sdk:
        validate_sdk(args.sdk.resolve(), expected)
    if not args.check_sdk_only:
        validate_app_id(args.app_id, args.development)
        if args.output.exists():
            raise ValueError(f"output already exists: {args.output}; choose a new --output directory")
    rust_info = subprocess.check_output(["rustc", "-vV"], text=True)
    target = next(line.removeprefix("host: ") for line in rust_info.splitlines() if line.startswith("host: "))
    metadata = command_json([
        "cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target,
        "--features", "rm-simulator-app/steam",
    ], env)
    package = next(p for p in metadata["packages"] if p["name"] == "steamworks-sys")
    if package["version"] != SYS_VERSION:
        raise ValueError("update the SDK contract when updating the locked steamworks-sys version")
    bundled_sdk = Path(package["manifest_path"]).parent / "lib/steam"
    validate_sdk(bundled_sdk, expected)
    sdk = args.sdk.resolve() if args.sdk else bundled_sdk
    relative = runtime_relative(target)
    runtime = sdk / "redistributable_bin" / relative
    bundled_runtime = bundled_sdk / "redistributable_bin" / relative
    if digest(runtime) != digest(bundled_runtime):
        raise ValueError("external SDK runtime differs from the locked crate runtime; use the bundled SDK")
    if args.check_sdk_only:
        print(f"SDK matches steamworks-sys {SYS_VERSION}; runtime {runtime}")
        return
    env["STEAM_SDK_LOCATION"] = str(sdk)
    command = ["cargo", "build", "--locked", "-p", "rm-simulator-app", "--features", "steam", "--message-format=json-render-diagnostics"]
    if args.profile == "release":
        command.append("--release")
    result = subprocess.run(command, cwd=ROOT, env=env, text=True, stdout=subprocess.PIPE, check=False)
    artifacts = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
    for item in artifacts:
        if item.get("reason") == "compiler-message" and item["message"].get("rendered"):
            print(item["message"]["rendered"], file=sys.stderr, end="")
    result.check_returncode()
    executable = next(Path(item["executable"]) for item in artifacts
                      if item.get("reason") == "compiler-artifact" and item.get("executable")
                      and item["target"]["name"] == "rm-simulator")
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    # Build and validate everything before creating the final distribution.
    with tempfile.TemporaryDirectory(prefix="steam-stage-", dir=output.parent) as temporary:
        stage = Path(temporary)
        shutil.copy2(executable, stage / executable.name)
        shutil.copy2(runtime, stage / runtime.name)
        if "windows" in target:
            launcher_name = "run.cmd"
            launcher = (f"@echo off\r\nset RM_SIMULATOR_STEAM_APP_ID={args.app_id}\r\n"
                        f'"%~dp0{executable.name}" %*\r\n')
        else:
            launcher_name = "run.sh"
            launcher = ('#!/bin/sh\nset -eu\n'
                        'package_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\n'
                        f'export RM_SIMULATOR_STEAM_APP_ID={args.app_id}\n'
                        f'exec "$package_dir/{executable.name}" "$@"\n')
        (stage / launcher_name).write_text(launcher)
        (stage / launcher_name).chmod(0o755)
        for artifact in LICENSE_ARTIFACTS:
            destination = stage / artifact
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / artifact, destination)
        manifest = {
            "app_id": args.app_id, "development": args.development,
            "target": target, "profile": args.profile,
            "steamworks_sys_version": SYS_VERSION, "sdk_api_sha256": expected,
            "files": {path.relative_to(stage).as_posix(): digest(path)
                      for path in sorted(stage.rglob("*")) if path.is_file()},
            "field_assets": "external; pass --cad-assets PATH when running",
            "native_dependencies": native_dependencies(executable, target),
        }
        (stage / "package-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        shutil.copytree(stage, output)
    print(f"Packaged {output}; launch with {output / launcher_name}")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError, StopIteration) as error:
        print(f"Steam packaging failed: {error}", file=sys.stderr)
        sys.exit(1)
