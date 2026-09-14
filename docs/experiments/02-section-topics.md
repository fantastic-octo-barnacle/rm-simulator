<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Experiment 2: section topics and coherent checkpoints

Status: stages 1–3 implemented as opt-in offline prototypes. Follow the [shared protocol](README.md); each stage's scope and
departures from the full trial matrix are recorded below. The next
[controlled tuning protocol](02-section-topics-tuning.md) separates instrumentation,
rate/priority screening, refinements and independent validation. Its
[T1 diagnostics and results](02-section-topics-tuning.md#t1-results-and-stop-decision)
are complete, including a [Zstd rerun](02-section-topics-tuning.md#t1-after-the-zstd-rebase);
the [T2 rate/priority screen](02-section-topics-tuning.md#t2-results-and-stop-decision)
is also complete. The [focused T3 completion ablation](02-section-topics-tuning.md#t3-results-and-stop-decision)
improves checkpoint delivery but fails the strict no-regression check. No candidate
passes the adoption gate; live networking remains unchanged.

## Hypothesis and ablations

Splitting the snapshot envelope reduces avoidable updates and confines loss
recovery, allowing fresher movement with fewer total bytes. Compare:

1. Current whole-envelope acknowledged delta stream at nominal 32 ms publication.
2. Section streams at the same cadence: isolates framing/baseline/recovery cost.
3. Independent section cadences and change-triggered updates.
4. Faster compact motion with slower coherent checkpoint publication.

Hold physics, AOI, projectile behavior and link budgets fixed. Existing deltas
already omit unchanged values, so splitting alone may increase bytes through
headers, acknowledgements and reduced compression across sections.

## Topic policy candidates

| Section | Candidate policy |
|---|---|
| chassis | Motion/aim and own correction at 32 or 16 ms; birth/life/removal explicit |
| projectiles | 32 or 64 ms initially; launch/removal delivery remains explicit |
| restore | Required checkpoint state; update exact dependencies, never assume slow churn |
| referee | Prompt changes plus timestamped clock anchor; periodic repair at 128/256 ms |
| runes/outposts | Motion anchors/parameters and prompt activation/HP/stop changes; periodic repair |
| bases | Revisioned geometry plus prompt HP/shield/protection changes |
| hits | Existing reliable deduplicated events, with bounded checkpoint recovery |

Checkpoint candidates are 32, 64 and 128 ms. These are publication settings,
not physics steps. Account separately for envelope tick/time, counters, input
acknowledgements and server metadata. Start with existing representations;
parameterized mechanism motion is a separate ablation with parity tests.

## Coherence and delivery contract

Capture the complete state once at tick T. A manifest identifies epoch,
checkpoint ID, T and exact section revisions. Reuse a revision only when its
value is still valid at T; time-dependent reconstruction requires a specified,
tested rule. `restore` duplicates hidden rune/outpost/referee state and contains
strike timestamps, so visible and hidden state must agree. Promote a checkpoint
atomically only after all dependencies arrive. Until then retain the last complete
checkpoint. Never restore from independently newest values.

Each topic has its own retained acknowledged baseline, replaceable queue and
service accounting under one connection budget. Aggregate small due topics into
datagrams; topics do not require one packet or native reliable lane each. Preserve
confirmation-before-Pong barriers. Bound pending manifests and baseline memory;
retire only after dependencies are safe, and rate-limit missing-revision recovery.
Lifecycle/HP transactions carry dependencies so independently delivered topics
cannot resurrect a robot or mismatch its visible and hidden state.

Fresh presentation sections may render before checkpoint promotion, with explicit
timestamps/life identities. Loss of a projectile section must not block available
owner presentation, but it may block a complete physics checkpoint. Missing
context bounds prediction rather than authorizing a reduced physics world.

## Measurements and gates

Measure bytes including acknowledgements, recovery and transport overhead;
per-topic encoded size/churn, pinned memory, fragments, useful delivered cadence,
owner/target age and checkpoint assembly latency. Lose/reorder each section in
turn; test two consecutive missing revisions, baseline retirement, join, pause,
respawn and multiple simultaneous damage changes. Add a permanently delayed topic
to check bounded memory, nonstarvation and continued presentation.

Proposed gate: at least 50% lower downstream bytes in the active two-player
workload, no worse p95 target/checkpoint age or correction error, and no lifecycle
or restore-consistency failure. Report 12-player results independently, including
server aggregate egress. Reject any apparent saving caused solely by stalled
checkpoints or missing entities. If only some topic splits help, retain those;
rollback to the whole-envelope codec is versioned at session establishment.

## Stage 1: implementation and scope

The `section-topics` Cargo feature exposes `server::section_topics` and the
`section_topics` example. It does not select a live transport, change protocol
negotiation, or alter the app. The stage-1 report below records its original stop.

The encoder splits the current compact player snapshot (including its full
precision fallback) into metadata, chassis, projectiles, restore, referee,
runes, outposts, bases and hits. Metadata retains envelope identity, tick/time,
counters, shot results and server fields. There is no additional quantization,
mechanism parameterization, AOI or projectile-policy change. A single capture
supplies every section and its manifest; revision reuse requires exact JSON
value equality. Empty sections still get their own delivery at this stage.

Every section uses the existing acknowledged-baseline encoder and decoder.
The only shared-code change extracts the decoder's validation callback so both
representations use the same pinning, delta selection and retirement rules.
Baseline identities and section value revisions are distinct. The assembler
keeps section values independently of baseline retirement and emits only the
newest complete manifest as an ordinary `SimulationState`. A missing dependency
produces no checkpoint; callers retain their previous complete state.

Prototype bounds are four pending manifests and eight revisions / 4 MiB of
serialized payload per topic, separately from each topic's two 1 MiB decoder
baselines. JSON allocation overhead is additional. Eviction abandons dependent
manifests rather than substituting a newer value. A later independent state can
restore progress; targeted repair and production memory budgets are stage 2.

Stage-1 correctness gates are exact equality with the existing decoded player
representation, identical restore/replay from that representation, no mixed
checkpoint promotion under delayed sections, and bounded caches. The seven
focused tests cover reordering, two consecutive missing projectile revisions,
old epochs, pause, join/removal, baseline retirement with reused values, a
permanently missing topic, malformed identities and full-precision fallback.
They do not establish live lifecycle/presentation behavior under impairment.

## Stage 1: measurement protocol

Build before capture:

```sh
cargo test --locked -p rm-simulator-server --features section-topics
cargo clippy --locked -p rm-simulator-server --all-targets \
  --features section-topics -- -D warnings
cargo build --locked -p rm-simulator-server --features section-topics \
  --example section_topics
# With the repository's Nix shell; otherwise substitute the active target directory.
target/nix/debug/examples/section_topics \
  --frames 1875 --warmup-frames 313 --seeds 5 \
  > docs/experiments/results/02-section-topics/runs.jsonl
python3 docs/experiments/results/02-section-topics/summarize.py
```

Each seed covers one idle robot, two moving robots, two moving/firing robots,
and twelve moving robots with two firing. Fire requests occur every 96 ms per
shooter; accepted launch counts and actual movement are recorded. Both codecs
consume the identical captured state at 32 ms publication, with the default
1 ms physics clock and unchanged projectile behavior. There are 10.016 s of
warm-up and 60 s measured per run. Scenario order is deterministically shuffled;
codec execution order alternates. Idle seeds are repeated controls.

This is a narrower **offline byte ablation**, not the full shared trial matrix:
synthetic flat ground and default mechanisms, no CAD, sockets, pacing, loss or
RTT. Feedback is immediate. Each manifest/section is separately compressed and
RMG1-framed; small-section datagram aggregation is deferred. Counts include
application fragment headers, all baseline acknowledgements and retirement
messages in both directions. UDP/IP/GNS overhead, retransmissions, inputs,
owner anchors and reliable hit streams are excluded. The twelve-robot case is
one peer observing twelve robots, not measured server aggregate egress.

The dev build is sufficient for this byte-only comparison; no CPU timing,
checkpoint latency or rendered perception claim is made. Source/binary hashes,
base revision, compiler and machine architecture are in
[metadata.json](results/02-section-topics/metadata.json). The same-build assertion
against the unmodified whole-envelope path is the control. The proposed 50%
live downstream reduction and no-age/correction-regression gates remain
unproven regardless of this ablation's byte result.

## Stage 1: results and decision

The twenty completed runs are in [runs.jsonl](results/02-section-topics/runs.jsonl);
[summarize.py](results/02-section-topics/summarize.py) reproduces this table. Rates
are mean downstream application KiB/s per observing peer, including RMG1 headers
and reliable retirement work. Ranges are across five seeds, not confidence
intervals. `idle` has one robot, `drive` and `fire` have two, and `twelve` has twelve.

| Workload | Whole KiB/s | Sections KiB/s | Downstream increase (range) | Fragment multiplier |
|---|---:|---:|---:|---:|
| idle | 16.89 | 48.92 | +189.67–189.67% | 9.40× |
| drive | 46.14 | 77.99 | +68.96–69.15% | 5.40× |
| fire | 140.75 | 170.66 | +21.17–21.40% | 2.63× |
| twelve | 293.36 | 320.69 | +9.29–9.35% | 1.83× |

All 37,500 measured checkpoints were delivered by each codec and checked against the existing player representation.
Peak retained section payload: 364,380 bytes (excludes baseline and allocator overhead).

Acknowledgement traffic rose from 67.85 to 699.15 application bytes/s per peer
for all four workloads. Including both directions, the mean increases were
192.57% idle, 70.28% drive, 21.68% fire and 9.53% twelve. The firing workloads
each launched 1,250 shots during every measured minute. No apparent saving is
being attributed to missed checkpoints or failed launches.

**Decision: reject same-cadence, separately framed section delivery as a standalone
bandwidth optimization.** It adds roughly 27–32 KiB/s downstream here. Even empty
or unchanged topics send envelopes, and each topic runs its own baseline
handshake. Separate compression also gives up whole-envelope compression context.
These results establish the cost that later scheduling/aggregation must recover;
they do not reject all topic splits or demonstrate any loss-isolation benefit.

Validation passed: 207 server library tests (including seven new focused tests),
one server binary test, 32 doctests, server Clippy with warnings denied, formatting,
crate-boundary checks and hooks on the changed files. Full workspace `just verify`
was not run; no PR was opened.

The experimental codec remains available for comparison, and live networking is
unchanged. **Stage-1 stop decision:** stage 2 should add one-budget topic scheduling,
small-message aggregation, safe suppression of unchanged revisions, and bounded
dependency repair, then test these over the scripted link. Integration must also
revisit the current fragment assembler's four-frame cap: nine independently
fragmented topics cannot simply be connected to the live receiver without
accounting for concurrent assemblies. Stage 3 would test changed publication
cadences and early presentation only after those delivery invariants hold.

## Stage 2: delivery and bounded recovery

`section_topics::delivery` adds an experimental sender and receiver. This still
runs behind `section-topics`, without live negotiation or changes to the app.
The original stage-1 encoder and example remain the always-send control.

- One token bucket covers periodic data, exact-revision repair and ordered
  reliable controls. Round-robin fragment service reserves enough tokens for
  the next class rather than allowing smaller packets to starve it. Small
  fragments share a datagram, up to 1,024 application bytes. A fragment carries
  up to 1,000 payload bytes, matching the existing codec's payload size.
- A section receipt identifies the latest retained value revision in its epoch.
  Suppression requires that exact receipt, separately from the baseline ACK.
  Receipt loss therefore causes redundant traffic, not an assumed dependency.
  Feedback coalesces receipts and baseline answers under its own upstream budget.
- Under backpressure the sender finishes queueing out one coherent publication
  and retains only the newest successor capture. It does not wait for its
  receipt. Replacing every topic independently with each 32 ms capture can
  otherwise leave no manifest whose complete dependencies were sent. Configured
  publication remains 32 ms; actual transport coalescing is explicitly counted.
- Missing-revision requests are rate-limited to one repair opportunity per
  96 ms. The client selects one aged incomplete checkpoint and protects its
  revisions, up to two seconds, rather than continually moving the repair target.
  The host pins the exact values for that target separately from baseline/history
  rotation. It retransmits the manifest and requested values on replaceable
  repair queues; it never substitutes newer values into the requested manifest.
  A quiet final manifest is also retried until its checkpoint is acknowledged.
- Regular section, repair and manifest assemblies have separate slots. The
  fragment layer rejects mismatched sizes, classes, lanes and conflicting
  duplicates. Reliable controls retain application order and cannot be replaced
  by periodic traffic. Tests exercise a fragmented full confirmation before Pong.

The bounds are 32 manifests and 32 revisions / 4 MiB serialized values per topic;
a 2 MiB encoded outgoing queue; one successor snapshot and one host repair target
of at most 4 MiB each; and 4 MiB total partial-frame allocation. The client protects
its latest section value plus the selected repair revision within its existing
cache cap. The baseline decoder still pins at most two 1 MiB baselines per topic.
These are separate bounds; cached payload metrics are not total process memory.

Focused tests cover lost/duplicated/reordered traffic and blackout recovery,
unchanged-value suppression and epoch reset, cache loss after receipt, logical
omission of each topic in turn, an impaired peer beside a healthy peer, malformed
fragments, damage/outpost destruction, revival, placement, join/removal, and
confirmation-before-Pong ordering. The permanent-topic tests omit only that
logical topic's fragments; a physical lost aggregate can lose multiple topics.
The scripted-link tests separately exercise physical datagram loss.

## Stage 2: trial protocol

Build and validate first, then run the explicit trial in isolation:

```sh
cargo test --locked -p rm-simulator-server --features section-topics
cargo clippy --locked -p rm-simulator-server --all-targets \
  --features section-topics -- -D warnings
cargo test --locked -p rm-simulator-server --features section-topics --lib \
  section_topics::delivery::trials::stage2_trials \
  -- --ignored --exact --nocapture \
  > docs/experiments/results/02-section-topics/stage2/trial-output.txt
python3 docs/experiments/results/02-section-topics/stage2/summarize.py
```

The recorded run invokes the already-built test executable directly; its hash
and the source hashes are in [stage2/metadata.json](results/02-section-topics/stage2/metadata.json).
`RM_SECTION_TRIAL_FRAMES`, `RM_SECTION_TRIAL_WARMUP` and `RM_SECTION_TRIAL_SEEDS`
allow short preflights; the recorded trial uses 1,875 measured frames, 313 warm-up
frames and five seeds. The summary refuses an incomplete matrix or a failed run.

There are 50 cells: two or twelve moving robots with two shooters, five seeds,
and five link profiles. Each uses 10.016 s warm-up plus 60 s measured, a 1 ms
physics clock, 32 ms publication and the same captured source for both variants.
The production `PeerCodec`/`ClientCodec`, including their real byte pacers,
fragmentation and ACK paths, are the whole-envelope control. The observing peer
has no owner anchor, making the measured traffic checkpoint/feedback traffic.
Every delivered checkpoint is compared exactly to the existing decoded compact
player representation for its capture ID. All profiles for a workload/seed
share a captured-stream hash; profile order is reproducibly shuffled.

| Profile | Downstream / upstream application budget | Impairment in each direction |
|---|---|---|
| clean | 512 / 10 KiB/s | none |
| rtt40 | 512 / 10 KiB/s | 20 ms one-way delay |
| rtt100_loss | 512 / 10 KiB/s | 50 ms one-way + 0–10 ms jitter; 3% loss; 2% duplication; every 17th unreliable packet held for two sends; 100 ms reliable recovery delay |
| blackout | 512 / 10 KiB/s | 20 ms one-way; 500 ms blackout beginning 20 s into measurement; 100 ms reliable recovery delay |
| limited | 40 / 10 KiB/s | 50 ms one-way + 0–10 ms jitter; 1% loss; 100 ms reliable recovery delay |

The driver advances an explicit clock in 2 ms increments. It samples checkpoint
age throughout the measured interval, including stalls, and records delivered
checkpoint counts and coalescing. A separate available-chassis age measures
when an exact chassis section and its manifest were present; this is a diagnostic,
not rendered target age or permission to predict from a partial world. The
control's chassis availability advances with complete checkpoints.

Rates count application datagrams released by the codecs, including aggregation,
fragmentation, ACKs and repair. They do not count native UDP/IP/GNS overhead or
carrier retransmission bytes. The scripted link models reliable recovery as
ordered delay, not actual native retransmission packets. No CAD, sockets, owner
anchors, input streams, rendered perception, prediction correction error or CPU
benchmark is included. Twelve robots means one observing peer, not measured
aggregate server egress. At a saturated budget, equal bytes cannot establish a
saving: checkpoint age and delivery counts must also be inspected.
Only the section codec's queue/cache/partial-frame peaks are instrumented;
the whole-codec memory fields in the raw output are zero placeholders, not
measurements of its memory use.

## Stage 2: results and decision

All 50 cells completed. The [raw test output](results/02-section-topics/stage2/trial-output.txt)
and [extracted records](results/02-section-topics/stage2/runs.jsonl) preserve every
seed. The table reports mean rates/counts and the range of paired changes across
five seeds; the ranges are not confidence intervals. Positive age changes mean
the section codec was older. There were 1,875 offered captures per measured run.

| Robots | Link | Whole → sections KiB/s | Downstream change range | p95 checkpoint age change range | Mean checkpoints whole → sections |
|---:|---|---:|---:|---:|---:|
| 2 | clean | 140.59 → 159.54 | +12.92…+13.90% | +0…+0 ms | 1875 → 1875 |
| 2 | rtt40 | 141.26 → 160.48 | +13.25…+14.18% | +0…+0 ms | 1875 → 1875 |
| 2 | rtt100_loss | 142.60 → 162.16 | +13.07…+14.17% | +4…+10 ms | 1591 → 1523 |
| 2 | blackout | 141.26 → 160.52 | +13.31…+14.21% | +0…+0 ms | 1859 → 1859 |
| 2 | limited | 40.00 → 40.00 | -0.01…+0.02% | +144…+246 ms | 413 → 250 |
| 12 | clean | 292.53 → 309.26 | +5.24…+6.34% | +0…+0 ms | 1875 → 1875 |
| 12 | rtt40 | 295.32 → 312.38 | +5.38…+6.23% | +0…+0 ms | 1875 → 1875 |
| 12 | rtt100_loss | 298.70 → 317.05 | +5.65…+6.86% | +2…+10 ms | 1353 → 1298 |
| 12 | blackout | 295.32 → 312.41 | +5.43…+6.23% | +0…+0 ms | 1859 → 1859 |
| 12 | limited | 40.00 → 40.00 | -0.04…+0.06% | -10448…-5252 ms | 11 → 113 |

whole: 72,933 measured deliveries checked exactly against the captured player representation.
sections: 72,010 measured deliveries checked exactly against the captured player representation.
Peak section cache payload: 1,413,212 bytes.
Peak queued payload: 30,454 bytes.
Peak partial-frame allocation: 21,523 bytes.

These payload peaks exclude the sender's separately retained source/repair state,
baseline storage and allocator overhead; they are not total process memory.
No measured section-codec age sample lacked an initial complete checkpoint.

The result is mixed, and **the stage-2 variant is not adopted**:

- On clean links, both codecs deliver all offered checkpoints at the same p95
  age, but section delivery still costs 12.92–13.90% more downstream for two
  players and 5.24–6.34% more for twelve. It does not meet the proposed 50% saving.
- With 100 ms nominal RTT and impairment, complete checkpoints are 2–10 ms older
  at p95 across the matrix. Exact chassis sections become available 2–12 ms
  earlier, but the app does not yet use them for presentation.
- At 40 KiB/s both senders saturate the budget. Two-player section checkpoints
  are 144–246 ms older at p95 and fewer are delivered. Their mean p95 age is
  706.8 ms versus 498.4 ms for the whole-envelope control. Equal bytes here are
  not a bandwidth saving.
- Twelve-player constrained recovery improves substantially: about 113 versus
  11 complete checkpoints per minute, with mean p95 age 1,261.2 ms versus
  8,406.0 ms. This addresses a whole-envelope recovery weakness in this workload,
  but neither result establishes acceptable gameplay latency.
- Feedback adds cost upstream. Clean section traffic averages about 2.75 KiB/s
  versus 0.066 KiB/s for the control; with loss/jitter it rises to about
  5.26–5.30 KiB/s. Actual pilot inputs are not included in these figures.

Validation passed: 216 normal server library tests, one server binary test and
33 doctests; the explicit 50-cell trial also passed. Server Clippy with warnings
denied, formatting, crate-boundary checks and hooks on changed files pass. Full
workspace `just verify` was not run, and no PR was opened.

**Stage-2 stop decision.** Live transport remains on the original codec. Stage 3
would separately test changed publication cadences and early presentation. It
must retain complete-checkpoint age and correction-error gates; delivering less
context or rendering a fresh chassis section does not justify restoring a partial
physics world. Any eventual adoption still needs the omitted CAD, real GNS wire,
input/owner traffic, aggregate-server and rendered trials.


## Stage 3: independent presentation and checkpoint cadences

`section_topics::delivery::presentation` adds two independently timestamped,
complete pose lists to the same bounded round-robin sender and reassembler.
Publication uses the injected connection clock even while simulation time is
paused; captures retain the actual simulation timestamp.
Robot entries include identity, placement/revival generation, body/gun poses and
defeat state; projectile entries include identity and position. Each list carries
epoch, capture identity and simulation time. A newer complete roster removes
absent entities. A checkpoint can fill either presentation list but cannot roll
back a newer one. Epoch advancement clears both old rosters. Pose lists never
produce `SimulationState`, and only exact complete checkpoints can reach restore.

This is a headless presentation prototype, not app integration. Robot shape,
referee/rune/base visuals and physics remain checkpoint-owned. A renderer must
wait for configuration before drawing an unfamiliar robot. The trial uses
zero-order hold: no interpolation or extrapolation. Pose error is distance from
the held pose to authoritative current truth, with gun quaternion angular error;
it is **not** prediction replay correction error or measured visual smoothness.
Missing and stale projectile samples are counted separately, so distance errors
on matched identities cannot conceal undelivered launches/removals.

Pose lists are independently compressed JSON (deflate level 1), without delta
baselines. This is deliberately a separate presentation representation; complete
checkpoints keep the original compact player representation and acknowledged
section deltas. Pose lists include full-precision positions, making this a
conservative implementation candidate rather than a minimal binary wire format.
The same byte budget pays for duplicate poses in checkpoints, pose lists,
feedback, repairs and reliable controls. There are now 23 fragment classes;
complete-pose lanes have the same active-plus-latest-successor bounds and 1 s
partial expiry. Each decompressed pose frame is capped at 4 MiB. Stored pose
lists, baselines and allocator overhead are not included in section-cache metrics.

### Trial protocol

First screen all twelve combinations: chassis 16/32 ms, projectiles 32/64 ms,
complete checkpoints 32/64/128 ms. Each uses two and twelve moving robots with
two shooters, clean / 100 ms RTT with loss / 40 KiB/s limited links, one seed,
2.016 s warm-up and 10 s measurement: 72 cells, 210 accepted launches each. This is a screening run, not a
five-seed gate result. Blackout and 40 ms RTT are reserved for the longer follow-up.
The source is captured every 16 ms, physics stays at 1 ms, commands change every
256 ms and the two shooters fire every 96 ms. The production whole-envelope
control still publishes every 32 ms. Within each player/seed group, every cadence
and link profile consumes the same source stream, checked by SHA-256.

Select the clean-link bandwidth winner, **32/64/128 ms**, for the five-seed,
five-profile follow-up: 50 cells, 10.016 s warm-up and 60 s measurement each.
Selection is exploratory and includes seed 1 again; it is not an independent
holdout. Existing stage-2 link parameters and budgets apply. The 500 ms blackout
starts 20 s into measurement. Both paths use the same injected clock and emit
application datagrams through their actual pacers. Owner anchors, input traffic,
native carrier retransmissions, CAD, aggregate multi-peer egress, rendered
presentation and prediction replay remain outside the trial.

Every delivered checkpoint is checked exactly against the compact source capture.
Checkpoint and chassis ages are sampled every 2 ms including stalls. Body, aim
and projectile errors are sampled at the exact 16 ms source captures after that
instant's delivery processing. Missing entities count separately; these samples
are correlated measurements, not independent statistical observations. Ranges
across seeds are descriptive. Slower checkpoint cadence is intentional and does
not count as a bandwidth saving without reporting its increased age.

Reproduce the screen after building the feature test binary:

```sh
RM_SECTION_STAGE3_SCREEN=1 RM_SECTION_TRIAL_FRAMES=625 \
RM_SECTION_TRIAL_WARMUP=126 RM_SECTION_TRIAL_SEEDS=1 \
cargo test --locked -p rm-simulator-server --features section-topics --lib \
section_topics::delivery::trials::stage3::stage3_trials -- --ignored --exact --nocapture
```

For the selected follow-up, set only `RM_SECTION_STAGE3_CADENCE=32/64/128`;
the defaults are 3750 source captures, 626 warm-up captures and five seeds.
Omit both stage-3 filters to run the full 600-cell long matrix, which was not
needed for this screening decision and is not claimed here.

### Screening results

| Chassis / projectile / checkpoint ms | Clean downstream change, 2 robots | Clean downstream change, 12 robots | p95 checkpoint / chassis age change |
|---|---:|---:|---:|
| 16/32/32 | +83.00% | +70.39% | +0 / -16 ms |
| 16/32/64 | +25.61% | +17.32% | +30 / -16 ms |
| 16/32/128 | -3.04% | -9.36% | +90 / -16 ms |
| 16/64/32 | +59.58% | +59.40% | +0 / -16 ms |
| 16/64/64 | +2.18% | +6.33% | +30 / -16 ms |
| 16/64/128 | -26.47% | -20.35% | +90 / -16 ms |
| 32/32/32 | +72.37% | +49.44% | +0 / +0 ms |
| 32/32/64 | +14.99% | -3.62% | +30 / +0 ms |
| 32/32/128 | -13.67% | -30.31% | +90 / +0 ms |
| 32/64/32 | +48.95% | +38.46% | +0 / +0 ms |
| 32/64/64 | -8.44% | -14.61% | +30 / +0 ms |
| 32/64/128 | -37.10% | -41.29% | +90 / +0 ms |

All 72 screening cells passed exact checkpoint reconstruction checks. The clean
case selects 32/64/128 ms for least traffic at both player counts. None of the
twelve combinations reached a 50% saving in the two-robot clean screen. The
16 ms chassis settings improved p95 chassis age by 16 ms in that case, at a
measurable bandwidth cost. These short screening runs do not establish a
production gate.

The selected long follow-up was interrupted after 21 of 50 cells. Its raw
`results/02-section-topics/stage3/trial-output.txt` has no passing footer; the
summarizer rejects the incomplete matrix. These partial observations are not a
completed five-seed result. The new tuning protocol treats all inspected seeds as
development data.

### Stage-3 validation and scope limits

The final server build passed 222 library tests, one binary test and 34 doctests
(257 total); the two long experiment tests are explicitly ignored by the normal
suite. Six new normal tests cover early-pose isolation, checkpoint/epoch ordering,
complete-roster removal at paused time, permanently missing checkpoint/projectile
lanes, paused publication away from a checkpoint boundary, invalid cadence and
capture ordering, and a real moving/firing cadence smoke trial. Existing tests
continue to cover complete restore/replay parity, reliable confirmation-before-Pong,
damage/revival/join/removal, bounded repair and impaired peers.

Clippy with warnings denied, formatting and crate-boundary checks pass. Full
workspace `just verify` was not run; no PR was opened. The experimental feature
still does not select a live transport or change app presentation.

The pose lists intentionally do not reconcile independently presented defeat
flags against older referee HP, predict robot motion, animate full robot models,
or reconstruct mechanisms from motion anchors. A live presentation adapter and
lifecycle transactions would need explicit tests before adoption. Fresh pose
age alone cannot establish the original prediction-correction gate.
