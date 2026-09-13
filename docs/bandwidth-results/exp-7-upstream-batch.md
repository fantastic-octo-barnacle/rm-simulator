<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Experiment 7 — compact repeated upstream input-batch fields

Investigation handoff: [`../bandwidth-experiments.md`](../bandwidth-experiments.md),
experiment 7 ("Compact upstream repetition, then retest pacing"). This report
covers one lossless wire change plus the in-process attribution measurement, and
answers whether a pacing change is justified. It does not lower the sample rate
or the press/release redundancy.

## Revisions and provenance

| Item | Revision |
|---|---|
| Branch | `perf/bw-exp7` |
| Worktree base | `7d07f1c` (`feat(net): add bounded tracing and in-process owner channels`) |
| Shared probe imported, unchanged except the one magic line below | upstream commits `3a41fcc`, `97bc488`, `8c2a13b`, `47a553d` (`perf/bw-exp1` tip), cherry-picked onto this branch as `e86ec9a`, `1cf05f5`, `9af7dbb`, `989047f` |
| Baseline capture revision ("before") | `989047f`, candidate edits reverted from the working tree |
| Candidate ("after") | this commit; `INPUT_BATCH_MAGIC = b"RMI3"`, `PROTOCOL_VERSION = 28` |
| Instrument | `crates/rm-simulator-server/src/bandwidth_probe.rs` at `47a553d` (cherry-picked as `989047f`), plus a one-line `account_up` magic update |

The candidate commit message is `perf(net): compact repeated input batch fields`.
Because this file is part of that commit, the commit cannot contain its own SHA;
the candidate SHA is reported to the coordinating agent and is the `perf/bw-exp7`
commit that contains this file.

### Measurement provenance and a discarded-capture warning

The mandated shared `CARGO_TARGET_DIR` (`/Users/hxyulin/dev/RM/rm-simulator/target`)
is used by several worktrees at once. During this experiment `cargo test` several
times reported `Finished` **without** compiling `rm-simulator-server`, and then
ran an artifact built from a different worktree's source: the output carried
foreign warnings (`pacer_sent_bytes`, `old_index`) and foreign `keyed_*` tests.
Those captures are invalid and were discarded:

- discarded: `baseline-raw.txt`, `baseline-fixed-raw.txt`, `baseline-v2-raw.txt`,
  `after-raw.txt`;
- kept: `baseline-v4-raw.txt` and `after-v4-raw.txt` in `/tmp/rm-bw-exp7/`.

Every kept run printed `Compiling rm-simulator-server v0.1.0
(/Users/hxyulin/dev/RM/rm-simulator/worktrees/exp7/crates/rm-simulator-server)`
and contained this experiment's own tests. To force a rebuild while still using
the mandated target directory, the measurement command adds a package-scoped
profile override that changes the server crate's artifact hash and rebuilds only
that crate:

```sh
CFG='profile.dev.package.rm-simulator-server.debug=1'
CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/target \
  cargo test -p rm-simulator-server --locked --config "$CFG" \
  bandwidth_ -- --nocapture
```

The override changes debug info only, not code generation. The candidate result
was reproduced identically under the literal plain command as well (see
`after-v2-raw.txt`), so the config is an isolation device, not a measurement knob.

A second, unrelated hazard is recorded for the family: `git stash` is
repository-global, not per-worktree. A concurrent experiment's stash was popped
into this worktree and this experiment's stash was popped elsewhere. No work was
lost (the foreign diff is preserved as `/tmp/rm-bw-exp7/foreign-wip.patch` and as
dangling stash commits), the final tree was verified to contain this experiment's
`RMI3` edits, and no capture reported below was taken while the foreign files
were applied. No result in this document depends on a stashed tree.

## What changed

`udp_codec.rs` replaces the deflated `RMI2` batch (one count byte, then 80 bytes
per frame: chassis `u32`, epoch `u64`, sequence `u64`, sampled time `u64`,
placement revision `u64`, duration `u32`, five `f64` commands) with `RMI3`:

- one count byte, then one 20-byte header carrying the values the newest frame
  shares: chassis `u32`, input epoch `u64`, placement revision `u64`;
- per frame, a tag:
  - **compact** (`0x01`): an exact changed-value mask over the five command
    values (bits 0..=4, bits 5..=7 must be zero), a flags byte (bit 0: sequence
    absolute; bit 1: sampled time absolute; bits 2..=7 must be zero), a relative
    sequence delta and relative sampled-time delta as unsigned LEB128 varints
    (or an absolute `u64` when frame zero or a backwards delta makes a delta
    unrepresentable), a duration varint, then only the changed command values, each
    at full `f64` bit precision;
  - **legacy** (`0x00`): the previous fixed 80-byte frame, used when a frame's
    chassis/epoch/placement is not the header's, so nothing is quantized or lost.
- the changed-value mask compares `f64::to_bits()`, so `-0.0` is not confused
  with `0.0` and every value round-trips bit for bit;
- the decoder still accepts the older `RMI2` framing, and both encoder and
  decoder enforce the 1,000-byte single-datagram bound, the 12-frame cap, the
  reserved-bit refusals, the inflated-body limit (993 bytes) and a non-finite
  command refusal. The worst constructible batch body (eleven legacy frames plus
  a full compact frame) is 972 bytes, so it still fits one datagram.

`select_inputs` is untouched: the newest four samples plus movement transitions
and the 250 ms window are unchanged. `PROTOCOL_VERSION` moves 27 → 28 because the
client→host wire contract changed; a mixed-version pair is refused at the hello,
and the new decoder can still read an `RMI2` batch.

## Before/after, in-process attribution

One client connection drives one pilot, as the pacer's single replaceable owner
slot requires; twelve players are twelve such peers and this table is the
per-player upstream figure. Workloads and all counters come from the probe at
`UNLIMITED` budget, 10 s each, deterministic replay. "B/frame" is
`batch_bytes / frames`.

### Upstream (authoritative)

| Workload | up B/s before | up B/s after | up kbps before | up kbps after | kbps Δ | batches | frames before/after | batch bytes before | batch bytes after | B/frame before | B/frame after |
|---|---|---|---|---|---|---|---|---|---|---|---|
| idle | 4561.1 | 3371.3 | 36.5 | 27.0 | −26.0% | 625 | 2494 / 2494 | 44,974 | 33,076 | 18.03 | 13.26 |
| drive | 9170.4 | 7072.8 | 73.4 | 56.6 | −22.9% | 625 | 3054 / 3054 | 91,067 | 70,091 | 29.82 | 22.95 |
| fire | 9544.8 | 7447.2 | 76.4 | 59.6 | −22.0% | 625 | 3054 / 3054 | 91,067 | 70,091 | 29.82 | 22.95 |
| twelve | 9544.8 | 7447.2 | 76.4 | 59.6 | −22.0% | 625 | 3054 / 3054 | 91,067 | 70,091 | 29.82 | 22.95 |

`up B/s` is the whole upstream stream (`up_bytes`, batches plus control), so the
fire/twelve rows include the unreliable shot-retry datagrams (`controls` = 97);
the batch-only reduction is −23.0% for drive/fire/twelve and −26.5% for idle.
The identical frame counts before and after are the evidence that the sample rate
and the redundancy selection did not change.

### Downstream and the sum (illustrative only)

The probe's downstream counter is now wired (periodic owner path, feedback loop),
but the probe does **not** apply its scripted pilot input to the simulation, so
its field stays idle: `idle`, `drive` and `fire` downstream are byte-identical and
`projectiles = 0` even for the fire workload. Downstream is therefore an
idle-field figure, not a workload figure, and the sum below is context only — not
an acceptance number and not a substitute for the harness.

| Workload | down B/s (both) | down kbps (both) | up kbps before | up kbps after | sum kbps before | sum kbps after |
|---|---|---|---|---|---|---|
| idle | 45,757.7 | 366.1 | 36.5 | 27.0 | 402.6 | 393.0 |
| drive | 45,757.7 | 366.1 | 73.4 | 56.6 | 439.4 | 422.6 |
| fire | 45,757.7 | 366.1 | 76.4 | 59.6 | 442.4 | 425.6 |
| twelve | 58,513.9 | 468.1 | 76.4 | 59.6 | 544.5 | 527.7 |

Downstream bytes, world fragments, complete checkpoints, selected/independent
compressed bytes and owner-anchor bytes are identical before and after, which
confirms the change is upstream-only.

## Decoded-frame equivalence evidence

The candidate is lossless by construction and tested as such. `fixed_frame()`
is the exact 80-byte encoding the codec used before this change; it is both the
per-frame legacy escape and the equality witness in the tests, so equality is
compared on raw bits (a plain `f64 ==` would not see `-0.0` vs `0.0`).

Tests added (all green in `cargo test -p rm-simulator-server --locked`), with the
workload each covers:

| Test | Evidence |
|---|---|
| `compact_input_batch_round_trips_a_long_randomized_reversal_sequence` | 400 samples, drive direction reversing on almost every sample with full-mantissa aim/steer, 80 batches, bit-exact |
| `compact_input_batch_round_trips_an_aim_sweep` | 200 samples of low-mantissa aim increments, 28 batches, bit-exact |
| `compact_input_batch_round_trips_movement_with_fire` | 120 samples of tracking while firing with stray `Fire` commands in the history, 30 batches, bit-exact |
| `compact_input_batch_round_trips_a_lost_release_and_the_stop_lease` | a lost release keeps driving and the `InputStream` still neutralizes the drive one 250 ms lease after the last applied sample while preserving aim; an arriving release round-trips to a zero drive |
| `compact_input_batch_preserves_negative_zero_and_full_precision` | only the sign of zero changes between frames; both signs survive bit-exactly |
| `compact_input_batch_falls_back_to_the_fixed_frame_for_another_life` | a frame naming another placement revision takes the legacy 80-byte escape and still round-trips |
| `older_fixed_rmi2_batches_still_decode` | the versioned decoder reads the old framing |
| `compact_batch_carries_the_scripted_workload_not_just_its_bytes` | idle vs drive batches are 52 vs 59 bytes but decode to `forward = 0.0` vs `2.0` and `aim = 0.5`, so similar byte counts do carry the workload |
| `compact_input_batch_refuses_malformed_headers_masks_counts_and_oversized_batches` | short header, count 13, count 0, reserved mask bit, reserved flag bit, a packet over 1,000 bytes, an inflated body past the limit; the worst constructible batch still fits one datagram |
| `redundant_history_retains_release_and_stays_below_one_datagram` (existing, unchanged) | release retention, the 250 ms lease, the 12-frame cap and the one-datagram bound still hold |
| `compressed_input_history_preserves_samples_and_bounds_decoding` (existing, unchanged) | a normal four-frame batch round-trips and stays under 1,000 bytes (1012 JSON bytes → 67 bytes) |

The corrected probe also decodes every upstream batch in its measurement loop
(`PeerCodec::receive` on each client datagram), so a decode mismatch would have
failed the measurement itself; all four workloads ran to completion.

Full-suite results after the change:

```
cargo test -p rm-simulator-server --locked        # literal command, own build path
test result: ok. 181 passed; 0 failed   (lib)
test result: ok. 1 passed; 0 failed
test result: ok. 28 passed; 0 failed
```

`udp_codec -- --nocapture` passes 14/14. The earlier six `keyed_*` failures came
from the foreign artifact described above and do not reproduce on this tree.

## Pacing retest

`RM_NET_UP_KIB_S` and `RM_NET_DOWN_KIB_S` are KiB/s application budgets (clamped
to 4..=2048), not kbps on-wire limits; the LAN default is 64 KiB/s and
`RM_NET_PROFILE=limited` uses 10 KiB/s.

| Workload | offered upstream before | offered upstream after |
|---|---|---|
| idle | 4.45 KiB/s | 3.29 KiB/s |
| drive | 8.96 KiB/s | 6.91 KiB/s |
| fire / twelve | 9.32 KiB/s | 7.27 KiB/s |

The compacted offered stream now fits the existing 10 KiB/s `limited` upstream
budget with about 27% headroom; the uncompacted fire workload (9.32 KiB/s) had
almost none, and NET-001's measured 103 kbps (≈12.9 KiB/s) exceeded it. It fits
the 64 KiB/s LAN default by a wide margin, as before.

**No pacing change is justified by this measurement.** The pacer bounds, delays
or replaces offered bytes; it cannot make an over-budget stream smaller, and the
offered stream is already below both configured budgets. Lowering
`RM_NET_UP_KIB_S` would only add a new way to delay control, shot-retry and
baseline-feedback traffic. The sample rate and redundancy were deliberately not
reduced to make a budget fit. A 64 kbps (8 KiB/s) upstream target is now
plausible for this workload before native overhead, but that is a harness
question, not a default change.

## Honest limits

- In-process only. No real UDP/GNS framing, no encryption or native header
  overhead, no congestion control, no impairment (loss, reordering, duplication,
  delay, blackouts), no shared-egress contention.
- The probe measures application datagrams. NET-001's 103 kbps against this
  probe's 76.4 kbps baseline suggests the real path adds roughly 35%, but the two
  workloads differ, so no on-wire claim is made here.
- The probe's downstream is an idle-field figure (its pilot script is not applied
  to the simulation); the sum table above is context, not an acceptance number.
- The probe drives one client connection = one pilot. Twelve-player upstream is
  twelve peers; no multi-peer upstream aggregate was measured.
- The pacer budgets were `UNLIMITED` in this probe, so behavior under a binding
  upstream budget (queueing, control expiry, replace/expire counts) was not
  exercised.
- No shot-timing change was measured. Cadence and redundancy are unchanged and
  the codec is bit-exact, so timing cannot change as a side effect, but no
  end-to-end click-to-shot timing test was run.
- The protocol version was bumped; compatibility with a real older client was not
  exercised beyond the decoder test for `RMI2`.
- Decode CPU: the probe's `up_decode` counter is coarse and shared with
  downstream decode; it shows no clear regression (idle +13%, drive +1%, fire
  −1%), but this is not a controlled CPU benchmark.

## Recommendation

- **Full harness trial: yes.** The change is lossless (bit-exact against the
  previous 80-byte encoding), keeps cadence and redundancy, is bounded and
  versioned, and cuts in-process upstream application bytes by 22–27%
  (36.5 → 27.0, 73.4 → 56.6, 76.4 → 59.6 kbps; 18.0 → 13.3 and 29.8 → 22.9
  bytes per frame). The absolute win is real but smaller than a 50–64 kbps target
  implies, because deflate already compressed the repeated identity; native
  overhead and any further sample/redundancy sweep are the remaining gap. Only
  the harness can say whether the on-wire upstream rate lands in target.
- **Pacing change: no.** The compacted offered stream fits the existing 10 KiB/s
  and 64 KiB/s budgets with margin; lowering a budget cannot reduce an offered
  stream and risks control/retry starvation. Retest pacing only after a harness
  trial shows the offered stream still near a configured limit.
