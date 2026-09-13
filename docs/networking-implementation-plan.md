<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Networking implementation record and remaining validation

Status checked 12 September 2026. Live protocol is 26; see the
[README runtime overview](../README.md#server-and-clients). The original design was against `cbe4b91`, protocol 17.
Stages 1 through 5 are implemented through protocol 21. Stage 6 remote buffering
and diagnostic work is implemented without a protocol change; solver retention
is evaluated below and is not enabled across live rewinds. Multi-seed multiplayer
acceptance remains outstanding. This expands the
[roadmap](multiplayer-networking.md) into implementation slices. The [harness and stats design](network-harness-and-stats.md)
remains the measurement contract. Releases remain paused.

## Implementation status

Protocol 22 removed the shooter-view fire path (`CompensatedFire`, `ShotView`,
`InputEvidence`, the per-peer view archive and the shot admission worker) and the
ordered TCP snapshot delta chain (`SnapshotDelta` and its revision chain), neither
of which had a live producer. It also dropped the unread input acknowledgements.
Passages below that specify either layer are historical. Protocol 24 adds live
base scoring and training-bot state. The physics extraction preserves this wire
contract; `Field::restore` still rebuilds the complete simulation.

Stage 5 adds adaptive input lead, transition-aware redundancy and scheduled firing.
The host reports a fifth-percentile arrival margin over the last 64 newly received
sequences. After eight observations, the client increases lead by 16–32 ms when
that margin is negative, within a 32–150 ms range. Five fresh feedback reports
with at least 16 ms margin allow a 2 ms reduction. These are initial experimental
settings, not measured optimal values. Duplicate feedback cannot adjust lead;
control epochs and placement revisions reset it. Sent samples remain immutable,
and decreasing lead preserves monotonically ordered input times.

UDP applies each received batch in one host operation, newest sequence first,
so old redundant samples neither split across host ticks nor skew arrival feedback.
It repeats the newest four controls and up to eight older movement transitions
within the 250 ms useful window. Direction changes use a 0.1 m/s or rad/s
deadband so small body-relative steering adjustments do not fill the history.
Twelve packed samples fit below 1,000 application bytes without quantization. Set `RM_NET_FIXED_INPUT_LEAD=1` on a client to compare
the previous RTT-based lead and `RM_NET_INPUT_HISTORY=4` to compare four-sample
redundancy. Harness manifests retain both overrides.

Future shots enter a host-owned simulation queue, limited to 32 per chassis.
A reliable `ShotScheduled` acknowledges receipt and scheduling together, stopping
intent retries without confirming a projectile. At the intended tick, controls
apply before the shot's exact aim derives the muzzle; physics advances afterward.
Late eligible shots execute at the current tick. `ShotResult` reports the terminal
execution tick/projectile or rejection. Cooldown rejections are now terminal for
that ID. Pause, defeat, placement changes and disconnect cancel queued shots.
Results remain in recoverable snapshots as well as reliable outcome messages.

Client firing predicts legal cadence and retains reservations until authoritative
state confirms execution. The prediction worker captures the muzzle at the shot's
intended tick, retaining up to 512 chassis samples for requests that arrive after
replay passed that tick. The initial displayed muzzle is provisional until that
capture is available; authoritative projectile checkpoints repair the trajectory.
Retry expiry at 250 ms is distinct from an unresolved outcome, which is counted
when its bounded five-second retention ends. No historical damage or catch-up
projectile simulation is added to the host.

### Stage 5 validation

`just verify` passes 347 Rust tests and 30 harness tests, plus formatting,
clippy, crate-boundary and dependency checks. New coverage includes scheduling
receipts versus execution, exact-tick moving muzzles, ammunition reservation and
once-only charging, terminal cooldown rejection, bounded queues, pause/placement/
defeat cancellation, input history selection and bounded adaptive lead.

Both final one-client CAD trials completed with clean process shutdown. The
12-second direct run had 41 active samples, a global shot-count increase of 38,
two local rejections and no unresolved outcomes. The eight-second constrained
smoke had 26 active samples, a global shot-count increase of 25, seven local
rejections and one unresolved outcome. Counts span sampled endpoints. The last recovery sample had no pending shots or
connection warning. Lead reached its 150 ms cap in the constrained run. These short sampled results do
not establish playability or improvement over stage 4. The longer multi-seed,
two-client acceptance gate remains outstanding.

### Stages 1–4 delivery

Protocol 29 follow-up: owner anchors now use RMO4 configuration references after
an explicit acknowledgement of reliable configuration delivery; compact RMI3
input batches preserve exact values, sample selection and redundancy. Cadence
and pacing are unchanged. The protocol 20 configuration encoding described below
is historical. See [bandwidth experiments](bandwidth-experiments.md) for the
integration status and measured experimental revisions.

Local stats and input execution telemetry are implemented. Protocol 19 adds a
separate owner datagram, capped at 1,000 application bytes including its tag.
Protocol 20 packs dynamic f64 values directly and compresses the configuration.
Both ordinary and high-precision movement fit the cap without quantization.
Unusual oversized configurations fall
back to full checkpoints; anchors never fragment. World checkpoints still repeat
at the existing cadence and repair collision context without an extra request.
Spatial filtering is deferred. Missing context remains an explicitly aged,
bounded approximation, and a new context causes the prediction worker to restore
again even when the owner revision is unchanged. Physics tuning is unchanged.

Pacing now gives ordered control, replaceable owner state and world transfers
round-robin byte service. Credit accumulation is capped at 32 ms of budget or
one datagram, whichever is larger. Small packets cannot repeatedly consume a large packet's
reserved turn. A transfer expires after 250 ms; queued control fails after two
seconds. Reliable confirmation snapshots and Pongs retain their order. Development
budgets use `RM_NET_UP_KIB_S` and `RM_NET_DOWN_KIB_S`, defaulting to 10/40 KiB/s.
Longer multi-seed quality comparisons remain separate from the quick commit checks.

Protocol 20 applies structural deltas to the compact world representation. Two
baselines of at most 1 MiB each remain pinned on the receiver. The sender rotates
only after an acknowledged replacement and retirement of the previous baseline.
Old full copies cannot resurrect a retired ID. State freshness uses the host
publication ID, so a retried old baseline cannot hide a newer confirmation. Missing ACKs still allow current
independent snapshots; missing baselines request repair. Epoch changes discard old
baseline contracts. Owner anchors remain full and separate. Set
`RM_NET_FULL_CHECKPOINTS=1` on the server to compare independent full checkpoints
under the same pacing and owner behavior. Input batches also pack their four
unchanged f64 control samples directly; shot intents use bounded compression.
Queued identical retries coalesce without refreshing their expiry. Harness manifests record this override.

Quick checks cover app/server tests, native loopback command barriers, deterministic
loss/reordering/baseline lifecycle tests, harness tests and clippy. The codec fixture
used 34,482 encoded bytes versus 88,666 independent bytes over 160 updates. This
excludes owner/control traffic and transport overhead; it is not a gameplay claim.
Full multi-seed, two-rendered-client acceptance remains outstanding, including the
previously identified Metal allocation limit on this machine.

### Protocol 20 short native verification

Both final one-client CAD trials completed and shut down their owned processes.
The direct run used 12 active seconds; the constrained smoke used eight seconds,
256/512 kbps caps, 25 ms one-way base delay, downstream jitter, 1% upstream loss
and a 500 ms upstream blackout. These are smoke checks, not multi-seed acceptance.

| Trial | Sampled owner gap maximum | Proxy received up/down | Global shot delta / local rejections |
|---|---:|---:|---:|
| Direct, 41 active samples | 47.1 ms | 8.26 / 41.81 KiB/s | 38 / 2 |
| Constrained, 24 active samples | 24.3 ms | 9.84 / 42.53 KiB/s | 24 / 12 |

Rates count UDP payload before simulated loss and exclude IP/UDP headers. Owner
gap measures receipt freshness, not end-to-end input latency or collision-context
freshness. The constrained run's smaller sampled maximum does not imply lower
latency. Shot counts span sampled endpoints; they are not an acceptance percentage.
Shot expiry remains a reason to do stage 5. Compact and Detailed overlays were
visually inspected. Reproduce the workloads with `direct-single.json` and
`smoke-single.json` in `scripts/network-scenarios` and a new output directory.

An earlier direct trial exposed queued duplicate shots and JSON owner packets
exceeding the cap during ordinary CAD motion. The final implementation coalesces
and expires queued retries and uses fixed-width numeric packing. Tests cover both
regressions, including high-precision movement and reliable confirmation ordering
when an older baseline is retransmitted.

## Architecture decision

Keep one authoritative host simulation and immediate client prediction using the
shared physics. Clients already simulate their own chassis; adding prediction
again would not address the current delivery and scheduling problems. Prediction
results remain provisional, including local projectile contacts.

Prioritize independently usable owner corrections, bounded traffic and explicit
input execution acknowledgements. Then reduce repeated state with acknowledged
UDP deltas and align shot execution with movement. Keep each experiment separately
selectable in development builds so a regression has a useful comparison.

The short native harness trials establish that the proxy runs and constrains
traffic. They do not establish multiplayer quality. The constrained single-client
trial had large checkpoint gaps; the direct trial did not. Combined impairments
cannot identify a single cause. Two rendered clients failed on this machine with
a Metal allocation error. Run that gate on suitable hardware before claiming
healthy-peer isolation or robot-contact quality.

## State delivery contract

Proposed message names below describe contracts, not existing Rust types.
All state messages carry connection generation, control epoch, source tick and
stream revision. Revisions are ordered within a stream, never across unrelated
streams. Placement revision identifies robot resets as well as a new life.

| Message | Contents | Delivery and replacement |
|---|---|---|
| `OwnerAnchor` | Full-precision owner pose, velocities, gimbal, held controls, placement revision, finalized tick, applied input identity, collision context reference | Independent unreliable message; replace unsent older anchor |
| `CollisionContext` | Complete membership and states of relevant moving bodies at one tick, plus mechanism/config revisions | Independently recoverable bounded update; never merge arbitrary latest poses for replay |
| `EntityMotion` | Remote render poses and projectile corrections, source tick and entity generation | Unreliable, replace stale unsent state per entity/group |
| `WorldControl` | Configuration, roster, lifecycle changes, referee state and reset epochs | Reliable ordered control with bounded outstanding bytes |
| `ShotOutcome` | Shot ID, status, actual execution tick, projectile identity or rejection reason | Reliable bounded outcome delivery, idempotent application |
| `TimingFeedback` | Input arrival/execution aggregates and correlated probe data | Replaceable, piggyback where possible |

Keep authoritative health, ammunition and match state in recoverable replicated
state. An outcome event alone must not be the only record of a lasting change.
Unknown configuration or entity generations defer dependent application in a
bounded buffer and request recovery. Omission from a motion message never deletes
an entity. A full lifecycle checkpoint repairs missing membership.

Initially send owner anchors at the existing remote checkpoint cadence, about
31 Hz. Target at most 900 encoded application bytes, preserving chassis precision.
Measure the actual encoded fields and GNS datagrams before fixing that limit.
If they do not fit, redesign the fields or codec; do not quietly fragment the
critical anchor. Remote motion frequency starts unchanged to isolate separation.

An anchor is independently decodable, but complete collision replay still needs
context. Client reconciliation must distinguish those two facts.

### Collision context and reconciliation

For the first separation experiment, retain a complete dynamic collision context
at a single tick. Defer spatial relevance filtering until this works in robot
contacts. Static geometry stays in verified client CAD and never crosses the wire.
Mechanisms derive from versioned authoritative parameters and explicit time.

1. Decode the anchor and reject stale generations/revisions. Compare against the
   exact same-tick prediction history before changing that history.
2. Restore the authoritative owner state at its tick and discard finalized input
   intervals. An applied sequence is not an acknowledgement of every lower sequence.
3. If matching context exists, replay unfinalized controls using shared physics.
   Tag results with anchor, context and prediction generation so stale worker
   results cannot replace newer results.
4. If context is missing, accept the owner correction and request context recovery.
   Continue only bounded provisional prediction, with context age exposed. Never
   label this result coherent or a confirmed contact. Initially preserve the
   existing 100 ms remote extrapolation and one-second overall continuation limits.
5. When context arrives, rebuild from a matching anchor/context pair and replay.
   Keep a bounded history of pairs; if the pair expired, request a newer complete
   pair instead of inserting old bodies into the current simulation.

An owner anchor need not wait for distant rendering state. It cannot guarantee
accurate robot collisions while the required collision context is unavailable.
Record both anchor freshness and coherent-replay freshness so the optimization
cannot hide a collision regression behind better update-gap numbers.

## Sending under bandwidth pressure

Each peer gets a paced application budget and bounded pending work before GNS.
Maintain one replaceable pending item per motion stream, a bounded reliable queue,
and a bounded recovery transfer. Encoding happens after selection. Never block
host simulation or a frame while waiting for bandwidth.

Start with configured budgets in harness experiments. Reserve capacity for control,
owner anchors and outcomes, then service collision context, nearby motion and
remaining state. Use weighted service and age deadlines so recovery and lifecycle
state cannot starve. Bulk recovery must not sit ahead of control confirmations.
If critical traffic alone exceeds the budget, report overload and reduce optional
traffic to zero; increasing the queue is not a remedy.

Trial application targets for the 128/512 kbps profile are 10 KiB/s upstream and
40 KiB/s downstream per player. These leave some capacity for transport overhead
and recovery, but are not measured wire limits or supported-network promises.
Charge actual encoded bytes; separately measure native/proxy bytes including
retries. Tune the reserve from those measurements. Limit token accumulation to
one update interval so restoration cannot release seconds of queued traffic.

Retain the current ordered confirmation-snapshot/Pong path in the first slice.
Any later lane split must gate Pong exposure on application of its confirmation
revision. Native lane availability and semantics must be checked against the
pinned Rust wrapper before implementation. Do not introduce lanes just to ship
owner separation.

## UDP baseline lifecycle

Implement deltas after the independent streams pass loss tests. Keep owner anchors
full initially; apply deltas to the larger context and remote-state groups first.

A receiver acknowledges only a completely decoded and retained baseline. A sender
may reference only such acknowledged state in the same stream and epoch. Bound
both caches by bytes and count. Prototype two retained baselines per stream plus
one pending replacement, with an explicit transfer byte cap.

Use an ordered retirement handshake. The sender stops constructing deltas against
an old baseline, then sends `RetireBaseline`. On receipt the client retires it and
acknowledges retirement before the slot is reused. Delayed unreliable deltas may
still name the retired baseline; discard them and count the miss. They must not
resurrect it or corrupt current state. A new connection/epoch clears the contract.

Unknown baselines trigger a rate-limited independent checkpoint request. Repeat
baseline acknowledgements until progression confirms them. Lost ACKs waste bytes
but must not stop full-state recovery. Recovery transfers have bounded assembly
size, lifetime and concurrent count, and share the connection budget.

## Input and shot timeline

Keep complete held controls sampled about every 16 ms. Add distinct received and
executed identities, actual execution ticks and a finalized simulation boundary.
The host never waits for a missing input. A duplicate cannot renew the stop lease.
When several late controls arrive together, supersede obsolete held states rather
than replaying their old durations. Record the actual transition that took effect.

Initially preserve the 250 ms lease and late-input limit and 200 ms future limit.
Compare current four-sample redundancy with byte-bounded history that preferentially
retains useful press/release transitions. History must fit the input message budget;
unlimited repetition under congestion is counterproductive.

Adapt lead from server-observed arrival margin, not just half of reliable RTT.
Trial a 32 to 150 ms range, fast increases on sustained late arrivals and slow
reductions after stable delivery. The exact controller and target percentile are
chosen from measured arrival distributions. Do not rewrite timestamps on inputs
already sent. Increasing lead changes future samples; decreasing it must preserve
monotonic intended ticks. Reset estimation across clock/control epochs.

A shot intent carries immutable ID, epoch, placement revision, intended tick and
exact aim. The host validates ownership and enqueues it once. For on-time execution,
apply controls for tick T, derive the muzzle with that shot's aim, launch, and then
advance physics from T to T+1. Client prediction uses the same ordering.

`Received` and `Scheduled` stop unnecessary intent retries but do not confirm a
projectile. Only `Executed` binds the provisional projectile to an authoritative
ID and actual launch tick. Reconcile ammo from authoritative state without double
charging. Keep unresolved execution distinct from local retry expiry.

Initially retain a 250 ms age bound and a bounded queue of at most 32 intents per
peer. Reject cooldown-ineligible intents explicitly in the first scheduling
experiment; do not let retry arrival choose their execution time. Predict legal
cadence locally and measure cooldown rejections before considering a bounded
reschedule policy. Eligible late shots launch at the current authoritative tick.
No backdated damage, historical catch-up or accumulated firing burst is permitted.
Pause, defeat and placement/epoch changes cancel pending intents.

## Chassis smoothness investigation before stage 6

Stage 5 is committed as `ca18304`. A follow-up direct native trial with local
prediction diagnostics completed using `direct-single.json`; artifacts are in
`/tmp/rm-prediction-smoothness-01`. Across the first and last active samples,
355 prediction results were accepted and 268 discarded, about 43% discarded.
The 41 active samples showed prediction backlog p95/max of 68 ms. Sampled
last-replay duration was p95 0.984 ms and maximum 1.406 ms. These are sparse
observations of the latest worker job, not a distribution of every replay or a
measurement of every displayed frame.

The frame polls new owner snapshots before consuming the previous prediction
result. Acceptance requires that result's snapshot ID to equal the newest owner
snapshot ID. A newly arrived anchor can therefore discard already completed
physics even when the worker is fast. The camera and own chassis hold the last
accepted pose without advancing it between worker results. This is a concrete
candidate for uneven motion, rather than evidence that input lead directly adds
local input delay. Current-frame movement is also sampled after prediction
submission, adding a separate frame-order latency concern.

The follow-up fix accepts results from the current prediction generation when
both target time and authoritative baseline are monotonic, and the target has
not fallen behind the current owner tick. Each replay retains its captured
collision context; pause, placement, defeat and configuration changes still
invalidate its generation. New owner data continues to drive the next replay.

A second issue was out-of-order clock observations: an older world snapshot could
reset the clock after a newer independent owner anchor, causing repeated target
ticks followed by jumps. Clock observations now carry the input epoch, ignore
older same-epoch times and older epochs, and reset on a new epoch. This preserves
pause/resume and actual timeline resets without treating delayed context as a
rewind. Neither fix adds interpolation delay or changes host physics.

Console diagnostics expose worker completion count, last replay milliseconds/ticks,
accepted/discarded result counts, and moving/held-moving frame counts. A held frame
means the predicted tick did not change while predicted speed exceeded 0.1 m/s;
these counters describe prediction updates, not GPU frame pacing. Regression tests
cover newer-anchor acceptance, time/baseline rollback rejection, generation
invalidation and out-of-order clock observations. Stage 6 was deferred until
this smoothness fix was validated.

Validation: `just verify` passed with 349 Rust tests and 30 harness tests.
In the direct scenario, the handoff-only experiment recorded 191 held ticks in
322 moving frames and backlog p95/max 68/69 ms. With both fixes, the same scenario
recorded zero held ticks in 317 moving frames, zero discarded results among 616
collected results, and backlog p95/max 20/21 ms. Counts span the first through
last active sample. Artifacts are `/tmp/rm-smoothness-fixed-direct` and
`/tmp/rm-smoothness-final-direct`. These short single-seed trials identify the
regression and verify the counters; they are not a broad frame-pacing benchmark.

The impaired smoke trial also completed with no cleanup errors. Across its active
samples it recorded zero held ticks in 275 moving frames, zero discarded results
among 398 collected results, and backlog p95/max 26/45 ms. Artifacts are
`/tmp/rm-smoothness-final-impaired`. The existing shot-outcome issue remains:
one unresolved shot outcome was recorded. This change addresses chassis update
smoothness and does not claim to resolve that separate scheduling issue.

## Stage 6 buffering and physics diagnostics

F3 now offers automatic or manual remote-motion buffering, manual adjustment in
10 ms steps within 0–250 ms, and reset to automatic defaults. Settings last for
the session. Automatic mode starts at 64 ms, targets the p95 observed snapshot
age over 120 frames plus 16 ms, and responds to underruns, within 32–250 ms.
Delay rises promptly and falls at 5 ms per second. The remote view tick is
monotonic during automatic adjustment. Growing the delay can briefly hold the
view while filling the buffer. Changing the manual setting explicitly resets
that view mapping. Extrapolation remains capped at 100 ms, then holds.

Effective delay, resulting view age and underrun episode counts appear in F3
and console network diagnostics. An underrun is counted on entry, not every
render frame. Pause and lifecycle resets discard timing observations while
preserving the chosen mode/manual setting. Local input, own chassis prediction,
and authoritative damage do not use this buffer. Predicted projectile collision
context samples remote chassis at the same view tick as their displayed poses.

Exact-tick correction telemetry already separates position, orientation, velocity
and aim. Trial summaries now include all four, plus view age and buffer delay.
Context clues distinguish stale collision context, nearby robot/possible contact,
wheel contact and unknown. Input timing clues are reported separately and are
not claimed to be the cause of a correction. Robot proximity uses a 1.5 m radius;
contact classification is heuristic, and missing wheel contacts stay unknown.

The deterministic contact experiment compares 50 successive 64 ms replays from
cold restoration against retaining the solver while restoring chassis state at
the same tick. Maximum position error in metres:

| Scene | Cold restoration | Retained solver |
|---|---:|---:|
| Flat ground | 0.000176 | 0.000023 |
| Ramp | 0.000206 | 0.000023 |
| Robot contact | 0.001515 | 0.000023 |

This supports further solver-checkpoint work, but does not validate retaining
future contact caches during a rewind. Production still restores the shared
physics model and verified CAD. No larger camera correction or different client
movement model was introduced. Solver retention across live rewinds and broad
multi-seed gameplay acceptance remain open, rather than being inferred from this
aligned-tick experiment.

Stage 6 validation: `just verify` passed with 351 Rust tests and 30 harness tests.
The F3 panel was rendered and visually checked at `/tmp/rm-stage6-f3.png`.
The two-client smoke trial at `/tmp/rm-stage6-two-clients` reached readiness but
failed when Metal could not allocate GPU counter sample buffers; both renderers
reported device loss. Owned-process cleanup completed without errors. This is
not a passing multiplayer presentation check. Longer multi-seed and two-renderer
acceptance remains outstanding.

The single-client impaired smoke trial at `/tmp/rm-stage6-single` completed with
no cleanup errors. Its 25 active samples reported local prediction backlog
p95/max 24/24 ms and automatic buffer delay at the 250 ms cap. With no second
chassis this checks timing and diagnostics, not remote-motion quality. Eight shot
rejections and two unresolved outcomes remain separate stage 5 follow-up issues.

## Implementation and review slices

| Slice | Main modules | Completion evidence |
|---|---|---|
| 1. Local measurements and stats display | Server transport/client diagnostics; app session, prediction, console, HUD/settings | One DTO feeds console/HUD; unavailable data remains explicit; no added snapshot requests; bounded overhead |
| 2. Host execution telemetry and baseline | `host.rs`, `input_stream.rs`, protocol, simulation; harness reports | Versioned instrumentation-only build; exact-tick corrections, input/shot execution correlation; matched uninstrumented comparison |
| 3a. Independent owner stream | Protocol, `host.rs`, `gns_transport.rs`, client inbox, prediction | Optional-state loss cannot block anchors; stale results rejected; matching-context and missing-context contact tests |
| 3b. Pacing and priority | Host outbox and transport writer | Bounded bytes/age under saturation; recovery progresses; healthy peer compared at matched host load |
| 4. Acknowledged deltas | UDP codec and per-peer replication state | Lost/reordered ACK, retirement, reset and recovery tests; fewer wire bytes without longer usable correction gaps |
| 5a. Adaptive input delivery | Input stream, client clock/lead and replay | Fewer late transitions; monotonic timing; lost release stops; finalized intervals never replay twice |
| 5b. Scheduled firing | Host/simulation shot queue, protocol, predicted projectiles | Once-only execution/damage/ammo; same-tick moving muzzle; bounded expiry; no post-outage burst |
| 6. Physics and presentation tuning | Prediction restore, remote interpolation and scene adaptation | Same-tick contact error and correction tails improve with view age reported |

The Python harness is already built and verified. Do not rebuild it as slice 1.
Extend its report adapter when the shared stats schema becomes available. Keep
local/offline play and ordered TCP working; UDP-specific codecs cannot leak into
world rules or the passive renderer. Allocate wire protocol versions when each
contract changes, with clear incompatible-version errors.

For each behavioral slice, run deterministic production-code tests for loss,
reordering, duplication, queue saturation and epoch changes. Then run native
latency-only, loss-only and bandwidth-only cases before combined constraints.
Use at least five seeds with 10 seconds warmup, 60 seconds active and 10 seconds
recovery. Restore the link explicitly in recovery experiments. Include moving
fire, key release, ramp/wall contact, robot contact, pause and placement changes.

Freeze numeric improvement/regression thresholds after the baseline and before
candidate runs. Always require bounded memory/queues, ownership enforcement and
once-only combat effects. Report missing samples and unresolved outcomes. A
50% loss profile tests degradation and recovery, not competitive playability.

Slices 1 through 4 are implemented in separate commits. Stage 5 is implemented
in the working tree; physics/interpolation tuning remains deferred. Run relevant checks per slice and
`just verify` before merging. Releases remain paused.
