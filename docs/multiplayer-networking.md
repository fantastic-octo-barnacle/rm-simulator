<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Networking priorities and experiments

Status checked 13 September 2026. Live protocol is 29; the runtime overview is
in the [README](../README.md#server-and-clients). The baseline and experimental proposals
below were written against `47f3a78`, protocol 17. They are historical where they
conflict with the implementation status here or in the [implementation record](networking-implementation-plan.md).

Local stats, execution feedback, owner anchors, paced traffic, acknowledged UDP
deltas, adaptive input delivery and scheduled firing are implemented. Remote
buffering and diagnostics are also implemented. Solver retention across live
rewinds is not enabled; multi-seed multiplayer acceptance remains outstanding.
Release packaging and publishing remain paused.

The companion [network harness and in-game stats design](network-harness-and-stats.md)
defines how to measure these experiments and expose useful diagnostics to players.
The [implementation plan](networking-implementation-plan.md) records implemented contracts, recovery behavior, historical proposals and
validation limits.
Earlier designs and detailed trial logs remain in git history. The
[shooter-view checkpoint](shooter-view-validation-checkpoint.md) is historical.

Protocol 22 removed two layers that no client produced or consumed: the
shooter-view compensated fire path (`CompensatedFire`, `ShotView` and
`InputEvidence`, the host's per-peer view archive and the shot admission worker)
and the ordered TCP snapshot delta chain. Firing is server-authoritative from
independent physics, and TCP now carries plain compact snapshots; the
acknowledged UDP baselines described below are unaffected. Sections that
describe either layer are historical.

Client prediction no longer runs reduced worlds. `Field::restore` rebuilds a
whole field from a snapshot plus the client's verified collision geometry, and
both own-chassis reconciliation and provisional shots replay it through the
ordinary stepping, command and fire paths. Passages below that describe a
separate chassis-prediction world, an isolated projectile query or a
reconstructed collision scene are historical.

Protocol 27 separates reliable armor-contact feedback from replaceable snapshots.
The app deduplicates contacts and gives their flashes a receipt-timed lifetime.
Mouse and assist sampling precede input/fire submission and prediction exchange;
camera placement follows the accepted motor pose. Shots flush current controls
even between the 16 ms input refreshes. Auto-aim acquires displayed robot poses
and solves impact from the latest authoritative motion, with explicit stale-state
and fire-gate status.

The default downstream LAN allowance is 512 KiB/s per peer. `RM_NET_PROFILE=limited`
selects 40 KiB/s downstream and 10 KiB/s upstream; the default LAN upstream
allowance is 64 KiB/s. Explicit byte-rate overrides take precedence. The pacer
permits one smaller packet to bypass a waiting class, then reserves its turn so
world fragments cannot starve. Host reports describe each class's queue age,
bytes and cumulative service. Automatic interpolation sheds delay at 50 ms/s,
while retaining the two-second jitter window and monotonic presentation time.
The [latency investigation](latency-investigation-2026-09-13.md) records the
original measurements and the implementation follow-up.

## Decision

Keep server authority, shared client physics prediction, and GameNetworkingSockets
UDP. Improve delivery, scheduling and reconciliation before considering another
transport or ownership model. Local movement, aim and provisional firing should
respond immediately. The server owns positions, cadence, ammunition, damage and
match results. A locally predicted hit can be overturned.

Bandwidth pressure, loss and delay interact. Oversized updates require more
packets, incomplete updates extend prediction, and stale queued traffic adds delay.
Measure these separately before tuning a combined poor-network profile.

At high latency, local controls can stay responsive while remote actions and
confirmed outcomes arrive late. At sustained 50% loss, target bounded degradation
and recovery; smooth, accurate competitive combat is not an acceptance promise.

## Implemented baseline

| Area | Protocol 17 behavior | Remaining weakness |
|---|---|---|
| Ownership | One host worker owns the live Simulation; clients predict their robot and projectiles | Remote collisions and restored solver state can disagree |
| Input | Complete controls about every 16 ms, current plus three prior samples in one compressed unreliable message | Redundancy covers only a short history; late transitions execute at receipt |
| Timing | Reliable probes, at most once per second; input lead is minimum recent RTT / 2 + 32 ms, capped at 150 ms | Retransmissions and asymmetric paths affect timing estimates |
| Input admission | Future limit 200 ms; fresh controls up to 250 ms late can apply; 250 ms stop lease | Client and server can execute the same transition at different ticks |
| State | Independent compressed full player checkpoints about every 32 ms, split into 1,000-byte application fragments | All fragments must arrive before even the owner's correction is usable |
| Shots | Exact aim, unique IDs, launch at receipt from current server pose; retry every 40 ms for up to 250 ms | Arrival timing changes muzzle position, launch time and cooldown eligibility |
| Prediction | Continuous local physics through checkpoint gaps up to one second; restore and replay unfinalized inputs | More extrapolation increases uncertainty; contacts can magnify small errors |
| Presentation | Remote interpolation automatic 32–250 ms or manual 0–250 ms, extrapolation at most 100 ms; small camera corrections blend | Larger buffers increase view age; smoothing does not fix simulation errors |
| Diagnostics | Connection toasts, console fields and a standalone rate/loss/latency harness | No in-game stats panel or shared host/client measurement schema yet |

Protocol 17 removes trails and impact markers, reconstructs mechanism armor poses,
and packs projectile position/velocity as f32. Server physics retains f64 and spin;
client projectile spin resets on checkpoints, so bounced trajectories can differ.
Chassis reconciliation values retain full precision. TCP keeps its ordered delta
codec and HTTP retains full diagnostics. Neither is the proposed UDP delta scheme.

In short one-player strafe/fire trials, compact traffic used about 36 KiB/s upstream
and 119 KiB/s downstream on the decent profile, versus 90 and 217.5 previously.
These are proxy-observed UDP payload rates before simulated loss, including GNS
traffic but excluding IP/UDP headers. They establish byte savings, not link-capacity
requirements or improved shot reliability. Poor-profile trials still had expired
shots. Different packet layouts do not receive identical loss histories with a
shared random seed. Full verification at this commit passed 323 tests.

## Experiments in priority order

### 1. Measure a reproducible baseline

Implement the common stats contract, player display and automated harness first.
Add directional bandwidth caps, finite queues and burst loss to actual UDP tests.
Record correction distance, input execution delay, shot outcomes and queue age
alongside bytes. Compare one healthy peer with an impaired peer on the same host.
The [companion design](network-harness-and-stats.md) specifies scenarios and gates.

### 2. Make owner corrections small and independently useful

Separate owner reconciliation, nearby dynamic state, and slower world/configuration
updates. Target one MTU-safe message for an owner correction before adding optional
state. Measure actual GNS packetization; a small application message alone does
not prove a one-datagram wire footprint.

Every update needs a session epoch, state tick and revision. Owner corrections
also need life revision, finalized tick and processed input identity. Entity
birth/removal and authoritative outcomes must survive loss through acknowledged
delivery or repeated state. An omitted entity does not mean a despawn.

Retain coherent collision context for owner replay. Associate nearby interacting
bodies and articulated scenery with an explicit context tick/revision; retain
bounded history and report stale or missing context. Never fabricate a same-tick
world by merging unrelated latest poses. If context is missing, apply the owner
anchor with bounded prediction and request context recovery independently of
distant state. Test robot-to-robot contacts before enabling partial replication.

Success means loss of optional state no longer blocks fresh owner corrections,
without increasing collision error or losing lifecycle transitions.

### 3. Budget and prioritize each connection

Use per-client byte budgets and pacing alongside GNS congestion control. Include
retries, acknowledgements and recovery traffic in measurements. Replace stale
unsent motion; do not grow queues to conceal overload. Prioritize controls, owner
corrections and combat outcomes, then nearby motion, then distant updates. Aging
must prevent lower-priority state from starving. Lower distant update frequency
before lowering owner input sampling or reconciliation accuracy.

Use separate GNS lanes where useful for reliable control/outcome traffic and bulk
recovery. Lane priority affects sending; it does not create cross-lane receive
order. Preserve existing command barriers explicitly: confirmation snapshots must
be applied before their Pong is exposed. Keep this pair on one ordered path or
add an application dependency gate. Valve documents the lane guarantees in
[ISteamNetworkingSockets](https://partner.steamgames.com/doc/api/ISteamnetworkingSockets#ConfigureConnectionLanes).

Add UDP deltas only against a baseline the receiver acknowledged decoding and
retaining. Use bounded per-connection baseline caches, explicit baseline IDs and
epoch resets. Specify retention until a safe retirement point; acknowledgements
must not refer to evicted state. Missing baselines trigger rate-limited independent
recovery. Lost acknowledgements may waste bytes but must not stall progression.
Encode after choosing the outbound update. Never use the TCP last-sent chain for
unordered UDP. This follows the acknowledged-baseline approach in
[Snapshot compression](https://gafferongames.com/post/snapshot_compression/).

Prototype binary encoding after measuring message-class costs and fragment counts.
Preserve chassis precision initially. Any later quantization requires error tests
on terrain, armor aiming and collisions, not just a smaller payload benchmark.

### 4. Adapt movement timing to delivered inputs

Use bounded, non-retransmitted timing probes; distinguish transport RTT from host
processing and reliable recovery time. Piggyback acknowledgements where practical.
Clock-offset and one-way-delay estimates remain uncertain on asymmetric paths.

Have the server report input arrival margin, actual execution tick and missing
intervals. Adapt the scheduling lead from these observations, with a capped jitter
allowance, hysteresis and a slow decrease after recovery. Keep ticks monotonic and
reset on pause/life changes. More lead means a longer local prediction horizon and
later visibility to peers; it does not remove latency.

Compare bounded input history lengths under random and burst loss. Repeat important
press/release transitions while useful, preserving immutable sequence contents.
Old duplicates cannot renew the stop lease. Explicitly reconcile late execution
and finalized missing intervals; do not replay them a second time at their intended
ticks. Keep the shared world advancing on its 1 ms clock without waiting for a peer.
Riot describes the buffering/prediction tradeoff in
[VALORANT's netcode](https://www.riotgames.com/en/news/peeking-valorants-netcode).

### 5. Schedule shots on the movement timeline

Add a bounded host-owned shot queue. Each intent has an immutable shot ID, intended
tick, exact aim and life/epoch identity. On-time shots execute at their tick using
the authoritative muzzle. Distinguish received, scheduled, executed and rejected
outcomes so receipt alone cannot confirm a projectile or consume client ammo twice.

The server enforces actual simulation-time cadence. Queue an eligible shot once;
network retry timing should not be the cooldown scheduler. Define a maximum
lateness and queue wait. Initially, eligible late shots can launch from current
authoritative state; expired ones reject. Never backdate ammunition, bypass cadence,
drain an accumulated firing burst or carry shots across pause, defeat or a new life.
Predict locally on the same intended timeline, then reconcile by shot ID.

Historical projectile catch-up is a separate later experiment. It would need
historical moving geometry over the flight interval, bounded CPU work, and explicit
damage-order policy. A single armor rewind is insufficient for physical bullets.
Do not reintroduce shooter-view hit guarantees as part of delivery improvements.

### 6. Diagnose physics error and adapt remote presentation

Compare predicted and authoritative robot states at the same tick before applying
visual smoothing. Measure translation, orientation, velocity and aim separately.
Classify observed context as late/missing input, static contact, moving-body contact,
stale collision context or unknown; these are clues, not proven causes.

Use verified client CAD and the shared physics functions. Do not introduce a
different movement model for the client. Restoring the whole field rather than a
reduced one removed the terrain and contact error on the measured traces; what
remains unanswered is whether checkpointing solver warm starts is worth its wire
cost, since `Field::restore` deliberately rebuilds them.

Implemented automatic and manual remote interpolation buffering controls in the F3 menu,
with bounded delay, reset to defaults and effective view-age/underrun readouts.
Automatic interpolation adapts to observed arrival variation and underruns.
Manual mode allows 0–250 ms. Extrapolation lasts at most 100 ms, then holds. Increasing the buffer makes remote
motion smoother but older. Keep own input immediate and measure the resulting aim
and hit disagreement. Larger camera offsets cannot substitute for reconciliation.

### Deferred experiments

Lower-rate distant projectile corrections need explicit spawn, removal and bounce
repair; initial position/velocity does not describe future collisions. Consider
small selective redundancy or forward error correction only after reducing
fragmentation and measuring spare capacity. Extra copies on a saturated link can
make queues worse. Session resumption and historical hit compensation each require
their own policy and lifecycle design.

## Delivery order and decision gates

| Stage | Deliverable | Gate before proceeding |
|---|---|---|
| A | Shared stats, in-game display, rate/burst harness | Measurements agree with controlled fixtures and reports expose missing data |
| B | Baseline runs preserving protocol 17 gameplay | Multiple seeds, useful durations, two players and actual CAD; identify any instrumentation-only protocol update |
| C | Owner correction separation and per-peer budgets | Optional loss cannot block the owner; no cross-tick collision corruption or healthy-peer regression |
| D | Acknowledged UDP deltas and recovery | Lost baseline/ACK/recovery tests pass; lower bytes without longer correction gaps |
| E | Adaptive input timing and scheduled firing | Less late execution and shot expiry; cadence, deduplication and lifecycle invariants hold |
| F | Contact/reconciliation and interpolation experiments | Same-tick errors and visible correction tails improve, with remote view age reported |

Make one behavioral change per comparison and retain a baseline switch where
practical. Fix correctness failures before interpreting performance results.
Choose numeric playability targets from the baseline, then freeze them before
evaluating a candidate. Treat severe profiles as recovery tests. Do not weaken
damage, cadence or ownership rules to improve success counts.

All stages preserve crate boundaries, bounded nonblocking frame-facing work and
the host's sole Simulation ownership. New wire contracts require versioning and
clear incompatibility errors. Run relevant tests while iterating and `just verify`
before merging. No release is authorized by this roadmap.
