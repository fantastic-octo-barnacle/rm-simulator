<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Experiment 8: ZSTD with a trained checkpoint dictionary

Prototype and measurement of a selectable wire codec, comparing the existing
DEFLATE against ZSTD and against ZSTD with a trained checkpoint dictionary on
**identical** checkpoint streams for the four canonical probe workloads.

This is an in-process result, not a network trial: no socket, no loss, no
reordering and no blackouts. The codec comparison excludes pacing and native
framing by design, while the full production probe reported near the end does
include them, so the two numbers answer different questions. Fragment counts
are reported because they matter under loss, but delivery under impairment is
not measured. See [Limitations](#limitations).

## What was implemented

- `crates/rm-simulator-server/src/compression.rs` is now the one place that
  turns a wire frame into bytes. It offers `Mode::Deflate` (production
  default), `Mode::Zstd` and `Mode::ZstdDictionary`, selected for a whole
  process by `RM_NET_CODEC` (`deflate`, `zstd`, `zstd-dict`) with
  `RM_NET_DEFLATE_LEVEL` and `RM_NET_ZSTD_LEVEL` overriding the effort.
- Frames are self-identifying: a ZSTD frame carries `RMZ1` (no dictionary) or
  `RMZ2` (trained dictionary) in front of it, and a DEFLATE frame carries no
  prefix. The DEFLATE wire is byte-identical to before, so no protocol version
  changes and one decoder reads a mixed pair of peers.
- The dictionary is trained offline and compiled into both binaries with
  `include_bytes!`, so it costs no wire bytes and cannot go missing at run
  time. Regenerate it with the `train_checkpoint_dictionary` example.
- The four compression sites (snapshot envelope, owner configuration, `RMC1`
  command, `RMI3` input batch) all route through the module, so the codec is
  switchable without touching the framing above it.

The codec is a development/comparison switch. **Production defaults are
unchanged: `RM_NET_CODEC` unset means DEFLATE level 1.**

## Revision and commands

- **Worktree:** `.worktrees/zstd-compression`, branch `feat/zstd-compression`.
- **Base revision:** `dd21466`.
- **Dictionary:** 32768 bytes,
  `c87e1ef9ab1ce438c13b2498a3abe0cddf392dd764933ebd0d063549645b7ebe`,
  5395 samples / 70,945,868 bytes from five deterministic synthetic gameplay
  scenarios at the 32 ms publication period (`idle`, `patrol`, `skirmish`,
  `melee`, `rune`).
- **Measurement:**
  `cargo run --release --locked -p rm-simulator-server --example compression_comparison`
- **Dictionary:**
  `cargo run --locked -p rm-simulator-server --example train_checkpoint_dictionary`
- **Validation:** `just verify` and
  `RM_NET_CODEC=zstd cargo test -p rm-simulator-server --all-features --locked`
  plus the same server command with `RM_NET_CODEC=zstd-dict` (209 unit tests
  and 34 doctests passing in each mode). Dictionary mode also passed all five
  app `net_harness` tests, including impairment and recovery. These are
  correctness checks, not impaired-link bandwidth measurements. Retraining
  reproduced the embedded dictionary byte-for-byte.

## Method

Three streams are measured per workload:

- **`envelope`** compresses the independent checkpoint envelopes with each
  codec directly. Every codec sees identical bytes, so this isolates the codec
  from the encoder's delta-versus-independent choice.
- **`selected`** drives the production acknowledged-baseline encoder per codec,
  so the delta choice, the wire bytes and the decode path are production code.
  A codec that compresses deltas better simply sends more of them.
- **`envelope-loo`** trains a dictionary on the *other three* workloads'
  independent envelopes and evaluates the fourth, as an additional
  generalization check alongside the separately trained embedded dictionary.

Workloads are the canonical probe ones (`idle`, `drive`, `twelve`, `fire`) at
16 ms steps, 250 frames. The embedded dictionary was trained on **different**
scenarios and a different command mix, so the `envelope` and `selected` rows are
already out of sample; the `envelope-loo` rows are a second, in-distribution
check.

A second measurement drives the full production codecs through the in-process
probe (`cargo test -p rm-simulator-server bandwidth_attribution_baseline --
--nocapture`, `cadence=remote`, 10 s) under each `RM_NET_CODEC`. That path adds
the owner anchor, reliable control, the byte pacer and native fragmentation on
top of the world checkpoint, so it is the closest available stand-in for the
per-player rate in [NET-001](../../KNOWN_ISSUES.md).

## Results

These release results were regenerated during review after fixing the comparison
example to return the decoder's `Retired` acknowledgement to the encoder. The
previous run stopped baseline rotation after its first replacement; it is
superseded here. Each selected stream now completes seven retirements and emits
eight full frames plus 242 deltas. The helper also serializes the envelope by
reference, avoiding an extra checkpoint clone. The full production probe below
is the earlier measurement and was unaffected by the example's handshake bug.

Selected acknowledged-delta stream, kbps and change against `deflate-1`:

| Workload | `deflate-1` | `deflate-4` | `zstd-1` | `zstd-3` | `zstd-9` | `zstd-dict-1` | `zstd-dict-3` | `zstd-dict-9` |
|---|---|---|---|---|---|---|---|---|
| `idle` | **144.2** | 129.1 (-10%) | 147.1 (+2%) | 141.7 (-2%) | 138.3 (-4%) | 64.5 (-55%) | 60.8 (-58%) | 56.7 (-61%) |
| `drive` | **570.2** | 470.5 (-17%) | 505.6 (-11%) | 490.9 (-14%) | 470.6 (-17%) | 351.3 (-38%) | 341.2 (-40%) | 338.7 (-41%) |
| `twelve` | **1707.5** | 1071.6 (-37%) | 1109.2 (-35%) | 1061.9 (-38%) | 946.6 (-45%) | 958.5 (-44%) | 933.3 (-45%) | 824.9 (-52%) |
| `fire` | **978.0** | 840.6 (-14%) | 861.8 (-12%) | 841.1 (-14%) | 805.6 (-18%) | 705.5 (-28%) | 684.0 (-30%) | 664.5 (-32%) |

Independent envelopes, kbps and change against `deflate-1`:

| Workload | `deflate-1` | `deflate-4` | `zstd-1` | `zstd-3` | `zstd-9` | `zstd-dict-1` | `zstd-dict-3` | `zstd-dict-9` |
|---|---|---|---|---|---|---|---|---|
| `idle` | **655.5** | 551.0 (-16%) | 608.9 (-7%) | 596.7 (-9%) | 567.2 (-13%) | 118.0 (-82%) | 116.4 (-82%) | 83.3 (-87%) |
| `drive` | **1468.7** | 1181.9 (-20%) | 1262.8 (-14%) | 1241.6 (-15%) | 1170.8 (-20%) | 471.0 (-68%) | 452.4 (-69%) | 414.3 (-72%) |
| `twelve` | **3058.6** | 1962.6 (-36%) | 1994.9 (-35%) | 1937.7 (-37%) | 1752.4 (-43%) | 1204.8 (-61%) | 1160.4 (-62%) | 961.0 (-69%) |
| `fire` | **1896.4** | 1558.8 (-18%) | 1637.1 (-14%) | 1606.2 (-15%) | 1515.6 (-20%) | 814.4 (-57%) | 792.4 (-58%) | 739.3 (-61%) |

The embedded dictionary is out of sample; a dictionary trained on the other
three canonical workloads is the in-distribution bound:

| Workload (envelope) | `deflate-1` | embedded `zstd-dict-3` | leave-one-out | gap vs embedded |
|---|---|---|---|---|
| `idle` | 655.5 | 116.4 (-82%) | 90.5 (-86%) | -22% |
| `drive` | 1468.7 | 452.4 (-69%) | 351.6 (-76%) | -22% |
| `twelve` | 3058.6 | 1160.4 (-62%) | 1044.3 (-66%) | -10% |
| `fire` | 1896.4 | 792.4 (-58%) | 726.8 (-62%) | -8% |

Total fragments across 250 checkpoints on the selected stream (1000-byte chunks plus a
21-byte header each); fewer fragments means less of a checkpoint is wasted when
one datagram is lost:

| Workload | `deflate-1` | `deflate-4` | `zstd-9` | `zstd-dict-3` | `zstd-dict-9` |
|---|---|---|---|---|---|
| `idle` | 258 | 258 | 258 | 250 | 250 |
| `drive` | 506 | 265 | 265 | 250 | 250 |
| `twelve` | 999 | 718 | 514 | 509 | 502 |
| `fire` | 618 | 557 | 535 | 457 | 452 |

CPU, mean microseconds on one process. `compress` is the codec alone on
identical envelopes; `encode` is the whole production encoder (JSON parse, patch
build, delta choice, compress); `decode` is inflate plus baseline decode:

| Workload | Stream | `deflate-1` | `zstd-3` | `zstd-dict-3` | `zstd-dict-9` |
|---|---|---|---|---|---|
| `idle` | envelope compress | 12.4 | 7.8 | 2.3 | 13.8 |
| `drive` | envelope compress | 28.5 | 15.5 | 8.7 | 50.5 |
| `twelve` | envelope compress | 63.2 | 28.0 | 36.8 | 153.1 |
| `fire` | envelope compress | 39.0 | 22.5 | 19.0 | 92.8 |
| `idle` | envelope decode | 8.9 | 3.1 | 2.0 | 1.4 |
| `drive` | envelope decode | 19.5 | 5.5 | 5.3 | 7.3 |
| `twelve` | envelope decode | 45.7 | 9.0 | 10.5 | 13.3 |
| `fire` | envelope decode | 26.8 | 7.1 | 9.1 | 12.7 |
| `idle` | selected encode | 163.2 | 150.3 | 141.0 | 159.6 |
| `drive` | selected encode | 354.9 | 331.8 | 323.3 | 401.1 |
| `twelve` | selected encode | 1246.2 | 1220.3 | 1197.8 | 1429.1 |
| `fire` | selected encode | 418.4 | 385.6 | 385.5 | 527.8 |
| `idle` | selected decode | 56.5 | 51.9 | 50.6 | 51.1 |
| `drive` | selected decode | 114.7 | 107.2 | 106.9 | 110.6 |
| `twelve` | selected decode | 383.0 | 372.9 | 359.1 | 365.1 |
| `fire` | selected decode | 143.9 | 130.1 | 133.0 | 138.4 |

The selected-stream encode includes JSON parsing and patch building, so its
cost is substantially higher than compression alone. The dictionary lowers
mean encode and decode time on all four workloads.

Full production probe, `cadence=remote`, 10 s per workload, `deflate-1` against
`zstd-dict-3`. This includes owner anchors, control, pacing and fragmentation:

| Workload | down kbps `deflate-1` | down kbps `zstd-dict-3` | change | world fragments | complete checkpoints | up kbps `deflate-1` | up kbps `zstd-dict-3` |
|---|---|---|---|---|---|---|---|
| `idle` | 268.1 | 196.6 | -27% | 333 → 313 | 313 → 313 | 27.0 | 30.3 |
| `drive` | 353.6 | 258.0 | -27% | 333 → 313 | 313 → 313 | 56.6 | 58.3 |
| `fire` | 671.7 | 524.3 | -22% | 856 → 589 | 313 → 313 | 59.6 | 61.3 |
| `twelve` | 877.9 | 695.8 | -21% | 1128 → 853 | 313 → 313 | 59.6 | 61.3 |

No complete checkpoint is lost in either run, so the fragment reduction is pure
headroom. Upstream regresses by 1.5–3.3 kbps: the `RMI3` batch is a small binary
body, so the checkpoint dictionary does not match it and ZSTD's frame header is
slightly larger than DEFLATE's. That is 0.2–0.4 KiB/s against a 10 KiB/s
upstream budget, but it is a regression the implementation could remove by
leaving the two upstream lanes on DEFLATE.

## Interpretation

- **Plain ZSTD saves bytes on moving workloads.** At level 3 it cuts the
  selected stream by 2% (`idle`), 14% (`drive`), 38% (`twelve`) and 14%
  (`fire`) against DEFLATE level 1, while also reducing pure codec CPU.
- **The dictionary adds further savings.** `zstd-dict-3` cuts the selected
  acknowledged-delta stream by 58% (`idle`), 40% (`drive`), 45% (`twelve`) and
  30% (`fire`) against `deflate-1`, and the independent envelopes by 58–82%. It
  also halves the fragment count on `drive` and cuts it 26% on `fire`, so a lost datagram
  wastes less of a checkpoint. This is the first measured change that beats
  DEFLATE on the selected stream by a wide margin rather than a few percent.
- **It generalizes.** The embedded dictionary was trained on gameplay-shaped
  scenarios, not on the four evaluation workloads, and a dictionary trained on
  the three other evaluation workloads is only 8–22% better. Most of the win
  comes from the checkpoint schema itself (key names, common numeric spellings,
  repeated collections), not from memorizing one run.
- **It composes with the delta scheme.** The encoder still chose 242 deltas out
  of 250 frames under `zstd-dict-3`, so the acknowledged-baseline design keeps
  its leverage; the dictionary shrinks both the deltas and the independent
  fallback.
- **Level 3 is the recommended setting.** Level 3 saves another 3–6% over level 1 on
  the selected stream; level 9 saves another 1–12% over level 3 at higher
  compression cost.
- **CPU does not block it.** The release measurements show cheaper dictionary
  compression and decompression on independent envelopes, and lower mean
  selected-stream encode and decode costs than DEFLATE on every workload.
- **It is not enough for the 200 kbps budget.** Even at the selected stream,
  `fire` falls only 978.0 → 684.0 kbps and `twelve` 1707.5 → 933.3 kbps. The
  full production probe says the same: remote downstream falls 268.1 → 196.6
  kbps idle, 671.7 → 524.3 firing and 877.9 → 695.8 with twelve chassis, so it
  reaches the budget while idle but not while firing. The codec stacks with the
  experiment 5 cadence lever rather than replacing it.
- **It costs a little upstream.** The `RMI3` input batch regresses 1.5–3.3 kbps
  because the dictionary is trained on JSON checkpoints and does not match a
  small binary body. Upstream stays far inside its budget, but a per-lane codec
  (DEFLATE upstream, dictionary ZSTD downstream) would remove the regression
  outright.

## Limitations

- **No socket and no impairment.** The codec comparison excludes pacing and
  native framing; the full production probe includes both but still runs
  in-process, with no delay, loss, reordering or blackout. Fragment counts are
  reported, but whether a smaller checkpoint actually arrives more often is
  untested. A scripted-link run should be the next step.
- **Synthetic training data.** Five deterministic scenarios with scripted
  driving are gameplay-shaped, not real play. They cover mode changes, turret
  tracking, fire bursts, bot churn and referee activity, but real aim, corner
  cases and map-specific motion will differ. Retrain from captured traffic
  before shipping.
- **Dictionary compatibility is a build constraint.** Both peers must embed the
  same dictionary. A dictionary retrain must bump
  `PROTOCOL_VERSION` to reject incompatible peers during the handshake.
- **Only the downstream world lane is measured per codec.** The upstream `RMI3`
  input batch was measured through the full production probe (it regresses
  slightly, above), but not on identical bytes the way the downstream comparison
  is. The probe sends reliable `Fire` commands, so it does not measure the
  compressed `RMC1` shot-retry lane. The owner
  anchor (`RMO4`) is uncompressed binary and unaffected.
- **One machine, one run.** CPU is wall time in one release process; byte
  counts are deterministic, timings are not.

## Recommendation

- **Keep the codec selectable and keep DEFLATE as the production default** until
  a matched host/client harness pair and impaired-link trials run. The in-process
  win is large enough to justify that trial, unlike experiments 2, 3 and 6.
- **Take `zstd-dict-3` to a harness pair** with and without a constrained link,
  measuring complete-checkpoint gaps, delivered bytes and client decode CPU on
  the real app thread. The fragment reduction (999 → 509 on `twelve`) is the
  part that only a lossy run can validate.
- **Retrain the dictionary from captured real traffic** before any production
  trial, then re-run this comparison as a schema-drift check whenever the
  checkpoint changes.
- **Do not raise the level to 9.** The extra 1–12% comes with higher
  compression cost; level 3 is the candidate.
- **If adopted, keep the upstream lanes on DEFLATE.** The dictionary does not
  match the `RMI3` batch and `RMC1` command bodies; a per-lane codec keeps the
  downstream win and removes the 1.5–3.3 kbps upstream regression.
