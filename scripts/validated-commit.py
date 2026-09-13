#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Reuse successful manual validation or release runs for the exact source SHA.

Build caches are never evidence of test success. Missing or unavailable run
history makes the caller run validation again. Only workflow_dispatch runs in
this repository qualify; PR runs cannot supply a release validation receipt.
"""
import json
import os
from pathlib import Path
import subprocess

WORKFLOWS = ("full-check.yml", "release.yml")


def successful_run(runs, repository, sha, current_id):
    """Return an exact-commit trusted workflow success, or None."""
    for run in runs:
        if (run.get("head_sha") == sha
                and run.get("status") == "completed"
                and run.get("conclusion") == "success"
                and run.get("event") == "workflow_dispatch"
                and run.get("repository", {}).get("full_name") == repository
                and run.get("path") in {f".github/workflows/{name}" for name in WORKFLOWS}
                and str(run.get("id")) != str(current_id)):
            return run
    return None


def lookup(repository, sha, current_id):
    """Ask GitHub for authoritative run history; fall back to actual checks."""
    for workflow in WORKFLOWS:
        endpoint = (f"repos/{repository}/actions/workflows/{workflow}/runs"
                    f"?head_sha={sha}&event=workflow_dispatch&status=success&per_page=100")
        try:
            response = subprocess.run(["gh", "api", endpoint], capture_output=True,
                                      text=True, timeout=30)
        except subprocess.TimeoutExpired:
            print(f"History lookup timed out for {workflow}; validation remains required.")
            continue
        if response.returncode:
            print(f"No usable history for {workflow}; validation remains required.")
            continue
        run = successful_run(json.loads(response.stdout)["workflow_runs"], repository, sha, current_id)
        if run:
            return run
    return None


def main():
    run = None
    if os.environ.get("FORCE_VALIDATION", "false") != "true":
        run = lookup(os.environ["GITHUB_REPOSITORY"], os.environ["GITHUB_SHA"],
                     os.environ["GITHUB_RUN_ID"])
    reused = "true" if run else "false"
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"reused={reused}\n")
    message = (f"Reusing successful validation: {run['html_url']}" if run
               else "No reusable validation. Running the complete check suite.")
    print(message)
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(summary).open("a") as output:
            output.write(message + "\n")


if __name__ == "__main__":
    main()
