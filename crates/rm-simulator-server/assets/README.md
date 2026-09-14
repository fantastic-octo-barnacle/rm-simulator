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
[`../../../docs/bandwidth-results/exp-8-zstd-dictionary.md`](../../../docs/bandwidth-results/exp-8-zstd-dictionary.md)
whenever it is regenerated. Bump `protocol::PROTOCOL_VERSION` when replacing
the dictionary so incompatible peers are rejected during the handshake.
