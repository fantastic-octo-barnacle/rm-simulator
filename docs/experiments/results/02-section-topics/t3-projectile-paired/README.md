<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# How large is the paired projectile regression?

The small aggregate p95 difference hides sizeable transient differences in both
directions. This selected pair supports a small average disadvantage, not a
uniform centimetre-scale error penalty or a validated gameplay impact.

## Scope and reconstruction

This is seed 102, two robots, the constrained 40 KiB/s downstream link, comparing
unchanged DRR 2:1:1 with fixed completion priority. It was selected after seeing
its +1.10 cm overall p95 regression and worse missing coverage. It is exploratory,
not an independent validation sample. No network experiment was rerun or policy
changed. An ignored test exports deterministic truth positions for 4,376 captures;
that export took 7 seconds. Both original captured-stream hashes match the export.
Saved pose assembly and checkpoint events reconstruct each policy's current view.

The original error p95, maximum, sample counts, all age statistics and missing /
stale counts reproduce exactly. Error means differ by at most 5.65e-10 m from the
original Rust aggregates. The Python reconstruction check uses 1e-8 m numerical
tolerance (initial 1e-10 check was too strict); this is not a policy acceptance
tolerance. The export metadata pins the initial analyzer; `analysis.json` pins
its final revision with this numerical check adjustment. The network inputs and
exported positions did not change.

## Same projectile IDs at the same times

There are **216,357 common projectile/frame observations** over 3,750 sampled
frames (60 s). Comparing only these common observations removes the difference
in the sets of IDs visible under each policy.

| Quantity | Result |
|---|---:|
| Mean position error, baseline → candidate | 3.1621 → 3.1885 m (+2.64 cm) |
| p95 position error on common observations | 6.2254 → 6.2380 m (+1.25 cm) |
| p99 position error on common observations | 7.5516 → 7.5305 m (−2.11 cm) |
| Median paired error change | 0 m |
| Paired observations over 1 m worse | 15.24% |
| Paired observations over 1 m better | 14.72% |
| p95 of the signed paired error change | +1.81 m |
| Largest paired worsening / improvement | +7.15 / −7.24 m |

A difference between two distribution percentiles (+1.25 cm) is not the
percentile of per-observation differences (+1.81 m). The latter exposes changing
arrival phases that the former largely averages away. Counts represent repeated
projectile/frame observations, not unique projectiles, users, or independent
trials. The 1 m and other reported thresholds are descriptive probes only, not
new acceptance gates.

The error is identical for all 83,372 common observations with the same received
capture (38.53%). Where the candidate capture is older (70,667 observations), its
mean error is 1.17 m worse. Where it is fresher (62,318), its mean error is 1.23 m
better. Position error need not increase monotonically with age for every moving
or bouncing projectile, but freshness explains the direction on average. The
centimetre-scale aggregate penalty remains on the common-ID population, so it
is not solely caused by a different visible population.

Coverage still matters separately: of 239,979 truth projectile/frame observations,
19,036 are missing from both policies, 2,544 are visible only under the baseline,
and 2,042 only under the candidate. That is 502 additional missing observations
for the candidate, or +0.209 percentage points. These have no positional error
in the common-ID analysis and must not be treated as zero-error observations.

## Are the larger changes brief?

For each 16 ms sample, average the error change across common projectile IDs.
There are 123 sampled episodes where that mean is over 1 m worse: median duration
48 ms, p95 128 ms, maximum 304 ms, totalling 6.848 s of the 60 s window. Above
0.1 m, there are 224 episodes, median 80 ms and maximum 368 ms. Durations count
contiguous 16 ms sample bins; no sub-frame continuity is inferred. These are
frame-average changes, distinct from the individual-observation fractions above.

## Practical interpretation and stop

The net additional error is small relative to the existing 6.2 m held-position
p95, but transient metre-scale differences are not negligible if these held
positions were rendered directly. This experiment neither renders them nor runs
application prediction, so it cannot determine perceptibility, aiming impact,
collision/scoring correctness, or actual correction magnitudes. It also cannot
show that the candidate creates uniquely bad spikes: comparably large
improvements occur, and its common-ID p99 error is slightly lower.

Keep completion priority experimental. The strict no-regression result still
fails. The next useful stage is to measure this tradeoff through the existing
whole-world prediction/presentation path rather than declare the centimetre
aggregate harmless or add another priority heuristic. Stop here; no additional
seeds, rate changes, production changes, commit or push.

## Reproduce and validate

Build the server feature test binary first. `run.py BINARY` writes a new `truth/`
directory and refuses to overwrite existing exports. `python3 analyze.py` checks
original input hashes and truth chunk hashes, reconstructs both metrics, then
writes `analysis.json` and `frames.jsonl.gz`. The retained truth chunks allow
analysis without another Rust run. Source, binary and capture provenance are in
`truth/metadata.json`; output summary provenance is in `analysis.json`.

Validation: feature test compilation, the ignored truth-export test, aggregate
and coverage reconstruction, clippy with warnings denied, formatting, Python
syntax, crate boundaries and changed-file hooks passed. No new production Rust
code was introduced; the only Rust addition is the ignored export test.
