# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Trace summaries respect local clocks, repeated outcomes and incomplete files."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('network_trace', Path(__file__).resolve().parents[1] / 'network-trace.py')
trace = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trace)


class TraceTests(unittest.TestCase):
    def test_pairs_first_outcome_once_and_keeps_byte_stages_separate(self):
        rows = [{'type': 'header', 'schema_version': 1}]
        for stage, kind, at in [('enqueue_attempt', 'shot', 1000000), ('dequeue', 'shot', 2000000),
                                ('publish', 'shot_result', 12000000), ('publish', 'shot_result', 13000000)]:
            rows.append(dict(stage=stage, kind=kind, elapsed_ns=at, shooter=7, shot=1,
                             input=4 if kind == 'shot' else None))
        rows.extend([dict(stage='receive', kind='world_fragment', elapsed_ns=14000000, bytes=1000),
                     dict(stage='submit_native', kind='inputs', elapsed_ns=15000000, bytes=80),
                     {'type': 'end', 'capped': False}])
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'trace.jsonl'
            path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
            result = trace.summarize(path)
        self.assertTrue(result['complete'])
        self.assertEqual(result['shot_outcome']['samples'], 1)
        self.assertEqual(result['shot_outcome']['p95_ms'], 11)
        self.assertEqual(result['local_queue']['p95_ms'], 1)
        self.assertEqual(result['application_bytes'], {'receive.world_fragment': 1000, 'submit_native.inputs': 80})

    def test_missing_required_fields_skip_rows_before_aggregation(self):
        rows = [[], dict(stage='receive', kind='inputs', bytes=50),
                dict(stage='enqueue_attempt', kind='shot', elapsed_ns=1),
                dict(stage='publish', kind='shot_result', elapsed_ns=2),
                dict(stage='publish', kind='shot_rejected', elapsed_ns=3),
                dict(stage='receive', kind='inputs', elapsed_ns=4, bytes=10),
                {'type':'end'}]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'trace.jsonl'
            path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
            result = trace.summarize(path)
        self.assertEqual(result['malformed_lines'], 5)
        self.assertEqual(result['events'], {'receive.inputs':1})
        self.assertEqual(result['application_bytes'], {'receive.inputs':10})
        self.assertFalse(result['complete'])

    def test_partial_tail_is_reported_without_inventing_outcomes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'partial.jsonl'
            path.write_text('{"type":"header","schema_version":1}\n{"elapsed_ns":')
            result = trace.summarize(path)
        self.assertFalse(result['complete'])
        self.assertEqual(result['malformed_lines'], 1)
        self.assertEqual(result['shot_outcome']['samples'], 0)


if __name__ == '__main__':
    unittest.main()
