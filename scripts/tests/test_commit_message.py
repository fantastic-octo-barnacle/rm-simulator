# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Regression coverage for the shared commit and PR convention."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'check-commit-message.py'
SPEC = importlib.util.spec_from_file_location('commit_message', SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class CommitMessages(unittest.TestCase):
    def test_valid_messages(self):
        for message in ('fix: repair startup', 'feat(server)!: change protocol',
                        'docs: explain setup\n\n- Document hooks\n\nRefs: #12',
                        'chore: ' + 'x' * 65):
            with self.subTest(message=message):
                self.assertIsNone(MODULE.validate(message))

    def test_invalid_messages(self):
        for message in ('', 'Update files', 'unknown: change files',
                        'fix(Server): repair startup', 'fix: ', 'fix: repair.',
                        'fix: repair\nMissing separator', 'chore: ' + 'x' * 66):
            with self.subTest(message=message):
                self.assertIsNotNone(MODULE.validate(message))

    def test_range_checks_every_commit(self):
        with tempfile.TemporaryDirectory() as directory:
            def git(*args):
                return subprocess.check_output(['git', *args], cwd=directory,
                                               stderr=subprocess.DEVNULL, text=True).strip()
            git('init')
            git('config', 'user.name', 'Test')
            git('config', 'user.email', 'test@example.com')
            git('-c', 'core.hooksPath=/dev/null', 'commit', '--allow-empty', '-m', 'chore: baseline')
            base = git('rev-parse', 'HEAD')
            git('-c', 'core.hooksPath=/dev/null', 'commit', '--allow-empty', '-m', 'bad message')
            git('-c', 'core.hooksPath=/dev/null', 'commit', '--allow-empty', '-m', 'fix: final step')
            result = subprocess.run(['python3', str(SCRIPT), '--range', base + '..HEAD'],
                                    cwd=directory, capture_output=True, text=True)
            self.assertEqual(result.returncode, 1)
            self.assertIn('use type(scope)', result.stdout)
            result = subprocess.run(['python3', str(SCRIPT), '--range', 'HEAD^..HEAD'],
                                    cwd=directory, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout)
