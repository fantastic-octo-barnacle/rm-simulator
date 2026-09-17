// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Packed periodic checkpoints with fine projectile fixed point and an embedded
//! dictionary trained on those exact bytes. The UDP baseline state machine owns
//! acknowledgement, retention, recovery and ordering; this module only encodes.
//! Confirmations, owner anchors and commands use their own framings; a
//! confirmation travels as a positional `RMM1` message (see
//! [`crate::snapshot_codec`]).

pub mod bitpack;
pub mod fixed_point;

/// Binary dictionary frame marker, distinct from plain ZSTD's [`crate::compression::MAGIC`].
pub const MAGIC: &[u8; 4] = b"RMBZ";

/// The fine fixed-point dictionary trained for protocol 41 and kept for 42 and
/// 43. Protocol 42 left independent frames unchanged, and a dictionary
/// retrained on its predicted deltas measured 0.4–1.3% larger sent bytes with
/// two or more chassis in `network_bandwidth`. Protocol 43 only drops the rules
/// record's one-bit variant tag. Replacing these bytes requires a
/// protocol version bump and held-out bandwidth evaluation.
pub fn dictionary() -> &'static [u8] {
    include_bytes!("../../assets/binary-fixed-fine.zstd")
}

/// Dictionary compression level for packed checkpoints. Level 6 measures
/// 10–18% smaller independent frames than level 3 for ~10–35 µs per frame on
/// the host, against the 89–374 µs bitpack encode beside it; decompression is
/// level-independent, so this costs clients nothing. The level is not part of
/// the wire format — ZSTD decodes any level against the same dictionary — so
/// there is no selector for it and no protocol bump when it changes.
pub const CHECKPOINT_LEVEL: i32 = 6;

/// Smallest packed delta worth a compression attempt. Application choice,
/// measured on the protocol 41 `network_bandwidth` workloads: no delta under
/// 128 bytes shrank at all (idle deltas are about 12 bytes, two drivers about
/// 61), because the ZSTD frame and `RMBZ` magic cost more than the dense
/// Golomb bits leave to find, while twelve players' 300-byte deltas shrink by
/// about 29%. A delta below this is sent packed without trying.
pub const DELTA_COMPRESSION_MIN_BYTES: usize = 128;

pub(crate) struct Compressor {
    inner: zstd::bulk::Compressor<'static>,
    /// Packed frames emitted uncompressed because dictionary compression
    /// would not have shrunk them. The bare `RMB1` framing is the
    /// already-decoded signal, so no new header was needed.
    raw_fallbacks: u64,
    /// Deltas sent packed without a compression attempt.
    skipped_deltas: u64,
}
impl Compressor {
    /// The dictionary compressor for packed `RMB1` checkpoints.
    pub(crate) fn new() -> Self {
        Self {
            inner: zstd::bulk::Compressor::with_dictionary(CHECKPOINT_LEVEL, dictionary())
                .expect("embedded binary dictionary"),
            raw_fallbacks: 0,
            skipped_deltas: 0,
        }
    }

    /// Compresses one packed `RMB1` checkpoint with the embedded dictionary,
    /// or returns it unchanged when the compressed frame would be no smaller.
    /// Both framings decode through [`crate::compression::decompress`]: `RMBZ`
    /// dictionary frames inflate, while a bare `RMB1` frame passes through as
    /// the already-inflated checkpoint that [`crate::udp_snapshot::parse`]
    /// inspects.
    pub(crate) fn compress(&mut self, raw: &[u8]) -> Vec<u8> {
        let body = self
            .inner
            .compress(raw)
            .expect("binary checkpoint compression");
        if MAGIC.len() + body.len() >= raw.len() {
            self.raw_fallbacks += 1;
            return raw.to_vec();
        }
        let mut frame = MAGIC.to_vec();
        frame.extend(body);
        frame
    }

    /// [`Compressor::compress`] for a packed delta: one shorter than
    /// [`DELTA_COMPRESSION_MIN_BYTES`] is returned unchanged without an
    /// attempt and counted in [`Compressor::skipped_deltas`], not as a
    /// fallback.
    pub(crate) fn compress_delta(&mut self, raw: &[u8]) -> Vec<u8> {
        if raw.len() < DELTA_COMPRESSION_MIN_BYTES {
            self.skipped_deltas += 1;
            return raw.to_vec();
        }
        self.compress(raw)
    }

    /// Packed frames returned uncompressed by [`Compressor::compress`].
    pub(crate) fn raw_fallbacks(&self) -> u64 {
        self.raw_fallbacks
    }

    /// Small deltas [`Compressor::compress_delta`] sent without an attempt.
    pub(crate) fn skipped_deltas(&self) -> u64 {
        self.skipped_deltas
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn embedded_dictionary_matches_the_measured_protocol_artifact() {
        assert_eq!(dictionary().len(), 32768);
        assert_eq!(
            format!("{:x}", Sha256::digest(dictionary())),
            "1484dbcb040b052659ce5bae1150b9152027c9f4a540a6703d9bdca2eaf5abd4"
        );
        assert_eq!(
            dictionary(),
            include_bytes!("../../assets/binary-fixed-fine.zstd")
        );
    }

    #[test]
    fn compressible_frames_keep_the_dictionary_framing() {
        let mut compressor = Compressor::new();
        let mut packed = bitpack::MAGIC.to_vec();
        packed.extend([7u8; 256]);
        let framed = compressor.compress(&packed);
        assert!(framed.starts_with(MAGIC));
        assert!(framed.len() < packed.len());
        assert_eq!(compressor.raw_fallbacks(), 0);
    }

    #[test]
    fn small_deltas_skip_the_compression_attempt() {
        let mut compressor = Compressor::new();
        let mut small = bitpack::MAGIC.to_vec();
        small.extend([7u8; DELTA_COMPRESSION_MIN_BYTES - 5]);
        assert_eq!(compressor.compress_delta(&small), small);
        assert_eq!(
            (compressor.skipped_deltas(), compressor.raw_fallbacks()),
            (1, 0)
        );
        let mut large = small.clone();
        large.push(7);
        assert!(compressor.compress_delta(&large).starts_with(MAGIC));
        assert_eq!(compressor.skipped_deltas(), 1);
    }

    #[test]
    fn incompressible_frames_pass_through_as_bare_packed_checkpoints() {
        // Pseudo-random bytes defeat the dictionary, so the gate must return
        // the packed frame itself rather than a larger compressed one. The
        // shared decompressor already treats bare `RMB1` as inflated.
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut packed = bitpack::MAGIC.to_vec();
        packed.extend((0..1024).map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        }));
        let mut compressor = Compressor::new();
        let framed = compressor.compress(&packed);
        assert_eq!(framed, packed);
        assert_eq!(compressor.raw_fallbacks(), 1);
        assert_eq!(
            crate::compression::decompress(&framed, packed.len()).unwrap(),
            packed
        );
    }
}
