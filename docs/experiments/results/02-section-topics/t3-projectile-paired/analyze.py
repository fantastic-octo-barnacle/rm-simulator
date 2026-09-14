#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Common-ID/common-time analysis; descriptive thresholds are not acceptance gates."""
import gzip
import hashlib
import json
import math
from pathlib import Path

HERE=Path(__file__).resolve().parent
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
meta=json.loads((HERE/'truth/metadata.json').read_text())
assert meta['complete']
truth={}
for chunk in meta['chunks']:
    p=HERE/'truth'/chunk['name'];assert sha(p)==chunk['sha256']
    for line in gzip.decompress(p.read_bytes()).decode().splitlines():
        capture,ns,positions=json.loads(line)
        assert ns==capture*16000000
        truth[capture]={i:xyz for i,xyz in positions}
assert len(truth)==4376

def stats(values):
    if not values:return None
    s=sorted(values)
    return {'samples':len(s),'mean':sum(s)/len(s),'p50':s[len(s)//2],'p95':s[len(s)*95//100],'p99':s[len(s)*99//100],'min':s[0],'max':s[-1]}

policies=[];input_hashes={};reconstruction=[]
for variant in ['drr-128-211','completion-128-211']:
    root=HERE.parent/'t3-replication/screen';p=root/f'102-2-limited-{variant}.json.gz'
    old_meta=json.loads((root/'metadata.json').read_text());assert sha(p)==old_meta['case_sha256'][p.name]
    input_hashes[p.name]=sha(p)
    r=json.loads(gzip.decompress(p.read_bytes()));assert r['captured_stream_sha256']==meta['captured_stream_sha256']
    d=r['diagnostics']
    events=[(t['assembled_ms'],t['enqueue_ms']//16) for t in d['transfer_events'].values() if t['class']==21 and t['assembled_ms'] is not None]
    events += [(e['usable_ms'],e['checkpoint']) for e in d['checkpoint_events']]
    events.sort();index=0;latest=0;frames={};errors=[];ages=[];missing=stale=0
    for capture in range(1,4377):
        ms=capture*16
        while index<len(events) and events[index][0]<=ms:
            latest=max(latest,events[index][1]);index+=1
        if capture<=626:continue
        shown=truth[latest];current=truth[capture]
        samples={i:math.sqrt(sum((x-y)**2 for x,y in zip(pos,shown[i]))) for i,pos in current.items() if i in shown}
        errors.extend(samples.values());ages.append((capture-latest)*16)
        missing+=len(current.keys()-shown.keys());stale+=len(shown.keys()-current.keys())
        frames[capture]={'capture':latest,'errors':samples}
    for values,key in [(errors,'projectile_m'),(ages,'projectile_age_ms')]:
        actual=stats(values)
        differences={k:abs(actual[k]-v) for k,v in r['errors'][key].items()}
        reconstruction.append({'variant':variant,'metric':key,'absolute_differences':differences})
        assert max(differences.values())<1e-8,(variant,key,differences)
    assert missing==r['errors']['missing_projectile_samples'] and stale==r['errors']['stale_projectile_samples']
    policies.append(frames)

deltas=[];base_common=[];candidate_common=[];frame_rows=[];worst=[]
coverage={'both_missing':0,'only_baseline_visible':0,'only_candidate_visible':0,'both_visible':0}
strata={k:[] for k in ['same_capture','candidate_older','candidate_fresher']}
for capture in range(627,4377):
    a,b=[p[capture] for p in policies];ae,be=a['errors'],b['errors'];common=ae.keys()&be.keys()
    coverage['both_visible']+=len(common)
    coverage['only_baseline_visible']+=len(ae.keys()-be.keys())
    coverage['only_candidate_visible']+=len(be.keys()-ae.keys())
    coverage['both_missing']+=len(truth[capture].keys()-(ae.keys()|be.keys()))
    bucket='same_capture' if a['capture']==b['capture'] else 'candidate_older' if b['capture']<a['capture'] else 'candidate_fresher'
    ds=[]
    for i in sorted(common):
        delta=be[i]-ae[i];deltas.append(delta);ds.append(delta);base_common.append(ae[i]);candidate_common.append(be[i]);strata[bucket].append(delta)
        worst.append((delta,capture*16,i,ae[i],be[i],a['capture'],b['capture']))
    frame_rows.append({'time_ms':capture*16,'common_projectiles':len(common),'age_delta_ms':(a['capture']-b['capture'])*16,'error_delta_m':stats(ds)})
assert all(d==0 for d in strata['same_capture'])
thresholds={str(t):{'worse_fraction':sum(d>t for d in deltas)/len(deltas),'better_fraction':sum(d< -t for d in deltas)/len(deltas)} for t in [0,0.01,0.1,0.5,1,2]}
# Contiguous sampled-frame runs with mean common-ID error increase above 0.1 / 1m.
# Durations count 16ms sample bins; no sub-frame continuity is inferred.
runs={}
for threshold in [0.1,1]:
    lengths=[];current=0
    for f in frame_rows:
        if f['error_delta_m'] and f['error_delta_m']['mean']>threshold:current+=1
        elif current:lengths.append(current*16);current=0
    if current:lengths.append(current*16)
    runs[str(threshold)]={'episodes':len(lengths),'duration_ms':stats(lengths),'total_sampled_ms':sum(lengths)}
out={'scope':'Post-hoc one selected development pair; no inference from independent frames and no acceptance tolerance',
     'input_sha256':input_hashes,'truth_source_sha256':meta['captured_stream_sha256'],'script_sha256':sha(Path(__file__)),
     'reconstruction_checks':reconstruction,
     'numerical_reconstruction_tolerance':1e-8,
     'replay_checks':'Original aggregates reproduced within numerical tolerance; coverage counts exact. Tolerance is only reconstruction validation, not policy acceptance.',
     'common_baseline_error_m':stats(base_common),'common_candidate_error_m':stats(candidate_common),
     'paired_error_delta_m':stats(deltas),'paired_absolute_error_change_m':stats([abs(d) for d in deltas]),
     'coverage':coverage,'descriptive_thresholds_m':thresholds,
     'age_strata':{k:stats(v) for k,v in strata.items()},'frame_mean_regression_episodes':runs,
     'largest_regressions_columns':['delta_m','time_ms','projectile_id','baseline_error_m','candidate_error_m','baseline_capture','candidate_capture'],
     'largest_regressions':sorted(worst,reverse=True)[:20]}
(HERE/'analysis.json').write_text(json.dumps(out,indent=2)+'\n')
(HERE/'frames.jsonl.gz').write_bytes(gzip.compress(''.join(json.dumps(f)+'\n' for f in frame_rows).encode(),mtime=0))
print(json.dumps({k:v for k,v in out.items() if k not in ['largest_regressions','input_sha256']},indent=2))
