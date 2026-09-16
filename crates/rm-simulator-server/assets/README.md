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
| SHA-256 | `9996f46a6bdb5718d08e53cdbf2cedee17645513c5966966000c978e1b7b9686` |
| Samples | 5314 full and retained-baseline delta candidates, 35,441,531 bytes |
| Sample digest | `e5de894fd2d29e7bcb043c8881a5fda3321d30c338245770a91a188ed294a47d` |
| Encoding | RMB0, packed tags and fixed-point grids; fine projectile precision |
| Compression frame | RMBZ + ZSTD level 3 |
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
