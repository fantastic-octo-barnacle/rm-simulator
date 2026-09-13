#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Validate a local commit message, PR title, or the commits in a Git range."""
import argparse
import os
from pathlib import Path
import re
import subprocess

SUBJECT = re.compile(
    r"(?:feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)"
    r"(?:\([a-z0-9]+(?:-[a-z0-9]+)*\))?!?: \S(?:.*\S)?"
)


def validate(message):
    """Return a policy error, or None when the message follows the convention."""
    lines = message.splitlines()
    if not lines or not SUBJECT.fullmatch(lines[0]):
        return "use type(scope): summary; scope is optional (see CONTRIBUTING.md)"
    if len(lines[0]) > 72:
        return "subject exceeds 72 characters"
    if lines[0].endswith('.'):
        return "omit the subject's trailing period"
    if len(lines) > 1 and lines[1]:
        return "separate the subject and body with a blank line"
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument('--file', type=Path)
    modes.add_argument('--pr-title', action='store_true')
    modes.add_argument('--range', dest='revision_range')
    args = parser.parse_args()
    if args.file:
        # Match Git's default cleanup of editor comments and blank outer lines.
        message = '\n'.join(line for line in args.file.read_text().splitlines()
                            if not line.startswith('#')).strip('\n')
        messages = [(str(args.file), message)]
    elif args.pr_title:
        title = os.environ['PR_TITLE']
        if '\n' in title or '\r' in title:
            parser.error('PR title must be one line')
        messages = [('PR title', title)]
    else:
        if args.revision_range.startswith('-'):
            parser.error('revision range cannot be an option')
        commits = subprocess.check_output(
            ['git', 'rev-list', args.revision_range, '--'], text=True).splitlines()
        messages = [(sha, subprocess.check_output(
            ['git', 'show', '-s', '--format=%B', sha], text=True)) for sha in commits]
    errors = [(name, validate(message)) for name, message in messages]
    for name, error in errors:
        if error:
            print(f'{name}: {error}')
    return int(any(error for _, error in errors))


if __name__ == '__main__':
    raise SystemExit(main())
