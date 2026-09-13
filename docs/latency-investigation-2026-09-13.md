<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Hit feedback and auto-aim latency investigation

Original investigation dated 13 September 2026. The diagnosis and proposals
below describe the release before changes. The implementation follow-up at the
end records the subsequent fixes and validation. The reported setup is two copies of the downloaded
macOS first release, one hosting and one joining. Manual aim and driving feel
responsive; hit feedback and auto-aim are the principal complaints.

The downloaded package identifies commit
`84181fad744e55fd42d661d32f3aaaba59c5bfd2`, tag `v0.0.1-alpha.1`.
Checkout `d06d864f41e6829359e77995e75ae4cd704afe1a` has identical crate source;
the intervening change is CI configuration. This investigation therefore applies
to the release source, rather than assuming an old networking implementation.

Auto-aim already runs locally. Its dependence on server observations, several
different presentation times, and snapshot-based hit effects explain why it can
feel delayed despite responsive manual controls. The fixed downstream budget
adds measurable delay on loopback. An exact end-to-end hit-delay measurement
remains outstanding.

## Measurements and their limits

Built the checkout's app, server and examples with `--locked`. Ran the existing
`direct-single.json` scenario with the downloaded release executables and their
bundled field. Repeated with only `RM_NET_DOWN_KIB_S=512` changed. Both use a
headless server and one rendered client through the harness's unimpaired local
UDP proxy. This is not the user's two-window listen-host workload.

| Release measurement | Default 40 KiB/s | 512 KiB/s |
|---|---:|---:|
| App response RTT, median | 6.83 ms | 5.85 ms |
| App response RTT, p95 | 32.95 ms | 7.21 ms |
| Full collision-context receipt age, p95 | 74.44 ms | 20.74 ms |
| Full collision-context receipt age, maximum sampled | 134.76 ms | 20.96 ms |
| Configured remote buffer delay, median | 143 ms | 52 ms |
| Configured remote buffer delay, p95 | 165 ms | 59 ms |
| Prediction backlog, median | 19 ms | 19 ms |
| Input lead, median | 32 ms | 32.68 ms |

These are short, single trials with 46 and 48 active samples, respectively.
RTT samples repeat the latest probe, so their percentiles are sampled state,
not independent packet observations. Context receipt age means time since the
last full checkpoint arrived, not one-way transit time. The buffer figures are
configured presentation delays; this single-player run contains no enemy robot
whose actual visible delay could be measured. GNS reported RTT rounded to 0 ms.

Crucially, the scripted controls were blocked by UI state and neither run fired
a shot. The harness nevertheless passed its liveness gates. Do not cite these
trials as successful driving, auto-aim or hit-latency tests. Raw event replies
show `ui.blocks_input=true` and `camera.captured=false`. The exact blocking UI
flag was not exposed. Fix that instrumentation before relying on the harness
for gameplay acceptance. Brief example probes overlapped the beginning of the
512 KiB/s trial; repeat isolated trials before freezing performance thresholds.

The checkout's independent CAD replay probe measured median costs of 0.29,
0.65, 1.06 and 2.23 ms for 32, 64, 128 and 200 ms replays. It does not establish
performance with two rendered clients or many colliding robots, but gives no
reason to blame single-robot physics replay for hundreds of milliseconds of lag.

All 11 existing `auto_aim::tests` passed. Their direct-world motor tracking
examples produced five hits from five shots to activate a small rune, and
29 hits from 36 shots at a rotating outpost. These validate solver and motor
behavior under their test conditions, not the asynchronous network presentation
path. `git diff --check` also passed. No runtime code was changed.

The bandwidth example's actual delta encoder sent 284,719 bytes for two moving
robots without firing and 467,924 bytes with firing over 250 updates. At its
62.5 Hz workload these are 71.2 and 117.0 kB/s before owner anchors and transport
overhead. Merely halving these rates to approximate 31.25 Hz gives 35.6 and
58.5 kB/s; this is budget arithmetic, not a measured 31.25 Hz workload, whose
deltas differ. Even that estimate leaves little or no room within 40 KiB/s.
The example's separately labelled “GNS application bytes/s” uses a different
encoding; it must not be mistaken for the live delta stream.

Manifests, binary/package identity and summaries are retained locally in
`/tmp/rm-network-pr-local-evidence/latency-2026-09-13/`, outside Git.
Raw trials remain in `/tmp/rm-latency-{release,release-512,default}-20260913`.

## Findings in the code

1. **The LAN is artificially constrained.** `gns_transport.rs` supplies a
   40 KiB/s downstream pacer and 10 KiB/s upstream pacer. These are application
   budgets even on localhost. `pacing.rs::Pacer::next` shares tokens among
   control, owner and fragmented world data. If its selected packet cannot fit,
   it returns immediately instead of considering a smaller urgent packet.
   Its burst bucket is only 32 ms of bytes. A large world transfer can therefore
   delay the data auto-aim needs even when native socket queues are empty.
   The release A/B result supports this as a contributor.

2. **Fast own-robot updates conceal stale target state.** `host.rs` publishes
   network snapshots every 32 ms, versus owner-only embedded updates every
   4 ms. UDP extracts a compact own-robot anchor from network snapshots.
   `session.rs::poll_owner_anchor` refreshes the generic checkpoint freshness
   clock, but `auto_aim.rs::targets` reads the full `session.snapshot`.
   That function returns no targets when `fire_time_ns - snapshot.time_ns`
   exceeds 300 ms. The age includes scheduled input lead, not just transport
   age. Healthy local movement is thus not proof that auto-aim has fresh enemies.

3. **Auto-aim and the visible opponent use different times.** Remote robot
   drawing samples delayed history in `scene.rs::publish_scene`. Auto-aim
   extrapolates the last authoritative robot pose using linear/angular velocity
   to scheduled firing time and then projectile impact. Predicting impact is
   necessary, but selecting a target around that future pose can disagree with
   the target under the displayed crosshair. Stops, turns and contacts break
   the constant-velocity assumption. Acquire on the visible pose, then solve
   impact separately with an explicit uncertainty estimate.

4. **Aim calculation, camera presentation and firing are not one sample.**
   `main.rs` orders polling/prediction, camera update, auto-aim, drive, then fire.
   Auto-aim changes `Player` after the camera transform has been written, so
   assisted camera rotation appears next frame. Prediction was also submitted
   before that frame's assisted aim. `Session::drive` throttles aim refresh to
   16 ms unless drive intent changes. `begin_local_shot` copies the last sent
   input, so a shot can use an older sample than the aim solution just computed.
   This is a timing mismatch worth testing, not proof every assisted shot misses.

5. **“Waiting” includes real motor and assist gates.** Auto-fire tests the
   predicted actual turret orientation, not the instantaneous camera aim.
   Default gimbal response is 25 ms, with velocity and acceleration limits.
   The assist also tests facing, scoring area, detection speed, geometry and
   intervening robots. Rune assist enforces at least 500 ms between attempts,
   flight time plus 150 ms, and can wait 1.2 s before retrying an unchanged blade.
   These are local assist settings. The HUD collapses too many distinct causes
   into “waiting”, “no shot”, or an empty status.

6. **Hit confirmation is bundled into replaceable world state.**
   `ShotResult` confirms launch acceptance, not an armor hit. Projectile
   prediction returns provisional balls, not authoritative scoring feedback.
   Armor contacts travel in `FieldSnapshot.hits`. `scene.rs::flashing` only
   shows contacts younger than 50 ms relative to that snapshot's simulation
   time. If the first delivered/coalesced checkpoint containing a hit is already
   more than 50 ms past it, the client never flashes it. Transport delay alone
   does not expire an old snapshot's flash: that old snapshot can instead show
   the flash late or hold it until another snapshot arrives. Both behaviors
   come from tying effect lifetime to snapshot age rather than presentation.

7. **Remote delay recovers slowly.** `interpolation.rs` uses the 95th percentile
   of observed full-state age plus 16 ms, bounded to 32–250 ms. It increases
   immediately but falls only 5 ms per second. A startup burst can leave seconds
   of extra visual delay after the connection improves. This fits the measured
   large remote buffer setting despite low median app RTT.

## Proposed fixes, in order

First repair the feedback and timing paths; a networking rewrite is not required
to get useful improvements.

1. Add a bounded, deduplicated authoritative hit-event stream with match epoch,
   event id, projectile/shooter identity, target, simulation tick and scoring
   outcome. Keep launch acknowledgement separate. Deliver recent events promptly
   with acknowledgement/retry and retain snapshot recovery. Never put events in
   the replaceable motion slot. Render a confirmed hit once on receipt for a
   presentation-owned duration; expire very old recovery events without replaying
   a burst of effects. HP and referee state remain authoritative. A provisional
   impact effect may be immediate, but must not claim confirmed damage.

2. Split input polling, mouse/assist sampling, prediction submission and camera
   placement into explicit ordered stages. A fire command should carry the exact
   aim sample used for that firing decision, even between ordinary input refresh
   intervals. Test turret gating at that sample's intended execution tick.
   Preserve asynchronous whole-field prediction and same-tick authoritative
   muzzle derivation. Do not replace motor dynamics with camera angles.

3. Refactor auto-aim into observation, acquisition, impact solution and fire-gate
   results. Acquire against displayed targets; solve using timestamped latest
   state and projectile flight time. Track full-target freshness separately from
   own-robot anchor freshness. Show why firing waits: stale target, turning
   barrel, blocked path, cadence, or rune confirmation. Continue local tracking
   through a short bounded observation gap while disabling uncertain auto-fire.
   Keep all scoring and permission checks on the host.

4. Add a LAN bandwidth profile and validate a downstream allowance such as
   256–512 KiB/s per peer. Treat 512 as an experiment supported by this A/B,
   not a final universal default. Reserve service for inputs, hit events and
   owner corrections; make the pacer work-conserving across independent classes
   while retaining reliable ordering and preventing world starvation. Add
   queue age and bytes by class to diagnostics. Preserve congestion control.

5. If player-count tests still miss budgets, separate frequent motion/target
   updates from full restorable world checkpoints. Use a compact typed wire
   representation and explicit revisions for configuration. Every client must
   still restore a complete field from a coherent checkpoint; lightweight
   presentation updates are not permission to construct a reduced physics world
   or silently merge incompatible rule epochs. Raise motion publication rate
   only after measuring the complete byte and CPU budget. Tune interpolation
   from measured arrival variation with faster bounded recovery.

Keep the explicit 1 ms rule clock and host ownership. Do not lower physics
accuracy or add competitive rule enforcement to fix a presentation problem.

## FPS networking comparison

Valve's [Source lag-compensation implementation](https://github.com/ValveSoftware/source-sdk-2013/blob/master/src/game/server/player_lagcompensation.cpp)
accounts for latency and interpolation when selecting historical player state.
That addresses a shooter aiming at a delayed opponent. It is useful precedent
for making the view time explicit, but should not be transplanted directly into
this simulator's moving projectile collisions. A projectile has flight time and
can meet intervening geometry after launch. Rewinding a target as if every bullet
were hitscan would change the simulation's hit policy. Any bounded historical
launch/flight compensation needs a separate design and tests.

Glenn Fiedler's [snapshot interpolation](https://www.gafferongames.com/post/snapshot_interpolation/)
explains the latency cost of buffering remote state and why increasing delivery
rate reduces the delay needed for loss protection. His
[state synchronization](https://gafferongames.com/post/state_synchronization/)
distinguishes simulation corrections from smoothed rendered corrections and
describes prioritizing updates within a packet budget. Those approaches support
fixing timing, delivery priority and presentation here while retaining the host's
authoritative physics.

## Acceptance work

Repair the harness so a driving/firing scenario fails if controls remain blocked,
the chassis never moves, or no rounds launch. Expose auto-aim target, observation
age, chosen execution time and exact fire-gate rejection reason. Record input
sample, launch execution, impact detection, hit-event receipt and first displayed
feedback as separate times. The existing shot confirmation metric ends at launch.

Test the actual release-style listen host plus one client, then two machines on
wired LAN, with stationary, strafing, spinning and colliding targets. Compare
default and larger budgets with several isolated repetitions. Include runes,
outposts, sustained fire, loss/reordering and an unfocused host window. Two
rendered apps on one machine also need frame-time/GPU measurements.

Add deterministic regressions for a hit delivered 100 ms late displaying once,
duplicates and epoch resets, a fire between aim refreshes carrying current aim,
auto-aim acquiring the rendered target, and owner anchors arriving while target
snapshots are stale. Proposed engineering targets are confirmed effects within
one rendered frame of event receipt, no lost recent hit effects under snapshot
coalescing, and low tens of milliseconds of added LAN delivery delay beyond
physical flight. These are proposed targets, not measurements already achieved.


## Implementation follow-up

Protocol 27 implements the first feedback, input/assist and delivery fixes.
The host publishes scored armor contacts on the existing reliable ordered lane
with an epoch and monotonic event id. Native transport handles retry and ordering.
The bounded client queue is independent of snapshot replacement. The app deduplicates
event and snapshot recovery, displays recent contacts for a receipt-timed lifetime,
and discards recovery older than 250 ms. Pause epochs clear active effects.
No predicted impact claims confirmed damage.

Completed prediction is consumed before assist. Mouse sampling, assist, chassis
input, fire submission, the next replay request and camera placement now have
explicit order. Fire flushes the latest sampled controls
when they were not already sent for that execution tick. Camera placement still
uses the simulated motor pose. Auto-aim acquires displayed robot poses and solves
impact from the latest authoritative state. It reports the fire gate and disables
auto-fire beyond 150 ms of target age, with tracking bounded to 300 ms. Own anchors
cannot extend enemy observation freshness. Whole-field replay remains asynchronous.

The LAN profile uses 512 KiB/s downstream and 64 KiB/s upstream per peer. The
`limited` profile preserves 40 and 10 KiB/s; explicit rate overrides take precedence.
Native congestion control remains enabled. A waiting packet permits one smaller
packet to pass, then retains service. This bounded bypass replaces the initial
unlimited-bypass experiment, which failed the existing world-starvation test.
Per-class queue bytes, age and cumulative service are available in the console.
Automatic interpolation releases excess delay at 50 ms/s rather than 5 ms/s.

The stricter gameplay trial exposed a second bandwidth bottleneck that the original
idle tests could not show. With downstream raised but upstream still 10 KiB/s,
sustained fire produced 9 unresolved shots in the single-client run and 24 in the
rendered-host run. The single-client run's sampled launch-confirmation p50/p95 was
154/250 ms, while the native transport reported 0 ms RTT and an empty native send
queue. The application upstream queue held roughly 1.5–2.3 kB and expired packets.
Raising the LAN upstream allowance and avoiding duplicate same-frame aim submissions
reduced sampled confirmation p50/p95 to 40/58 ms, with zero execution-time offset
at p95 and no unresolved shots in the next trial. The confirmation metric includes
the scheduled input lead; it is not impact latency. These are single developmental
trials, with subsequent final-build repetitions recorded below.

Console capture now explicitly bypasses OS focus blocking until release or
disconnect. The trial checks both displacement and launches. Launch acceptance
has a per-client counter, so another player's shots cannot satisfy the check.
Console feedback diagnostics distinguish impact tick, receipt age and first scene
submission. Scene submission does not measure GPU scanout.

Regression coverage includes delayed contact delivery over the real host/client
codec, duplicate event/snapshot recovery, epoch changes, effect expiry without a
new snapshot, current aim between refreshes, visual acquisition, stale full-target
state despite own updates, and bounded pacing without world starvation. The explicit
1 ms clock, host ownership, whole-field restore and projectile collision paths are
preserved. There is no hitscan rewind or additional rule enforcement.

Frequent compact motion packets remain a conditional follow-up for player-count
workloads that still exceed their delivery budgets. Two-machine wired LAN testing
and subjective auto-aim feel remain outside these local automated measurements.


### Final validation

`just verify` passed, including 535 Rust tests/doctests, all-feature checks and
Clippy, Python tests, crate boundaries, MPL notices and dependency policy.
Rebuilt the default-feature app and server with `cargo build --locked` before
running three isolated trials. No compilation overlapped these final trials.

| Final trial | Client-confirmed launches | Unresolved | App RTT p50/p95 | Launch confirmation p95 | Execution offset p95 |
|---|---:|---:|---:|---:|---:|
| Headless server, rendered client | 89 | 0 | 4.7/7.5 ms | 59.5 ms | 0 ms |
| Rendered listen host, rendered client, run 1 | 81 | 0 | 6.4/8.9 ms | 84.2 ms | 0 ms |
| Rendered listen host, rendered client, run 2 | 81 | 0 | 6.5/11.1 ms | 46.7 ms | 0 ms |

Each run passed the stricter direct scenario, collected 48 active samples with
zero query gaps, and moved the client robot about 6.6 m. The client rendered
offscreen; the listen-host window rendered visibly. These tests cover two app
processes on this machine, not a second physical LAN machine. As in the original
trials, timing distributions sample the latest reported values, rather than
recording every shot or packet. Launch confirmation includes input lead and frame
polling; it must not be described as impact-to-feedback latency.

The final manifests and summaries are retained as `final-direct-*`,
`final-listen-1-*` and `final-listen-2-*` under that local evidence directory. The
`development-upstream10-*` and `development-lan64-*` records preserve the workload
that exposed the upstream limit and the first passing change. Those development
runs differ in aim-submission code as well as allowance, so they are not a
single-variable bandwidth benchmark.

To repeat the rendered-host trial after building, supply the app as the harness's
host executable:

```sh
python3 scripts/network-harness.py scripts/network-scenarios/direct-single.json \
  --server target/debug/rm-simulator --cad-assets field \
  --output /tmp/rm-listen-latency-new
```

For a manual test, launch `target/debug/rm-simulator` twice and host/join through
Multiplayer. Both instances need this protocol 27 build; the downloaded first
release still contains the old code.

Recorded artifact paths use `<checkout>`, `<downloads>` and `<home>` placeholders
for local directories. Measurements and binary hashes are unchanged.
