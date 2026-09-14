<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# T3 projectile-error investigation

Post-hoc inspection of existing constrained-link records for seeds 101–103,
both robot counts and both fixed DRR policies. No new simulation runs or policy
changes. Run `python3 docs/experiments/results/02-section-topics/t3-projectile-analysis/analyze.py`
from the worktree to reproduce [the analysis](analysis.json). The output records
script and input hashes and checks each input against its original run metadata.

## What the metric measures

`delivery/trials/stage3.rs::Error::sample` samples every 16 ms after warmup.
For each truth projectile that also exists in the current received pose list,
it measures Euclidean distance between the received position and the current
truth position. Positions are held: no interpolation, extrapolation, velocity
prediction or application prediction runs here. Thus the 6–7 m constrained-link
p95 is a stale-position proxy, not a compression error or measured prediction
correction. The small T3 changes are relative to that large existing lag.

The percentile pools matched projectile/frame observations. It weights frames
with more matching projectiles more heavily, excludes missing projectile IDs,
and repeatedly samples the same projectile. Missing-truth and stale-shown IDs
have their own coverage metrics. Projectile age instead has one observation
per frame, so its p95 and position-error p95 need not move together. The two
policies also have slightly different matched populations: this is not a paired
per-projectile error comparison, and these samples are not independent trials.

## Findings from the recorded timelines

The analyzer reconstructs the newest received projectile capture from independent
pose-list assembly events and complete checkpoint promotions. For this fixture,
source time equals capture time; recorded checkpoint timestamps verify that
mapping. Reconstructed mean, p95, maximum and count exactly match every recorded
projectile-age distribution in all **12 constrained cases**. Controls do not update
this experiment's presentation view. Older checkpoints cannot roll back newer
poses because `presentation::accept` requires an increasing capture identity.

| Seed | Robots | Change in mean pose age | Change in p95 position error | Assembled lists/s: DRR → completion |
|---|---:|---:|---:|---:|
| 101 | 2 | +2.83 ms | +4.27 cm | 5.60 → 5.43 |
| 102 | 2 | +2.37 ms | +1.10 cm | 5.72 → 5.47 |
| 103 | 2 | +3.40 ms | +2.10 cm | 5.55 → 5.47 |
| 101 | 12 | +1.47 ms | +3.93 cm | 4.48 → 4.47 |
| 102 | 12 | +2.66 ms | +4.37 cm | 4.48 → 4.50 |
| 103 | 12 | −3.02 ms | −10.78 cm | 4.40 → 4.45 |

The intended publication rate is 15.625 lists/s (64 ms), but measured delivery is
only 4.4–5.7 lists/s under the constrained budget. About 61–70% of offered lists
are replaced before service over the connection lifetime. Successfully assembled
lists take about 169–195 ms from capture to assembly in the measured window;
mean held-view age is 262–314 ms because it continues growing between arrivals.
Increasing publication frequency alone would not establish more delivery capacity.

At **2 robots**, lifetime projectile payload-plus-fragment-header bytes decline
by 2.5–3.3% with completion priority. The normal-plus-repair checkpoint group uses
more bytes despite substantially less repair traffic. DRR weights are unchanged,
but these are work-conserving shares: changing when checkpoint queues have work
changes idle capacity available to other groups. This is consistent with the
lower projectile delivery rate; it is not evidence that the weight implementation
changed or that checkpoint priority directly preempts the projectile group.

Faster checkpoints help a little: in the candidate, checkpoints provide fresher
projectile captures than the pose-only stream at 109–126 of the 3,750 measured
sample times (versus 2–12 for the control). Removing only that refresh from the
recorded arrival timeline would raise candidate mean age by another 2.2–2.8 ms.
This is a presentation-only counterfactual with the recorded network unchanged,
not a simulation of disabling checkpoint traffic. Checkpoint refresh partially
offsets the independent stream slowdown; it does not cause a rollback regression.

At **12 robots**, lifetime projectile group bytes differ by less than 0.25%, and
no checkpoint ever supplies a fresher projectile capture at a measured sample.
Arrival timing and loss outcomes therefore matter more than a large share change.
Seed 102 delivers slightly more lists but has worse mean age: arrival count alone
does not measure freshness. Seed 103 improves both mean age and p95 position error,
so there is no uniform per-seed error penalty at this count. Reordering packets
changes their encounters with the fixed time-bin loss/delay trace even when the
underlying trace is identical between the policies.

## What this establishes, and what remains open

The age reconstruction and byte counts establish delivery differences without
rerunning the experiment. Their direction is consistent with all six p95 error
changes, but they do not decompose the exact position-error percentile into age,
projectile motion and matched-population effects. Existing artifacts retain
aggregate error distributions, not per-projectile error samples or source poses.
A lower age p95 can coexist with a higher error p95: for example seed 103 at
2 robots has age p95 384 → 368 ms, but mean age 262.20 → 265.60 ms and error p95
6.250 → 6.271 m. No significance or practical-acceptance threshold is inferred.

A focused next diagnostic would retain `(sample time, projectile ID, received
capture, position error)` on one fixed pair, then compare common IDs at common
times and stratify error by age. That would distinguish freshness from population
selection before changing rates or priorities. Application prediction remains a
separate follow-up; an ad-hoc ballistic extrapolator here would not validate the
whole-world prediction path. Keep the original strict no-regression failure.

Validation: all 12 raw input hashes, reconstructed age distributions and the
analysis output's provenance checked; Python syntax and changed-file hooks passed.
No Rust or scheduler code changed, so no new Rust test run was needed.
