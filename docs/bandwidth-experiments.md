<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Bandwidth experiments

The surviving experiment summary: the isolated trials behind the current
delivery contracts, their measured savings, and the outstanding bandwidth
target. Every rate, hash and revision id here is evidence for the named
revision, not a property of the current build.

Investigation and isolated experiments dated 13 September 2026, based on
`7d07f1c`. The proposals below describe the original baseline; the results retain
their measured experimental revisions.

## Integration status

Protocol 29 integrates the canonical experiment 0 attribution probe, experiment 1
owner configuration references (`8eb429d`) and experiment 7 input batch compaction
(`826b790`). Protocol 29 distinguishes the combined RMO4/RMI3 contract from both
incompatible experiment-only protocol 28 variants. Host and client must use
matching builds.

Cadence, baseline rotation, checkpoint representation, outcome recovery, numeric
precision and pacing defaults are unchanged. Experiments 2–6 are retained as
investigation records, not enabled production changes. The subsequent cadence and
deflate follow-up measures 64 ms world checkpoints with 32 ms owner updates and a
separate selected-stream deflate 1/4 sweep. Both remain experimental, with
production defaults unchanged and constrained-link acceptance failed. A later
ZSTD dictionary experiment adds a selectable wire codec (`RM_NET_CODEC`), leaves
the DEFLATE wire and the production default untouched, and measures a 30–58% cut
of the selected stream.

**Superseded by protocol 35.** The selectors this investigation introduced have
been removed: packed checkpoints with the trained dictionary are now the only
periodic encoding and plain ZSTD is the only codec for every other message, so
`RM_NET_CODEC`, `RM_NET_SNAPSHOT` and the effort-level overrides no longer
exist. The measurements below stand as the evidence for that choice; the
DEFLATE baselines they mention are no longer reproducible in the current build.

The subsequent binary and fixed-point experiment compares lossless binary
checkpoints, packed baseline deltas and several motion precision assumptions
against both compression baselines. It is a standalone prototype with round-trip
and physics replay measurements; it does not change the live wire or prediction
precision.

The isolated measurements below are not measurements of the combined build.
Experiment 1's short unimpaired UDP pair corroborates its bandwidth saving, but
does not complete the loss/blackout or multi-seed acceptance matrix. Experiment 7
has only in-process bandwidth evidence. NET-001 remains open; see
[known issues](../KNOWN_ISSUES.md). The canonical probe still needs populated
hit and shot-result workloads before drawing recovery-history conclusions.

The integration follow-up corrected event measurements, repeated the RTT trial
and ran targeted live trials; a subsequent wire-tag tracing fix followed.
Historical per-class trace counts before that
fix misclassified RMO4/RMI3 traffic as control; total byte counts are unaffected.

## Problem and budget

[NET-001](../KNOWN_ISSUES.md) records approximately 855 kbps downstream and
103 kbps upstream for one remote client moving/firing against a listen host.
Those are UDP proxy payload rates; the documented header allowance raises them
to about 882/115 kbps. The desired 100–200 kbps per-player budget requires a
roughly 4–9× downstream reduction. Different historical two-client trials cost
more; do not mix their rates with this baseline.

Use 200 kbps downstream including the stated network overhead as the first
experimental target, with 100 kbps as a stretch. The earlier investigation
proposes 50–64 kbps upstream; that is a separate engineering target, not a new
user requirement. Report directions separately and their sum so the reviewer
can judge an aggregate interpretation too.

At the current 31.25 Hz publication rate, 200 kbps allows only 800 bytes per
publication for *all* downstream traffic, including overhead. There is no
evidence yet that any single idea below achieves that target.

## Current path and costs

Paths below are relative to the repository root. Symbol names are navigation
anchors so the handoff remains usable after line numbers move.

| Area | Current behavior and implication |
|---|---|
| `crates/rm-simulator-server/src/host.rs`, `BROADCAST_PERIOD`, `publish_snapshot` | One worker owns the simulation and roster. Remote snapshots publish every 32 ms. Per-peer outboxes replace unsent periodic snapshots, preserving reliable messages and confirmation order. |
| `net.rs`; `host.rs`, `Outbound`; `gns_transport.rs` | Local play and remote play share the per-peer UDP codec: the in-process owner runs it over two datagram channels with compression skipped, and GNS drives it over sockets for everyone else. Only the socket and ZSTD differ, so local overhead is not a separate budget. |
| `udp_codec.rs`, `PeerCodec::send` | A periodic pilot snapshot produces both an owner anchor and a world checkpoint. Native pending bytes above 64 KiB skip world production but still produce the anchor. Nonperiodic confirmations take the full independent path. |
| `owner_stream.rs`, `OwnerAnchor::encode` | The `RMO4` anchor is binary and names its chassis configuration by an acknowledged 8-byte `ConfigRevision` instead of repeating it; the earlier `RMO3` layout repeated a deflated JSON configuration in every anchor and is the pre-experiment baseline. The anchor includes 29 f64 values and five f64 values per wheel. With four wheels, the `RMO3` layout is about 438 bytes before compressed configuration: about 109.5 kbps at 31.25 Hz before native/network overhead, by source arithmetic, not measurement. `RMO5` (protocol 37) quantized the dynamics to 202 bytes and `RMO6` (protocol 40) packs both quaternions smallest-three for 198. |
| `snapshot_codec.rs`, `PlayerSnapshot` | Compact checkpoints already omit and rebuild the field clock (`tick * tick_ns()`), the rune, outpost and referee views (from the restore, falling back to explicit views when they disagree), rune target and outpost armor poses, wheel hubs and tyre targets (only spin travels) and wheel contacts; projectile position/velocity use f32 and spin is omitted. Every absolute timestamp is a whole-tick code (0 = the clock's current time) with an exact sub-tick remainder list; protocol 39 measured checkpoint-relative ages larger in every workload because they change every frame. Since protocol 40 the field path implies each chassis/projectile fixed-point grid (no grid index on the wire), a delta sends an exponential-Golomb step difference (order per grid, picked on the training workloads; it beat a gamma-coded width header by 5% and the old 6-bit width by 7% in raw delta bytes, and dropping the per-component changed bit measured 0.8% larger), and chassis pose/turret rotations are smallest-three at 15 bits (2 + 45 bits against 4 × 16). Since protocol 41 the frame is laid out by change rate: byte-aligned slow records (header, shot results, hits, bases, rules with each rune and outpost aligned, chassis identity/configuration/command) come first and the dense chassis motion and projectiles after, and a chassis configuration or projectile policy equal to its preset travels as a variant index (a configuration otherwise costs about 123 packed bytes). Emulated from the packed tree on the `network_bandwidth` states, with a full-frame dictionary retrained per candidate (mean independent bytes per frame for 0/2/12 players/firing): the protocol 40 layout 35/206/526/499; presets without reordering 34/311/501/602, worse because the one-bit index shifts the bit phase of everything after it; presets plus slow-first reordering without alignment 36/141/444/433; the same with byte alignment 34/146/410/438; a byte-aligned slow body compressed apart from a raw fast body 35/187/948/512. The aligned single stream was taken. Preset index was chosen over a configuration sent once per epoch on the reliable lane because it needs no new acknowledged state and keeps an independent checkpoint decodable alone. Other restoration values retain f64. Failed-contact diagnostics are filtered. Do not propose these existing reductions as new work. |
| `udp_snapshot.rs`, `Encoder::snapshot` | Acknowledged-baseline patches reference a pinned baseline, never the last transmitted revision. Each candidate is compared with an independent compressed alternative. Rotation becomes eligible after 32 encoded frames; proposal/retirement acknowledgements constrain actual rotation. At most two receiver baselines stay pinned. Since protocol 32 the production path encodes those checkpoints as bitpacked fine fixed point with the embedded ZSTD dictionary, and since protocol 35 that is the only periodic encoding any more: the `RM_NET_SNAPSHOT=json` deflated-JSON comparison path was removed with the selector. Since protocol 41 a packed delta under 128 bytes (`DELTA_COMPRESSION_MIN_BYTES`) is not offered to ZSTD: no delta that small shrank in any workload (idle deltas average 12 bytes, two drivers 61), 128–256-byte firing deltas did not shrink either, and twelve players' 300-byte deltas shrink about 29%. The skipped attempts cost about 0.4 µs idle and 2.5 µs with two drivers per frame. The dictionary is trained on independent frames and only the deltas that are compressed; against training on every delta it measured 0.3–1.5% smaller independent frames. |
| `snapshot_codec.rs`, `difference` | Arrays get element patches only when their lengths match. Spawn/despawn and changing history lengths can replace whole arrays. Equal-length insertion/removal can also shift identities and amplify patches. |
| `simulation.rs`, `Simulation::state`; `host.rs`, `observe_simulation` | Every publication includes up to 32 recent shot results per shooter. Registered hits also have a reliable event path while snapshot history supplies recovery. Repetition is a candidate cost, not proof recovery data can safely be deleted. |
| `udp_codec.rs`, `select_inputs`, `input_batch`, `ClientCodec::submit` | Input is already packed and deflated: 80 bytes/frame before compression. Batches retain four newest samples plus selected movement transitions within the 250 ms useful window, up to 12 frames. Aim/fire has its own unreliable retry path; other control is reliable. |
| `pacing.rs`, `Pacer` | Bounded control, replaceable owner and world transfers share byte pacing with round-robin service. A started world transfer can finish while a newer pending one is coalesced. Pacing alone cannot make an oversized offered stream fit. |
| `udp_codec.rs`, `packets`, `Frames` | World messages use 1000-byte chunks plus 21-byte application fragment headers. Incomplete unreliable frames expire after 250 ms. Losing one fragment can waste the rest of that checkpoint, so report fragments and usable checkpoints as well as byte savings. |
| `crates/rm-simulator-app/src/session.rs` | Owner anchors aid correction, but full snapshots supply coherent world context. Aim assist requires a full target checkpoint within the last 300 ms and never fires on stale observation; owner anchors cannot extend that freshness. Lower checkpoint rates affect both prediction and remote presentation. |

## Experiment plan

Run each candidate alone against the same baseline before combining winners.
Prefer lossless format changes first; precision changes need a separate review.
The executed outcomes for all eight are in [Results](#results-executed-13-september-2026).

### 0. Attribute the bytes before implementing reductions

Use the existing encoding counters and control/owner/world pacer counters
described in [network tracing](network-tracing.md). Record counter deltas over
the actual active interval: raw world bytes, independent versus selected
compressed bytes, framed world bytes, owner bytes, produced update counts,
sent/queued bytes by class, oldest queued age, skipped/replaced/expired updates.
Compare these with proxy offered/forwarded bytes and native rates. Do not sum
production, submission and receipt as though they were separate traffic.

Add local-only attribution if needed for projectile arrays, chassis/config,
shot-result/hit histories, baseline proposals and reliable confirmations.
Raw JSON section sizes identify candidates but do not add up to compressed
contributions; use controlled ablation in an offline probe to estimate those.

[`examples/network_bandwidth.rs`](../crates/rm-simulator-server/examples/network_bandwidth.rs)
is a useful codec microbenchmark, but its loop
steps 16 ms, assumes immediate baseline feedback, and omits live owner anchors,
control, pacing and native overhead. Its separate printed compressed JSON rate
is not the complete live player path. Extend a probe to drive the production
codec at 32 ms under scripted feedback before using it to predict savings.

### 1. Stop repeating owner configuration

**Hypothesis:** owner anchors consume a substantial fixed share before world
data is considered. In `owner_stream.rs` and the codec, prototype a versioned
configuration reference established through acknowledged reliable delivery or
an acknowledged full checkpoint. Keep numeric dynamics lossless initially.

Measure configuration bytes/update and the resulting total downstream rate.
Cache configuration by an explicit immutable identity/revision; never infer
availability from having sent it. Unknown references must defer the anchor and
recover through a complete checkpoint. Test join, loss of setup, respawn,
placement changes, epoch reset and reconnect. Retain the one-datagram bound.

This alone cannot hit the budget: the roughly 438-byte numeric/header portion
already consumes about half the 200 kbps allowance at current cadence.

### 2. Make world deltas follow entity identities

**Hypothesis:** projectile creation/destruction and rolling histories defeat
index-based JSON patches during sustained firing. Prototype keyed collection
patches with explicit add/update/remove operations for projectiles, chassis and
identified events/results. Preserve required logical ordering when rebuilding
the snapshot; identity-based wire storage must not reorder physics inputs or
contact history accidentally.

Start with lossless values and existing acknowledged-baseline ownership. Compare
compressed selected bytes, array replacements and fragments/update for idle,
movement, and sustained firing at two and twelve players. Test middle deletion,
simultaneous births/deaths, reordered/duplicate packets, stale baselines and
epoch changes. Decode into the same complete restoration contract.

### 3. Replace verbose checkpoint representation losslessly

**Hypothesis:** fixed schema binary fields, presence/change masks and shared
configuration references outperform deflated JSON patches. Prototype one hot
section first, selected by experiment 0, then compare binary+deflate against
the existing encoder. Preserve f64 dynamics and bounded decoding. Derive only
fields whose reconstruction is demonstrably equivalent; audit `Field::restore`
and consumers before excluding anything.

Measure final compressed/framed bytes and encode/decode CPU, not raw JSON size.
Avoid a new dependency unless the measured result warrants it. Separately sweep
the existing compressor's effort if useful; CPU cost and final byte savings
determine whether that is worthwhile.

### 4. Tune acknowledged baseline rotation

**Hypothesis:** the nominal 32-frame baseline lifetime leaves large dynamic
patches, while more frequent proposals may cost more full frames and feedback.
Sweep 8/16/32/64 encoded frames and, only after that, a size-triggered rotation
policy. Count proposal/retry/retirement bytes as well as delta bytes.

Run with delayed/lost acknowledgements, reordering and blackouts. Preserve the
two-baseline bound, retirement acknowledgement, epoch checks and independent
fallback. Reject a change whose attractive lossless-LAN result becomes excess
full-frame traffic under loss. This is a tuning hypothesis, not a promised win.

### 5. Separate cadence experiments from encoding

First try owner/world at current/current, current/15.625 Hz and 15.625/15.625 Hz
as isolated prototypes. Measure bytes alongside same-tick correction error,
replay CPU, context age/gaps and remote underruns. A lower shared host broadcast
rate is an easy diagnostic but slows both streams; independent rates require
explicit transport scheduling.

If substantial savings still require 5–10 Hz complete checkpoints, investigate
a compact remote-motion presentation stream alongside frequent owner updates.
That is a larger app/protocol change. It must still restore and replay the whole
field from coherent checkpoints; presentation-only packets cannot silently
become complete context. The 300 ms context limit leaves little loss margin at
5 Hz. Do not simply relax that limit to make a test pass.

### 6. Reduce repeated outcome recovery data carefully

After measuring its share, prototype acknowledgement/cursor-based outcome
recovery or stable-ID deltas for repeated shot-result and hit histories.
Keep live reliable hit/result delivery and deduplication. Verify reconnect,
late join, blackout, dropped acknowledgements and old-life outcomes; an event
must not disappear merely because a snapshot carrying it was produced.
Prefer identity deltas first because deleting history changes recovery semantics.

### 7. Compact upstream repetition, then retest pacing

Prototype one batch header for shared chassis/epoch/placement identity plus
relative sequence/time fields and exact changed-value masks. Retain newest
samples and press/release transition redundancy initially. Test rapid reversals,
aim sweeps, movement with fire, lost release and the stop lease. Only then sweep
sample/redundancy rates; lowering them can change shot timing and input quality.

Once offered traffic fits, tune pacing below measured path capacity with space
for native framing/retries and preserve service for complete checkpoints.
`RM_NET_UP_KIB_S`/`RM_NET_DOWN_KIB_S` are KiB/s application budgets, not kbps
on-wire limits. For example, 200 kbps is only about 24.4 KiB/s before overhead.
Do not advertise meeting NET-001 by forcing the proxy to drop excess bytes.

## Validation handoff

Build matching baseline/candidate binaries before timing. Record revision,
dirty diff, binary hashes, protocol version, CAD manifest hashes, settings,
platform, seeds and workload. Change protocol compatibility when changing wire
contracts. Keep raw captures and generated reports outside Git.

Use the existing scenario JSON under `scripts/network-scenarios/` and the console
tracing described in [network tracing](network-tracing.md)
as starting points. The smoke scenarios are functionality checks, with loose
gap/underrun gates and some missing metrics skipped; passing them does not prove
playability or meeting the bandwidth budget. Author stricter experiment scenarios.

- Workloads: idle/paused; driving and aiming; sustained firing; ramps/walls and
  robot contact; join/leave/respawn. Measure one remote player, two real clients
  with a healthy control, then twelve-player load with client type stated.
- Links: unlimited baseline; 200 kbps downstream cap, then 100 kbps; separately
  test upstream limits; add delay/jitter, 1/5/10% loss, reordering, and 0.5/1/2 s
  blackouts. Include restored capacity and shared server-egress contention.
- Run at least 10 s warmup, 60 s active, 10 s recovery over five seeds for
  acceptance candidates. Report per-peer and total host rates, means and burst
  windows; separate initial synchronization and recovery bursts from steady state.
- Track complete-context gaps/age independently of owner freshness; correction
  distributions, remote underruns, command lateness, shot confirmations, unresolved
  outcomes, queues/drops, recovery time and CPU/frame cost. Freeze regression
  tolerances against the baseline before accepting a candidate.
- Correctness: bounded queues/baselines, full restoration and deterministic tick
  stepping, no duplicate shots/damage, no cross-life input, ownership enforcement,
  eventual stop after lost release, ordered reliable outcomes and full confirmation
  before Pong. Use deterministic codec/link and app harness tests where console
  diagnostics cannot assert these properties. Do not weaken known failing gates.

Useful entry points (the microbenchmark is not an acceptance test):

```sh
cargo run -p rm-simulator-server --example network_bandwidth --locked --release
just network-test
python3 scripts/network-harness.py --help
just network-trial scripts/network-scenarios/smoke-single.json --output /tmp/rm-bandwidth-smoke
cargo test -p rm-simulator-server --locked
cargo test -p rm-simulator-app --locked
```

Select built binaries explicitly with harness `--server` and `--app`, and use
`--baseline` for matched comparisons. Run targeted restoration/physics tests when
changing snapshot precision or reconstruction, and `just verify` before any PR.
Compare tracing enabled/disabled for shortlisted candidates. Visual/manual play
review remains necessary for presentation; launch confirmation is not impact
latency or click-to-visible-hit latency.

Recommended first implementation pair: instrument missing attribution, then
test owner configuration references and identity-based projectile/history deltas
separately. Use their measured residual cost to decide whether a broader binary
checkpoint format or new presentation stream is justified.

Defer lossy chassis/aim quantization until lossless experiments establish the
remaining gap. If tested, isolate f32 or fixed-point precision as its own
candidate with explicit position, angle and contact/scoring error bounds across
long replay, ramps and robot collisions. Exact equivalence cannot be assumed.

## Results (executed 13 September 2026)

Every experiment in the order above was run in an isolated worktree on its own
branch (`perf/bw-exp0` … `perf/bw-exp7`). The execution log, the measurement
instrument and the harness hazards found while running are retained in Git
history. Headline outcomes:

| # | Experiment | Outcome |
|---|---|---|
| 0 | Attribute the bytes | Remote pilot, one field: 368 kbps idle → 454 driving → 741 firing → 955 with twelve chassis. The owner anchor is a **fixed 211.8 kbps** (846 B at 31.25 Hz, 408 B of it the repeated `ChassisConfig`) and dominates idle; the world checkpoint dominates once the field moves (528.6 of 741 kbps when firing). Compressed section shares: `chassis` 1097 B/frame, `projectiles` 520, `restore` 386. |
| 1 | Stop repeating owner configuration | **Win.** Referencing the configuration by an acknowledged identity takes the anchor 846 → 444 B and the owner stream 211.8 → 110.8 kbps, about −95…−100 kbps on every remote workload. A matched real-UDP harness pair measured 707.3 → 612.6 kbps downstream (−13.4%). Residual anchor ≈ 111 kbps, so this is necessary but not sufficient. |
| 2 | Identity-based world deltas | **Correct but never selected.** Lossless keyed patches were built and verified for chassis, projectiles, hits and shot results, but the size guard selected them **0 times in 44,597 array decisions**: moving values make every projectile an update (identity plus header repeated per ball), and the small chassis array already wins on index patches. 0 bytes, 0 kbps in every workload. |
| 3 | Lossless binary checkpoint | **No standalone win.** A binary `state.chassis` section cuts the independent frame 12–21% but the *selected* acknowledged-delta stream only 1.2–3.8%, because the deltas already avoid re-sending the chassis. Found and fixed a float-widening defect that had inflated one capture by 30%. Separately, raising the existing deflate level from 1 to 4 cuts the independent frame 12–14% for negligible CPU and deserves its own measurement on the selected stream. |
| 4 | Baseline rotation sweep | **Keep 32.** Lifetimes 8/16/32/64 aggregate to 333.7/320.7/311.5/306.9 kB/s over the four workloads; 32 wins the payload-dominated `twelve` (955.1 kbps) and `fire`, and 64's small idle/drive gain is outweighed. A size-triggered policy was rejected: 70% of the time it never fires, and at 55/45% it thrashes `fire` by +38/+50%. Under loss no setting becomes excess full-frame traffic (delivered bytes 1.008–1.057x the same lifetime's lossless total; a 0.5 s blackout stalls every lifetime for 544 ms). |
| 5 | Separate cadence from encoding | **Largest measured lever.** 31.25/15.625 Hz gives −20.5…−36.8%; 15.625/15.625 gives ≈ −49%. Decoupling to 62.5/15.625 is a *loss* until the anchor shrinks. At 15.625 Hz the complete-context gap is 64 ms nominal and three consecutive lost checkpoints (256 ms) still fit the 300 ms limit; the fourth exceeds it. |
| 6 | Reduce repeated outcome recovery | **Leave it in place.** The share is zero on the canonical workloads (they record no shot results and land no hits). In a populated scenario the two collections cost 1015 B/frame ≈ 20% and identity-keyed splices save 6.6%, which does not move NET-001 while owner anchors and `state.chassis` dominate. |
| 7 | Compact upstream repetition | **Win.** A versioned RMI3 batch (shared header, exact changed-value masks, LEB128 relative fields, exact fallback) cuts upstream 22–27%: 36.5 → 27.0 kbps idle, 73.4 → 56.6 driving, 76.4 → 59.6 firing, losslessly. After compaction the offered upstream fits the existing 10 KiB/s `limited` budget, so **no pacing change is justified**. |
| 8 | ZSTD with a trained dictionary | **Win, shipped in protocol 35.** Measured behind a then-selectable wire codec (`RM_NET_CODEC`) that left the DEFLATE wire unchanged; that selector is gone and the dictionary is now the production codec. Plain ZSTD cut the selected stream by up to 38%; a 32 KiB trained dictionary cuts the selected acknowledged-delta stream 30–58% and independent envelopes 58–82% against `deflate-1`, halves fragments on `drive` and cuts them 26% on `fire`, and lowers CPU in both directions. Out of sample: a leave-one-out dictionary is only 8–22% better. |

### What the results say about the target

No single experiment reaches the 100–200 kbps target.
The owner stream alone is 111 kbps after experiment 1, and the world stream is
still 156–743 kbps. The measured levers compose — experiment 1 (anchor) plus
experiment 5 (cadence) would put the owner near 55 kbps at 15.625 Hz on top of a
halved world stream — but that combination has not been measured, and
experiments 3 and 6 show that re-encoding the world checkpoint is not where the
remaining bytes are. Before more format work, the next measurements should be:

1. the experiment 1 + 5 composition, since it is the only pair with measured
   wins in different streams;
2. the deflate level 1 → 4 change on the **selected** stream (experiment 3's
   sweep measured it only on the independent frame) — **measured by experiment
   8**: level 4 cuts the selected stream 10–37% (idle −10%, drive −17%,
   twelve −37%, fire −14%) at higher codec CPU, and the
   dictionary codec beats it everywhere;
3. a populated firing scenario for the canonical probe, because no canonical
   workload records a `ShotResult` or lands a hit, so `shot_results` and
   `state.hits` are empty and every conclusion about outcome history rests on a
   synthetic counterpart.

Experiment 8 also supplies the next candidate for a harness pair: a
dictionary-trained ZSTD is the largest single codec win measured on the
*selected* stream, and its fragment reduction (999 → 509 on `twelve`) can only
be validated under loss. That recommendation was taken: since protocol 35 the
dictionary is the production codec for periodic checkpoints and plain ZSTD (not
DEFLATE) carries every other message.

### Coordination hazards

Two hazards are worth carrying forward: a shared
`CARGO_TARGET_DIR` lets a concurrent `cargo test` silently run another
worktree's artifact, and `git stash` is repository-global across worktrees.
