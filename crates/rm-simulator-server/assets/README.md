<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Server assets

## `binary-fixed-fine.zstd`

The periodic-checkpoint dictionary, embedded with `include_bytes!` by
[`../src/binary_snapshot/mod.rs`](../src/binary_snapshot/mod.rs). It must be
identical on both peers, so it ships inside the binary instead of being
negotiated on the wire. It is the `fixed-fine` artifact selected by the binary
experiment recorded in Git history.

| Field | Value |
|---|---|
| Bytes | 32768 |
| SHA-256 | `1484dbcb040b052659ce5bae1150b9152027c9f4a540a6703d9bdca2eaf5abd4` |
| Samples | 4823 full frames and retained-baseline deltas of at least 128 bytes (smaller deltas are never compressed), 4,026,319 bytes |
| Sample digest | `0f0a52d529c18f82fcb4b616ee864b72d71eab484fd6146afdf8387b1c1f5d6b` |
| Encoding | Protocol 41 RMB1 positional bitpack laid out by change rate (byte-aligned slow records, then dense motion; chassis configuration and projectile policy presets), path-implied fixed-point grids with exponential-Golomb deltas, smallest-three chassis rotations, fine projectile precision, derived views and tick-coded stamps omitted |
| Compression frame | RMBZ + ZSTD level 6 |
| Trainer | `train_binary_dictionaries`, separate training/evaluation scenarios |

Protocol 42 kept these bytes. Its independent frames are unchanged; only
deltas changed (dead-reckoned baselines, realigned sequences, 12-frame
rotation), and the trainer now samples those deltas, but a dictionary retrained
that way measured 0.4–1.3% larger sent bytes with two or more chassis in
`network_bandwidth`, so the trainer no longer reproduces this artifact.

The live encoder calls the same bitpack and quantization implementation as the
trainer. The application frame format is unchanged from the measured artifact,
so its dictionary was copied byte-for-byte. Tests check the hash of the embedded
bytes. Regenerate with the binary experiment's commands after any encoding or
precision change, replace this file, update this record and bump
`PROTOCOL_VERSION` again.

Other wire messages use plain ZSTD with no dictionary
([`../src/compression.rs`](../src/compression.rs)); only the packed checkpoints
carry a trained dictionary.
