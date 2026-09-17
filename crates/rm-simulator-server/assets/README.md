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
| SHA-256 | `d0b9d360de64060659adeb755d6005c4a1273e1bd6be83f52acd27f0434aa2fb` |
| Samples | 5314 full and retained-baseline delta candidates, 6,223,193 bytes |
| Sample digest | `7ad720ca88614e7b45bb201c6a050cd99c6e71947499bcca28881842ecf55c5a` |
| Encoding | Protocol 39 RMB1 positional bitpack, fixed-point grids, fine projectile precision, derived views and tick-coded stamps omitted |
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
