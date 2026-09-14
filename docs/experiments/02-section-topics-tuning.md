<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Section topics: controlled rate and scheduling experiments

Status: T1 instrumentation implemented; T2–T4 remain experiment designs. T1
results and measurement definitions appear below. This extends
[Stage 3](02-section-topics.md#stage-3-independent-presentation-and-checkpoint-cadences).
Stop after each stage below and report its results before continuing.

## Question and evidence

Can joint rate and scheduling changes reduce bandwidth **without transferring the
cost into checkpoint delay, pose error, missing entities or reliable-message delay**?
Keep the original adoption gate; a useful tradeoff is not automatically a pass.

The completed 72-cell Stage-3 screen varied three periods, but selected a winner
using clean-link bytes alone. Its 32/64/128 ms setting saved about 37% / 41% for
2 / 12 robots while increasing clean p95 checkpoint age by 90 ms. This is evidence
of a tradeoff, not an optimized operating point. The selected long follow-up was
interrupted: `stage3/trial-output.txt` contains 21 of 50 cells and no passing
footer. The summarizer correctly rejects it. Do not pool it as a completed trial
or treat its already inspected seeds as fresh validation data.

Current scheduling rotates over fragments of up to 1000 payload bytes; it has no
configurable weights or deficit accounting. A logical checkpoint competes through
multiple queues while each presentation list has one. Equal visits therefore do
not imply equal service to logical functions. A complete checkpoint also waits
for its last required revision: improving average topic age can miss that bottleneck.

## Model before search

For each stream i, first measure encoded bytes per publication B_i and period T_i.
The diagnostic offered load is sum(B_i / T_i), plus manifests, repair, feedback
and framing. B_i depends on cadence, change rate and baseline acknowledgements;
measure it for each setting instead of assuming it is constant. Compare offered
load with the shared service budget. At saturation, sent bytes alone cannot
identify the better setting: both may consume the entire budget.

Age at receipt is source age plus encoding, waiting, serialization and link delay.
For checkpoints, completion is governed by the last required dependency. This
suggests testing lower queue occupancy and useful completion, not merely faster
publication. Age-of-information research demonstrates that sending immediately
is not always freshness-optimal under its queueing assumptions; that motivates a
hypothesis here, not a theorem about this codec.
[Sun et al., Update or Wait](https://arxiv.org/abs/1601.02284).

A 128 ms periodic checkpoint cannot match a 32 ms control's clean checkpoint-age
distribution merely through priorities when there is effectively no queueing.
If that limit dominates, investigate representation/compression or checkpoint
construction cost in a separate ablation. Do not relax the age gate after seeing
results. Pose prediction may reduce visible error but does not make a checkpoint
newer, and requires a separate replay/render experiment.

## T1: instrumentation and trustworthy controls

Implement instrumentation first, with scheduling and publication behavior fixed.
Retain the production whole-envelope control, the current Stage-3 scheduler and
its 32/64/128 ms reference. Verify identical deterministic output with and without
instrumentation, apart from explicitly excluded measurement fields.

Record per logical function and per physical queue:

- Offered, suppressed, replaced, transmitted, repaired and usefully delivered
  bytes; fragments and datagram overhead. Attribute all bytes exactly once and
  reconcile the per-class totals to direction totals. Mark shared overhead by a
  fixed documented rule. Keep normal checkpoint and repair service distinct.
- Source capture, enqueue, first/last send, assembly and usable receipt times;
  time without service while backlogged, queue bytes and partial-frame lifetime.
  Track complete-checkpoint dependencies and which one completes last.
- Source age p50/p95/p99/max, missing-state duration, checkpoint completion rate,
  body/aim/projectile held-pose errors, missing and stale entity fractions with
  explicit denominators. Report matched-entity errors alongside coverage.
- Reliable confirmation and Pong latency/order under an injected control workload;
  repair completion and peak retained memory. Absence of controls in the old
  workload cannot demonstrate their scheduling safety.

Separate simulation time from injected transport time. During pauses, report
capture/receipt delay as well as simulation-state age. Measure blackout recovery
from link restoration to fresh usable state; whole-run p95 can hide a short outage.

Use a time-indexed network impairment trace shared across candidates so different
packet counts do not silently assign different time periods to bad network states.
Specify the rule for same-time packet loss and independent packet loss explicitly.
Keep the existing packet-index script as a separately labeled sensitivity test.
Store workload, impairment and execution-order seeds independently and hash the
source and impairment traces. Timing CPU or native transport requires a separate
run; deterministic samples are not CPU benchmarks.

T1 exit: exact restore and lifecycle invariants pass, byte totals reconcile, and
measurements diagnose at least a clean, saturated and blackout case. Summarize
which dependencies and queues cause delay. Stop before parameter search.

## T2: controlled screening of rates and priorities

Use byte-based weighted deficit round robin (DRR) as the first configurable
scheduler. Define logical groups: complete checkpoints (including manifest and
repair), chassis poses, projectile poses and ordered reliable controls. Schedule
checkpoint dependencies within their group. Otherwise adding more topic queues
implicitly changes their aggregate share. DRR provides a concrete byte-accounting
reference; RFC 8290 describes a byte-based DRR scheduler, although its CoDel packet
dropping policy is not proposed for these application messages.
[RFC 8290, section 4](https://www.rfc-editor.org/rfc/rfc8290.html#section-4).

Keep every group's weight positive, cap stored deficit, and allow idle groups'
unused capacity to be borrowed. Reliable controls retain FIFO confirmation-before-
Pong order; never replace them. Reserve a fixed positive control weight and record
its achieved delay under load. A positive weight guarantees neither a latency
bound under overload nor delivery during loss: test both explicitly.

Start with this finite, reproducible full factorial, crossing the three periods
with four weight settings so rate/priority interactions can be observed:

| Factor | Candidate values |
|---|---|
| Chassis period | 16, 32 ms |
| Projectile period | 32, 64 ms |
| Complete-checkpoint period | 32, 64, 128 ms |
| Checkpoint : chassis : projectile weights | 1:1:1, 2:1:1, 1:2:1, 1:1:2 |
| Reliable control weight | Fixed at 1, subject to T1 safety checks |

This is 48 configurations, not 48 claims of improvement. Keep MTU, token bucket,
queue limits, compression, baseline retention, repair policy and aligned phase
fixed. Include the existing fragment-rotation scheduler at all 12 cadences to
separate scheduler changes from cadence changes; run the whole-envelope control
once per block. Unit weights under DRR are not assumed equivalent to the old
scheduler. Screening is 61 variants × 2 robot counts × 3 profiles (clean,
100 ms RTT/loss, limited) × 3 fresh seed blocks = 1098 runs. Start with 10.016 s
warm-up and 60 s measurement; freeze any duration revision after T1, before T2.

Use tuning seed IDs 101–103 with distinct workload and impairment seed namespaces.
Randomize execution order within each workload/network block and compare paired
run results. Blocking controls nuisance variation; it does not make correlated
2 ms samples independent repetitions.
[NIST randomized block designs](https://www.itl.nist.gov/div898/handbook/pri/section3/pri332.htm).

Keep a Pareto set: a candidate is dominated if another is no worse on every
prespecified objective and strictly better on at least one. Report bandwidth,
checkpoint age, chassis/projectile age and error, coverage and control delay;
retain per-profile results instead of hiding a bad profile in an average. Reject
correctness failures regardless of performance. Select at most three candidates
for further exploration; include the reference even if it is dominated. Use
paired run-level differences and label uncertainty from three seeds exploratory.
Do not manufacture a single weighted score whose weights were chosen after results.

T2 exit: report the frontier, rate-by-weight interactions and bottleneck changes.
If no candidate approaches the existing gates, report that outcome. Stop.

## T3: targeted refinements and ablations

Choose refinements from T1/T2 evidence, then freeze their ranges before running:

| Observed bottleneck | Separate experimental change |
|---|---|
| Bursts from aligned deadlines | Offset pose publication phases in 16 ms increments within each period; preserve offered rate |
| Last checkpoint dependency waits | Within the checkpoint group, prioritize known missing revisions of the selected checkpoint, with aging to prevent starvation |
| Pending poses repeatedly obsolete | Change-triggered poses with minimum spacing and maximum silence; complete roster/lifecycle updates remain mandatory |
| Too many bytes even without queueing | Compact representation or delta pose lists, tested separately with baseline-loss and numeric-error checks |
| Fixed weights fail across conditions | Bounded age-aware service using receiver-confirmed freshness, compared against fixed DRR |

Do not let a sender use future loss or the receiver's private state. Any receiver
freshness feedback must be delayed through the link and counted in upstream bytes.
Age-aware priority can use normalized overdue age, but first define target ages,
feedback expiry and a maximum service gap. Test permanent loss so an impossible
dependency cannot monopolize service. Avoid indefinite preemption of partly sent
large messages: record useful completions and wasted partial bytes.

Run one refinement at a time against the selected fixed policy before testing
interactions. Fit response surfaces only if residuals support them; saturation,
packet boundaries and replacement create discontinuities. For this small discrete
space, an auditable enumeration is preferable to an opaque optimizer. Bayesian
search becomes useful only if the expanded space makes enumeration impractical,
and still needs the same untouched validation set.

T3 exit: freeze at most three final candidates and all analysis/acceptance rules.
Stop before opening validation results.

## T4: independent validation and decision

Use untouched seed IDs 1001–1010, with distinct workload and impairment namespaces,
for the frozen candidates and both controls. Include all five existing link
profiles, 2 and 12 robots, plus separately reported lifecycle/control stress and
unseen movement/fire patterns. Inspect no holdout result while tuning; if a result
causes retuning, it becomes development data and a new holdout is required.

The independent unit is a workload/network seed block, not a frame, robot, packet
or repeated execution of the same deterministic seed. Report each run, paired
candidate-minus-control differences and 95% bootstrap intervals resampling whole
seed blocks (preserve their profile grouping). Ten blocks give limited tail
precision: intervals describe this workload distribution, not all gameplay.
Report p99 and blackout/lifecycle maxima without claiming rare-event guarantees.
If statistically declaring a winner among multiple finalists, freeze a multiple-
comparison adjustment before validation. Do not repeatedly add seeds until a
preferred candidate passes; an inconclusive fixed-size result stays inconclusive.

The original 50% downstream saving and no p95 age/correction regression gate remains
unchanged. Zero observed correctness failures is required but is not a proof of
zero failure probability. Headless pose error cannot establish prediction replay
or perceptual quality. A promising offline candidate must subsequently pass real
app replay, rendered lifecycle/HP reconciliation, native transport/input traffic
and aggregate multi-peer trials before production adoption.

Publish configuration IDs, source/build hashes, complete raw records, failure
records, analysis code and a reproducible manifest of the frozen search and seed
partitions. Distinguish improvements over Stage 3 from meeting the production gate.
The decision may be adopt, reject or revise; stopping with no feasible candidate
is a valid scientific result.

## T1 implementation and measurement contract

Instrumentation is compiled only under `cfg(test)` in the existing experimental
codec. It observes offers, duplicate suppression, unsent replacement, individual
fragment service, reassembly/discards, decoded revisions and checkpoint promotion.
It does not alter rates, priority, frame contents, queue limits or feedback.
The diagnostic fixture repeats every full case with observation disabled and
enabled and asserts exact equality of all ordinary results, including hashes of
packet bytes/reliability/send time and decoded messages/receipt time. Detailed
observer records are the only excluded field.

The fixed reference is chassis/projectiles/checkpoints 32/64/128 ms. T1 runs six
cells: 2/12 robots × clean/limited/blackout, development workload seed 71, transport
seed `71 ^ 0x7000`, 10.016 s warm-up and 60 s measurement. Each measured workload
launches 1250 shots. Both codecs consume the same hashed source per robot count.
This is a diagnostic set, not repeated independent evidence for an optimum.

The new link precomputes separate upstream/downstream 2 ms bins. Clean has no
impairment; limited has 40–60 ms one-way delay and approximately 1% dropped bins;
blackout has 20 ms one-way delay and a 500 ms outage starting 20 s after warm-up.
Every unreliable packet sent within a dropped bin is lost. There is deliberately
no independent per-packet loss in these T1 cases. This is correlated time-bin
loss, not a relabeling of Stage 3's packet-index loss; results cannot be substituted
for the older matrix. Reliable packets use an ordered abstract recovery delay of
100 ms (after blackout end for outage traffic). Carrier retransmission bytes and
native congestion behavior are not simulated. Rates remain 512 KiB/s downstream,
or 40 KiB/s in limited, and 10 KiB/s upstream. Shared bin hashes are verified across
robot counts and both codec legs; packet counts do not advance the impairment RNG.

Confirmation snapshots followed by Pongs are injected 1008 ms after warm-up and
every 4992 ms thereafter, plus one 16 ms into the blackout interval (the same
schedule is used in all profiles). These captures are off the production 32 ms
periodic boundary, so confirmations are identified without confusing periodic
frames. Both paths retain complete independent confirmations: production uses its
existing compact player representation, the experimental path uses its existing
full JSON representation. Assertions compare each against its exact independently
decoded reference and require confirmation assembly before Pong. Production
intentionally suppresses stale snapshots after decoding, including confirmations
when a newer periodic snapshot already arrived. A passive reliable-frame mirror
therefore measures assembly before this filter; application confirmation counts
are recorded separately. The mirror uses the actual RMG1 framing and player
decoder, emits no feedback and cannot change traffic. This representation
asymmetry is part of the current controls, not evidence about scheduling alone.
No player input or actual host-worker round trip is claimed.

Interpret the records as follows:

- Main byte, age, held-error and coverage results cover the 60 s measurement
  window. Queue/transfer/dependency diagnostics and upstream lifetime totals cover
  the entire connection, including warm-up. They are labeled separately. There
  is no end drain: incomplete transfers and pending controls are explicitly
  counted instead of disappearing from successful-delivery latency distributions.
- Every transmitted fragment's payload and 19-byte header belongs to exactly one
  physical queue and logical group. The shared four-byte datagram header is a
  separate bucket. These sum exactly to sender application bytes. Queue ledgers
  also reconcile offered minus duplicate-suppressed and replaced bytes to fully
  sent plus still queued payload. Exact-revision suppression before encoding is
  counted by the existing sender counter; hypothetical encoded bytes for those
  unmaterialized frames are not invented.
- Fresh-pose bytes are assembled pose payloads that advance the presentation
  capture. Innovative-section bytes are assembled frame payloads that introduce
  a previously unseen epoch/topic/revision into the decoder. They are not claimed
  to be bytes used by prediction, and assembly alone is not useful presentation.
  Complete usable checkpoints are counted independently. Raw transfer timestamps
  support further analysis of losses, partial sends and censoring.
- Queue delay is enqueue-to-first-send, with enqueue-to-last-send reported too.
  Capture-to-encode delay exposes checkpoint backpressure before bytes enter a
  queue. Receiver assembly time and enqueue-to-assembly expose fragmentation/link
  effects. The checkpoint record includes source capture, simulation time,
  encoding, first manifest receipt and usable receipt. The completion-trigger
  class is the frame that unlocked the checkpoint, which may be a manifest or a
  reliable baseline, not necessarily the topic with the greatest mean age.
- Missing-dependency time sums waiting across retained checkpoint manifests.
  It is workload exposure, not a count of independent samples. Service-gap maxima
  include continuing backlog, sampled on the 2 ms fixture clock. Per-class queue
  peaks observe offers immediately; partial-memory peaks sample after delivery.
- Freshness ages use the injected source-capture clock. Simulation-state age is
  recorded separately against the latest 16 ms source capture, so a paused world
  cannot disguise old transport captures. A pause-specific equivalence test
  verifies that simulation-state age can be zero while capture age remains
  positive. Blackout recovery requires a usable capture generated at or after
  link restoration, separately for checkpoint, chassis and projectiles.
- Missing robots/projectiles divide by authoritative entity samples; stale
  projectiles divide by shown projectile samples. The records retain numerator
  and denominator and matched-entity error distributions, so poorer coverage
  cannot silently improve the error statistic. These are held-pose measurements,
  not prediction replay or rendered quality.

Reproduce from the checkout root after building the feature test binary:

```sh
cargo test --locked -p rm-simulator-server --features section-topics --lib --no-run
# Use the executable path printed by Cargo, which includes a build-specific hash.
python3 docs/experiments/results/02-section-topics/t1/run.py PATH_TO_TEST_BINARY NEW_OUTPUT_DIRECTORY zstd-dict
python3 docs/experiments/results/02-section-topics/t1/summarize.py NEW_OUTPUT_DIRECTORY
```

The output directory must not exist: the runner refuses to overwrite historical
measurements. Select `deflate`, `zstd`, or `zstd-dict` as the final argument; effort
is fixed at DEFLATE 1 / Zstd 3. With no argument, the summarizer validates the
original `t1/` records.

The runner records codec settings, dictionary, binary, Rust/compiler, workspace Rust-source and Cargo hashes,
checks they stay fixed throughout the run, and stores each exact JSON case as a
compressed artifact. The console transcript retains the passing test footer and
artifact names. The summarizer verifies the complete matrix, trace hashes, byte
ledgers, resource bounds, control completion/order and source/impairment pairing
before producing compact `runs.jsonl`. All six off/on repetitions must pass before
metadata marks the run complete. A failed or interrupted run remains incomplete.

The initial long run stopped on an observer assertion that equated application
confirmation delivery with reliable assembly. Investigation found the production
stale-snapshot filter's documented behavior and its existing regression test in
`udp_codec.rs`. The observer was corrected as described above; production code
was not changed. The initial failure transcript and source metadata are retained
under `t1/failures/`; those incomplete records are not included in the final
matrix. This is an instrumentation correction, not a transport fix.

## T1 results and stop decision

These are the original pre-Zstd DEFLATE measurements. They remain immutable
historical records; the post-rebase reruns are reported separately below.

All six cases completed and passed full-run instrumentation-off/on equality.
Each cell below is one seed and one 60 s measurement, with the same source and
exogenous impairment schedule on both codec legs. Values are descriptive;
there are no confidence intervals or claims of generalization from this set.
Negative downstream change means fewer bytes. At the constrained budget both
paths saturate, so changes of less than 0.05% are not meaningful savings.

| Robots | Link | Downstream change | p95 checkpoint age, whole → sections ms | p95 chassis age, whole → sections ms | p95 Pong latency, whole → sections ms |
|---|---|---:|---:|---:|---:|
| 2 | Clean | -35.26% | 30 → 120 | 30 → 30 | 0 → 0 |
| 2 | Limited | -0.05% | 500 → 970 | 500 → 202 | 426 → 2706 |
| 2 | Blackout | -35.66% | 50 → 142 | 50 → 50 | 604 → 604 |
| 12 | Clean | -39.69% | 30 → 120 | 30 → 30 | 0 → 28 |
| 12 | Limited | -0.04% | 13646 → 2126 | 13646 → 502 | 684 → 4770 |
| 12 | Blackout | -40.30% | 50 → 142 | 50 → 50 | 604 → 604 |

Zero latency means delivery in the same injected-clock iteration, not zero CPU
cost. All 13 confirmation/Pong pairs per path per cell assembled in order, with
none pending at the end. Production application filtering suppressed 8 of the
13 assembled confirmations in the two-robot limited case; the observer records
those separately, rather than classifying them as packet loss.

Fresh poses also do not imply uniformly lower error:

| Robots | Link | p95 held projectile error, whole → sections m | Missing projectile samples, whole → sections |
|---|---|---:|---:|
| 2 | Clean | 0.338 → 0.899 | 0.52% → 1.04% |
| 2 | Limited | 7.075 → 10.803 | 9.43% → 15.72% |
| 12 | Clean | 0.341 → 0.905 | 0.52% → 1.04% |
| 12 | Limited | 31.015 → 11.868 | 83.34% → 17.27% |

These errors use matched entities only; the adjacent missing fractions use all
truth-projectile samples. In the limited cases, p95 held body error improves
0.410 → 0.189 m for two robots and 5.141 → 0.457 m for twelve. The very stale
production twelve-robot result is specific to this saturated synthetic workload
and time-bin loss trace, not a claim about typical gameplay.

The lifetime diagnostic records identify three mechanisms:

1. **The last checkpoint dependency depends on workload.** In the clean two-robot
   case, normal projectiles complete 540/548 checkpoints; in the clean twelve-
   robot case, normal chassis completes all 548. Under limited service, normal or
   repaired projectiles complete 169/186 two-robot checkpoints, while normal or
   repaired chassis completes all 75 twelve-robot checkpoints. A permanently
   fixed preference for one topic is therefore a hypothesis to test, not an
   established best policy.
2. **Reliable service is a distinct bottleneck.** The ordered control/baseline
   queue's p95 enqueue-to-first-send wait is 1830 ms for two robots and 2936 ms
   for twelve under congestion. Individual maximum backlogged service gaps are
   only 198/220 ms: being served occasionally does not prevent a long FIFO wait.
   Byte-weighted service and an explicit logical control share should be tested
   in T2. Confirmation representation remains a confound when comparing against
   production, so scheduler ablations must keep the experimental encoding fixed.
3. **Repair consumes substantial capacity.** Physical repair classes account for
   31.5% / 26.7% of lifetime downstream application bytes in the limited 2/12 robot
   cases. Twelve-robot repair-chassis p95 enqueue wait is 908 ms. Checkpoint
   backpressure adds p95 source-capture-to-encode delays of 120/124 ms before
   normal queue delay even begins. Repair service and useful checkpoint completion
   need to be considered together; increasing publication alone adds offered work.

After the blackout ends, both robot counts receive fresh chassis/projectile poses
in 32 ms. Fresh complete checkpoints take 32 ms on production and 96 ms on the
experimental path. Whole-run p95 would obscure this recovery distinction.

**T1 stop decision:** instrumentation is ready for controlled screening. Do not
adopt the current setting: clean savings remain below 50%, clean checkpoints are
90 ms older at p95, and the two-robot constrained case worsens checkpoint age,
projectile error/coverage and control delay. Preserve the T2 crossed rate/weight
experiment and both controls; prioritize testing logical control service and the
checkpoint dependency bottleneck. No T2 configuration has been implemented or run.
The existing scheduler and live networking remain unchanged.

Validation: 225 library tests, one binary test and 34 doctests passed (260 total;
three long experiments ignored by the normal suite), followed by the isolated T1
experiment. Clippy with warnings denied, formatting, crate-boundary checks and
changed-file hooks passed. Full workspace `just verify` was not run; no PR was
opened. Existing exact restore, loss, lifecycle and confirmation-order tests remain
in the passing suite. T1 does not establish native carrier, prediction replay,
rendered lifecycle/HP reconciliation or multi-peer production gates.

Records: [metadata](results/02-section-topics/t1/metadata.json),
[compact rows](results/02-section-topics/t1/runs.jsonl),
[passing transcript](results/02-section-topics/t1/trial-output.txt),
[runner](results/02-section-topics/t1/run.py), and
[analysis](results/02-section-topics/t1/summarize.py). The six `*.json.gz` files in
the same directory retain complete per-transfer and checkpoint events. SHA-256
values in metadata bind each raw case; summaries can be regenerated from them.


## T1 after the Zstd rebase

The experiment now uses the shared compression selection for section values,
normal/repair manifests, pose lists, reliable controls and feedback, as well as
the whole-snapshot control. The diagnostic reliable-frame mirror and example
also decode either format with the existing decompressed byte caps. This changes
compression plumbing only: cadence remains 32/64/128 ms, scheduling remains the
original fragment rotation, and the live default remains DEFLATE.

The reruns use the same development source and impairment seeds as the original
T1, not new holdout observations. Both legs select the same codec and effort:
DEFLATE 1 for the regression rerun, dictionary-assisted Zstd 3 for the Zstd run.
The existing embedded checkpoint dictionary is fixed and hashed; it was not
retrained on these results. This is a conditional comparison using that dictionary,
not evidence of generalization to unseen workloads or a codec/level optimization.

An initial Zstd run failed byte parity in the first clean case: decoded-message
hashes and section bytes matched, but whole-snapshot bytes differed between
replays. Starting each replay with fresh thread-local compressor/decompressor
contexts removes state carried over from earlier replays. This reset is compiled
only for tests with `section-topics`; contexts are still reused throughout each
replay, including warmup. The full off/on equality requirement is unchanged.
The failed transcript and source hashes remain in
[`t1-zstd-dict-initial-failure`](results/02-section-topics/t1-zstd-dict-initial-failure/).

All six Zstd cells passed instrumentation-off/on equality. Each value below is
one development seed, 10.016 s warmup and 60 s measurement; percentages compare
Section Topics against the **Zstd whole-snapshot** control in the same cell.

| Robots | Link | Downstream change | p95 checkpoint age, whole → sections (ms) | p95 chassis age, whole → sections (ms) | p95 Pong, whole → sections (ms) |
|---|---|---:|---:|---:|---:|
| 2 | Clean | −31.17% | 30 → 120 | 30 → 30 | 0 → 0 |
| 2 | Limited | −0.02% | 334 → 766 | 334 → 182 | 312 → 1398 |
| 2 | Blackout | −31.52% | 50 → 142 | 50 → 50 | 604 → 604 |
| 12 | Clean | −36.97% | 30 → 120 | 30 → 30 | 0 → 8 |
| 12 | Limited | +0.02% | 928 → 1714 | 928 → 468 | 536 → 3678 |
| 12 | Blackout | −37.60% | 50 → 142 | 50 → 50 | 604 → 604 |

The constrained links still saturate, so the ±0.02% byte differences are not
meaningful savings. Zstd changes the baseline materially: the 12-robot whole
control's p95 checkpoint age is 928 ms, versus 13,646 ms in historical DEFLATE
T1. Section Topics now has worse checkpoint age in that cell, even though chassis
age remains better. Comparing a new candidate only against the old DEFLATE
baseline would give a misleading conclusion.

Held-projectile accuracy must still be read with coverage:

| Robots | Link | p95 matched projectile error, whole → sections (m) | Missing truth projectile samples, whole → sections |
|---|---|---:|---:|
| 2 | Clean | 0.338 → 0.899 | 0.52% → 1.04% |
| 2 | Limited | 5.239 → 8.008 | 7.60% → 11.19% |
| 12 | Clean | 0.341 → 0.905 | 0.52% → 1.04% |
| 12 | Limited | 12.646 → 9.896 | 16.20% → 13.81% |

Checkpoint completion remains workload-dependent: projectiles trigger 539/548
clean completions with 2 robots, while chassis triggers all 548 with 12 robots.
Under congestion, normal/repair projectiles trigger 230/247 completions with
2 robots; normal/repair chassis triggers all 94 with 12. These counts include
warmup. Repair payload and fragment headers consume 23.3% / 27.8% of lifetime
downstream for constrained 2 / 12 robots. Ordered control/baseline p95 queue wait
is still 1156 / 2326 ms. All 13 confirmation/Pong pairs per leg per cell complete
in order. Blackout recovery is unchanged: checkpoint/chassis/projectile fresh
capture recovery takes 32/32/32 ms for whole snapshots and 96/32/32 ms for sections.

T1's instrumentation exit is satisfied; the original adoption gate still fails.
Clean 2-robot savings remain below 50%, checkpoint age increases, and projectile
coverage/error and reliable-control delay still impose tradeoffs. The next stage
remains T2's joint cadence and logical byte-weight search, with both controls
using the same fixed codec. No rates or priorities have been tuned here.

Zstd records: [metadata](results/02-section-topics/t1-zstd-dict/metadata.json),
[compact rows](results/02-section-topics/t1-zstd-dict/runs.jsonl), and
[passing transcript](results/02-section-topics/t1-zstd-dict/trial-output.txt).
The six compressed raw cases retain detailed events as before.

The six rebased DEFLATE raw rows **exactly match** the original T1 rows after
removing only the new codec label, including packet/message hashes and every
instrumentation field. The DEFLATE rerun and Zstd run share identical source,
binary, dictionary, capture and impairment hashes. On clean links Zstd reduces
whole-snapshot bytes by 18.74% / 16.11% for 2 / 12 robots, and Section Topics
bytes by 13.60% / 12.32%, relative to the respective DEFLATE paths. Thus absolute
traffic improves on both paths, while Section Topics' relative clean-link
advantage shrinks. These paired descriptive differences do not establish an
optimal compression choice or independent validation.

Regression records: [metadata](results/02-section-topics/t1-deflate-rebased/metadata.json),
[compact rows](results/02-section-topics/t1-deflate-rebased/runs.jsonl), and
[passing transcript](results/02-section-topics/t1-deflate-rebased/trial-output.txt).
Run `python3 docs/experiments/results/02-section-topics/t1/compare_codecs.py`
to verify exact historical DEFLATE equality and regenerate the codec comparison.

Validation after integration: the server suite passes with DEFLATE and dictionary
Zstd (234 library tests, one binary test and 37 doctests per mode, 272 total per
mode). Both complete T1 matrices pass all six off/on comparisons (24 full
replays across the two codecs). Clippy with warnings denied, formatting,
crate-boundary checks and changed-file hooks pass. Historical records remain
unchanged. T2 has not started; no live codec default, rate or priority changed.
