#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Fresh-process rendering sweeps. Build the binary before starting this script."""
import argparse
import copy
import csv
import hashlib
import itertools
import json
import os
from pathlib import Path
import platform
import random
import subprocess
import sys
import time


def expand(config):
    unknown = config.keys() - {"base", "sweep", "cases", "repeats", "seed"}
    if unknown:
        raise ValueError(f"unknown sweep keys: {sorted(unknown)}")
    base = config.get("base", {})
    axes = config.get("sweep", {})
    if any(not isinstance(v, list) or not v for v in axes.values()):
        raise ValueError("each sweep axis needs a nonempty list")
    repeats = config.get("repeats", 2)
    if not isinstance(repeats, int) or not 1 <= repeats <= 100:
        raise ValueError("repeats must be 1..100")
    cases = []
    for index, values in enumerate(itertools.product(*axes.values())):
        case = copy.deepcopy(base)
        for key, value in zip(axes, values):
            parts = key.split(".")
            node = case
            for part in parts[:-1]:
                node = node.setdefault(part, {})
            node[parts[-1]] = value
        case["name"] = f"{base.get('name', 'sweep')}-{index:03}"
        cases.append(case)
    if not axes and config.get("cases"):
        cases.clear()
    for index, override in enumerate(config.get("cases", [])):
        case = copy.deepcopy(base)
        merge(case, override)
        case.setdefault("name", f"case-{index:03}")
        cases.append(case)
    if len(cases) * repeats > 10000:
        raise ValueError("sweep exceeds 10,000 runs")
    runs = [(i, repeat, case) for repeat in range(repeats) for i, case in enumerate(cases)]
    random.Random(config.get("seed", 2026)).shuffle(runs)
    return runs


def merge(target, source):
    for key, value in source.items():
        if isinstance(value, dict) and isinstance(target.get(key), dict):
            merge(target[key], value)
        else:
            target[key] = copy.deepcopy(value)


def file_hash(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def power():
    if sys.platform != "darwin":
        return None
    return subprocess.check_output(["pmset", "-g", "batt"], text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("config", type=Path)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/rm-simulator-bench"))
    parser.add_argument("--cad-assets", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--backend", choices=["auto", "metal", "vulkan", "dx12", "gl"], default="auto")
    parser.add_argument("--detail", choices=["summary", "passes", "raw"], default="passes")
    parser.add_argument("--cpu-only", action="store_true")
    parser.add_argument("--headless", action="store_true")
    parser.add_argument("--screenshot", action="store_true")
    parser.add_argument("--require-ac", action="store_true")
    parser.add_argument("--cooldown", type=float, default=3.)
    parser.add_argument("--list", action="store_true", help="print run order without running or creating output")
    args = parser.parse_args()
    config = json.loads(args.config.read_text())
    runs = expand(config)
    if args.list:
        print(json.dumps(runs, indent=2))
        return 0
    if not 0 <= args.cooldown <= 3600:
        parser.error("cooldown must be 0..3600 seconds")
    if args.require_ac and sys.platform != "darwin":
        parser.error("--require-ac currently supports macOS only")
    environment = os.environ.copy()
    binary = args.binary.resolve(strict=True)
    assets = args.cad_assets.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=False)
    manifest = {"schema_version": 1, "config": config, "binary": str(binary),
                "binary_sha256": file_hash(binary),
                "platform": platform.platform(), "environment": {k: v for k, v in environment.items()
                    if k in ("WGPU_BACKEND", "VULKAN_SDK", "VK_DRIVER_FILES", "VK_ICD_FILENAMES", "MVK_CONFIG_USE_METAL_ARGUMENT_BUFFERS", "RUST_LOG", "DYLD_LIBRARY_PATH")},
                "command_options": vars(args) | {"config": str(args.config), "binary": str(binary), "cad_assets": str(assets), "output": str(args.output)},
                "runs": []}
    rows = []
    for order, (index, repeat, case) in enumerate(runs):
        before = power()
        if args.require_ac and "AC Power" not in (before or ""):
            raise RuntimeError("AC power is required; stopping sweep")
        name = f"{order:03}-case{index:03}-repeat{repeat:02}"
        case_path = args.output / f"{name}.json"
        case_path.write_text(json.dumps(case, indent=2) + "\n")
        directory = args.output / name
        command = [str(binary), "--cad-assets", str(assets), "--case", str(case_path),
                   "--output", str(directory), "--backend", args.backend, "--detail", args.detail]
        for flag in ("cpu_only", "headless", "screenshot"):
            if getattr(args, flag):
                command.append("--" + flag.replace("_", "-"))
        print(f"[{order+1}/{len(runs)}] {name}: {case}", flush=True)
        started = time.time()
        with (args.output / f"{name}.log").open("w") as log:
            try:
                result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT,
                                        timeout=case.get("timeout_seconds", 120) + 30, check=False, env=environment)
                code = result.returncode
            except subprocess.TimeoutExpired:
                code = 124
        after = power()
        report_path = directory / "report.json"
        report = json.loads(report_path.read_text()) if report_path.exists() else {}
        valid = code == 0 and report.get("status") == "ok" and (not args.require_ac or "AC Power" in (after or ""))
        entry = {"order": order, "case_index": index, "repeat": repeat, "directory": name, "command": command,
                 "exit_code": code, "valid": valid, "started_unix_s": started,
                 "elapsed_s": time.time() - started, "power_before": before, "power_after": after}
        manifest["runs"].append(entry)
        (args.output / "sweep.json").write_text(json.dumps(manifest, indent=2) + "\n")
        row = {"order": order, "case_index": index, "repeat": repeat, "name": case["name"], "valid": valid,
               "backend": (report.get("adapter") or {}).get("backend", ""), "detail": args.detail,
               "cpu_only": args.cpu_only, "geometry": case.get("geometry", "normal"),
               "actual_resolution": "x".join(map(str, report.get("actual_resolution") or [])),
               "error": report.get("error") or "",
               "gpu_dropped": report.get("gpu_readbacks", {}).get("dropped", ""),
               "gpu_invalid": report.get("gpu_readbacks", {}).get("invalid", "")}
        for group in ("cpu_frame", "gpu_render"):
            for key in ("samples", "mean_ms", "median_ms", "p95_ms", "p99_ms", "rate_from_mean_hz", "one_percent_low_hz"):
                row[f"{group}_{key}"] = report.get(group, {}).get(key, "")
        rows.append(row)
        with (args.output / "comparison.csv").open("w", newline="") as stream:
            writer = csv.DictWriter(stream, fieldnames=rows[0].keys())
            writer.writeheader()
            writer.writerows(rows)
        print(f"  {'ok' if valid else 'FAILED'}: {report.get('gpu_render', {})}", flush=True)
        if order + 1 < len(runs):
            time.sleep(args.cooldown)
    return 0 if all(row["valid"] for row in rows) else 1


if __name__ == "__main__":
    raise SystemExit(main())
