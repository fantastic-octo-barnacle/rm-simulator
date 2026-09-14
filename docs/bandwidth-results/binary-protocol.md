<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Binary and fixed-point checkpoint experiment

Investigation dated 2026-09-14, based on `37bf13ca24e8e20108dd68c3398154ec8456b802`.
This is a runnable, standalone codec experiment, not an enabled live protocol.
It preserves the current world checkpoint contents and tests whether a different
representation can substantially reduce bandwidth, including the effects of
training new compression dictionaries for that representation.

## Findings

**Yes: fine fixed point with a newly trained binary dictionary reduces world
checkpoint payload bandwidth by 55–81% against current DEFLATE, and 39–64%
against the existing JSON dictionary on these five workloads.** All rates below
are kbps per receiving peer at 31.25 Hz, before the additional traffic classes.

| Workload | Current JSON + DEFLATE | JSON + existing dictionary | Lossless bitpacked + new dictionary | Fine fixed + new dictionary | Saving vs current |
|---|---:|---:|---:|---:|---:|
| idle | 150.0 | 79.5 | 56.6 | 33.8 | 77.4% |
| drive | 284.9 | 171.9 | 144.5 | 66.0 | 76.8% |
| twelve | 887.3 | 478.9 | 457.8 | 171.3 | 80.7% |
| fire | 600.5 | 440.3 | 531.0 | 269.1 | 55.2% |
| skirmish | 2523.0 | 2095.4 | 2166.9 | 970.7 | 61.5% |

Retraining matters even after the binary representation is fixed: it saves
another **3–23%** versus using the old JSON dictionary on those same binary
frames. The candidate re-evaluates full-versus-delta selection with each codec;
these are complete selected-stream comparisons, not just recompression of one
codec's choices.

| Workload | Fine fixed + old JSON dictionary | Fine fixed + new dictionary | Extra saving from retraining |
|---|---:|---:|---:|
| idle | 44.1 | 33.8 | 23.3% |
| drive | 76.8 | 66.0 | 14.1% |
| twelve | 190.7 | 171.3 | 10.2% |
| fire | 292.1 | 269.1 | 7.8% |
| skirmish | 1001.8 | 970.7 | 3.1% |

Lossless binary alone is less decisive. It helps the idle and driving cases,
but the firing and crowded workloads can still be larger than dictionary-
compressed JSON. Bitpacking does not guarantee the smallest compressed stream.
The major improvement here comes from quantizing selected motion values while
preserving the whole checkpoint and using retained-baseline deltas.

**Recommendation:** use the fine-projectile candidate for further experiments;
do not adopt the coarse projectile policy for prediction. The fine candidate
uses 1 mm chassis position, 0.01 m/s chassis velocity, 0.01 mm projectile position
and 0.0001 m/s projectile velocity resolution. Short replay results are encouraging,
but crowded-contact divergence remains, as detailed below. The experiment is
not sufficient evidence to change the live precision defaults yet.

These are checkpoint savings, not total connection savings. Adding the unchanged
111 kbps owner stream and the measured application fragment headers gives an
illustrative driving total of **407 → 182 kbps**, firing **726 → 389 kbps**, and
crowded skirmish **2,690 → 1,104 kbps**. Those sums still exclude control,
acknowledgements, retransmissions and native network headers. This does not yet
establish the desired 100–200 kbps total downstream budget under firing.


## What is already binary

The remaining large JSON stream is the world checkpoint. The current `RMI3`
pilot input batches already use binary fields and changed-value masks. `RMO4`
owner anchors already use a binary layout and reference an acknowledged chassis
configuration. A standard four-wheel owner anchor is 444 bytes, or **111 kbps**
at the remote publication rate of 31.25 Hz. This experiment measures those
anchor bytes but changes only the experimental world checkpoint representation.

The control is the actual compact player checkpoint and production
`udp_snapshot::Encoder`/`Decoder`, including compression-dependent selection
between full and delta frames. Comparing against raw JSON alone would overstate
the benefit. The existing JSON-trained ZSTD dictionary is a second control.

## Formats

`binary_protocol_comparison` compares these formats under no compression,
DEFLATE level 1, ZSTD level 3, the existing JSON dictionary, and a newly trained
dictionary appropriate to each binary format:

| Format | Representation |
|---|---|
| `json-full` | Independent production JSON envelopes. |
| `json-selected` | Production acknowledged-baseline selection and decoding. |
| `binary-full` / `binary-delta` | Lossless value encoding, byte-aligned flags and fields. |
| `bit-full` / `bit-delta` | Same lossless encoding with packed tags, flags and numeric payloads. |
| `fixed-full` / `fixed-delta` | Packed encoding after coarse motion quantization. |
| `fixed-fine-delta` | Same chassis precision, much finer projectile precision. |
| `fixed-chassis-delta` | Quantized chassis dynamics; original compact projectile values. |

The lossless formats preserve every compact JSON value, including f64 bits,
integer values, optional fields, collection changes and negative zero. A full
frame carries its object keys; a delta inherits unchanged structure and key
order from its named baseline. Changed floating-point values use the meaningful
bits of their XOR with that baseline. Fixed-point values use a grid identifier
and signed integer; their deltas encode a signed integer difference. Values
that cannot use a fixed grid retain their exact f64 representation.

Tags and change flags are packed into bits; string data stays byte-aligned to
retain repeated-key compression. Packing every field is not automatically a
win after compression: byte alignment can help a compressor find repetition.
The full-versus-delta choice is made after compression for every candidate.

No constants or configuration are silently supplied by the benchmark. Full
frames are self-contained. Baseline identities and epochs are explicit. Array
length or object-key changes replace that container. The decoder bounds frame
size, depth, node count and lengths and rejects invalid tags, nonfinite floats,
wrong epochs, wrong baseline ids, truncated input and trailing data.

## Precision assumptions

These are application experiments, not competition rule constants. Quantization
happens on a copy of the checkpoint; it does not change the authoritative world.

| Fields | Resolution | Fixed range (approximately) | Maximum rounding error per component |
|---|---|---|---|
| Chassis position and wheel hubs | 0.001 m | ±131 m, 18 signed bits | 0.5 mm |
| Chassis linear velocity and wheel target speed | 0.01 m/s | ±328 m/s, 16 signed bits | 0.005 m/s |
| Body/gimbal angular velocity | 0.001 rad/s | ±131 rad/s, 18 signed bits | 0.0005 rad/s |
| Held aim and wheel spin | 0.0001 rad | ±839 rad, 24 signed bits | 0.00005 rad |
| Quaternion components | 1/32767 | Unit components, 16 signed bits | 1/65534 before normalization |
| Coarse projectile position / velocity | 0.001 m / 0.01 m/s | Same position/velocity ranges above | 0.5 mm / 0.005 m/s |
| Fine projectile position / velocity | 0.00001 m / 0.0001 m/s | ±336 m / ±839 m/s, 26/24 signed bits | 0.005 mm / 0.00005 m/s |

Out-of-range components escape to their original precision instead of clamping.
There were no range escapes in the evaluation workloads. Quaternion components
are normalized at the physics adapter boundary, after baseline decoding, so
the retained wire values remain on their integer grid. Exact, already-on-grid
values elsewhere can also use the smaller representation without changing
their value.

Commands, chassis configuration, IDs, clocks, lifetimes, retirement bookkeeping,
scoring data, referee state, rune/outpost state and hidden rule state remain
exact relative to the existing compact checkpoint. Its existing projectile f32
conversion still applies when a checkpoint is decoded into protocol types.

## Retrained dictionaries

Five **new 32 KiB dictionaries** are stored in
[`binary-protocol/dictionaries`](binary-protocol/dictionaries). They are not
copies of the production JSON dictionary. Each is trained on its actual binary
layout and precision policy, including independent/full and retained-baseline
delta bytes. `training.csv` records each dictionary's SHA-256 and a digest of
its length-delimited training samples.

The training set contains 2,700 checkpoints from the existing deterministic
training generator: zero-player idle, two-player patrol, four-player skirmish
with bot churn, twelve-player melee, and a four-player referee/rune scenario.
These differ from the evaluation generator, commands, durations and fire
patterns; the shared `skirmish` label does not mean the same workload.
Each format contributes 2,700 full candidates and 2,614 deltas against baselines
rotated every 32 frames, for **5,314 samples per dictionary**. Both candidate
forms are included deliberately, rather than training only on whichever a
previous compressor happened to select. Evaluation frames are never used for
training or dictionary selection.

Every dictionary is trained twice and compared byte-for-byte. Every training
sample is binary-decoded and checked, then compressed/decompressed with the
resulting dictionary and checked again. The shared workload extraction also
reproduces the production JSON dictionary before and after refactoring with
SHA-256 `c87e1ef9ab1ce438c13b2498a3abe0cddf392dd764933ebd0d063549645b7ebe`.
The production asset has not been replaced.

Experimental dictionary frames use `RMBZ`, distinct from production `RMZ2`.
Both prefixes cost four bytes in the measurements. ZSTD's dictionary id is
also carried by its frame; tests verify rejection with the wrong dictionary.
No dictionary transfer is charged: this models a dictionary shipped with both
peers. Only the chosen format's dictionary would need to ship in an integrated
build, rather than all five experimental alternatives.

**Any integration must ship the encoding and its newly trained dictionary
together, bump `PROTOCOL_VERSION`, and reject incompatible peers.** Repeat
training and held-out evaluation after changing wire layout or quantization.
A successful decode with an old dictionary is not evidence that its compression
results remain representative. Dictionary digests should be recorded with the
protocol revision, and wire changes should not reuse the production `RMZ2`
identity for an incompatible dictionary.

## Replay checks and limitations

Ten sampled checkpoints per workload are restored twice through `Field::restore`:
once with the current compact checkpoint and once with quantized motion. Both
restores therefore lose solver warm starts equally. Each pair then advances
through the ordinary physics/rules path to 32 ms and 128 ms, without new inputs.
The same commands remain held in both worlds.

| Workload and precision | Maximum projectile position disagreement at 32 ms | At 128 ms |
|---|---:|---:|
| Fire, coarse | 15.26 mm | 90.94 mm |
| Fire, fine | 0.126 mm | 0.895 mm |
| Fire, chassis only | 0 | 0 |
| Skirmish, coarse | 19.61 mm | 84.62 mm |
| Skirmish, fine | 0.188 mm | 31.26 mm |
| Skirmish, chassis only | 0.063 mm | 30.71 mm |

Across the sampled worlds, the maximum chassis position disagreement at 128 ms
was 1.30 mm. Maximum chassis orientation disagreement was 0.00988 rad, about
0.57 degrees, in the crowded skirmish. The skirmish's projectile divergence even
with unchanged projectile values demonstrates that quantizing colliding chassis
can affect later projectile motion. Small initial component error does not bound
post-contact trajectory error.

No projectile-membership or scored-state disagreement appeared in these samples.
That is a limited observation, not proof of equivalent scoring: the evaluation
does not deliberately probe armor edges, speed thresholds or populated hit
histories. The scenes use the synthetic flat-floor world, not the CAD ramps,
clearances or undulating road. Longer prediction, terrain contacts and targeted
scoring cases are needed before accepting a live precision policy.

The codec trial also assumes immediate application acknowledgements, no packet
loss and a full baseline every 32 frames. Each delta references the retained
baseline, never the preceding delta. Unit tests cover missing/wrong baselines,
reordered/duplicate deltas and epoch mismatch, but this prototype is not wired
into the production loss/recovery state machine or sockets. A live trial must
keep the existing ownership, confirmation ordering, bounded baseline retention,
recovery and pacing behavior intact.

## Reproduce and interpret

```sh
cargo build --release --locked -p rm-simulator-server \
  --example train_binary_dictionaries --example binary_protocol_comparison
cargo run --release --locked -p rm-simulator-server \
  --example train_binary_dictionaries -- \
  docs/bandwidth-results/binary-protocol/dictionaries \
  > docs/bandwidth-results/binary-protocol/training.csv \
  2> docs/bandwidth-results/binary-protocol/training.txt
cargo run --release --locked -p rm-simulator-server \
  --example binary_protocol_comparison -- \
  docs/bandwidth-results/binary-protocol/dictionaries \
  > docs/bandwidth-results/binary-protocol/results.csv \
  2> docs/bandwidth-results/binary-protocol/replay.txt
```

Build before measuring and run the processes sequentially. The recorded run
used release Rust 1.98.1 on an Apple M3 Pro (arm64), with the default 1 ms physics
tick. Each evaluation workload has 320 publications, one every 32 ms: 10.24 s
at **31.25 Hz**. The workloads are one idle chassis; two driving; twelve
driving; two driving with one firing every 128 ms; and twelve with varying
commands and sustained fire, including one departure at frame 150 and a new
join at frame 200. The last workload is a synthetic stress case, not a measured
typical match. These differ from the older 16 ms/250-frame canonical probe;
compare rows from this run, not absolute rates across the two experiments.

[`results.csv`](binary-protocol/results.csv) reports checkpoint payload bytes,
1,000-byte fragment counts, and payload kbps. `framed_kbps` also charges the
current 21-byte application fragment header. It excludes owner anchors,
acknowledgements, control, Pongs, native GNS/UDP/IP headers, retransmissions and
pacing drops. The four-byte compression prefix is included where applicable.
CPU timings include parsing, delta construction/selection and compression on
encode, and the current JSON-value-to-protocol adapter on decode. Simulation,
training and round-trip assertions are outside the timing regions. They are
single-process timings, not network latency or a statistically robust CPU study.

[`replay.txt`](binary-protocol/replay.txt) holds rounding/replay maxima and the
unchanged owner-stream cost. [`training.csv`](binary-protocol/training.csv) and
[`training.txt`](binary-protocol/training.txt) record training provenance.

Validation: all 209 server library tests, the server binary test and 34 server
doctests passed; the five experimental tests pass in both example targets.
Server all-target/all-feature clippy passes with warnings denied, as does
workspace formatting. No dependencies, live protocol flags, default codecs or
physics rules were changed.
