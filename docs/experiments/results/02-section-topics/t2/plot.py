#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Plot the frozen T2 screen: uv run --with matplotlib==3.11.2 python plot.py SCREEN_DIR."""
import gzip
import hashlib
import json
from pathlib import Path
import sys

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

root = Path(sys.argv[1]).resolve()
rows = [json.loads(line) for line in gzip.decompress((root / "runs.jsonl.gz").read_bytes()).decode().splitlines()]
analysis = json.loads((root / "analysis.json").read_text())
focus = ["whole", *analysis["nominations"]]
colors = {"whole":"#222222", "rr-128":"#7a5195", "drr-128-211":"#00876c", "drr-64-111":"#d95f02"}
labels = {"whole":"Whole snapshot", "rr-128":"Original rotation · 128 ms", "drr-128-211":"DRR 2:1:1 · 128 ms", "drr-64-111":"DRR 1:1:1 · 64 ms"}
plt.rcParams.update({"font.size":10, "axes.spines.top":False, "axes.spines.right":False, "svg.hashsalt":"section-topics-t2"})
fig, axes = plt.subplots(1,2,figsize=(12.5,5.8))
for ax, players in zip(axes,[2,12]):
    selected = {r["variant"]:r for r in rows if r["players"]==players and r["profile"]=="limited"}
    for v,r in selected.items():
        if v in focus: continue
        eligible = r["metrics"]["delivered_checkpoints"] > 0
        ax.scatter(r["metrics"]["checkpoint_age_ms"]["p95"],r["controls"]["pong_latency_ms"]["p95"],color="#aaaaaa",marker="o" if eligible else "x",s=30 if eligible else 65,alpha=.8,zorder=2)
    for v in focus:
        r=selected[v]; x=r["metrics"]["checkpoint_age_ms"]["p95"];y=r["controls"]["pong_latency_ms"]["p95"]
        ax.scatter(x,y,color=colors[v],s=90,marker="*" if v=="whole" else "o",label=labels[v],zorder=3)
        offset = (9, 8) if v in ["whole","drr-64-111"] else (9,-16)
        if players == 2:
            offset = {"whole": (9,8), "rr-128": (-35,40), "drr-128-211": (15,-38), "drr-64-111": (16,12)}[v]
        ax.annotate(f"{x:,.0f} / {y:,.0f} ms",(x,y),xytext=offset,textcoords="offset points",fontsize=9,color=colors[v],arrowprops={"arrowstyle":"-","color":colors[v],"alpha":.5} if players==2 and v!="whole" else None)
    ax.set_xscale("log")
    ax.set_xlabel("p95 complete-checkpoint age (ms, log scale)")
    ax.set_ylabel("p95 Pong delay (ms)")
    ax.set_title(f"{players} robots · 40 KiB/s downstream")
    ax.grid(alpha=.18,which="both")
    ax.set_ylim(0,4200)
    ax.set_xlim((280,2400) if players==2 else (700,110000))
axes[1].annotate("× No new checkpoints during\nthe 60 s measured window",(66000,2820),xytext=(6000,3900),arrowprops={"arrowstyle":"->","color":"#777777"},color="#555555",fontsize=9)
fig.suptitle("T2: faster controls trade against complete-checkpoint freshness",fontsize=15,y=.97)
handles,legend_labels=axes[0].get_legend_handles_labels()
fig.legend(handles,legend_labels,loc="lower center",ncol=2,frameon=False,bbox_to_anchor=(.5,.075))
fig.text(.5,.025,"One seed · dictionary Zstd 3 · 60 s simulated measurement · lower-left is better\nGrey points: other tested settings. Highlighted candidates follow the frozen nomination rules; none passes the adoption gate.",ha="center",fontsize=9,color="#555555")
fig.tight_layout(rect=(0,.20,1,.92))
fig.savefig(root/"tradeoffs.png",dpi=160)
fig.savefig(root/"tradeoffs.svg",metadata={"Date":None})
svg = root / "tradeoffs.svg"
svg.write_text("\n".join(line.rstrip() for line in svg.read_text().splitlines()).rstrip() + "\n")
(root/"plot-metadata.json").write_text(json.dumps({"matplotlib":matplotlib.__version__,"script_sha256":hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),"compact_rows_sha256":hashlib.sha256((root/'runs.jsonl.gz').read_bytes()).hexdigest()},indent=2)+"\n")
print("Rendered tradeoffs.png and tradeoffs.svg with matplotlib",matplotlib.__version__)
