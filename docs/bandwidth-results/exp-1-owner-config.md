<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Experiment 1: reference the owner configuration by identity

Step 1 of [bandwidth-experiments.md](../bandwidth-experiments.md): stop
repeating the deflated `ChassisConfig` in every RMO3 owner anchor. The anchor
now names an immutable configuration revision, and the configuration itself
travels once on the reliable control lane after the peer acknowledges it.

All numbers come from the in-process attribution probe
`crates/rm-simulator-server/src/bandwidth_probe.rs` on a `ManualTime` clock at
fixed 2 ms steps. No socket, no wall clock, no native transport; this is a
codec measurement, not a harness trial.

## Revisions and commands

- **Baseline revision under measurement:** `f4cd613` (`test(net): adopt the
  canonical bandwidth attribution probe`) with the experiment's feature absent.
  Its networking behaviour is `7d07f1c`; every probe change is test-only.
- **Probe revision:** the canonical probe from `perf/bw-exp0` commit
  `48f20e6` (`test(net): add and run bandwidth attribution probe`), adopted as
  `f4cd613` and measured there. A following `style(net)` commit applies a
  clippy- and rustfmt-only rewrite of the same `owner_anchor` expression, with
  no behaviour change. The probe models the three host cadences and applies the
  scripted pilot input to the simulation. This supersedes the local probe
  repairs `97bc488`/`8c2a13b`/`47a553d`, which were used only for the first
  reproduction and are not what the tables below measure.
- **Candidate revision:** the `perf(net): reference owner configuration by
  identity` commit on `perf/bw-exp1`, which is this commit (the tip of the
  branch; `git log -1 --format=%H`). It is measured from `f4cd613` plus the
  feature diff below.
- **Cross-check:** experiment 0 measured the same baseline on `perf/bw-exp0`
  and reported remote idle 368.3, drive 453.8, fire 741.0, twelve 955.1 kbps,
  owner 211.8 kbps, anchor 846 bytes. This worktree's independent baseline run
  reproduced all of those exactly.

### Commands

```sh
# Build and run the verified probe capture (the whole-crate command).
CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/target \
  cargo test -p rm-simulator-server --locked bandwidth_attribution_baseline -- --nocapture

# Whole crate, including doctests. The extra profile override is a harness
# workaround, not a code change (see below).
CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/target \
  cargo test -p rm-simulator-server --locked \
  --config 'profile.dev.package.rm-simulator-server.codegen-units=32'
```

### How the captures were actually taken (shared target directory)

All worktrees share `CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/target`
and the lib-test artifact name `rm_simulator_server-<metadata-hash>` is the same
in every checkout, because the hash does not include the workspace path. A
concurrent `cargo test` from a sibling worktree can therefore be executed
instead of this one: during this experiment a plain run reported exp2's
`keyed_*` tests, and `--config 'profile.dev.package...debug=1'` (a fix another
worktree had already used) still resolved to exp7's artifact.

Verified captures were taken by touching this crate's sources, building,
immediately copying the lib-test binary, and checking that the copy contains
markers unique to this build (`probe cadence=` and the new test names) before
running the copy:

```sh
cd /Users/hxyulin/dev/RM/rm-simulator/worktrees/exp1
touch crates/rm-simulator-server/Cargo.toml crates/rm-simulator-server/src/*.rs
CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/target \
  cargo test -p rm-simulator-server --locked --no-run
cp /Users/hxyulin/dev/RM/rm-simulator/target/debug/deps/rm_simulator_server-<hash> \
  /tmp/exp1/server-tests-candidate
grep -a -c 'owner_config_is_carried_once' /tmp/exp1/server-tests-candidate   # must be > 0
/tmp/exp1/server-tests-candidate --nocapture bandwidth_attribution_baseline \
  > /tmp/exp1/canonical-candidate.log
```

The whole-crate run was made reliable by pinning a per-package profile that no
sibling worktree had used (`codegen-units=32`), which gives this crate a
private artifact hash inside the shared target directory. It reported
`183 passed; 0 failed` for the lib tests, `1 passed` for the binary, and
`28 passed; 0 failed` for the doctests. The baseline probe was captured the
same way from `f4cd613`.

- Baseline raw capture: `/tmp/exp1/canonical-baseline.log`
- Candidate raw capture: `/tmp/exp1/canonical-candidate.log`
- Both are outside Git, per the handoff.

## What changed on the wire

- `OwnerAnchor` (`owner_stream.rs`) replaces the `u16` length plus deflated
  JSON configuration with an eight-byte `ConfigRevision`, the 64-bit FNV-1a of
  the canonical configuration JSON. The identity is a pure function of the
  configuration: the same values always name the same revision, changed values
  never reuse the old one, and nothing is inferred from "the host already sent
  it".
- `ServerMessage::OwnerConfig` carries the configuration once on the reliable
  control lane, deflated like every other control frame. `PeerCodec::receive`
  answers it with the existing RMA1 feedback envelope
  (`Feedback::ConfigStored` / `ConfigMissing`).
- The host resends an unacknowledged configuration every 16 anchor
  opportunities (about 0.5 s at 32 ms), mirroring the baseline `Retire`
  schedule, and emits **no anchor** until the peer acknowledges. An unknown
  reference is dropped by the client and answered with a bounded request; the
  connection is not failed.
- The anchor magic moved `RMO3` → `RMO4` and `PROTOCOL_VERSION` 27 → 28,
  because the anchor layout and the message set are not interchangeable with
  the old wire contract. Both ends must be rebuilt together.

## Results

Anchor size, one chassis, four wheels:

| | bytes |
|---|---:|
| baseline RMO3 anchor | 846 |
| candidate RMO4 anchor | 444 |
| removed embedded configuration (deflated JSON + length) | 402 |
| configuration share of the baseline anchor | 48.2% |

The anchor is 402 bytes (47.5%) smaller. The nominal owner rate is
`444 × 31.25 × 8 = 111.0 kbps`; measured delivered owner traffic was
`13852.8 B/s = 110.8 kbps`. At 32 ms the residual numeric anchor is still 444
bytes, so the owner stream alone remains ~111 kbps. **This experiment does not
reach the 200 kbps downstream budget by itself.**

Remote cadence (one pilot, 32 ms publications only, 10 s; the cadence that
matches the 855 kbps NET-001 figure):

| workload | baseline kbps | candidate kbps | Δ kbps | baseline owner | candidate owner | world (both) |
|---|---:|---:|---:|---:|---:|---:|
| idle | 368.3 | 267.7 | −100.6 | 211.8 | 110.8 | 155.9 |
| drive | 453.8 | 353.1 | −100.7 | 211.8 | 110.8 | 241.4 |
| fire | 741.0 | 640.4 | −100.6 | 211.8 | 110.8 | 528.6 |
| twelve | 955.1 | 854.5 | −100.6 | 211.8 | 110.8 | 742.8 |

Owner cadence (4 ms anchors plus 32 ms world, 5 s): total 1850.8 → 1046.9
(idle), 1934.8 → 1130.8 (drive), 2150.8 → 1346.9 (fire), 2365.1 → 1561.2
(twelve) kbps; owner 1692.0 → 887.3 kbps on every workload.

Every-publication counterfactual (5 s): total 2945.0 → 2141.0 (idle),
3578.8 → 2774.9 (drive), 5270.9 → 4467.0 (fire), 6780.6 → 5976.7 (twelve)
kbps; owner 1692.0 → 887.3 kbps.

The saving is **not** owner-only: a remote pilot's 32 ms publication carries an
owner anchor (`PeerCodec::send` emits one for every periodic frame), so the
reduction applies to the remote path that the 855 kbps measurement describes.
World bytes are identical before and after in every cell, which is the expected
ablation signature.

Control bytes rose from 63.9 to 111.0 B/s over the remote runs: the one
configuration frame (471 bytes including its 21-byte RMG1 header) is the only
new downstream traffic, and it is sent once per connection. One publication in
313 carried no anchor because it offered the configuration instead
(`produced_owner_updates=312`, `produced_world_updates=313`).

### Offered versus delivered owner bytes

The probe reports `produced_owner_bytes` from the host encoder counters and
`down_owner_bytes_s` from the datagrams the client actually received:

| run | produced updates | produced bytes | delivered bytes | delivered anchors |
|---|---:|---:|---:|---:|
| baseline remote | 313 | 264 798 | 264 798 | 313 |
| candidate remote | 312 | 138 528 | 138 528 | 312 |

Produced and delivered owner bytes are exactly equal, so no anchor was replaced
or expired: the delivered owner rate is 31.25 Hz (313 anchors / 10 s baseline,
312 / 10 s candidate), not 62.5 Hz. The apparent "about twice as many
datagrams as anchors" in `down_packets=655` is 313 anchors plus 333 world
fragments plus 9 control datagrams, not dropped anchors.

Why no replacement, by code path: `Pacer::owner` (`pacing.rs:206`) replaces the
single owner slot and counts `replaced` only when a slot is already occupied;
`Pacer::next` (`pacing.rs:253`) serves class 1 with `self.owner.take()`
(`pacing.rs:306`) once the token bucket allows it. The probe drains
`PeerCodec::next` every 2 ms step with a 64 MiB/s budget, whose burst allowance
is `max(rate × 0.032, 1024)` ≈ 2 MiB, so the 444-byte anchor leaves the slot on
the same step it is queued; the next `owner(...)` call is 32 ms later, when the
slot is empty. The equality above confirms `replaced` stayed zero for the owner
class in both runs.

## Test coverage

Wire format and identity (`owner_stream.rs`):

- `config_identity_is_content_derived_and_never_reused_for_changed_values`
- `anchor_round_trips_against_its_named_configuration_only`
- `anchor_still_rejects_malformed_input`
- `reference_replaces_the_embedded_configuration_for_fewer_bytes`

Handshake and connection lifecycle (`udp_codec.rs`):

| Scenario | Test |
|---|---|
| first join (config once, then referenced) | `owner_config_is_carried_once_then_anchors_reference_it` |
| lost configuration frame (bounded resend) | `unacknowledged_config_is_resent_only_on_the_bounded_schedule` |
| unknown reference defers and recovers | `unknown_anchor_reference_defers_and_recovers` |
| respawn / placement change (changed identity) | `changed_owner_configuration_is_not_confused_with_the_old_one` |
| epoch reset | `a_configuration_reference_survives_an_input_epoch_reset` |
| reconnect | `a_reconnecting_peer_reestablishes_the_configuration_before_anchors` |

The whole crate passes: 183 lib tests, 1 binary test and 28 doctests, 0 failed.

## What is not covered

- **No impairment run.** Every capture is lossless: no packet loss, reordering,
  duplication, delay or blackout, and no scripted link. The bounded resend and
  the recovery request are unit-tested, but `ConfigStored`/`ConfigMissing` were
  never exercised under loss, and the host's application-level resend was never
  raced against native retransmission.
- **No real UDP or GNS socket.** The probe drives the codec with no socket and a
  manual clock; `gns_transport.rs` was not run, so the reliable lane's real
  retransmission, RTT and congestion behaviour of the configuration frame is
  unmeasured.
- **No compression-interaction study.** The configuration frame is deflated
  JSON at level 1 like other control messages; its interaction with the
  acknowledged-baseline encoder and with TCP is not measured.
- **No ACK-loss soak.** A long run where every `ConfigStored` is lost would keep
  anchors suppressed for the connection's life (the correction path degrades,
  while full checkpoints keep the client correct). That behaviour was reasoned
  from the code, not measured.
- **No CPU claim.** The probe prints encode/decode times, but these captures ran
  while sibling worktrees were compiling, so the differences are not a
  controlled measurement and no CPU result is asserted.
- **The probe never delivers a `Welcome`**, so its client counts delivered
  anchors but does not assert app-side application; that is covered only by the
  unit tests above.
- **Multi-peer handshake is untested.** The `twelve` workload is one peer
  against a twelve-chassis world; twelve simultaneous config handshakes are not
  exercised.

## Recommendation

**Take it forward to a full harness trial.** The change is lossless, the
protocol is explicitly versioned, the saving is real on the remote path, and
the whole crate is green. It removes ~101 kbps from every remote workload
(about 27% at idle, 14% under sustained fire) and 402 of the 846 bytes in every
anchor.

Before accepting it, the harness trial must add what this experiment lacks:
a real-UDP run with the pinned binary pair, 1/5/10% loss plus blackouts to
exercise the configuration handshake and its resend, and a check that anchors
stay suppressed but recoverable when the configuration acknowledgement is lost.
It should not be marketed as meeting NET-001: the residual anchor is still 444
bytes (~111 kbps at 31.25 Hz), and world checkpoints dominate under fire and
twelve-player load. Expect roughly the measured −100 kbps per remote pilot, not
a path to the 200 kbps budget on its own.
