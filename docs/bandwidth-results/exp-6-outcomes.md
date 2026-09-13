<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Experiment 6: key repeated outcome history by identity

Measurement and prototype for step 6 of
[bandwidth-experiments.md](../bandwidth-experiments.md): repeated `shot_results`
and detected-hit histories. The conclusion is a **leave the current repetition in
place** recommendation, and this document shows why the measured share does not
justify the wire change even though the prototype is lossless and passes every
recovery test.

## Revision and commands

- **Worktree and base revision:** `worktrees/exp6` on `perf/bw-exp6` at
  `7d07f1c` (`feat(net): add bounded tracing and in-process owner channels`),
  the same base as experiments 2/4/5.
- **Measurement instrument:** `crates/rm-simulator-server/src/bandwidth_probe.rs`
  taken verbatim from `perf/bw-exp0` (`c9ee3d2`; the file is identical at
  `48f20e6` and at `perf/bw-exp1` `f4cd613`) and registered as
  `#[cfg(test)] mod bandwidth_probe;`. The only probe additions are described
  below; none of the canonical measurement paths changed.
- **Candidate commit:** `perf(net): key repeated outcome history by identity`, the
  commit that carries this file. Its `Patch::Splice` primitive and all tests were
  the tree measured for every "candidate" number below; the "baseline" numbers
  come from the same tree with the splice call site not yet in place, captured
  before that commit.
- **Extra harness flag.** The prescribed profile override alone still resolved to
  a sibling worktree's `rm_simulator_server-<hash>` lib-test artifact (verified:
  the copied binary contained foreign `cadence_matrix_report` tests). Adding
  `--config 'profile.dev.package.rm-simulator-server.codegen-units=16'` gives this
  worktree a private artifact hash `rm_simulator_server-e16e088a74a76c9f`. Every
  capture below came from a copy of that file beside the worktree, after
  confirming the compile line named `worktrees/exp6` and the binary listed
  `bandwidth_probe::tests::bandwidth_probe_exp6_build_marker` (a unique test name
  that prints `probe_marker=exp6-probe-marker-2f9c41d7`) and no foreign
  `cadence_matrix_report`/`cadence_context_report` tests.
- **Canonical command shape:**

  ```sh
  CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/target \
    cargo test -p rm-simulator-server --locked --no-run \
      --config 'profile.dev.package.rm-simulator-server.debug=1' \
      --config 'profile.dev.package.rm-simulator-server.codegen-units=16'
  cp /Users/hxyulin/dev/RM/rm-simulator/target/debug/deps/rm_simulator_server-e16e088a74a76c9f \
     target-probe/probe-lib-tests
  ./target-probe/probe-lib-tests bandwidth_ --nocapture --test-threads=1
  ```

  Every capture begins with `probe cadence=` (14 records); the raw files are
  `/tmp/bw-exp6-baseline.txt` (pre-change) and `/tmp/bw-exp6-candidate.txt`.

## The share, measured first: it is zero on the named workloads

The canonical `fire` and `twelve` workloads fire `Command::Fire`, which records
no `ShotResult`, and on the bare probe layout they land no armor contact. Their
ablated contribution, summed over every produced frame of the 3 s remote run and
divided by its 94 frames:

| Section | `fire` (B/frame) | `twelve` (B/frame) |
|---|---:|---:|
| `total` (independent frame) | 2951 | 4845 |
| `state.hits` | **0** | **−4** |
| `shot_results` | **−2** | **−3** |
| `state.hits` + `shot_results` | **−2 (−0.07%)** | **−7 (−0.14%)** |

These reproduce experiment 0's rows exactly. A negative value means the empty
Array helped the compressor rather than costing bytes: on the workloads this
experiment was told to measure there is literally nothing to remove.

How many frames repeat an unchanged outcome set (produced world frames over the
10 s remote run, 313 frames):

| Workload | result frames non-empty | hit frames non-empty | frames repeating the previous result set | frames repeating the previous hit set |
|---|---:|---:|---:|---:|
| `fire` | 0 | 0 | **312 / 313 (99.7%)** | **312 / 313** |
| `twelve` | 0 | 0 | **312 / 313 (99.7%)** | **312 / 313** |

The repetition is real but of the *empty* set. Under the acknowledged-baseline
codec an empty Array already costs nothing, so a keyed delta can only remove
bytes that are not there.

### Why the canonical probe sees nothing, and the populated counterpart

`Simulation::state` publishes each shooter's newest 32 results and every detected
hit from the last second. The probe's scripted pilot never produces either, so
the exp-0 ablation rows for these two sections have always been measured on empty
Arrays. To size the repetition at all, experiment 6 adds
`bandwidth_probe::tests::bandwidth_attribution_populated_outcomes`, which drives a
real `Simulation` on a bare field with one stationary outpost: `FireAimed` every
64 ms so at least 32 results accumulate, and a `SpawnProjectile` strike on the
outpost every 50 ms so the detector keeps detected hits inside the one-second
window. It reports the same ablation rows plus the actual selected stream.

| Quantity (3 s, 94 produced frames) | Value |
|---|---:|
| newest published shot results | 32 |
| detected hits in the newest frame | 20 |
| frames repeating the previous result set | 47 / 94 |
| frames repeating the previous hit set | 34 / 94 |
| `shot_results` ablation | **578 B/frame** |
| `state.hits` ablation | **437 B/frame** |
| total independent frame | 5089 B/frame |
| two sections' share of the independent frame | **1015 B/frame ≈ 20%** |
| selected stream | 354858 B / 3 s = 3775.1 B/frame = **946.3 kbps** |

So when the histories are actually populated they are about a fifth of the
independent compressed frame, and they change on most frames. This is the honest
correction to the exp-0 attribution, and it is the only evidence that the
repetition is worth anything at all.

## Candidate: identity-keyed splices

The prototype adds one patch primitive to `snapshot_codec`:

- `Patch::Splice(Splice { removed, inserted, changed })` edits an Array by
  baseline index: `removed` drops whole entries, `inserted` places new entries
  before a baseline anchor (the array length appends), and `changed` patches
  surviving entries. Positional and self-contained, so it rebuilds the exact new
  order without shipping a position list.
- `difference`/`apply` keep their existing contract; the splice is chosen only for
  Arrays whose parent key names an identified history, only when every element
  carries the identity fields, and only when the identity is unique inside the
  array. Anything ambiguous falls back to the previous whole-container
  replacement.
- Identity fields: `shot_results` uses `(shooter, shot_id)`; detected hits use
  `(time_ns, projectile, target)` (`target` disambiguates a ball that strikes two
  faces in one tick). A duplicate identity is not a stable identity, so the
  splice is refused.
- The outer size guard is unchanged: a splice is returned only when it is
  smaller than `Patch::Set` of the same subtree. Nothing deletes history; the
  `SimulationState`, `state()` and the decoded `ServerMessage::Snapshot` are
  byte-identical to the independent array encoding.

### Before / after

The workloads this experiment names (10 s, remote cadence, unlimited pacer):

| Workload | Baseline selected | Candidate selected | Baseline down | Candidate down |
|---|---:|---:|---:|---:|
| `fire` | 643140 B / 10 s = 2055 B/frame | **identical** | 741.0 kbps | **741.0 kbps** |
| `twelve` | 906406 B / 10 s = 2896 B/frame | **identical** | 955.1 kbps | **955.1 kbps** |

Exactly identical: both Arrays are empty on those workloads, so the encoder never
emits a splice. The required before/after for `fire`/`twelve` is 741.0 → 741.0
kbps and 955.1 → 955.1 kbps.

The populated counterpart (3 s, same independent bytes in both builds, which is
the internal control that only the delta stream changed):

| | Baseline | Candidate | Delta |
|---|---:|---:|---:|
| independent bytes | 480705 | 480705 | 0 |
| selected bytes | 354858 | 331368 | −23490 |
| selected bytes/frame | 3775.1 | 3525.2 | −249.9 |
| selected kbps | 946.3 | 883.6 | **−62.6 (−6.6%)** |

So the splice removes about 250 B/frame, roughly a quarter of the two sections'
independent contribution, and 6.6% of the whole selected stream in this
single-pilot scenario. It cannot touch the other 80% of that frame (`chassis`,
`state.restore`, `projectiles`, `state.referee`), and it changes nothing at all
where the canonical workloads actually are.

### A second, separated candidate was not built

Work item 3 asks for an acknowledgement/cursor scheme only if identity deltas are
insufficient. They are sufficient to remove the repetition losslessly: the wire
already carries an acknowledged baseline, and the identity splice needs no new
client-side state. A cursor scheme would change recovery semantics (the client
would tell the host the newest outcome identity it has durably consumed, so the
host could shorten or stop repeating the history), which is exactly the change the
experiment says to prefer last. With a measured share of zero on the named
workloads, that semantic risk is not justified, so no cursor prototype is
included here.

## Recovery semantics and tests

The change is confined to the baseline patch encoder/decoder, so recovery is
re-verified at the codec boundary and in the populated runs. Named tests, all
passing on the candidate tree:

| Requirement | Test |
|---|---|
| Round-trip equivalence over a firing run | `snapshot_codec::tests::identity_splice_round_trips_a_firing_run` — 40 real outpost strikes, adjacent compact frames diffed and reapplied; also decodes the patched value and asserts it is the same `ServerMessage` as the independent array encoding |
| Order, mixed insert/remove, ambiguous identity fallback | `snapshot_codec::tests::identity_splice_preserves_order_and_refuses_ambiguous_identities` |
| A stale/wrong baseline must fail closed, never partially apply | `snapshot_codec::tests::identity_splice_rejects_a_baseline_it_does_not_match` |
| Duplicate and reordered packets | `udp_snapshot::tests::outcome_history_deltas_survive_loss_reordering_and_duplicates` (delivers the same frame twice and out of order, every decoded frame must equal its own complete history) |
| Stale baseline ⇒ `Feedback::Missing`, never a partial or silently empty outcome set | `udp_snapshot::tests::stale_outcome_baseline_yields_missing_then_recovers_every_event` (asserts `message.is_none()` and `Missing { id }`, then that recovery re-pins the whole populated baseline and the next delta carries the newest 9 results / 9 hits) |
| Epoch change drops old-life outcomes | `udp_snapshot::tests::outcome_epoch_change_drops_old_life_outcomes` |
| Reconnect and late join | `udp_snapshot::tests::late_join_and_reconnect_receive_every_outcome` (a fresh encoder's first frame is a `Full` baseline at frames 0, 7 and 31, and a fresh decoder recovers each whole history) |
| No outcome dropped by deduplication | `udp_snapshot::tests::duplicate_outcome_delta_is_idempotent_and_drops_nothing` |
| Blackout with dropped acknowledgements | `udp_snapshot::tests::blackout_with_dropped_acknowledgements_recovers_the_whole_history` (44 frames and every ack lost, including an unanswered baseline proposal; the next delivered frame still decodes to the newest 32 results / 20 hits) |
| The populated share itself | `bandwidth_probe::tests::bandwidth_attribution_populated_outcomes` |

Pre-existing recovery tests that still pass unchanged:
`udp_snapshot::tests::missing_ack_still_delivers_current_independent_state`,
`deltas_survive_loss_reordering_and_duplicates_without_chaining`,
`retirement_pins_two_baselines_until_ack_and_old_full_cannot_resurrect`,
`a_lost_retired_answer_is_resent_and_rotation_resumes`,
`missing_baseline_recovers_and_epoch_change_rejects_old_traffic`,
`sustained_updates_save_encoded_bytes_and_caches_stay_bounded`.

Whole-server result from the verified copy: **185 lib tests passed, 0 failed**,
plus the 1 `main.rs` test. `cargo clippy -p rm-simulator-server --all-targets
-- -D warnings` is clean and `cargo fmt -p rm-simulator-server -- --check` is
clean.

The consumer side was also exercised end to end: the app binary tests built from
this worktree (`rm-simulator-app` with its own private `codegen-units` hash) ran
**123 passed, 0 failed**, including all five `net_harness::tests` —
`delayed_contact_event_and_snapshot_recovery_flash_once` (a delayed reliable
contact plus snapshot recovery must flash exactly once) and
`impairment_keeps_the_invariants_and_the_session_recovers` (loss/jitter/blackout
through a real host and session over `scripted_link`).

**What the tests prove and what they do not.** They prove that a splice
reconstructs exactly the Array the independent encoding would have carried, that
a baseline the decoder cannot match is refused instead of decoded partially, that
duplicates and reordering are idempotent at the state level, and that a blackout
or a lost acknowledgement ends with the complete newest history. They do **not**
build a real blackout transport: the blackout test drops application frames and
feedback inside the codec, and the reliable per-event `ServerMessage::Hit` /
`ServerMessage::ShotResult` lanes are not exercised here. The live reliable
delivery and the app-side dedup (`HitFeedback::insert` by hit equality,
`shot_result`/`reconcile_shots` by `shot_id` and `authoritative`) are untouched
by this change because the decoded `SimulationState` is unchanged; that is an
argument from the decode contract, not an end-to-end app test.

**The event-disappearance property.** Nothing is deleted. Identities leave the
wire only when they leave the host state (a hit ages out of the one-second window;
a result ages out of the newest-32 window), exactly as today. Recovery of anything
older than the window remains the job of the reliable `Hit`/`ShotResult` lanes,
which this change does not touch. A snapshot carrying an event is still produced
from the same state, and the splice rebuilds that state exactly, so an event
cannot disappear merely because a snapshot carrying it was produced.

## Recommendation

**Leave the current repetition in place.** Do not adopt the identity splice on
this evidence.

- On the two workloads the experiment names, the share is zero and the
  before/after is exactly 741.0 → 741.0 kbps (`fire`) and 955.1 → 955.1 kbps
  (`twelve`); `state.hits` measures 0 / −4 B/frame and `shot_results` −2 / −3
  B/frame. The 99.7% of frames that repeat an unchanged set are repeating an empty
  set the codec already encodes for free.
- The only positive number is the synthetic populated run: 20% of one independent
  frame and a 6.6% selected-stream reduction (946.3 → 883.6 kbps, −62.6 kbps).
  That scenario is one pilot on a bare field with a stationary outpost, no loss,
  no owner anchors, no peer contention and no twelve real peers. It does not
  demonstrate a match-scale saving, and the absolute number does not move the
  NET-001 budget while the owner anchors (211.8 kbps floor) and `state.chassis`
  remain untouched.
- The change adds a new variant to a public wire patch primitive on the
  acknowledged-baseline path. It is lossless and every recovery test passes, but a
  protocol-shape change for a saving that is zero on the measured production
  workloads is risk without measured reward.

The prototype is committed on `perf/bw-exp6` as the ready-made candidate in case a
later twelve-player, real-client measurement shows the populated share holds at
match scale; it needs no new client state and no recovery-semantics change. Two
notes for whoever measures that: experiment 2 is concurrently converting the
world collections (including these histories) to identity-keyed deltas, which may
make this work redundant; and if a cursor scheme is ever considered, it must be
justified by a populated share measured on real peers, because it is the option
that does change recovery semantics.

## Honest limits

- **In-process only.** No socket, no wall clock, no native GSS/UDP headers,
  retransmissions or outbox replacement; every number is application bytes. The
  probe's pacer is unlimited, so offered bytes are attributed, not delivered
  bytes at a configured budget.
- **The canonical workloads under-exercise the sections.** `fire`/`twelve` fire
  `Command::Fire`, not `FireAimed`, so `shot_results` is empty and the bare layout
  scores no contact. The populated measurement is a separate, synthetic scenario and
  is not the reported `fire`/`twelve` path.
- **The populated scenario is synthetic:** one pilot, one stationary outpost, a
  bare rule field, no referee beyond the HP reset, no loss, one connection. The
  twelve-shooter case (up to 384 published results per frame) is not measured; it
  is the case most likely to raise the share, and it is untested here.
- **The ablation is on the independent frame, not the selected delta.** It
  apportions the self-contained checkpoint; the realized delta saving was measured
  separately as `selected_bytes` 354858 → 331368 over the same run.
- **Blackout is codec-level.** No real blackout transport was built; frames and
  acknowledgements were dropped inside the codec, and the reliable event lanes
  were not exercised end to end.
- **The app suite ran, but its binary provenance is weaker evidence than the
  server copy.** The app crate has no lib test target, so its test binary cannot
  carry this experiment's marker test; it was identified by the `worktrees/exp6`
  compile line and a private `rm-simulator-app` `codegen-units` hash, not by a
  unique symbol. The app consumer in `worktrees/exp6` was also read to confirm
  what recovery consumes (`session_shots.rs`, `session.rs`, `hit_feedback.rs`) and
  confirms the decoded state contract is unchanged.
- **Harness provenance.** The shared target directory's lib-test artifact name is
  the same in every worktree; all numbers come from a verified copy with a private
  `codegen-units` hash and the `exp6-probe-marker` test. The baseline capture was
  taken before the marker test's placeholder length assertion was fixed, so that
  capture reports 6 passed / 1 failed (only the marker test); every measurement
  test in it passed, and the candidate capture is 7/7.
