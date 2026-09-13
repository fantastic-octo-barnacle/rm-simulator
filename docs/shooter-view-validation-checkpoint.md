<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Shooter-view validation checkpoint

Historical design and validation record for commit `817eadd`, September 2026.
The [networking roadmap](multiplayer-networking.md) supersedes this design.
Current runtime behavior uses independent server projectiles and client prediction;
it no longer follows this checkpoint's shooter-view hit policy. All implementation
claims below describe the historical protocol 15 build.

Status: implemented development checkpoint. Release packaging and
publishing remain paused. Protocol 15 implements default local prediction,
refreshable held controls, exact-aim shot IDs, admitted view reconstruction and
isolated projectile validation. The host still owns every gameplay mutation.

The implemented input policy uses the latest valid held state with a 250 ms lease.
`duration_ticks` describes a sample; it does not grant simulation ticks or make the
server wait for a contiguous stream. The fixed-duration scheduling proposal below
is retained as a possible follow-up, not a claim about the current protocol.
Reconciliation currently corrects the body directly; a separate camera correction
offset is also a follow-up. Neither is required to validate the exact predicted
body and projectile query submitted by this implementation.

Current limits are 200 ms chassis replay, 500 ms configurable view admission,
1 second / 128 snapshots per peer, 64 active host queries, 64 queued requests,
256 queued results, and 64 ms between flight view samples. Jobs retain shared CAD
shapes rather than copying triangles. Missing evidence or exceeding a bound ends
prediction explicitly. Cross-platform numerical agreement and subjective play
quality under 150–200 ms one-way delay with 50% loss still need playtesting.

## Revised direction after playtesting

This checkpoint implements the shooter-view validation policy described below.
After testing the impaired connection, the user chose responsiveness over ensuring
that every locally visible hit is honored. The next refactor should keep a client
simulation advancing independently, use authoritative snapshots to reconcile it,
and allow provisional impacts to be corrected when the server disagrees.

Local chassis and projectile prediction already exist. The remaining change is
to make prediction continuous across snapshot gaps and remove the reliable
per-frame shooter-view evidence stream from the firing path. The server should
simulate accepted shots independently and own final damage, ammunition and match
state. Reconciliation must restore sufficient physics state and replay unacknowledged
inputs; visual corrections should not change collision state or duplicate shots.
This revised policy is recorded here but is not implemented by this checkpoint.

## Product decision for this checkpoint

Movement, gimbal aim and firing should respond immediately on the owning client.
Within a bounded compensation window, the server should honor a valid projectile
hit against the scene the shooter was shown. Other players may see damage after
moving away or reaching cover. This is the chosen tradeoff.

A visible projectile contact is provisional until validation. Definite hit
markers, damage and deaths follow server confirmation. We cannot selectively delay
only shots that will disagree with the server: that disagreement is not known at
trigger time. During an outage, show pending results and stale state explicitly;
do not turn an unverified prediction into confirmed damage.

The contract is bounded agreement with an allowed, reproducible client view.
It is not unconditional acceptance of a client-reported hit. A timestamp alone
cannot prove what a player saw, and arbitrary client geometry is never authoritative.

## Baseline before this refactor

- `host.rs` owns each live simulation and roster on one worker. Preserve this.
- GNS periodic snapshots are independent compressed frames. Commands, including
  movement and aim, currently share reliable ordered delivery. Loss can still
  stall fresh controls behind earlier commands.
- `prediction.rs` already restores chassis physics and replays numbered inputs.
  Prediction is opt-in and limited to 200 ms. Inputs currently change held
  commands; their acknowledgement is not a complete fixed-duration input protocol.
- The app displays remote chassis through a 64 ms interpolation buffer. On
  underrun it holds an endpoint. Mechanisms use analytic presentation motion.
  Those displayed poses can differ from historical authoritative poses.
- `Fire` uses the server's muzzle at request application time. `FireTiming` is
  diagnostic, not a validated shot contract. Ordinary fire requests also incur
  full confirmation snapshots through `Session::apply`.
- Projectiles have finite speed, drag, gravity, collision scoring and bounces.
  Their maximum flight is currently four simulation seconds. Rewinding armor
  once at launch does not reproduce collisions throughout flight.

## Ownership and module boundaries

| Component | Responsibility |
| --- | --- |
| World crate | Deterministic movement, gimbal, projectile stepping and collision/scoring primitives; explicit ticks only |
| Server host worker | Input admission, authoritative movement, shot acceptance, ammo/cadence checks, final damage and event ordering |
| Server prediction/view modules | Bevy-free input replay and shared reconstruction of displayed collision poses |
| Historical query worker | Evaluate immutable shot jobs against immutable collision history; return proposals, never mutate the live world |
| App | Input sampling, prediction orchestration, pending shot visuals, camera correction and feedback |
| Render crate | Apply caller-provided scene state; no rule simulation or networking |
| Transport | Deliver typed inputs, snapshots and results with bounded queues and explicit delivery classes |

The client simulates its own chassis and provisional projectiles. Remote bodies
participate as reconstructed collision context, but the client does not own their
movement, HP or match state. Full-world rollback is outside this refactor.

## Time and identity contracts

Use explicit simulation ticks and distinguish three times:

1. Input time: the tick assigned to a movement/aim command in the admitted input timeline.
2. View time: the scene time and snapshot references used to display remote objects.
3. Arrival time: when the server receives the message; used for admission bounds,
   never silently substituted for the intended aim sample.

Use a session epoch, entity life/placement revision, input sequence, shot ID and
snapshot ID. Snapshot IDs must be available above transport framing and identify
full reconstructed states, including TCP-decoded states. A paused world can have
several revisions at the same tick.

Clock estimates are measured by the server and bounded. Validate monotonic input
progress, elapsed-time budget, future timestamps and stale references. Never let
a client advance physics faster by submitting extra ticks. Pauses and resets
invalidate the affected prediction/history intervals.

Provisional tuning values, to be measured rather than treated as rules:

| Limit | Initial experiment |
| --- | --- |
| Movement input sampling | 60 Hz, with explicit durations in 1 ms ticks |
| Input redundancy | Current command plus the previous three commands |
| Missing input wait | Small bounded jitter buffer, initially at most 32 ms |
| Held-input lease without fresh packets | 250 ms, then neutral drive and released trigger |
| Maximum compensated view age | 500 ms, including interpolation delay |
| General collision-history retention | 1 second, subject to memory measurements |
| Projectile lifetime | Preserve the current four-second limit |

History age is checked when evidence is admitted. Once admitted, references
needed by an active shot must be pinned or copied into its bounded job history
until completion. A rolling 500 ms buffer alone cannot support four-second shots.
Set hard per-peer and global byte/work limits after the prototype measurements;
never retain an unbounded number of shot histories.

## Movement and gimbal reconciliation

Define `InputFrame` with epoch, sequence, start tick, duration and complete drive
and aim intent. The server admits a contiguous processed timeline and acknowledges
both input sequence and the resulting simulation tick in an authoritative snapshot.

Fresh input packets use unreliable delivery with bounded redundancy and refresh
even when controls do not change. Duplicates are harmless. Wait briefly for missing
inputs, then use the declared held-input policy; record substitutions in the
acknowledgement. Inputs arriving after their interval is finalized do not secretly
rewrite live physics. Discrete fire events retain separate deduplication/retry.

A packet refresh cannot perpetually preserve an old held command: track the latest
admitted sequence/time as well as packet arrival. Server input-time admission and
the lease policy must be tested together with the 150–200 ms one-way scenario.

The client restores the acknowledged authoritative chassis state and replays only
unprocessed input intervals. Correct collision state immediately. Smooth a separate
render/camera offset for small discrepancies; reset it on placement, defeat, scene
change or large correction. Never smooth the physics body through a wall.

Use the same gimbal constraints and muzzle transform on client and server. Keep
local aim responsive while reconciliation corrects the chassis underneath it.
Do not claim bit-identical replay across platforms: solver state and unknown robot
contacts already limit the existing replay. Measure errors against armor dimensions.

## Shot admission and muzzle validation

Replace diagnostic-only firing with a versioned `ShotIntent` containing:

- Session epoch, shooter life revision and unique shot ID.
- Input sequence, trigger tick and exact sampled aim.
- Authoritative snapshot baseline plus bounded input references needed to
  reconstruct the predicted shooter pose.
- Initial view descriptor for target and obstacle presentation.

A shot is self-contained enough to resolve aim even if the unreliable input packet
was lost. It cannot apply duplicate movement or invent a missing duration. A bounded
wait may resolve missing prerequisites; otherwise return an explicit rejection.
Later input must never change an already accepted shot's aim.

Reconstruct the shooter's predicted pose from an authorized baseline and admitted
inputs. Validate its discrepancy from the authoritative trajectory and legal
geometry. For small valid discrepancies, the compensated query may use this
reconstructed predicted muzzle. This is an explicit shooter-favoring allowance;
never accept an arbitrary muzzle transform from the client. Large corrections,
invalid wall crossings or unavailable history produce a reasoned rejection.

The host admits shots exactly once, reserves ammunition and checks cadence on a
monotonic, bounded shot timeline. Retransmitted or reordered intents cannot create
bursts, duplicate projectiles or extra ammo consumption. Preserve existing weapon,
defeat and role rules.

## Reconstructing the shooter's view through projectile flight

Extract a shared, Bevy-free presentation sampler. It must reproduce chassis
interpolation, endpoint holds, analytic mechanism motion, collision-relevant
configuration and discontinuity handling. The app must use its output both for
visible target poses and provisional projectile collision proxies.

A view descriptor identifies permitted snapshot endpoints, interpolation fraction,
mechanism evaluation time and any endpoint hold. The server retains the states it
sent and checks those references. A client can report which snapshots arrived;
it cannot refer to states never sent to its session or select an arbitrarily old
advantageous pose. Require bounded age and monotonic view progression, allowing
explicit pause/reset epochs. This limits claims; it does not prove actual pixels.

During a shot, send compact view-timeline segments whenever reconstruction changes.
Include those segments in bounded, retried shot evidence so validation does not
assume that all snapshots arrived. Evidence covers the path, not only the frame
of a claimed impact. Interpolation delay changes and snapshot underruns are part
of this timeline. Missing evidence produces pending or rejected validation, not a
fallback query against a different scene that is reported as an ordinary miss.

The historical query replays projectile age using the same flight and collision
primitives. At each collision interval it samples obstacles and armor from the
validated view timeline. Targets continue moving during flight; never freeze them
at trigger time. Test the earliest obstruction, non-scoring body collisions,
armor face orientation, impact-speed rules and bounces, not just a named plate.
The client may submit an impact candidate, but the server derives the result.

Compensated trajectories are independent queries; do not also let a live-world
copy apply damage. Remote projectile visuals can follow the server's accepted
trajectory updates, but only one host-committed impact event scores. Define how
existing bounce and impact effects are shared without maintaining two damage paths.

The launch-only historical policy described in the earlier investigation remains
a diagnostic comparison, not the selected production policy.

## Results, late damage and lifecycle rules

Use explicit `ShotAccepted`, `ShotRejected` and `ShotResolved` events keyed by
shot ID, with accepted muzzle/timing and any authoritative impact details.
The client matches its provisional projectile to that ID. Ordinary firing no
longer needs a full snapshot/Pong barrier; retain barriers for screenshots,
operator tools and operations that explicitly require confirmed state.

A query worker returns a result with its original session, entity revisions and
admission order. The host alone commits it, checks that it is still applicable,
and deduplicates it. Resolve simultaneous damage in a documented host order,
independent of worker completion races, using a bounded completion deadline.

Damage is committed now, not by rewinding the entire match. A historically valid
hit may affect a robot that has since moved behind cover. Never transfer damage
to a respawned/reset life or resurrect an expired rune scoring opportunity.
Record historical collision validity separately from current damage applicability.
The first implementation preserves current referee application semantics; changing
historical buffs or match outcomes would require a separate gameplay decision.

## Delivery plan and acceptance gates

| Step | Work | Gate before proceeding |
| --- | --- | --- |
| 1. Diagnostics | Record input age, acknowledgement tick, prediction error, view descriptors, snapshot gaps and shot result latency | Reproduce direct, delayed and severe-loss runs with attributable measurements |
| 2. Projectile/view prototype | One shooter, one moving plate and one blocker; shared presentation sampler and immutable flight query | Client/server collision agreement through interpolation, endpoint holds and multi-frame flight; measured history and evidence costs |
| 3. Input protocol | Versioned input durations, acknowledgements, redundancy, leases and exact shot/input binding | Lost stop, duplicate inputs, missing prerequisites, aim/fire reorder and time-budget abuse all have deterministic outcomes |
| 4. Local prediction | Reuse and extend existing replay; correct physics separately from visual smoothing | Flat terrain, ramps, robot contacts, gimbal motion, reset and defeat reconcile without stale worker results |
| 5. Shot lifecycle | Immediate provisional projectile, unique IDs, ammo/cadence admission and compact reliable results | Duplicate delivery cannot duplicate shots or damage; late rejection removes the provisional effect |
| 6. Full compensation | Historical view evidence, armor and blocking geometry, worker result commit | Earliest valid collision matches the permitted shooter view and respects lifecycle/referee boundaries |
| 7. Default rollout | Enable prediction and compensation only after gates; retain diagnostic uncompensated mode | Cross-platform and network trials pass, with memory/CPU limits and unresolved mismatch rates recorded |

Step 2 is deliberately early. If reconstructing the permitted view through slow
projectile flight is too expensive or still disagrees visibly, revise that design
before spreading new protocol assumptions across the app and host.

## Validation matrix

- Zero impairment, 50/100/200 ms RTT, and separately 150–200 ms delay each way.
- 0/5/20/50% independent packet loss; burst loss, duplication and reordering.
- Multiple moving robots and sustained fire; moving shield, spinning armor,
  terrain obstruction, non-scoring impacts and bounce-before-hit.
- Hold/release controls, aim then fire in one sample, fire then turn, disconnect,
  reconnect, pause, placement, defeat, respawn and history expiry.
- Invalid/future/old timestamps, impossible muzzle, skipped evidence, unsent
  snapshot references, duplicate shot IDs and exhausted work/memory budgets.
- Long projectile flights crossing history eviction, view underruns and target
  lifecycle changes. Test slow shots beyond 500 ms, not only close-range impacts.

For valid deterministic fixtures, require exact shot identity and scoring result,
and no duplicate damage. Establish positional tolerances from detector geometry,
not arbitrary large hitbox expansion. Record correction magnitude, snapshot age,
confirmation latency percentiles, rejection reasons, CPU, bandwidth and memory.

At 50% loss we require bounded resource use, eventual recovery and no invented
confirmed hits; we do not promise responsive or fully consistent play. Use
`scripts/network-impairment.py --transport udp` for actual datagram impairment and
retain TCP only as a comparison. Run the repository's full verification before
merging implementation. Do not build or publish releases during this refactor.

## Validation, 11 September 2026

The development implementation passes `just verify`, including formatting,
clippy with warnings denied, all workspace tests, dependency boundaries and
`cargo deny`. New coverage includes exact-aim ordering, owned/deduplicated shot
admission, host-only HP changes, identical independent four-second projectile
queries, view/render equivalence across lifecycle changes, expired evidence,
input leases and history overflow recovery.

A headless trial on the installed CAD package accepted eight predicted shots,
cleared their pending flights, and exercised movement and release. No runtime
notices occurred during that firing/movement sequence. Loading-clock anchoring
was corrected separately after the initial startup trial.

The repeated UDP proxy trial used seed 2026, 150–200 ms delay in each direction
and 50% packet loss. The proxy observed 2,544 drops out of 5,082 upstream packets
and 8,369 out of 16,595 downstream packets. The host accepted zero attempted shots
in the firing sequence because reliable input evidence exceeded its admission
window. The trial therefore does not establish usable shooting under those
conditions. Expired refresh packets are now dropped without reliable rejection
traffic, and historical evidence cannot apply or renew live movement.

No Windows numerical comparison or subjective multiplayer playtest has been
completed for this refactor. All test applications, hosts and proxies were
stopped. No release was packaged or published.
