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
| SHA-256 | `611c22cda004091f6ec1c715fa5704fe5706122781d8795bf3cffc3e2c7e8d0a` |
| Samples | 5314 full and retained-baseline delta candidates, 5,472,540 bytes |
| Sample digest | `6248be84bfa675afa365d812e4ca4cae2930150a11c79b85543765a80d48eab0` |
| Encoding | Protocol 40 RMB1 positional bitpack, path-implied fixed-point grids with exponential-Golomb deltas, smallest-three chassis rotations, fine projectile precision, derived views and tick-coded stamps omitted |
| Compression frame | RMBZ + ZSTD level 6 |
| Trainer | `train_binary_dictionaries`, separate training/evaluation scenarios |

The live encoder calls the same bitpack and quantization implementation as the
trainer. The application frame format is unchanged from the measured artifact,
so its dictionary was copied byte-for-byte. Tests check the hash of the embedded
bytes. Regenerate with the binary experiment's commands after any encoding or
precision change, replace this file, update this record and bump
`PROTOCOL_VERSION` again.

Other wire messages use plain ZSTD with no dictionary
([`../src/compression.rs`](../src/compression.rs)); only the packed checkpoints
carry a trained dictionary.
