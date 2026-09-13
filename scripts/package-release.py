#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Bundle native standalone binaries, relocate runtimes, smoke-test, and write a ZIP."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
BINARIES = ("rm-simulator", "rm-simulator-server")


def output(*command):
    return subprocess.check_output(command, text=True).strip()


def run(*command):
    subprocess.run(command, check=True)


def mac_libraries(bin_dir, licenses):
    """Copy transitive non-system dylibs and rewrite every edge before signing."""
    queue = list(bin_dir.iterdir())
    copied = {}
    edges = []
    while queue:
        binary = queue.pop()
        for line in output("otool", "-L", str(binary)).splitlines()[1:]:
            name = line.strip().split(" (compatibility", 1)[0]
            if name.startswith(("/usr/lib/", "/System/Library/")):
                continue
            source = Path(name)
            if name.startswith("@loader_path/"):
                source = binary.parent / name.removeprefix("@loader_path/")
            if not source.is_absolute() or not source.is_file():
                raise ValueError(f"unresolved dylib {name} in {binary}")
            destination = bin_dir / source.name
            edges.append((binary, name, "@loader_path/" + source.name))
            resolved = source.resolve()
            if source.name in copied:
                if copied[source.name] != resolved:
                    raise ValueError(f"conflicting dylibs: {source.name}")
                continue
            copied[source.name] = resolved
            shutil.copy2(resolved, destination)
            destination.chmod(0o755)
            # Homebrew keg license files accompany redistributed libraries.
            keg = resolved.parent.parent
            for license_file in keg.glob("*LICENSE*"):
                if license_file.is_file():
                    shutil.copy2(license_file, licenses / (source.name + "-" + license_file.name))
            queue.append(destination)
    for binary, old, new in edges:
        run("install_name_tool", "-change", old, new, str(binary))
    for name in copied:
        run("install_name_tool", "-id", "@loader_path/" + name, str(bin_dir / name))
    for binary in bin_dir.iterdir():
        for line in output("otool", "-L", str(binary)).splitlines()[1:]:
            dependency = line.strip().split(" (compatibility", 1)[0]
            if not dependency.startswith(("/usr/lib/", "/System/Library/", "@loader_path/")):
                raise ValueError(f"nonportable dependency after relocation: {dependency}")
        run("codesign", "--force", "--sign", "-", str(binary))
        run("codesign", "--verify", str(binary))


def linux_libraries(bin_dir, licenses):
    """Bundle linked libraries except glibc and its loader; keep relative lookup."""
    system = re.compile(r"^(ld-linux.*|lib(c|m|dl|pthread|rt|resolv|util)\.so\..*)$")
    copied = set()
    for binary in list(bin_dir.iterdir()):
        for line in output("ldd", str(binary)).splitlines():
            if "not found" in line:
                raise ValueError(line)
            match = re.search(r"=> (/\S+)", line)
            if not match:
                continue
            source = Path(match[1])
            if system.fullmatch(source.name) or source.name in copied:
                continue
            copied.add(source.name)
            shutil.copy2(source.resolve(), bin_dir / source.name)
    for binary in bin_dir.iterdir():
        run("patchelf", "--set-rpath", "$ORIGIN", str(binary))
    # Include distro copyright notices for bundled libraries and static GNS deps.
    for copyright_file in Path("/usr/share/doc").glob("*/copyright"):
        shutil.copy2(copyright_file, licenses / (copyright_file.parent.name + "-copyright"))


def windows_libraries(bin_dir, licenses, build):
    """Copy the release MSVC CRT from Visual Studio's redistributable directory."""
    vswhere = Path(os.environ["ProgramFiles(x86)"]) / "Microsoft Visual Studio/Installer/vswhere.exe"
    install = Path(output(str(vswhere), "-latest", "-products", "*", "-property", "installationPath"))
    candidates = sorted((install / "VC/Redist/MSVC").glob("*/x64/Microsoft.VC*.CRT"))
    if not candidates:
        raise ValueError("MSVC redistributable CRT not found")
    for dll in candidates[-1].glob("*.dll"):
        shutil.copy2(dll, bin_dir / dll.name)
    for copyright_file in build.glob("build/game-networking-sockets-sys-*/out/vcpkg/installed/*/share/*/copyright"):
        shutil.copy2(copyright_file, licenses / (copyright_file.parent.name + "-copyright"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--target", required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-(?:alpha|beta|rc)\.(?:rc)?[1-9][0-9]*)?", args.version):
        raise ValueError("invalid version")
    if not re.fullmatch(r"[a-z0-9_-]+", args.target):
        raise ValueError("invalid target")
    name = f"rm-simulator-v{args.version}-{args.target}"
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    build = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "release"
    system = platform.system()
    suffix = ".exe" if system == "Windows" else ""
    with tempfile.TemporaryDirectory() as temporary:
        stage = Path(temporary) / name
        bin_dir = stage / "bin"
        bin_dir.mkdir(parents=True)
        licenses = stage / "LICENSES/native"
        licenses.mkdir(parents=True)
        for binary in BINARIES:
            shutil.copy2(build / (binary + suffix), bin_dir / (binary + suffix))
        for artifact in ("README.md", "NOTICE.md", "LICENSE-MIT", "LICENSE-APACHE", "LICENSES/MPL-2.0.txt"):
            shutil.copy2(ROOT / artifact, stage / artifact)
        shutil.copytree(dist / "field", stage / "field")
        shutil.copy2(ROOT / "scripts/release-field.json", stage / "field-source.json")
        # Include the exact dependency lockfile used to compile the release.
        shutil.copy2(ROOT / "Cargo.lock", stage / "Cargo.lock")
        if system == "Darwin":
            mac_libraries(bin_dir, licenses)
        elif system == "Linux":
            linux_libraries(bin_dir, licenses)
        elif system == "Windows":
            windows_libraries(bin_dir, licenses, build)
            for launcher in (ROOT / "scripts/windows").iterdir():
                if launcher.suffix in (".cmd", ".ps1"):
                    shutil.copy2(launcher, stage / launcher.name)
        else:
            raise ValueError(f"unsupported platform: {system}")
        (stage / "START-HERE.txt").write_text(
            f"rm-simulator {args.version} ({args.target})\n\n"
            "Extract the entire ZIP. Run bin/rm-simulator (Windows: bin/rm-simulator.exe).\n"
            "Run bin/rm-simulator-server for a headless host.\n"
            "The field/ directory is included. Use --cad-assets PATH to select another map.\n"
            "This is the standalone build; Steam integration is disabled.\n"
            "Linux requires glibc 2.39+ and graphics/display drivers.\n"
            "macOS binaries are ad-hoc signed, not notarized.\n"
        )
        # Clear loader overrides so the test exercises the packaged lookup paths.
        env = {key: value for key, value in os.environ.items()
               if key not in ("LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH")}
        for binary in BINARIES:
            subprocess.run([str(bin_dir / (binary + suffix)), "--help"], cwd=temporary,
                           env=env, check=True, timeout=60, stdout=subprocess.DEVNULL)
        manifest = {"version": args.version, "target": args.target,
                    "commit": os.environ.get("GITHUB_SHA", output("git", "rev-parse", "HEAD")),
                    "files": {path.relative_to(stage).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
                              for path in sorted(stage.rglob("*")) if path.is_file()}}
        (stage / "package-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        archive = dist / (name + ".zip")
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            for path in sorted(stage.rglob("*")):
                if path.is_file():
                    bundle.write(path, path.relative_to(stage.parent))
        checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
        archive.with_suffix(".zip.sha256").write_text(f"{checksum}  {archive.name}\n")
        print(archive)


if __name__ == "__main__":
    main()
