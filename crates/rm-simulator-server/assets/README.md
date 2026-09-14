<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Server assets

## `checkpoint-dictionary.zstd`

A trained ZSTD dictionary for the wire compression in
[`../src/compression.rs`](../src/compression.rs). It is embedded with
`include_bytes!` and must be identical on both peers, so it ships inside the
binary instead of being negotiated on the wire.

| Field | Value |
|---|---|
| Bytes | 32768 |
| SHA-256 | `c87e1ef9ab1ce438c13b2498a3abe0cddf392dd764933ebd0d063549645b7ebe` |
| Trained | 2026-09-14, `zstd` 0.13.3 / `zstd-sys` 2.0.16 (ZSTD 1.5.7) |
| Samples | 5395 uncompressed envelopes (70,945,868 bytes) |
| Scenarios | `idle`, `patrol`, `skirmish`, `melee`, `rune` |
| Command | `cargo run --locked -p rm-simulator-server --example train_checkpoint_dictionary` |

The samples are the frames the production encoder compresses: each checkpoint's
independent envelope plus the delta the acknowledged-baseline encoder builds
against the previous checkpoint. They come from deterministic synthetic
gameplay runs at the production 32 ms publication period, mixing varied
driving, turret tracking, fire bursts, training bots joining and leaving, and
referee match control, so the dictionary sees the collection churn and moving
values the wire actually carries. The runs are deliberately not the workloads
the `compression_comparison` example evaluates, which keeps that measurement
out of sample.

`zstd::dict::from_samples` is deterministic for a fixed sample set, seed and
`zstd-sys` version, so re-running the command reproduces this file byte for
byte. Retrain it whenever the checkpoint schema changes: a dictionary tuned to
an older schema is at best dead weight and can make small frames larger. Update
this record and the measurement in
[`../../../docs/bandwidth-experiments.md`](../../../docs/bandwidth-experiments.md)
whenever it is regenerated. Bump `protocol::PROTOCOL_VERSION` when replacing
the dictionary so incompatible peers are rejected during the handshake.

The binary protocol experiment, recorded in Git history, used
`train_binary_dictionaries` to generate separate dictionaries for each
experimental layout and precision policy. Protocol 32 copies the selected fine
dictionary here as described below. The
JSON dictionary above remains a separate asset.
When changing encoded payloads or numeric precision, train on the new bytes and
evaluate on separate workloads; reusing this JSON dictionary alone is not an
adequate compression comparison.

## `binary-fixed-fine.zstd`

The protocol 32 periodic-checkpoint dictionary, embedded separately from the
JSON dictionary. It is the `fixed-fine` artifact selected by the binary
experiment recorded in Git history.

| Field | Value |
|---|---|
| Bytes | 32768 |
| SHA-256 | `9996f46a6bdb5718d08e53cdbf2cedee17645513c5966966000c978e1b7b9686` |
| Samples | 5314 full and retained-baseline delta candidates, 35,441,531 bytes |
| Sample digest | `e5de894fd2d29e7bcb043c8881a5fda3321d30c338245770a91a188ed294a47d` |
| Encoding | RMB0, packed tags and fixed-point grids; fine projectile precision |
| Compression frame | RMBZ + ZSTD level 3, distinct from JSON's RMZ2 |
| Trainer | `train_binary_dictionaries`, separate training/evaluation scenarios |

The live encoder calls the same bitpack and quantization implementation as the
trainer and comparison example. The application frame format is unchanged from
the measured artifact, so its dictionary was copied byte-for-byte. Tests check
the hash of the embedded bytes. Regenerate with the binary experiment's
commands after any encoding or precision change, replace this file, update this
record and bump `PROTOCOL_VERSION` again.
