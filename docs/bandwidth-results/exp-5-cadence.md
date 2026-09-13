<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Experiment 5: separate owner and world publication cadences

Isolated cadence prototypes for step 5 of
[bandwidth-experiments.md](../bandwidth-experiments.md) under
[NET-001](../KNOWN_ISSUES.md). Every number comes from the in-process
attribution probe `crates/rm-simulator-server/src/bandwidth_probe.rs`, driven on
a `ManualTime` clock at fixed 2 ms steps so a workload replays byte for byte. No
socket, no wall clock, no native transport.

This experiment answers one question and deliberately changes no production
default: **how much downstream bandwidth does the owner/world publication rate
cost, and what does lowering the world rate do to the client's complete
context?** It does not touch the 300 ms context limit in
`crates/rm-simulator-app/src/session.rs`; that limit is an input to the analysis,
not a tuning knob.

## Revision and commands

- **Base revision:** `7d07f1c` (`feat(net): add bounded tracing and in-process
  owner channels`), the `perf/bw-exp5` branch point.
- **Probe:** taken from experiment 0's canonical probe at `48f20e6`
  (`test(net): add and run bandwidth attribution probe`), file SHA-256
  `0892b4c964067d47e76ba832d73b377d42086938596c2675ba10d3df64b115b4`,
  extended in this worktree with the cadence parameter. The measured probe
  source is the file committed with this document; its added printed fields are
  listed below.
- **Codec:** `crates/rm-simulator-server/src/udp_codec.rs` gained
  `PeriodicStreams` and `PeerCodec::send_streams`. Production `send` is
  unchanged: it delegates to `send_streams(.., PeriodicStreams::BOTH)`, so one
  publication still produces both streams. The probe asserts byte-identity
  between the explicit 32/32 schedule and the production call
  (`split_32_32_matches_the_production_publication`).
- **Canonical command** (one capture, all 14 probe tests, `--test-threads=1
  --nocapture`):

  ```sh
  CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/target \
    cargo test \
      --config 'profile.dev.package.rm-simulator-server.debug=2' \
      --config 'profile.dev.package.rm-simulator-server.codegen-units=16' \
      -p rm-simulator-server --lib --locked bandwidth_probe \
      -- --test-threads=1 --nocapture
  ```

- **Why the profile override.** The repository root and all worktrees share
  `CARGO_TARGET_DIR`, and the lib-test artifact name
  (`rm_simulator_server-<metadata-hash>`) collides across checkouts. A
  concurrent `cargo test` from another worktree can be reused instead of this
  one. The override gives this worktree's server crate a private artifact hash,
  and the capture was accepted only after its compile line named
  `worktrees/exp5` and the output contained 16 `cadence owner_ms=` rows plus 30
  `probe cadence=` records. Two byte-identical cadence rows would have been
  treated as a suspect artifact; none are identical.

## Cadences measured

| Label | Owner anchors | World checkpoints | Expressible in production today? |
|---|---|---|---|
| `remote` / `split-o31.25_w31.25` | 31.25 Hz (32 ms) | 31.25 Hz (32 ms) | Yes; this is the current remote peer path |
| `split-o31.25_w15.625` | 31.25 Hz (32 ms) | 15.625 Hz (64 ms) | No; needs independent stream scheduling |
| `split-o15.625_w15.625` | 15.625 Hz (64 ms) | 15.625 Hz (64 ms) | Yes, by raising `host.rs` `BROADCAST_PERIOD` to 64 ms |
| `split-o62.5_w15.625` | 62.5 Hz (16 ms) | 15.625 Hz (64 ms) | No; the roadmap shape, and the point of this experiment |

`remote` is the production `PeerCodec::send` path (one publication, both
streams). The three `split-` rows drive `PeerCodec::send_streams` with an
explicit period per stream. On a tick where both periods fall due, one state is
still cut once and feeds both streams, so the coupled fallback is exercised in
every row including the decoupled one.

Workloads are the canonical four: `idle`, `drive` (driving and aiming), `fire`
(driving, aiming, firing every 8 input frames, 78 shots in 10 s), `twelve`
(twelve chassis, the first driving and firing). Each row is 10 s at the offered
rate (`UNLIMITED` pacer, one remote pilot's downstream stream).

## Instrument verification

- `split_32_32_matches_the_production_publication` proves the explicit 32/32
  split is byte-for-byte identical to production's `send`.
- The `remote` rows reproduce experiment 0's committed baseline exactly:
  idle 368.3, drive 453.8, fire 741.0, twelve 955.1 kbps, owner 211.8 kbps,
  world 156.0/241.4/528.6/742.8 kbps.
- `probe_is_workload_sensitive` and `periodic_streams_are_actually_exercised`
  require non-zero `produced_world_updates`, `accepted_anchors`,
  `selected_bytes`, `independent_bytes` and `world_fragments`, and the
  `fire` workload launches 78 shots with `projectiles=32` in the final state.
  A build whose bytes were all `down_control_bytes` fails here.
- `lower_world_cadence_keeps_ids_and_the_cadence_window` asserts that accepted
  checkpoints have strictly increasing `snapshot_id` and `time_ns` and that
  every accepted gap equals the intended world period exactly, for all four
  cadences. No regressions, no duplicates.
- `decoupled_anchors_arrive_between_checkpoints` asserts exactly three accepted
  anchors fall strictly between consecutive checkpoints at 62.5/15.625 Hz.
- `probe_replays_byte_for_byte` and `probe_accounting_is_consistent` are the
  canonical determinism and accounting identities; both pass unchanged.

## Per-cadence results (10 s, one remote pilot, offered rate)

`kbps` is application bytes including the 21-byte RMG1 fragment header, with no
native UDP/IP or GNS overhead; the field is the same one experiment 0 reports.
`owner`, `world` and `control` are the delivered class split; `wf/s` and
`frag/s` are world checkpoints and world fragments per second; `ckpt` and `inc`
are complete checkpoints reassembled and incomplete frames, per 10 s.

| Cadence | Workload | kbps | owner | world | control | wf/s | frag/s | ckpt | inc |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 31.25 / 31.25 | idle | 368.3 | 211.8 | 156.0 | 0.51 | 31.30 | 33.30 | 313 | 0 |
| 31.25 / 31.25 | drive | 453.8 | 211.8 | 241.4 | 0.51 | 31.30 | 33.30 | 313 | 0 |
| 31.25 / 31.25 | fire | 741.0 | 211.8 | 528.6 | 0.51 | 31.30 | 84.10 | 313 | 0 |
| 31.25 / 31.25 | twelve | 955.1 | 211.8 | 742.8 | 0.51 | 31.30 | 105.10 | 313 | 0 |
| 31.25 / 15.625 | idle | 292.9 | 211.8 | 80.8 | 0.23 | 15.70 | 16.70 | 157 | 0 |
| 31.25 / 15.625 | drive | 334.3 | 211.8 | 122.3 | 0.23 | 15.70 | 16.70 | 157 | 0 |
| 31.25 / 15.625 | fire | 479.5 | 211.8 | 267.4 | 0.23 | 15.70 | 42.40 | 157 | 0 |
| 31.25 / 15.625 | twelve | 603.2 | 211.8 | 391.2 | 0.23 | 15.70 | 56.20 | 157 | 0 |
| 15.625 / 15.625 | idle | 187.3 | 106.3 | 80.8 | 0.23 | 15.70 | 16.70 | 157 | 0 |
| 15.625 / 15.625 | drive | 228.7 | 106.3 | 122.2 | 0.23 | 15.70 | 16.70 | 157 | 0 |
| 15.625 / 15.625 | fire | 373.9 | 106.3 | 267.4 | 0.23 | 15.70 | 42.40 | 157 | 0 |
| 15.625 / 15.625 | twelve | 497.6 | 106.3 | 391.1 | 0.23 | 15.70 | 56.20 | 157 | 0 |
| 62.5 / 15.625 | idle | 504.1 | 423.0 | 80.9 | 0.23 | 15.70 | 16.70 | 157 | 0 |
| 62.5 / 15.625 | drive | 545.5 | 423.0 | 122.3 | 0.23 | 15.70 | 16.70 | 157 | 0 |
| 62.5 / 15.625 | fire | 690.6 | 423.0 | 267.4 | 0.23 | 15.70 | 42.40 | 157 | 0 |
| 62.5 / 15.625 | twelve | 814.4 | 423.0 | 391.1 | 0.23 | 15.70 | 56.20 | 157 | 0 |

Marginal change against the `remote` baseline, same workload:

| Cadence | idle | drive | fire | twelve |
|---|---:|---:|---:|---:|
| 31.25 / 31.25 | — | — | — | — |
| 31.25 / 15.625 | −20.5% | −26.3% | −35.3% | −36.8% |
| 15.625 / 15.625 | −49.1% | −49.6% | −49.5% | −47.9% |
| 62.5 / 15.625 | +36.9% | +20.2% | −6.8% | −14.7% |

Upstream is unaffected by cadence (62.5 Hz input in every row):
36.2–76.4 kbps depending on workload, unchanged from the baseline.

### What the numbers say

- **The owner anchor is the fixed cost, and it is large.** The anchor is 846
  bytes on this field, so 31.25 Hz costs 211.8 kbps and 62.5 Hz costs 423.0 kbps
  before any world data. At `idle` the anchor is 57% of the baseline stream.
  Cadence alone cannot reach the 200 kbps target: the only row under it is
  15.625/15.625 `idle` at 187.3 kbps.
- **Halving the world rate roughly halves world bytes.** 528.6 → 267.4 kbps on
  `fire`, 742.8 → 391.2 on `twelve` (−50.6% and −52.7% of the per-interval
  bytes, slightly under 50% because each interval's delta is larger). Halving
  the owner rate on top removes the other half of the fixed cost.
- **62.5 Hz owner anchors with today's 846-byte format cost more than the
  baseline's whole stream on `idle` and `drive`.** The decoupled shape is only
  worth pursuing after the anchor shrinks (experiment 1).
- **Halving both rates is the best cadence-only result, near −50% on every
  workload**, and it is a one-constant change (`BROADCAST_PERIOD` 32 → 64 ms).
  Its cost is that the pilot's own corrections also arrive at 15.625 Hz.

## Complete-context gap, correction and loss margin

All rows below are `fire` with `split-o62.5_w15.625` unless stated; the probe
samples context age every 2 ms of simulation time, and the manual clock advances
in lockstep with simulation time, so the age in ms is what the 300 ms
wall-clock gate in `session.rs` (`aim_observation_fresh`) would see.

| Scenario | Accepted ckpts / 3 s | Gap values (ms) | Max gap | Max context age | Over 300 ms | Incomplete frames |
|---|---:|---|---:|---:|---:|---:|
| clean | 47 | 64 | 64 | 64 | 0 | 0 |
| 1 whole checkpoint lost | 46 | 64, 128 | 128 | 128 | 0 | 0 |
| 3 consecutive lost | 44 | 64, 256 | 256 | 256 | 0 | 0 |
| 4 consecutive lost | 43 | 64, 320 | 320 | 320 | 22 ms | 0 |
| 1 fragment lost (2-fragment ckpt) | 46 | 64, 128 | 128 | 128 | 0 | 1 |

- **Nominal gap at 15.625 Hz: 64 ms.** Max context age at presentation is also
  64 ms, because the age just before the next checkpoint lands is the full gap.
  Mean context age is 33.0 ms (16.99 ms at 31.25 Hz).
- **One lost checkpoint: 128 ms**, exactly as the handoff anticipates.
- **Loss margin: three consecutive lost checkpoints.** 3 × 64 ms of loss gives a
  256 ms gap and 256 ms of maximum context age, still inside the 300 ms gate.
  The **fourth** consecutive loss gives a 320 ms gap and closes the gate for
  **22 ms** (11 probe steps) until the next checkpoint arrives. After one loss
  the remaining headroom is 172 ms, or 2.69 nominal intervals.
- **A lost fragment costs the same checkpoint.** On this field a 15.625 Hz
  checkpoint is 1–4 datagrams (mean 1.94 fragments, 43 of 47 publications
  fragmented), so a single lost fragment usually wastes the whole checkpoint and
  shows up as one incomplete frame; the context cost is the same 128 ms.
- **The 300 ms limit is not relaxed.** 15.625 Hz is the lowest world rate these
  numbers support: at 5 Hz the nominal gap alone is 200 ms and one loss reaches
  400 ms, over the gate before any impairment is considered.

### In-process correction and prediction impact

What the client must bridge from the last complete checkpoint, measured against
the true driver pose 10 s in (the leader has been driving since t = 0):

| Cadence | Context age mean/max | Correction mean/max | Anchor lag mean/max |
|---|---|---|---|
| 31.25 / 31.25 | 16.99 / 32.0 ms | 0.020 / 0.046 m | 32.0 / 32.0 ms |
| 31.25 / 15.625 | 32.97 / 64.0 ms | 0.029 / 0.091 m | 47.4 / 64.0 ms |
| 15.625 / 15.625 | 32.97 / 64.0 ms | 0.039 / 0.091 m | 64.0 / 64.0 ms |
| 62.5 / 15.625 | 32.97 / 64.0 ms | 0.025 / 0.091 m | 39.7 / 64.0 ms |

- Doubling the world period doubles the worst-case correction distance, from
  4.6 cm to 9.1 cm: about 1.5 m/s of chassis speed over the gap. The anchor has
  to repair that much stale context; the client's replay, not the wire, absorbs
  it.
- More frequent anchors at the same world rate spread the correction rather than
  shrinking its worst case: 62.5/15.625 lowers the mean correction from 3.9 cm
  to 2.5 cm while the 9.1 cm pre-checkpoint maximum is unchanged. That is the
  qualitative win the roadmap wants from frequent small owner corrections.
- The context-rebuild cost (`Field::restore` for each accepted checkpoint) halves
  with the world rate: 31.33 restores/s at 31.25 Hz versus 15.67 restores/s at
  15.625 Hz. The final capture measured 20.7 / 24.6 / 27.2 µs per restore
  (31.25, 31.25/15.625, 62.5/15.625), so total rebuild CPU was 0.65 ms/s at
  31.25 Hz against 0.39 ms/s at 15.625 Hz. Across captures on the shared build
  machine the per-restore cost ranged 15–29 µs, so read the halving as the
  signal and the absolute value as machine-load-dependent.
- Anchor lag is measured when the anchor is processed, and the pacer drains the
  owner class before the world class within one publication; at the coupled
  32/32 cadence this makes the coincident anchor appear 32 ms behind the
  checkpoint it rides with. Read the maximum (the gap) as the meaningful column,
  not the coupled mean.

## Recommendation

1. **Trial 31.25 Hz owner / 15.625 Hz world first, with loss and delay.** It
   keeps the owner correction rate exactly as today, halves the complete-context
   stream, and gives −20.5% to −36.8% per workload with a 64 ms nominal gap and
   a three-checkpoint loss margin. It needs the `send_streams` scheduling this
   experiment prototyped, because production currently cannot offer a world
   checkpoint without repeating the anchor.
2. **Trial 15.625/15.625 second if a human playability check accepts 15.6 Hz
   owner corrections.** It is the strongest cadence-only result (≈ −50%) and the
   cheapest to try, since raising `BROADCAST_PERIOD` to 64 ms expresses it
   without new scheduling. Do not adopt it on bandwidth alone: the pilot's own
   corrections would arrive half as often, which no measurement here evaluates.
3. **Defer 62.5/15.625 until experiment 1 shrinks the anchor.** The shape is
   right — frequent small corrections, a slower complete context, the same 64 ms
   gap and loss margin — but 846 bytes at 62.5 Hz costs 423 kbps, 2.1× the
   entire 200 kbps budget, so it is a loss until the anchor is small.
4. **Cadence is not sufficient by itself.** Even the best row needs the encoding
   work (owner configuration references, identity-based world deltas) to reach
   100–200 kbps. Report the cadence and encoding results separately and their
   sum, as the handoff asks.

## Honest limits

- **No socket and no native transport.** The probe drives `PeerCodec` and
  `ClientCodec` directly. `HostPeer::pump`, the bounded peer outbox, GNS,
  native framing/congestion and the host's 1 ms clock loop are outside the
  measurement.
- **No impairment except the scripted world-frame loss above.** No delay,
  jitter, reordering, duplication or blackout; a zero-latency loopback returns
  every client datagram to `PeerCodec::receive`, so acknowledged baselines
  rotate as they would on a perfect link. The loss rows are deliberate
  datagram withholding, not a modelled link.
- **Offered rate, not a capped delivery.** The pacer budget is `UNLIMITED`, so
  these are the bytes the codec offers. Under a real 200 kbps cap the pacer
  would replace and expire owner/world updates, and the delivered split would
  change; a harness trial with the actual budget is required.
- **No real 1 ms tick engine for the whole run.** The probe advances 2 ms per
  iteration and `Simulation::step` advances two 1 ms ticks at once. Tick
  partitioning is covered by the world crate's determinism tests, not here.
- **One client's downstream stream, not an aggregate.** `twelve` is a
  twelve-chassis field observed by one peer; it is not twelve peers' total host
  egress.
- **Anchors are counted by re-decoding the datagram** under the client
  codec's strictly-newer guard, because the probe never performs the Welcome
  handshake. A real session's first anchors before the welcome are not modelled.
- **No app-side prediction worker and no human playability check.** Replay CPU,
  correction feel and click-to-visible-hit latency are not measured; the
  context-rebuild cost above is the only prediction-side number this crate can
  produce.
- **Machine-load sensitivity.** The shared build/test machine runs several
  agents, so the CPU microsecond columns vary between captures by up to ~2×.
  Byte counts are deterministic and were reproduced across captures; CPU numbers
  should be read as direction, not absolute cost.
- **One probe artifact.** A world publication offering 0 fragments appears once
  in the decoupled trace: the pacer's token bucket starts empty at t = 0, so the
  first publication's datagrams drain on the next 2 ms step. It is a
  probe-start artifact, not a lost checkpoint; all 47 checkpoints are accepted.

## Fields added to the shared probe

The canonical printed field names are unchanged. This experiment added, and
lists here so the other experiments can ignore them:

- `cadence owner_ms= world_ms= owner_hz= world_hz= scheduled_owner_publications=
  scheduled_world_publications=`
- `world_frames_s= world_fragments_s= accepted_anchors=`
- `context_age_ms_mean= context_age_ms_max= context_over_limit_ms=
  checkpoint_gap_ms_mean= checkpoint_gap_ms_max=`
- `anchor_lag_ms_mean= anchor_lag_ms_max= correction_m_mean= correction_m_max=
  dropped_world_publications= dropped_world_fragments=
  damaged_world_fragments_offered=`

The probe tests added are `periodic_streams_are_actually_exercised`,
`split_32_32_matches_the_production_publication`,
`lower_world_cadence_keeps_ids_and_the_cadence_window`,
`decoupled_anchors_arrive_between_checkpoints`,
`decoupled_context_gap_and_loss_margin`,
`a_lost_fragment_wastes_one_checkpoint`, `cadence_matrix_report`,
`cadence_context_report` and `cadence_restore_cost_report`.
