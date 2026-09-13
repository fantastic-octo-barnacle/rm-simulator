#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Print the main-branch ruleset, or explicitly apply it after CI is published.

Requires PRs and the stable GitHub Actions check, prevents force pushes and
branch deletion, and requires resolved review threads and linear history.
Zero mandatory approvals permits a solo maintainer to merge their own PRs.
"""
import argparse
import json
import subprocess

MERGE_SETTINGS = {
    "allow_squash_merge": True,
    "allow_rebase_merge": True,
    "allow_merge_commit": False,
    "squash_merge_commit_title": "PR_TITLE",
    "squash_merge_commit_message": "PR_BODY",
}

RULESET = {
    "name": "main PR workflow",
    "target": "branch",
    "enforcement": "active",
    "bypass_actors": [],
    "conditions": {"ref_name": {"include": ["refs/heads/main"], "exclude": []}},
    "rules": [
        {"type": "deletion"},
        {"type": "non_fast_forward"},
        {"type": "required_linear_history"},
        {"type": "pull_request", "parameters": {
            "required_approving_review_count": 0,
            "dismiss_stale_reviews_on_push": True,
            "require_code_owner_review": False,
            "require_last_push_approval": False,
            "required_review_thread_resolution": True,
        }},
        {"type": "required_status_checks", "parameters": {
            "required_status_checks": [{"context": "CI required", "integration_id": 15368}],
            "strict_required_status_checks_policy": True,
            "do_not_enforce_on_create": False,
        }},
    ],
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="create or update the ruleset on GitHub")
    args = parser.parse_args()
    payload = json.dumps(RULESET, indent=2) + "\n"
    if not args.apply:
        print(json.dumps({"merge_settings": MERGE_SETTINGS, "ruleset": RULESET}, indent=2))
        return
    repository = subprocess.check_output(
        ["gh", "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"], text=True).strip()
    subprocess.run(["gh", "api", "--method", "PATCH", f"repos/{repository}",
                    "--input", "-"], input=json.dumps(MERGE_SETTINGS),
                   text=True, check=True, stdout=subprocess.DEVNULL)
    endpoint = f"repos/{repository}/rulesets"
    pages = json.loads(subprocess.check_output(
        ["gh", "api", "--paginate", "--slurp", endpoint], text=True))
    matches = [rule for page in pages for rule in page
               if rule["name"] == RULESET["name"] and rule.get("source") == repository]
    if len(matches) > 1:
        raise ValueError("multiple matching rulesets; reconcile them in GitHub settings first")
    if matches:
        endpoint += f"/{matches[0]['id']}"
    subprocess.run(["gh", "api", "--method", "PUT" if matches else "POST",
                    endpoint, "--input", "-"], input=payload, text=True, check=True)


if __name__ == "__main__":
    main()
