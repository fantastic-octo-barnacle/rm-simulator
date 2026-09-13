<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Experiment 3: replace the verbose chassis checkpoint losslessly

Prototype and measurement for step 3 of
[bandwidth-experiments.md](../bandwidth-experiments.md): fixed-schema binary
fields, presence/zero masks and a shared configuration table for one hot
section, compared against the existing JSON compact checkpoint **after deflate
and after application framing**, with encode/decode CPU.

This is an in-process probe result, not a network trial. No socket, no
impairment, no quantization; see [Limitations](#limitations).

## Chosen section and why

`state.chassis`, from experiment 0's compressed ablation
([exp-0-attribution.md](exp-0-attribution.md)): 1097 B/frame on `fire` (37% of
the 2951 B independent checkpoint) and 2501 B/frame on `twelve` (52% of
4845 B), the largest single section in both workloads. That attribution came
from the `fire`/`twelve` independent frames; my own probe reproduces its
*independent frame* totals to within 2% (3597/6182 B in experiment 0 against
3664.5/6106.8 B here) and its *selected* stream exactly
(600/942/2055/2896 B per frame), so I attacked the section experiment 0
ranked first rather than re-litigating the choice.

`state.projectiles` (520/530 B) is the second world-side candidate and stays
JSON here; `state.restore` is required by `Field::restore` and is never a
removal candidate.

## Revision and commands

- **Worktree:** `/Users/hxyulin/dev/RM/rm-simulator/worktrees/exp3`, branch
  `perf/bw-exp3`.
- **Base revision:** `7d07f1c` (`feat(net): add bounded tracing and in-process
  owner channels`) with experiment 0's probe cherry-picked as `94e5f00`,
  `e881558`, `5b1f458` (the `3a41fcc`/`48f20e6`/`c9ee3d2` chain from
  `perf/bw-exp0`). The probe file under measurement is the `48f20e6` revision
  with the cadence model and the workload-sensitivity assertions; the
  cherry-pick adds no changes to it beyond one clippy lint fix
  (`needless_question_mark` in `owner_anchor`).
- **Commit under measurement:** `f4c15e559c1c3ab697aff5ba6d647aafcd34d0e5`
  (`perf(net): prototype a binary player checkpoint section`), which contains
  the prototype, the probe additions and this results file.
- **Shared target directory:** every cargo command uses
  `CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/target`. The
  `rm_simulator_server-<metadata-hash>` artifact name is the same in every
  worktree and cargo's mtime fingerprint can treat a sibling worktree's fresh
  artifact as this one's, so a plain `cargo test` can silently execute another
  branch's code. The capture was therefore taken by touching this worktree's
  server sources, building with a private artifact hash, copying the freshly
  linked test binary next to the worktree, and verifying the copy contains this
  experiment's unique marker before running it:

  ```sh
  cd /Users/hxyulin/dev/RM/rm-simulator/worktrees/exp3
  CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/target \
    cargo test -p rm-simulator-server --locked --no-run \
    --config 'profile.dev.package.rm-simulator-server.debug=1' \
    --config 'profile.dev.package.rm-simulator-server.codegen-units=32'
  cp /Users/hxyulin/dev/RM/rm-simulator/target/debug/deps/rm_simulator_server-966fc3f7df417abb \
    target-probe/probe-lib-tests
  strings target-probe/probe-lib-tests | grep 'binary workload='   # provenance
  ./target-probe/probe-lib-tests bandwidth_ --nocapture --test-threads=1 \
    > /tmp/bw-exp3-capture.txt
  ```

  **Capture trusted:** `target-probe/probe-lib-tests`, hash
  `rm_simulator_server-966fc3f7df417abb`, with the compile line naming
  `worktrees/exp3`, containing `binary workload=` (this experiment's marker) and
  `probe cadence=remote`, and running the full probe set
  `bandwidth_attribution_baseline`, `bandwidth_attribution_sections`,
  `probe_replays_byte_for_byte`, `probe_accounting_is_consistent`,
  `probe_is_workload_sensitive`, plus this experiment's
  `bandwidth_binary_checkpoint` and `bandwidth_compressor_effort`.
- **Correctness suite:** `cargo test -p rm-simulator-server --locked` with the
  same two `--config` overrides rebuilt and ran this worktree's artifact:
  `183` lib tests, `1` bin test and `30` doc tests pass. The `--config`
  overrides only change the server crate's artifact hash; they do not change any
  measured byte.
- **Workloads:** the probe's documented remote cadence (32 ms world
  publications, 31.3 frames/s, 313 frames in 10 s), unlimited pacer, immediate
  baseline feedback, one scripted pilot, no loss. `idle`/`drive`/`fire` are one
  chassis, `twelve` is twelve.

## What was prototyped

`crates/rm-simulator-server/src/binary_checkpoint.rs` is a fixed-schema binary
representation of the compact checkpoint's `state.chassis` section:

- **Explicit field order.** 39 `f64` of configuration plus the full
  `ChassisSnapshot` are written with no keys and no separators.
- **Presence / zero masks.** One flag byte per chassis marks groups whose value
  is exactly zero (`angular_velocity_rad_s`, `command`, `held_aim_rad`,
  `gimbal_velocity_rad_s`, plus `defeated` and an empty wheel array); one flag
  byte per wheel carries the optional `WheelContact` and a zero `target_m_s`.
  A set bit means the bytes are absent and the decoder reconstructs the exact
  zero value, so the mask is lossless, not a precision change.
- **Shared configuration table.** Each distinct `ChassisConfig` appears once per
  frame and every chassis carries its table index, so twelve identical robots
  ship one configuration (`shared_configurations_collapse_to_one_table_entry`).
- **Full f64 dynamics.** Every dynamic value is an IEEE-754 `f64`; nothing is
  quantized. The only derived-away fields are the ones the existing JSON compact
  checkpoint already derives (`rune.target_poses`, `outpost.armors`,
  `wheels[].contact`, failed `hits`), and the chassis schema still has a
  presence bit for wheel contact.
- **Bounded decoding.** Counts, lengths and the whole input are capped; every
  read is bounds-checked; an unknown version, a non-zero reserved flag, a
  non-finite float, an out-of-range configuration reference or trailing bytes
  are rejected, and a decode returns a complete state or an error, never a
  partial one.
- **Raw-byte wire.** `udp_snapshot::BinaryWire` (`RMB2`) carries the encoded
  checkpoint as opaque bytes with no base64 or JSON escaping; the JSON envelope
  keys are omitted. The pinned baseline in `Encoder`/`Decoder` remains a decoded
  `Value`, so a binary frame serves as a baseline and a delta target under the
  unchanged two-baseline bound. `PeerCodec::enable_binary_checkpoints` is
  test-only and changes no production call site.

## Compressed and framed bytes before/after

All values are per published world frame, averaged over 313 frames of a 10 s
remote-cadence run. "independent" is the deflated self-contained comparison
frame; "selected" is what the acknowledged-delta encoder actually emitted
(`selected_bytes / 313`); "framed" adds the 21-byte application fragment headers
(`framed_world_bytes / 313`). Owner and control bytes are unchanged.

| Workload | independent JSON | independent binary | saved | selected JSON | selected binary | framed JSON | framed binary |
|---|---:|---:|---:|---:|---:|---:|---:|
| `idle` | 2248.6 | 1792.9 | −455.7 (−20.3%) | 600.5 | 577.5 | 622.8 | 599.2 |
| `drive` | 2489.8 | 2001.9 | −488.0 (−19.6%) | 941.8 | 917.3 | 964.2 | 939.6 |
| `fire` | 3664.5 | 3218.9 | −445.6 (−12.2%) | 2054.8 | 2030.3 | 2111.2 | 2086.6 |
| `twelve` | 6106.8 | 4836.4 | −1270.3 (−20.8%) | 2895.9 | 2840.6 | 2966.4 | 2909.5 |

Fragments per 10 s run: `idle` 333→323, `drive` 333→332, `fire` 841→839,
`twelve` 1051→1027. Complete reassembled checkpoints are 313 in every run for
both representations.

**The twelfth workload is where the format does most in absolute terms** — it
saves 1270 B on the independent frame, because the configuration table collapses
twelve identical 316-byte configurations to one and the per-chassis records
carry no keys. But against the *selected* stream the `twelve` saving is only
55.3 B/frame (1.9%): the acknowledged deltas already avoid re-sending the
chassis section, and the delta patches remain JSON.

## Resulting remote downstream and the gap to 200 kbps

Total downstream includes the unmodified owner stream (211.8 kbps, the 846-byte
RMO3 anchor at 31.25 Hz) and the 0.5 kbps control lane.

| Workload | down kbps JSON | down kbps binary | world JSON | world binary | gap to 200 kbps (binary) |
|---|---:|---:|---:|---:|---:|
| `idle` | 368.3 | 362.4 | 156.0 | 150.0 | −162.4 |
| `drive` | 453.8 | 447.6 | 241.4 | 235.3 | −247.6 |
| `fire` | 741.0 | 734.8 | 528.6 | 522.5 | −534.8 |
| `twelve` | 955.1 | 940.9 | 742.8 | 728.5 | −740.9 |

The JSON column reproduces experiment 0's remote baseline exactly
(368.3/453.8/741.0/955.1 kbps), which is the harness's own validation. The
binary checkpoint removes 1.5–1.6% of total downstream and 1.2–3.8% of the
selected world stream. It does not move the budget: the residual gap stays
162–741 kbps above the 200 kbps target, and 211.8 kbps of that is the owner
anchor alone, which this experiment does not touch.

## Encode/decode CPU

Wall-clock `Instant` timings from the same capture. Timings on this shared
machine are noisy — other worktrees were compiling into the same target
directory throughout — so read the byte columns as results and these as
indicative. Per-frame columns are the isolated representation cost measured in
the probe observer; `codec_*` are the production codec paths
(`PeerCodec::send` and client receive) over the whole run.

| Workload | encode ns/frame JSON | binary | decode ns/frame JSON | binary | codec encode µs JSON | binary | codec decode µs JSON | binary |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `idle` | 58109 | 44522 | 91754 | 61275 | 140915 | 95881 | 50003 | 33671 |
| `drive` | 47803 | 38125 | 52151 | 45776 | 119933 | 115566 | 42067 | 40939 |
| `fire` | 73948 | 60704 | 73832 | 61046 | 159136 | 148876 | 61468 | 56409 |
| `twelve` | 160642 | 89666 | 256173 | 184574 | 508957 | 912701 | 176429 | 335645 |

The isolated binary encode/decode is consistently faster than JSON in this
capture (a hand-rolled writer over the same state visit, and a smaller remaining
JSON text). The `twelve` codec encode row reverses in this capture and did not
in earlier ones (`719760` binary against `777258` JSON in one run), which is the
noise, not a reproducible regression.

One correctness-meets-performance finding is worth recording: the encoder's
pinned baseline must be *value-identical* to the JSON wire's, and the cheap way
to get it is to parse the remainder text and splice the chassis as a `Value`.
Building it with `serde_json::to_value` on the struct instead widens the hoisted
projectile wires' `f32` fields to full-precision `f64`; every patch then rewrites
every projectile number. The first capture did exactly that and `fire` selected
**838687** bytes against JSON's 643140, a 30% regression caused solely by float
text representation. The probe now asserts baseline value equality on every
frame; see the evidence below.

## Compressor-effort sweep

Deflating the two representations of the same independent frame at increasing
`miniz_oxide` levels, 6 s runs (`fire` 188 frames, `twelve` 188 frames):

| Workload | representation | level 1 | level 4 | level 6 | level 10 |
|---|---|---:|---:|---:|---:|
| `fire` | JSON | 3465.5 | 2989.6 | 2976.4 | 2976.4 |
| `fire` | binary | 3018.2 | 2643.7 | 2632.5 | 2632.5 |
| `twelve` | JSON | 5685.9 | 4486.4 | 4404.2 | 4402.0 |
| `twelve` | binary | 4510.2 | 3755.4 | 3711.4 | 3690.7 |

Raising the level from 1 to 4 cuts the independent frame by 12–14%,
independently of representation, and level 6 gains almost nothing more. The
recorded deflate CPU roughly doubles from level 1 to level 4, but in absolute
terms it is 0.06–0.22 ms/frame, i.e. well under 1% of a 32 ms publication
budget; the level-4 timings above are too noisy to price more precisely. This
sweep only deflates the independent frame — the selected delta stream would need
its own measurement — but a 12–14% cut that applies to every frame kind
including deltas is a larger and far cheaper lever than the 1.2–3.8% the binary
representation delivered on the selected stream.

## Reconstruction and restore evidence

The binary path is not a new checkpoint contract: it is the same compact
checkpoint with a different chassis encoding, and the remainder JSON (including
`state.restore`, the rule state `Field::restore` requires) is carried untouched.
Nothing new is dropped. The chassis schema carries every `ChassisSnapshot`
field, including a presence bit for `wheels[].contact`, which the existing JSON
compact path already derives away in `PlayerSnapshot::from_state` and
`Chassis::restore_prediction` rebuilds from the wheel ray.

Passing tests, all in the library test binary:

- `binary_checkpoint::tests::round_trip_matches_the_json_checkpoint_exactly` —
  a decoded binary checkpoint equals the decoded JSON checkpoint field for
  field, `state.restore` included.
- `binary_checkpoint::tests::moving_and_firing_state_round_trips` — the same
  equality after 40 input frames of driving, aiming and firing with projectiles
  in flight.
- `binary_checkpoint::tests::pinned_baseline_value_matches_the_json_wire_for_f32_projectiles`
  — the `Value` a binary frame pins is *identical* to the JSON wire's, which is
  what makes the delta target shared and is the regression test for the
  float-widening defect above.
- `binary_checkpoint::tests::projectiles_round_trip_and_extreme_ones_fall_back`
  — projectile wires survive their f32 compaction and a projectile that cannot
  survive f32 falls back to the JSON checkpoint rather than losing precision.
- `binary_checkpoint::tests::decode_rejects_truncation_bad_version_flags_and_non_finite`
  — every truncation prefix of a valid frame is rejected, as are a bad version,
  a reserved flag bit, a NaN field and trailing bytes.
- `binary_checkpoint::tests::shared_configurations_collapse_to_one_table_entry`
  — one configuration entry for two identical chassis.
- `udp_snapshot::tests::binary_wire_pins_baselines_and_deltas_like_the_json_wire`
  — 200 frames driven through both wires in lockstep decode to the same message
  and the same feedback, keep the decoder at ≤2 baselines, and the binary wire
  is smaller overall (197355 against 266541 independent bytes).
- The existing `udp_snapshot` tests for retirement acknowledgement, missing
  baselines, epoch resets, loss/reordering/duplication and the two-baseline cap
  now run through the shared decoder core the binary path also uses.

The probe additionally asserts, on every produced frame of all four workloads,
that the two wires pin the same baseline value and that both runs deliver 313
complete checkpoints with the same produced-update count.

## Limitations

- **In-process only.** No socket, no GNS/UDP/IP headers, no native pacing or
  outbox replacement. `framed_*` counts only the 21-byte application fragment
  header; IP/UDP/GNS overhead is excluded, exactly as in experiment 0.
- **No impairment.** No loss, reordering, duplication, delay, jitter or
  blackouts, and the pacer budget is unlimited. Under loss the encoder re-sends
  full baselines more often, so the independent-frame win (12–21%) matters more
  there than the 1.2–3.8% selected-stream win measured here; that case is
  unmeasured.
- **One pilot per connection.** `twelve` is twelve world states on one wire,
  with only the first pilot commanded; host egress contention across twelve real
  peers is not modelled.
- **CPU is noisy.** Wall-clock on a shared machine with concurrent cargo builds;
  no CPU pinning and no repeated-seed statistics.
- **The delta patches are still JSON.** Only the checkpoint/independent payload
  is binary. Identity-based binary deltas are experiment 2 and are not measured
  here.
- **No quantization.** Every dynamic value is a full `f64`; this experiment
  says nothing about f32 or fixed-point bounds.

## Recommendation

- **Do not take the binary checkpoint to a full harness trial on this result.**
  It buys 1.2–3.8% of the selected world stream and 1.5–1.6% of total
  downstream, while the residual gap to 200 kbps is 162–741 kbps and the
  unchanged owner anchor alone is 211.8 kbps. A wire-contract change of this
  size is not justified by that. If it is trialled at all, trial it under loss
  and recovery, where the 12–21% independent-frame saving is the number that
  matters, not on a clean unlimited link.
- **No new dependency is warranted** (the default answer holds). The prototype
  adds no crate; it uses the `miniz_oxide`, `serde` and `serde_json` already in
  the tree, and the measured result does not change that.
- **The measurement points elsewhere.** The world bytes now live in the JSON
  delta patches, so experiment 2 (identity-based delta patches) is the
  world-side target; experiment 1 (owner configuration references, a measured
  211.8→110.8 kbps) is the larger single win. Separately, raising the existing
  compressor from level 1 to level 4 cuts 12–14% off the independent frame for
  negligible CPU and should be measured on the selected stream before any format
  work.
