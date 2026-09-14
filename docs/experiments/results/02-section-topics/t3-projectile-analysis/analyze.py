#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Inspect existing T3 records; reconstruct held projectile age without new runs."""
import gzip
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
BASE = HERE.parent

def stats(values):
    values = sorted(values)
    return {'samples': len(values), 'mean': sum(values)/len(values), 'p95': values[len(values)*95//100], 'max': values[-1]}

results = []
hashes = {}
series = {}
for seed in [101, 102, 103]:
    root = BASE / ('t3' if seed == 101 else 't3-replication') / 'screen'
    metadata = json.loads((root/'metadata.json').read_text())
    for players in [2, 12]:
        for variant in ['drr-128-211', 'completion-128-211']:
            name = f'{"" if seed == 101 else str(seed)+"-"}{players}-limited-{variant}.json.gz'
            raw = (root/name).read_bytes()
            digest = hashlib.sha256(raw).hexdigest()
            assert digest == metadata['case_sha256'][name]
            hashes[str((root/name).relative_to(BASE))] = digest
            row = json.loads(gzip.decompress(raw))
            diag = row['diagnostics']
            transfers = [t for t in diag['transfer_events'].values() if t['class'] == 21]
            # Fixed fixture steps 16ms each capture, with no source pause/reset.
            # Pose enqueue timestamp is its capture timestamp. Checkpoints carry
            # both capture and simulation time; assert that same clock mapping.
            assert all(e['source_capture_ms'] == e['simulation_time_ns']/1e6 for e in diag['checkpoint_events'])
            events = [(t['assembled_ms'], t['enqueue_ms'], 'pose') for t in transfers if t['assembled_ms'] is not None]
            events += [(e['usable_ms'], e['source_capture_ms'], 'checkpoint') for e in diag['checkpoint_events']]
            events.sort()  # Same-time events lead to the same newest capture.
            pose_latest = 0
            pose_only_ages = []
            latest = 0
            index = 0
            ages = []
            accepted = []
            for ms in range(16, 70017, 16):
                while index < len(events) and events[index][0] <= ms:
                    at, capture, origin = events[index]
                    if origin == 'pose':
                        pose_latest = max(pose_latest, capture)
                    if capture > latest:
                        latest = capture
                        accepted.append((at, capture, origin))
                    index += 1
                if ms > 10016:
                    ages.append(ms-latest)
                    pose_only_ages.append(ms-pose_latest)
            series[seed,players,variant] = ages
            age = stats(ages)
            for k, value in age.items():
                assert abs(value-row['errors']['projectile_age_ms'][k]) < 1e-9, (seed, players, variant, k, value)
            measured = [t for t in transfers if t['assembled_ms'] is not None and t['assembled_ms'] > 10016]
            used = [e for e in accepted if e[0] > 10016]
            pose_used = [e for e in used if e[2] == 'pose']
            c = diag['classes'][21]
            results.append({
                'seed_id': seed, 'players': players, 'variant': variant,
                'error_m': row['errors']['projectile_m'], 'age_ms': age,
                'missing_fraction': row['errors']['missing_projectile_samples']/row['coverage_denominators']['truth_projectile_samples'],
                'stale_fraction': row['errors']['stale_projectile_samples']/row['coverage_denominators']['section_shown_projectile_samples'],
                'measured_assembled_lists': len(measured), 'measured_assembled_hz': len(measured)/60,
                'measured_accepted_pose_lists': len(pose_used),
                'checkpoint_fresher_at_sample_count': sum(a<b for a,b in zip(ages,pose_only_ages)),
                'pose_only_age_ms': stats(pose_only_ages),
                'measured_capture_to_assembly_ms': stats([t['assembled_ms']-t['enqueue_ms'] for t in measured]),
                'measured_accepted_pose_gap_ms': stats([b[0]-a[0] for a,b in zip(pose_used,pose_used[1:])]),
                'lifetime_projectile_class': c,
                'lifetime_group_fragment_bytes': diag['group_fragment_bytes'],
                'lifetime_replaced_list_fraction': sum(t['replaced'] for t in transfers)/len(transfers),
            })
pairs = []
for seed in [101,102,103]:
    for players in [2,12]:
        a = series[seed,players,'drr-128-211']
        b = series[seed,players,'completion-128-211']
        delta = [y-x for x,y in zip(a,b)]
        pairs.append({'seed_id': seed, 'players': players, 'age_delta_ms': stats(delta),
                      'candidate_older_samples': sum(d>0 for d in delta),
                      'candidate_fresher_samples': sum(d<0 for d in delta),
                      'same_age_samples': sum(d==0 for d in delta)})
output = {'paired_age_samples': pairs, 'scope': 'Post-hoc descriptive analysis of existing records; no policy changes or new simulation runs',
          'script_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
          'input_sha256': hashes, 'age_reconstruction_exact_matches': len(results), 'rows': results}
(HERE/'analysis.json').write_text(json.dumps(output, indent=2)+'\n')
for r in results:
    print(r['seed_id'],r['players'],r['variant'], 'Hz',round(r['measured_assembled_hz'],2), 'age',round(r['age_ms']['mean'],2),'checkpoint-fresher samples',r['checkpoint_fresher_at_sample_count'],'groups',r['lifetime_group_fragment_bytes'])
