#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Plot T3: uv run --with matplotlib==3.11.2 python plot.py SCREEN_DIR."""
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
variants=['rr-128','drr-128-211','completion-128-211']
labels=['Original rotation','DRR 2:1:1','DRR + completion preference']
colors=['#7a5195','#00876c','#2878b5']
plt.rcParams.update({'font.size':10,'axes.spines.top':False,'axes.spines.right':False,'svg.hashsalt':'section-topics-t3'})
fig,axes=plt.subplots(1,2,figsize=(12,5.7))
for ax,players in zip(axes,[2,12]):
    for i,(variant,label,color) in enumerate(zip(variants,labels,colors)):
        row=next(r for r in rows if r['players']==players and r['profile']=='limited' and r['variant']==variant)
        values=[row['metrics']['checkpoint_age_ms']['p95'],row['metrics']['available_chassis_age_ms']['p95'],row['controls']['pong_latency_ms']['p95']]
        bars=ax.bar([x+(i-1)*.24 for x in range(3)],values,width=.22,color=color,label=label,zorder=3)
        ax.bar_label(bars,labels=[f'{v:,.0f}' for v in values],padding=4,fontsize=9)
    ax.set_xticks(range(3),['Checkpoint age','Chassis age','Pong delay'])
    ax.set_ylabel('p95 milliseconds (lower is better)')
    ax.set_title(f'{players} robots · 40 KiB/s downstream')
    ax.set_ylim(0,1600 if players==2 else 4300)
    ax.grid(axis='y',alpha=.2,zorder=0)
fig.suptitle('T3: checkpoint completion improves, but strict no-regression check fails',fontsize=14,y=.97)
handles,names=axes[0].get_legend_handles_labels()
fig.legend(handles,names,loc='lower center',ncol=3,frameon=False,bbox_to_anchor=(.5,.085))
fig.text(.5,.025,'Fixed 32/64/128 ms cadence · same Zstd dictionary; DRR weights fixed at 2:1:1 · one reused development seed\nProjectile p95 error increases by 4.3 / 3.9 cm (2 / 12 robots); small coverage regressions remain. No production adoption claim.',ha='center',fontsize=9,color='#555555')
fig.tight_layout(rect=(0,.20,1,.92))
fig.savefig(root/'completion-tradeoffs.png',dpi=160)
fig.savefig(root/'completion-tradeoffs.svg',metadata={'Date':None})
svg=root/'completion-tradeoffs.svg';svg.write_text('\n'.join(line.rstrip() for line in svg.read_text().splitlines()).rstrip()+'\n')
(root/'plot-metadata.json').write_text(json.dumps({'matplotlib':matplotlib.__version__,'script_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'compact_rows_sha256':hashlib.sha256((root/'runs.jsonl.gz').read_bytes()).hexdigest()},indent=2)+'\n')
print('Rendered completion tradeoff plot with matplotlib',matplotlib.__version__)
