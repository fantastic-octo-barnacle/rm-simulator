#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Validate the fixed completion-order ablation and unchanged T2 controls."""
import gzip
import hashlib
from itertools import product
import json
from pathlib import Path
import sys

root = Path(sys.argv[1]).resolve()
meta = json.loads((root/'metadata.json').read_text())
assert meta['complete']
t2 = Path(__file__).resolve().parent.parent/'t2/screen'
old_meta = json.loads((t2/'metadata.json').read_text())
assert old_meta['complete']
rows={}
controls_checked=[]
for name,h in meta['case_sha256'].items():
    raw=(root/name).read_bytes();assert hashlib.sha256(raw).hexdigest()==h
    r=json.loads(gzip.decompress(raw));key=(r['players'],r['profile'],r['variant']);assert key not in rows;rows[key]=r
    assert r['compression']=={'mode':'ZstdDictionary','level':3} and r['prepared_dictionary_by_copy']
    assert r['cadence']=={'chassis_ms':32,'projectiles_ms':64,'checkpoint_ms':128}
    assert r['seed_id']==101 and r['warmup_ms']==10016 and r['measured_ms']==60000 and r['shots_launched_measured']==1250
    assert r['controls']['confirmation_before_pong'] and r['controls']['offered_pairs']==13
    assert r['metrics']['peak_queued_payload_bytes']<=2<<20 and r['metrics']['peak_partial_frame_bytes']<=4<<20
    d=r['diagnostics']
    assert sum(c['sent_payload_bytes']+c['fragment_header_bytes'] for c in d['classes'])+d['shared_datagram_header_bytes']==d['downstream_bytes']==r['sender_lifetime']['sent_bytes']
    assert d['downstream_bytes']<=r['downstream_budget_bytes_s']*70.016
    assert r['upstream_lifetime_bytes_datagrams'][0]<=10240*70.016
    for c in d['classes']:
        assert c['offered_payload_bytes']-c['duplicate_suppressed_payload_bytes']-c['replaced_unsent_payload_bytes']==c['fully_sent_payload_bytes']+c['queued_payload_bytes_at_end']
    for e in d['checkpoint_events']:
        assert e['source_capture_ms']<=e['encoded_ms']<=e['first_manifest_ms']<=e['usable_ms']
    if r['variant']!='completion-128-211':
        old_raw=(t2/name).read_bytes();assert hashlib.sha256(old_raw).hexdigest()==old_meta['case_sha256'][name]
        old=json.loads(gzip.decompress(old_raw));current=dict(r)
        for field in ['scope','execution_ordinal']: old.pop(field);current.pop(field)
        assert old==current, f'Control drift: {name}'
        controls_checked.append(name)
    else:
        priority=r['completion_priority']
        assert priority['priority_bytes']>0
        assert priority['priority_bytes']+priority['rotation_bytes']<=d['downstream_bytes']
        assert priority['burst_cap_bytes']==4092 and priority['focus_lifetime_ms']==1000
    del d['transfer_events'], d['checkpoint_events']
blocks=list(product([2,12],['clean','rtt','limited']))
variants=['rr-128','drr-128-211','completion-128-211']
assert set(rows)=={(p,n,v) for (p,n),v in product(blocks,variants)} and len(rows)==18
for p,n in blocks:
    assert 'test result: ok.' in (root/f'{p}-{n}-output.txt').read_text()
    assert sorted(rows[p,n,v]['execution_ordinal'] for v in variants)==[0,1,2]
    for field in ['captured_stream_sha256','impairment_sha256']:
        assert len({rows[p,n,v][field] for v in variants})==1


def metrics(r):
    m,e,c,d=r['metrics'],r['errors'],r['controls'],r['coverage_denominators']
    return {'checkpoint_age_p95_ms':m['checkpoint_age_ms']['p95'],'chassis_age_p95_ms':m['available_chassis_age_ms']['p95'],
        'projectile_age_p95_ms':e['projectile_age_ms']['p95'],'body_error_p95_m':e['body_m']['p95'],'aim_error_p95_rad':e['aim_rad']['p95'],
        'projectile_error_p95_m':e['projectile_m']['p95'],'missing_robot_fraction':e['missing_robot_samples']/d['truth_robot_samples'],
        'missing_projectile_fraction':e['missing_projectile_samples']/d['truth_projectile_samples'],
        'stale_projectile_fraction':e['stale_projectile_samples']/d['section_shown_projectile_samples'],
        'pong_p95_ms':c['pong_latency_ms']['p95'],'downstream_bytes':m['downstream_bytes'],'upstream_bytes':m['upstream_bytes']}

paired=[];regressions=[];progress=[]
for p,n in blocks:
    a=rows[p,n,'completion-128-211'];x=metrics(a)
    for v in ['rr-128','drr-128-211']:
        y=metrics(rows[p,n,v]);paired.append({'players':p,'profile':n,'control':v,'candidate_minus_control':{k:x[k]-y[k] for k in x}})
    base=metrics(rows[p,n,'drr-128-211'])
    regressions.extend({'players':p,'profile':n,'metric':k,'delta':x[k]-base[k]} for k in meta['plan']['guarded_metrics'] if x[k]>base[k])
    if a['metrics']['delivered_checkpoints']==0 or a['metrics']['samples_without_checkpoint'] or a['controls']['pending_pongs_at_end'] or a['controls']['pending_confirmations_at_end']:
        progress.append({'players':p,'profile':n})
improves=all(metrics(rows[p,'limited','completion-128-211'])['checkpoint_age_p95_ms']<metrics(rows[p,'limited','drr-128-211'])['checkpoint_age_p95_ms'] for p in [2,12])
summary={'scope':'One ablation on reused development seed; no independent validation', 'baseline_exact_matches':controls_checked,
 'primary_hypothesis_supported':improves and not regressions and not progress,'checkpoint_improves_both_limited_counts':improves,
 'guarded_regressions':regressions,'progress_or_control_failures':progress,'paired_differences':paired}
(root/'analysis.json').write_text(json.dumps(summary,indent=2)+'\n')
(root/'runs.jsonl.gz').write_bytes(gzip.compress(''.join(json.dumps(rows[k],sort_keys=True)+'\n' for k in sorted(rows)).encode(),mtime=0))
lines=['# T3 completion-order results','','One fixed ablation on the reused T2 development seed. No production adoption or independent validation claim.','',
 '| Robots | Link | Policy | p95 checkpoint ms | p95 chassis ms | p95 Pong ms | p95 projectile error m | Missing projectiles | Downstream bytes |',
 '|---|---|---|---:|---:|---:|---:|---:|---:|']
for p,n in blocks:
    for v in variants:
        x=metrics(rows[p,n,v]);lines.append(f"| {p} | {n} | {v} | {x['checkpoint_age_p95_ms']:.0f} | {x['chassis_age_p95_ms']:.0f} | {x['pong_p95_ms']:.0f} | {x['projectile_error_p95_m']:.3f} | {100*x['missing_projectile_fraction']:.2f}% | {x['downstream_bytes']} |")
(root/'summary.md').write_text('\n'.join(lines)+'\n')
print('Validated 18 cases and 12 exact T2 control replays; hypothesis supported:',summary['primary_hypothesis_supported'])
print('Checkpoint improvement in both limited cells:',improves,'guarded regressions:',regressions,'progress failures:',progress)
print('Wall seconds',meta['wall_seconds'])
