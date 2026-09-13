#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Summarize metadata traces without combining clocks from different files."""
import argparse
from collections import Counter, OrderedDict, deque
import json
from pathlib import Path


def summarize(path):
    """Count trace stages and pair local shot requests with their first outcome."""
    counts, byte_counts, work_ns = Counter(), Counter(), Counter()
    pending, launch_ms, queue_ms = OrderedDict(), deque(maxlen=10000), deque(maxlen=10000)
    queued = OrderedDict()
    encoding, max_queue_bytes, max_queue_age_ms = None, [0, 0, 0], [0, 0, 0]
    header, end, last_ns, malformed = None, None, 0, 0
    with Path(path).open() as source:
        for line in source:
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                malformed += 1
                continue
            if row.get('type') == 'header':
                header = row
                continue
            if row.get('type') == 'end':
                end = row
                continue
            stage, kind = row.get('stage'), row.get('kind')
            if not stage or not kind:
                malformed += 1
                continue
            key = f'{stage}.{kind}'
            counts[key] += 1
            if row.get('bytes') is not None:
                byte_counts[key] += row['bytes']
            if row.get('work_ns') is not None:
                work_ns[key] += row['work_ns']
            if row.get('encoding') is not None:
                encoding = row['encoding']
            for name, maximum in (('queue_bytes', max_queue_bytes), ('queue_age_ms', max_queue_age_ms)):
                if row.get(name) is not None:
                    for index, value in enumerate(row[name][:3]):
                        maximum[index] = max(maximum[index], value)
            at = row['elapsed_ns']
            last_ns = max(last_ns, at)
            identity = (row.get('shooter'), row.get('shot'), row.get('input'))
            if stage == 'enqueue_attempt' and kind in ('shot', 'input'):
                queued[identity] = at
                if kind == 'shot':
                    pending.setdefault((row.get('shooter'), row['shot']), at)
            elif stage == 'dequeue' and identity in queued:
                delay = at - queued.pop(identity)
                if delay >= 0:
                    queue_ms.append(delay / 1e6)
            elif stage == 'publish' and kind in ('shot_result', 'shot_rejected'):
                started = pending.pop((row.get('shooter'), row['shot']), None)
                if started is not None and at >= started:
                    launch_ms.append((at - started) / 1e6)
            for history in (pending, queued):
                while len(history) > 4096:
                    history.popitem(last=False)
    def distribution(values):
        ordered = sorted(values)
        if not ordered:
            return {'samples': 0, 'p50_ms': None, 'p95_ms': None}
        return {'samples': len(ordered), 'p50_ms': ordered[(len(ordered)-1)//2],
                'p95_ms': ordered[(len(ordered)-1)*95//100]}
    return {'path': str(path), 'header': header, 'end': end,
            'complete': end is not None and malformed == 0,
            'malformed_lines': malformed, 'observed_duration_s': last_ns / 1e9,
            'events': dict(counts), 'application_bytes': dict(byte_counts),
            'work_ns': dict(work_ns), 'local_queue': distribution(queue_ms),
            'shot_outcome': distribution(launch_ms),
            'latest_encoding': encoding, 'max_queue_bytes': max_queue_bytes,
            'max_queue_age_ms': max_queue_age_ms,
            'limits': 'Timing distributions retain the latest 10000 pairs; pending identities cap at 4096. Missing events are not inferred outcomes. Byte stages overlap; do not sum stages as wire bandwidth.'}


def main():
    """Print one independent summary per input file; keep raw traces outside Git."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('paths', nargs='+', type=Path)
    args = parser.parse_args()
    print(json.dumps([summarize(path) for path in args.paths], indent=2))


if __name__ == '__main__':
    main()
