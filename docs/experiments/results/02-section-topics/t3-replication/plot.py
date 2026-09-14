#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Plot replication with uv run --with matplotlib==3.11.2 python plot.py SCREEN_DIR."""
import gzip
import hashlib
import json
from pathlib import Path
import sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

root=Path(sys.argv[1]).resolve()
rows=[json.loads(line) for line in gzip.decompress((root/'runs.jsonl.gz').read_bytes()).decode().splitlines()]
blocks=[(102,2),(102,12),(103,2),(103,12)]
paired=[]
for seed,players in blocks:
    pair=[next(r for r in rows if r['seed_id']==seed and r['players']==players and r['profile']=='limited' and r['variant']==v) for v in ['drr-128-211','completion-128-211']]
    paired.append(pair)
plt.rcParams.update({'font.size':10,'axes.spines.top':False,'axes.spines.right':False,'svg.hashsalt':'t3-replication'})
fig,axes=plt.subplots(1,2,figsize=(12,5.4))
for i,(label,color) in enumerate([('Unchanged DRR 2:1:1','#00876c'),('Completion priority','#2878b5')]):
    values=[pair[i]['metrics']['checkpoint_age_ms']['p95'] for pair in paired]
    bars=axes[0].bar([x+(i-.5)*.36 for x in range(4)],values,width=.34,label=label,color=color,zorder=3)
    axes[0].bar_label(bars,padding=3,fontsize=9)
axes[0].set_ylabel('p95 checkpoint age (ms)')
axes[0].set_title('Checkpoint age improves in all four cases')
axes[0].set_ylim(0,3200)
axes[0].legend(frameon=False,fontsize=9)
values=[100*(pair[1]['errors']['projectile_m']['p95']-pair[0]['errors']['projectile_m']['p95']) for pair in paired]
bars=axes[1].bar(range(4),values,color=['#b35432' if v>0 else '#00876c' for v in values],width=.55,zorder=3)
axes[1].bar_label(bars,labels=[f'{v:+.2f}' for v in values],padding=4)
axes[1].axhline(0,color='#444444',linewidth=.8)
axes[1].set_ylim(-14,8)
axes[1].set_ylabel('Change in p95 projectile error (cm; lower is better)')
axes[1].set_title('Projectile error tradeoff varies by seed')
for ax in axes:
    ax.set_xticks(range(4),[f'Seed {s}\n{p} robots' for s,p in blocks])
    ax.grid(axis='y',alpha=.2,zorder=0)
fig.suptitle('T3 replication: consistent checkpoint benefit, strict criterion still fails',fontsize=14)
fig.text(.5,.025,'40 KiB/s downstream · fixed 32/64/128 ms cadence and 2:1:1 weights · two new development seeds\nPaired descriptive results; no pooled-frame inference or holdout validation. Other guarded regressions remain.',ha='center',fontsize=9,color='#555555')
fig.tight_layout(rect=(0,.12,1,.93))
fig.savefig(root/'replication-tradeoffs.png',dpi=160)
fig.savefig(root/'replication-tradeoffs.svg',metadata={'Date':None})
svg=root/'replication-tradeoffs.svg'
svg.write_text('\n'.join(line.rstrip() for line in svg.read_text().splitlines()).rstrip()+'\n')
(root/'plot-metadata.json').write_text(json.dumps({'matplotlib':matplotlib.__version__,'script_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'compact_rows_sha256':hashlib.sha256((root/'runs.jsonl.gz').read_bytes()).hexdigest()},indent=2)+'\n')
