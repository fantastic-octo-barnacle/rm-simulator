<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Experiment 2: make world deltas follow entity identities

Step 2 of [bandwidth-experiments.md](../bandwidth-experiments.md). The prototype
is implemented, lossless and correct, and **it does not reduce a single byte of
production traffic**: the identity-keyed patch is offered on 484 of 44,597 array
decisions in the probe's workloads and is never the smallest candidate, so the
encoder's existing size guard never selects it. The measured candidate output is
byte-for-byte identical to the baseline in every workload.

This is a negative result. It is reported as measured, not as a failure of the
probe: the ablation that motivated the hypothesis was correct about where the
bytes are, and the reason keying does not help here is a property of the data,
explained below.

## Revision and commands

- **Base revision under measurement:** `7d07f1c` (`feat(net): add bounded
  tracing and in-process owner channels`). `perf/bw-exp2` is that commit plus
  this experiment's working tree.
- **Canonical probe:** `crates/rm-simulator-server/src/bandwidth_probe.rs` from
  `perf/bw-exp0` at `48f20e6` + `c9ee3d2` (the corrected probe that applies pilot
  input, fires, loops baseline feedback, marks publications `periodic` and
  reports `probe cadence=`). It differs from that canonical file in exactly one
  function and nowhere else: `owner_anchor` had its redundant `Some(..)?` wrapper
  removed to satisfy `clippy -D warnings`. The only other support it needs is the
  `#[cfg(test)]` `PeerCodec::encoding_stats` accessor in `udp_codec.rs` (present
  in both worktrees) and the `#[cfg(test)] mod bandwidth_probe` declaration in
  `lib.rs`.
- **Measurement command:**

  ```sh
  CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/target \
    cargo test -p rm-simulator-server --locked bandwidth_ \
      --config 'profile.dev.package.rm-simulator-server.codegen-units=32' \
      -- --nocapture --test-threads=1
  ```

  The `--config` override gives the server crate a private artifact hash, and the
  freshly built test binary was copied to `/tmp/bw-exp2/bin/` and checked to
  contain both the probe's `every-publication` marker and this experiment's
  `keyed_*` tests before its output was trusted: the shared target directory
  otherwise lets a sibling worktree's `rm_simulator_server-<hash>` be executed
  (observed twice during this experiment; see the parent's build-provenance
  note). Every number below comes from a copied, marker-verified binary.
- **Commit added by this experiment:** `git log perf/bw-exp2` names it; the
  substantive commit is `perf(net): key world deltas by entity identity` and it
  contains the keyed patch, its tests, the probe and this file. This file cannot
  record its own commit hash without changing it, so the log is authoritative.
  Both measurements below were taken from this working tree; the parent commit
  `7d07f1c` is the baseline revision.
- **Baseline capture:** the same command against `git checkout HEAD --
  snapshot_codec.rs udp_codec.rs` with the same probe, built and run the same
  way.
- **Raw captures:** `/tmp/bw-exp2/baseline-probe.txt`,
  `/tmp/bw-exp2/candidate-probe.txt` (the canonical accepted output).

## What was implemented

`crates/rm-simulator-server/src/snapshot_codec.rs` gains an identity-keyed patch
variant beside the existing index patch:

```rust
pub enum Patch {
    Set(Value),
    Map(BTreeMap<String, Patch>),
    List(BTreeMap<usize, Patch>),   // index-based, unchanged
    Keyed(KeyedList),               // new: add / update / remove by identity
}
pub struct KeyedList {
    pub components: Vec<String>,    // identity path(s), e.g. ["id"] or ["[0]"]
    pub insert:  Vec<KeyedChange>,  // whole elements joining the array
    pub remove:  Vec<Key>,          // identities leaving the array
    pub update:  Vec<KeyedChange>,  // whole elements changing in place
    pub order:   Vec<Key>,          // target order, only when not identity order
    pub descending: bool,           // identity order direction
}
```

- **Identity is derived, not declared by the caller.** `identity_components`
  picks the first candidate that is unique and monotonic in *both* arrays, so a
  collection with no usable identity is left to the index path. Candidates are
  object field names (`id`, `projectile`, `shot_id`, `time_ns`, …) and, because
  the compact wire stores projectiles as positional tuples, tuple positions such
  as `[0]`.
- **Ordering is preserved explicitly.** When the target identities are monotonic
  the receiver sorts by identity in the declared direction; otherwise the target
  identity sequence travels in `order` and the receiver rebuilds to match it
  exactly, so a middle deletion or an unsorted history cannot move a survivor.
- **`difference` never emits a larger delta.** All candidates (keyed, index,
  whole-subtree) are serialized and the smallest wins, with the finer candidate
  winning an exact tie. No array becomes a whole-array replacement because the
  keyed candidate was tried.
- **The decoder still refuses what it cannot apply.** A keyed patch whose
  identities are not in the pinned baseline, or whose explicit order names an
  absent identity, is an `InvalidData` error that leaves the baseline intact. The
  one- and two-baseline pinning, `Feedback::Missing`, retirement and epoch checks
  are untouched.
- **The encoder verifies its own key choice** by running the decoder's exact
  inference and application path against the baseline before emitting a keyed
  patch. A key that cannot rebuild the target falls back instead of shipping a
  wrong delta.

## Baseline vs candidate

Every **wire counter** — world bytes, total bytes, kbps, `selected_bytes`,
`independent_bytes`, fragments, owner bytes, complete checkpoints and produced
updates — is identical between the two binaries in all 14 runs. The probe is
deterministic and both runs confirm it: of the 44 fields the probe reports per
run, only the three CPU timings differ (see the CPU section below).

Rates are application bytes per second (decimal kbps), excluding IP/UDP/GNS
headers. `frames` is `produced_world_updates`; the two `twelve` rows marked
"truncated" are captures the machine did not finish inside the run's wall-clock
share (94 of 313 publications), so they are shorter runs, not a different rate —
they are still byte-identical between the two binaries.

| Cadence | Workload | frames | World B/s base | World B/s cand | World kbps base | World kbps cand | Down kbps base | Down kbps cand | `selected_bytes` base | cand | `independent_bytes` base | cand | frags base | cand |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| remote | `idle` | 313 | 19494.1 | 19494.1 | 155.9 | 155.9 | 368.3 | 368.3 | 187,943 | 187,943 | 690,898 | 690,898 | 333 | 333 |
| remote | `drive` | 313 | 30178.2 | 30178.2 | 241.4 | 241.4 | 453.8 | 453.8 | 294,789 | 294,789 | 764,714 | 764,714 | 333 | 333 |
| remote | `fire` | 313 | 66080.1 | 66080.1 | 528.6 | 528.6 | 741.0 | 741.0 | 643,140 | 643,140 | 1,125,899 | 1,125,899 | 841 | 841 |
| remote | `twelve` (truncated) | 94 | 64946.0 | 64946.0 | 519.6 | 519.6 | 732.0 | 732.0 | 189,861 | 189,861 | 457,261 | 457,261 | 237 | 237 |
| owner | `idle` | 157 | 19794.6 | 19794.6 | 158.4 | 158.4 | 1850.8 | 1850.8 | 95,466 | 95,466 | 346,077 | 346,077 | 167 | 167 |
| owner | `drive` | 157 | 30290.0 | 30290.0 | 242.3 | 242.3 | 1934.8 | 1934.8 | 147,943 | 147,943 | 383,272 | 383,272 | 167 | 167 |
| owner | `fire` | 157 | 57298.4 | 57298.4 | 458.4 | 458.4 | 2150.8 | 2150.8 | 278,764 | 278,764 | 518,414 | 518,414 | 368 | 368 |
| owner | `twelve` | 157 | 84085.8 | 84085.8 | 672.7 | 672.7 | 2365.1 | 2365.1 | 410,202 | 410,202 | 873,770 | 873,770 | 487 | 487 |
| every-publication | `idle` | 1250 | 156063.6 | 156063.6 | 1248.5 | 1248.5 | 2945.0 | 2945.0 | 752,388 | 752,388 | 2,755,729 | 2,755,729 | 1330 | 1330 |
| every-publication | `drive` | 1250 | 235294.2 | 235294.2 | 1882.4 | 1882.4 | 3578.8 | 3578.8 | 1,148,541 | 1,148,541 | 3,052,654 | 3,052,654 | 1330 | 1330 |
| every-publication | `fire` | 1250 | 446807.4 | 446807.4 | 3574.5 | 3574.5 | 5270.9 | 5270.9 | 2,173,557 | 2,173,557 | 4,131,450 | 4,131,450 | 2880 | 2880 |
| every-publication | `twelve` (truncated) | 94 | 47168.3 | 47168.3 | 377.3 | 377.3 | 589.8 | 589.8 | 137,620 | 137,620 | 278,678 | 278,678 | 185 | 185 |

`down_owner_bytes_s` is 26479.8 for remote and 211500.0 for owner/every-publication
in both binaries at every workload, as it must be — this experiment changes only
the world delta, never the owner anchor, so experiment 1's owner-side result is
untouched.

**Per-workload reduction: 0 bytes and 0 kbps for idle, drive, fire and twelve, at
every cadence.** The saving does **not** appear on the churning `fire`/`twelve`
workloads, and it does not appear on `idle` either, because the identity patch is
never selected at all. Both the independent and the selected frame sizes are
byte-identical, so this is not a case of a saving hidden by pacing.

The per-section ablation of the independent frame is also byte-identical between
runs, and reproduces experiment 0's table exactly, which is the cross-check that
both captures encoded the same states:

| Section (B/frame) | fire | twelve |
|---|---:|---:|
| `total` | 2951 | 4845 |
| `state.chassis` | 1097 | 2501 |
| `state.projectiles` | 520 | 530 |
| `state.restore` | 386 | 608 |
| `state.referee` | 102 | 330 |
| `state.runes` | 168 | 154 |
| `state.outposts` | 47 | 70 |
| `state.hits` | 0 | −4 |
| `shot_results` | −2 | −3 |

## Collections converted

The identity-keyed patch was wired for every array the codec diffs, which in this
state means the two collections the ablation names as the top costs, plus the
histories beside them:

| Collection | Wire shape | Identity used | Converted |
|---|---|---|---|
| `state.chassis` (1097/2501 B per frame) | objects | `id` | yes |
| `state.projectiles` (520/530 B per frame) | positional tuples | `[0]` (the ball id) | yes |
| `state.hits` | objects | `time_ns`, tie-broken by `projectile` when hits share a tick | yes |
| `shot_results` | objects | `shooter` then `shot_id` | yes |

The codec selects an identity generically: the first candidate that is unique and
monotonic in both the baseline and the target array. Every one of the four above
was verified to produce a lossless keyed candidate on real states (484 accepted
candidates), which is why the negative result is about selection, not about the
collections failing to key.

## Why keying never wins: the measurements

Instrumenting the encoder over the four workloads (44,597 array decisions with a
keyed candidate considered 484 times) gives:

| Outcome | Count |
|---|---:|
| Array decisions seen | 44,597 |
| Keyed candidate produced and verified lossless | 484 |
| **Keyed candidate selected** | **0** |
| Bytes saved by keying | **0** |
| Decisions choosing `Set` with only `Set` offered | 23,773 |
| Decisions choosing `Map` over `Set` | 10,891 |
| Decisions choosing `List` over `Set` | 7,290 |
| Decisions choosing `Set` over `List` | 6,926 |

The two reasons, in order of impact:

1. **Projectiles move every publication.** The projectile wire tuple carries
   `position_m` and `velocity_m_s`, so essentially every live ball is an
   `update` on every frame, not a survivor. A keyed patch then carries one
   `(identity, whole element)` per ball, and the index patch carries one
   `(index, {"position_m": …, "velocity_m_s": …})` per ball — the same moving
   values, with the index patch actually a few bytes cheaper because it does not
   repeat the identity or the `components` header. Keying pays for itself only
   when most elements are *unchanged* between baseline and update, and that never
   happens for this array.
2. **Chassis are few and long.** The chassis array is only 1 element in the
   single-pilot workloads and 12 in `twelve`, and a chassis element is dominated
   by `config` (its geometry), which does not change. Keying would pay there —
   it is the largest section — but with twelve elements it competes against a
   `List` patch whose per-element nested diff is already minimal, and the size
   guard keeps the cheaper one. The 484 keyed candidates are exactly this shape
   (small arrays where the keying header dominates the saving).

Neither reason is a defect in the implementation: the encoder emits the identity
patch for the arrays where it is lossless and structurally necessary, and
declines it wherever the existing index/whole-array patch is smaller, which is
the documented "never larger than replacing the subtree" invariant.

## Tests

All tests live beside the code and pass. `cargo test -p rm-simulator-server
--locked` → 188 lib tests + 1 + 28, 0 failures.

`crates/rm-simulator-server/src/snapshot_codec.rs` (codec level, 7 new):

| Test | Covers |
|---|---|
| `keyed_deltas_keep_survivors_when_a_middle_element_dies_and_a_birth_joins` | middle deletion plus a birth; survivors are not re-sent; a duplicate application lands on the same state |
| `simultaneous_births_and_deaths_keep_the_unchanged_middle` | three deaths and four births in one delta with one moved survivor |
| `keyed_identity_skips_a_field_that_is_not_unique` | a repeating field is not an identity; the decoder still reaches the target |
| `keyed_component_tying_breaks_same_tick_hits` | two hits in the same millisecond; the id component carries the identity |
| `keyed_identity_change_is_a_removal_plus_an_insertion` | an element replaced at the same slot becomes remove + insert, with an explicit order |
| `keying_declines_when_the_index_or_whole_array_patch_is_smaller` | the size guard keeps `List`/`Set` where keying would cost more |
| `keyed_patches_refuse_identities_the_baseline_does_not_hold` | a stale remove and an order naming an absent identity are refused; the baseline is left intact |
| `keyed_deltas_survive_the_wire_and_stay_under_the_replacement` | a churning array keys, beats the replacement, and round-trips through serde |
| `keyed_and_index_deltas_decode_to_the_same_state` | the index and identity paths decode to the same state for a run of checkpoints |

`crates/rm-simulator-server/src/udp_snapshot.rs` (baseline codec, 5 new, all on a
real firing field):

| Test | Covers |
|---|---|
| `the_delta_codec_keeps_up_with_a_firing_field` | 39 checkpoints of a piloted field that drives and fires; live projectiles and a changing array length; index and identity deltas decode to the same state at every checkpoint |
| `a_middle_deletion_and_simultaneous_births_decode_exactly` | a projectile removed from the middle with two added at the end |
| `reordered_and_duplicate_packets_reach_the_same_baseline_state` | two deltas delivered out of order and twice each; every delivery equals the state its own frame encoded |
| `a_stale_baseline_yields_missing_and_never_a_partial_state` | the pinned baseline is dropped; the delta yields `Feedback::Missing` and no state, then the encoder recovers |
| `an_epoch_change_rejects_old_traffic` | a new epoch drops the old delta and the old full frame, and the pinning stays bounded |

The pre-existing retirement, two-baseline-cap, resend and epoch tests still pass
unchanged.

## Encode/decode CPU

The probe's CPU fields are the **only** reported numbers that differ between the
two binaries, and they are not a sound benchmark: both binaries ran on a machine
shared with the other experiment agents, and the direction of the difference is
not stable across repeats. Totals over 5 repeats of
`bandwidth_attribution_baseline` per binary:

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| `down_encode_us` | 19,081,265 | 22,562,211 | +18.2% |
| `up_decode_us` | 6,565,804 | 6,873,358 | +4.7% |
| whole-probe `loop_us` | 34,450,436 | 38,252,891 | +11.0% |

The single captured run in the table above shows the opposite sign (candidate
encode lower), which is the evidence that these figures are load noise rather
than a measurement. A candidate that never selects the new path should not
change encode time materially, and the honest statement is: **no CPU change is
established by this probe.** The mechanism that *could* cost is that every array
decision now also builds and serializes a keyed candidate (and verifies it by
running the decoder's rebuild) before discarding it.

## Honest limitations

- **In-process only.** The probe drives `PeerCodec`, the baseline `Encoder` and
  the `Pacer` against `ClientCodec` on a hand-advanced clock. No socket, no
  native transport, no GNS framing, no OS queueing.
- **Lossless only.** Every identity component and every element value is carried
  at full precision; no quantization or reconstruction was introduced.
- **No impairment.** The probe scripts immediate, lossless feedback. Loss,
  reordering, duplication, delay and blackout behaviour is asserted by the codec
  tests above, not measured under a scripted link.
- **No real UDP and no multi-client host.** Twelve-player load is one peer in a
  twelve-robot world, as in experiment 0; per-peer egress contention and native
  overhead are out of scope.
- **Baseline-rotation interaction is only what the probe exercises.** The probe
  acknowledges promptly, so `Feedback::Missing`, delayed retirement and the
  two-baseline pinning are exercised by unit tests rather than by the probe's
  workloads. A keyed patch is derived from the acknowledged pinned baseline
  exactly like the index patch, so ownership is unchanged, but rotation under
  loss was not measured.
- **The two `twelve` rows marked "truncated" are shorter runs** (94 of 313
  publications) because the machine did not finish them inside the run; they are
  marked as such and no conclusion rests on their absolute rate. They are still
  byte-identical between the two binaries for every wire counter.
- **The negative result is a property of these four workloads.** A workload with
  a large, mostly-static identified collection (many chassis that do not move,
  or projectiles that persist unchanged between publications) would key and
  would save bytes; nothing in the experiment sweeps for one. The churning
  projectile array and the small chassis array are where the budget actually is.

## Recommendation

**Do not run a full scripted-link/impairment trial of this candidate as it
stands.** The probe shows a 0-byte, 0-kbps change in all four workloads at all
three cadences, and the encoder-level instrumentation explains why: the identity
patch is never the smallest candidate for the arrays that carry the bytes. An
impairment trial would add a correct-but-unused wire form and could not show a
bandwidth improvement. The write-up above is sufficient evidence to defer it.

What the measurements do support, if experiment 2 is revisited:

1. **Attack the moving values, not the array structure.** `state.projectiles`
   (520/530 B per independent frame) is mostly `position_m`/`velocity_m_s` for
   balls that are all present in every frame. That is a *precision or binary
   representation* question — experiment 3's territory — not an identity one.
2. **The chassis section is where keying could pay**, because `config` dominates
   a chassis element and does not change. That needs the geometry to be sent by
   reference (experiment 1's configuration identity) rather than a keyed delta
   of the whole element; the current 12-element array is small enough that the
   index path already wins.
3. **Keep the implemented keyed patch only if a collection with real survivor
   stability appears** — for example a per-shooter result history bounded and
   ordered by identity, where old entries are unchanged and only the head moves.
   Even then, measure first: this experiment's 484 lossless keyed candidates
   were all cheaper to send as index patches.
