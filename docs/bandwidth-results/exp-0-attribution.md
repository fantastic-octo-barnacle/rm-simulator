<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Experiment 0: attribute the bytes before implementing reductions

Baseline attribution for [NET-001](../../KNOWN_ISSUES.md) and step 0 of
[bandwidth-experiments.md](../bandwidth-experiments.md). All numbers below come
from the in-process attribution probe
`crates/rm-simulator-server/src/bandwidth_probe.rs`, measured on a
`ManualTime` clock at fixed 2 ms steps so the same workload replays byte for
byte on any machine. No socket, no wall clock, no native transport.

## Revision and commands

- **Base revision under measurement:** `3a41fcc` (`test(net): add in-process
  bandwidth attribution probe`) plus the uncommitted probe fixes that this
  experiment committed; `bandwidth_probe.rs` had never been compiled before
  this work.
- **Commit added by this experiment:** `48f20e69803a8f81da06057aee46ce0c82185ebb`
  (`test(net): add and run bandwidth attribution probe`). It contains the
  verified probe and this results file; the capture binary was verified to
  contain the probe's unique `every-publication` marker before its output was
  recorded, so the numbers below belong to that tree.
- **`git status --short` at measurement time:**

  ```
   M crates/rm-simulator-server/src/bandwidth_probe.rs
  ?? KNOWN_ISSUES.md
  ?? docs/bandwidth-experiments.md
  ```

- **Canonical command:**

  ```sh
  CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/target \
    cargo test -p rm-simulator-server --locked bandwidth_ -- --nocapture
  ```

- **How the captured run was actually taken.** The cargo target directory is
  shared by the repository root and all eight worktrees, and the lib-test
  artifact name (`rm_simulator_server-<metadata-hash>`) is the same in every
  checkout because the metadata hash does not include the workspace path. A
  concurrent `cargo test` from another worktree overwrote the artifact between
  build and run, and the first capture executed that other build (its output had
  no `cadence=` field and only three probe tests). The verified capture was
  therefore taken by building, immediately copying the freshly built test
  binary to `worktrees/exp0/target-probe/probe-lib-tests`, checking that the
  copy contains the probe's unique `every-publication` marker, and running the
  copy:

  ```sh
  CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/target \
    cargo test -p rm-simulator-server --locked --no-run
  cp /Users/hxyulin/dev/RM/rm-simulator/target/debug/deps/rm_simulator_server-<hash> \
    target-probe/probe-lib-tests
  strings target-probe/probe-lib-tests | grep every-publication   # provenance
  ./target-probe/probe-lib-tests bandwidth_ --nocapture --test-threads=1 \
    > /tmp/bw-exp0-baseline.txt
  ```

  This is a harness workaround for a shared target directory, not a code
  change. Anyone re-running the canonical command should confirm the output
  starts with `probe cadence=remote`; otherwise another worktree's build was
  executed.

The regenerated raw output is `/tmp/bw-exp0-baseline.txt` (145 lines at the
time of writing). Five tests pass: `bandwidth_attribution_baseline`,
`bandwidth_attribution_sections`, `probe_replays_byte_for_byte`,
`probe_accounting_is_consistent`, `probe_is_workload_sensitive`.

## Headline code-path finding: `idle`, `drive` and `fire` were an idle field

The probe as committed (`3a41fcc`) never applied its own pilot input to the
simulation. It called `PeerCodec::receive` only for the baseline-feedback side
effect and discarded the returned `PeerRequest`, so the simulation was stepped
with no commands. Before the fix, `idle`, `drive` and `fire` produced
byte-identical downstream numbers (45757.7 B/s, 333 world fragments, 313
anchors, `projectiles=0`, `shots_fired=0`), and every workload measured a static
field. That is the central correctness issue of this experiment and it is fixed
in the committed probe: `PeerRequest::Inputs` and `PeerRequest::Message` are now
applied through the production `Simulation::apply`, and
`probe_is_workload_sensitive` fails if the workloads stop differing or if the
firing workload launches no shots. **No number from the pre-fix probe is
reported.**

## Publication cadence: what the probe models

`host.rs::advance_clock` publishes two kinds of periodic frame from one loop:
`publish_snapshot(false)` every `BROADCAST_PERIOD = 32 ms`, and
`publish_snapshot(true)` every `OWNER_BROADCAST_PERIOD = 4 ms` in between
(`host.rs` around `advance_clock`). `publish_snapshot` pushes an owner-only
frame only to a peer with `owner == true`, and `peer.owner` is set from
`owner_spawn.is_some()` at registration, so **only the in-process owner (the
listen host's own local player) ever receives the 4 ms frames**. A remote pilot
receives only the 32 ms full publications. `PeerCodec::send` then emits an owner
anchor for *every* periodic frame because it keys the anchor on `frame.periodic`
alone and its `chassis` field is set for any pilot.

The probe models the remote pilot as its primary configuration (`Cadence::Remote`):
one `peer.send` per 32 ms publication, no 4 ms anchors. That is the path the
reported 855 kbps remote figure measures. Two contrast configurations are
reported:

- `Cadence::Owner` — the in-process owner peer: 4 ms owner-only frames plus the
  32 ms world frame. Because `PeerCodec` cannot tell the two periodic kinds
  apart, the probe separates them through the codec's own congested-carrier path
  (`pending_bytes > CONGESTED_PENDING_BYTES` keeps the anchor and skips the world
  checkpoint), which is the mechanism a cadence fix would make explicit.
- `Cadence::EveryPublication` — a counterfactual, **not** the remote path: every
  4 ms publication is fed ungated, so each also produces a world checkpoint.
  This is what today's `PeerCodec::send` does when handed either periodic kind,
  and it sizes the code-path inefficiency.

`PeerCodec::send` cannot distinguish an owner-only publication from a full world
one; this is a real inefficiency, but because owner-only frames never reach a
remote peer it does **not** explain the 855 kbps remote baseline. It inflates the
local owner's own outbound stream and belongs to experiment 5 (cadence).

## Primary baseline: remote pilot, 10 s per workload, unlimited pacer

World frames are published at 31.3/s (313 in 10 s). The owner anchor is present
in every world frame. Rates are application bytes per second (decimal kbps);
they exclude IP/UDP/GNS headers and retransmissions.

| Workload | Down B/s | Down kbps | Owner B/s (kbps) | World B/s (kbps) | Control B/s (kbps) | World frags | Complete ckpts | Up B/s | Up kbps | Produced owner B/s | Shots |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `idle` | 46037.8 | 368.3 | 26479.8 (211.8) | 19494.1 (155.9) | 63.9 (0.5) | 333 | 313 | 4561.1 | 36.5 | 26479.8 | 0 |
| `drive` | 56721.9 | 453.8 | 26479.8 (211.8) | 30178.2 (241.4) | 63.9 (0.5) | 333 | 313 | 9170.4 | 73.4 | 26479.8 | 0 |
| `fire` | 92623.8 | 741.0 | 26479.8 (211.8) | 66080.1 (528.6) | 63.9 (0.5) | 841 | 313 | 9544.8 | 76.4 | 26479.8 | 78 |
| `twelve` | 119391.4 | 955.1 | 26479.8 (211.8) | 92847.7 (742.8) | 63.9 (0.5) | 1051 | 313 | 9544.8 | 76.4 | 26479.8 | 78 |

Selected encoded world bytes per frame (from `selected_bytes / 313`):
`idle` 600 B, `drive` 942 B, `fire` 2055 B, `twelve` 2896 B. Independent
comparison frames were 2207/2443/3597/6182 B respectively, so acknowledged
deltas are doing real work. Fragments per world frame: `idle` 1.06, `drive`
1.06, `fire` 2.69, `twelve` 3.36.

The `fire` and `twelve` totals (741 and 955 kbps) are in the same range as the
855 kbps live remote measurement, which is the expected result now that the
workloads actually move the field. The upstream figures (73–76 kbps for the
moving/firing runs) sit below the reported ~103 kbps because the probe's
scripted pilot is one smooth cadence, not the reported trial's traffic mix.

## Cadence contrast (5 s per workload)

| Workload | Configuration | Down B/s (kbps) | Owner B/s (kbps) | World B/s (kbps) | Control B/s (kbps) | Owner updates/s | World updates/s | Skipped world/s |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| `idle` | owner | 231351.4 (1850.8) | 211500.0 (1692.0) | 19794.6 (158.4) | 56.8 (0.5) | 250.0 | 31.4 | 218.6 |
| `idle` | every-publication | 368123.4 (2945.0) | 211500.0 (1692.0) | 156063.6 (1248.5) | 559.8 (4.5) | 250.0 | 250.0 | 0 |
| `drive` | owner | 241846.8 (1934.8) | 211500.0 (1692.0) | 30290.0 (242.3) | 56.8 (0.5) | 250.0 | 31.4 | 218.6 |
| `drive` | every-publication | 447354.0 (3578.8) | 211500.0 (1692.0) | 235294.2 (1882.4) | 559.8 (4.5) | 250.0 | 250.0 | 0 |
| `fire` | owner | 268855.2 (2150.8) | 211500.0 (1692.0) | 57298.4 (458.4) | 56.8 (0.5) | 250.0 | 31.4 | 218.6 |
| `fire` | every-publication | 658867.2 (5270.9) | 211500.0 (1692.0) | 446807.4 (3574.5) | 559.8 (4.5) | 250.0 | 250.0 | 0 |
| `twelve` | owner | 295642.6 (2365.1) | 211500.0 (1692.0) | 84085.8 (672.7) | 56.8 (0.5) | 250.0 | 31.4 | 218.6 |
| `twelve` | every-publication | 847576.6 (6780.6) | 211500.0 (1692.0) | 635516.8 (5084.1) | 559.8 (4.5) | 250.0 | 250.0 | 0 |

What the contrast says:

- At the intended owner cadence (4 ms anchors + 32 ms world), the **owner anchor
  stream dominates** for every workload: 1692.0 kbps of anchors against
  158.4/242.3/458.4/672.7 kbps of world. Anchors alone are already above the
  200 kbps whole-budget target.
- In the ungated counterfactual the world stream inflates by about **7.6–7.9×**
  (`idle` 158.4→1248.5, `drive` 242.3→1882.4, `fire` 458.4→3574.5,
  `twelve` 672.7→5084.1 kbps). Once moving or firing, that inflated world stream
  overtakes the owner stream (`drive`, `fire`, `twelve`); for `idle` the owner
  stream still dominates.
- `skipped_world_updates` confirms the gating: 1093 of 1250 owner-cadence
  publications skipped the world checkpoint in 5 s (218.6/s), leaving 157 world
  updates (31.4/s). The every-publication run skips nothing and produces 1250.

## Per-section compressed ablation

Controlled ablation of the independent compact checkpoint
(`encode_player_message`), summed over every produced frame of a 3 s remote run
and divided by the 94 frames. Each row is `whole − (whole without that section)`
after recompression, in bytes per published frame; a negative value means the
section helped the compressor rather than costing bytes. Rows do not sum to
`total`, because compression is not additive.

| Section | `fire` (B/frame) | `twelve` (B/frame) |
|---|---:|---:|
| `total` (independent frame) | 2951 | 4845 |
| `state.chassis` | **1097** | **2501** |
| `state.projectiles` (hoisted wire array) | **520** | **530** |
| `state.restore` (reported, never removable) | **386** | **608** |
| `state.referee` | 102 | 330 |
| `state.runes` | 168 | 154 |
| `state.outposts` | 47 | 70 |
| `state.tick` | 3 | 7 |
| `state.bases` | −2 | −5 |
| `state.hits` | 0 | −4 |
| `state.time_ns` | −2 | 0 |
| `shot_results` | −2 | −3 |

The chassis array is the single largest compressed contributor in both
workloads (37% of the `fire` independent frame, 52% of `twelve`), followed by
projectiles and hidden restore state. **Attack the chassis representation
first**: on the world side that is a keyed/binary `state.chassis` section
(experiments 2 and 3), and on the owner side the same chassis configuration is
re-sent, deflated, in every anchor (experiment 1). The projectile array is the
second world-side target and grows with sustained fire. `state.restore` is
reported only to show its share; it is required by `Field::restore` and must
never be removed.

## Arithmetic checks

Checked by hand on the captured output; all hold exactly as printed.

- `down_owner_bytes + down_world_bytes + down_control_bytes == down_bytes`
  (each delivered datagram lands in exactly one bucket in `account_down`), e.g.
  `fire`: 26479.8 + 66080.1 + 63.9 = 92623.8 B/s.
- `up_batch_bytes ≤ up_bytes`, e.g. `twelve`: 91067 ≤ 95448 B over 10 s.
- `down_kbps == down_bytes_s * 8 / 1000`, e.g. `fire`: 92623.8 × 0.008 = 741.0.
- Determinism: `probe_replays_byte_for_byte` replays `fire` for 2 s twice and
  compares total downstream, upstream, owner and world bytes.
- `probe_accounting_is_consistent` pins the class-sum identity, the batch
  subset, and `selected_bytes < independent_bytes` (baseline feedback reached
  the encoder).
- `probe_is_workload_sensitive` requires `idle` ≠ `drive` downstream world
  bytes, `drive` ≠ `fire` raw checkpoint bytes, `fire.shots_fired > 0`, a
  non-empty projectile/hit/result trace, and 12 chassis in `twelve`.

## Warnings and scope

- **Application bytes only.** IP/UDP/GNS headers and retransmissions are not
  included. `framed_world_bytes` includes the 21-byte application fragment
  headers; `down_*` counts delivered application datagrams. Native overhead is
  excluded.
- **Not the proxy-payload baseline.** The reported 855 kbps live figure is a UDP
  proxy payload rate with native sockets, outbox replacement and configured
  pacing. The probe offers an unlimited pacer, never loses or reorders a packet,
  and never delivers a `Welcome`, so the client's owner-anchor acceptance path
  is not modelled (anchor bytes are still delivered and attributed). The probe
  is the encoder/codec attribution instrument, not an end-to-end acceptance
  test.
- **`restore` must never be removed.** Its ablation row is reported for
  completeness only.
- **Do not sum production, pacing and delivery counters.** `raw_world_bytes`,
  `framed_world_bytes`, `selected_bytes` and `down_world_bytes` describe the
  same traffic at different stages.
- **The ablation is on the independent frame, not the selected delta.** Its
  contributions explain the self-contained checkpoint the encoder falls back to;
  the actual selected stream is smaller (600–2896 B/frame).
- **Single pilot per connection.** The twelve-player run is twelve such world
  states on one wire; only the first pilot is commanded, matching the probe's
  documented intent. Host egress contention across twelve real peers is not
  modelled.

## Code findings

1. **The handoff claim about `QueueStats { encoding: None, .. }` is false.**
   `HostPeer::pump` (`udp_codec.rs`, around the once-per-second delivery report)
   takes `self.codec.pacer.stats(...)`, then explicitly sets
   `stats.encoding = Some(self.codec.encoding.clone())` before sending
   `ServerMessage::DeliveryStats(stats)`. `pump` is called by
   `gns_transport.rs` in the production UDP loop; the client stores the message
   (`net.rs`) and the network trace records `event.encoding` from it
   (`network_trace.rs`). `EncodingStats` is therefore sent on the wire once per
   second per peer and is observable client-side and in traces; the
   `#[cfg(test)] encoding_stats` accessor is a convenience, not the only
   observability. `pacer.stats()` does default `encoding` to `None`, but `pump`
   overwrites it.
2. **`PeerCodec::send` keys the world checkpoint off `frame.periodic` alone.**
   `host.rs::publish_snapshot(true)` frames are periodic, so they take the same
   `snapshot_codec::encode_player_message` + deflate + `encoder.snapshot` +
   `pacer.world` path as full world publications. Scoped correctly: only the
   in-process owner receives those 4 ms frames (`publish_snapshot` filters on
   `peer.owner`, and `peer.owner` is `owner_spawn.is_some()`), so a remote peer
   is unaffected; this inflates the local owner's own stream by ~7.6–7.9× on the
   world side and is a cadence candidate (experiment 5), not an explanation of
   the remote baseline. The anchor is emitted before the congestion check, so it
   is never skipped even when the world checkpoint is.
3. **The owner anchor is 846 B, not ~438 B as the handoff's arithmetic
   assumed.** `owner_anchor_bytes` is 846 in every run. The fixed numeric part
   is the handoff's 438 B (4 magic + 4×8 ids/time + 4 id + 3 flags + 2 length +
   29×8 owner f64 + 1 wheel count + 4 wheels × 5×8 = 438 B), so the deflated
   chassis configuration is about **408 B, or 48% of every anchor, repeated
   every time**. At 31.25 Hz that is 26.5 kB/s ≈ **211.8 kbps of anchors
   alone**, roughly double the handoff's ~109.5 kbps owner estimate and already
   above the 200 kbps whole-downstream target. Experiment 1's payoff is larger
   than the handoff assumed.
4. **The committed probe had several correctness defects, all fixed here:**
   it never called `account_down` (all downstream counters were zero); it built
   `Outbound` without setting `periodic = true` (so a snapshot would have taken
   the reliable control path); it never returned the client's baseline feedback
   to the host encoder (so `Encoder::deltas` stayed zero and every frame was
   independent); it never applied decoded input to the simulation (all
   workloads were an idle field); its ablation paths omitted `CompactSnapshot`,
   `state.field` and the hoisted projectile array (only the `total` row was
   produced); and `down_anchors` was counted both on delivery and on client
   acceptance. The module docs now state the probe's scope explicitly.
5. **The probe's `remote` configuration still carries owner anchors.** Because
   `PeerCodec` emits an anchor for every periodic frame and a remote pilot also
   has a chassis, a remote peer receives 31.3 anchors/s in addition to the world
   checkpoints. The measured 211.8 kbps anchor floor therefore applies to the
   remote path too, and it is the entire downstream budget for the `idle`
   workload's owner share.
6. **Sustained fire keeps ~32 projectiles in flight** in this bare field and
   never scores an armor hit (`hits=0`, `shot_results=0` because the probe
   submits `Command::Fire`, not `FireAimed`). The projectile array still churns
   enough to be the second-largest world-side ablation contributor, which is the
   relevant result; armour-hit history churn is not exercised by this probe.

## Recommended next step

Experiment 1 (owner configuration references) targets a measured 846 B/anchor,
~408 B of which is the repeated deflated chassis configuration; that is the
single clearest lossless win and it attacks the anchor floor that dominates the
`idle`/`drive` budgets. In parallel, a keyed or binary `state.chassis` section
addresses the largest world-side compressed contributor for `fire` and
`twelve`. Do not start precision changes until those lossless candidates are
measured against this baseline.
