// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Packed periodic checkpoints with fine projectile fixed point and an embedded
//! dictionary trained on those exact bytes. The UDP baseline state machine owns
//! acknowledgement, retention, recovery and ordering; this module only encodes.
//! Confirmations, owner anchors and commands use their own framings; a full
//! confirmation stays exact JSON.

pub mod bitpack;
pub mod fixed_point;

/// Binary dictionary frame marker, distinct from plain ZSTD's [`crate::compression::MAGIC`].
pub const MAGIC: &[u8; 4] = b"RMBZ";

/// The fine fixed-point dictionary embedded in protocol 32. Replacing these
/// bytes requires a protocol version bump and held-out bandwidth evaluation.
pub fn dictionary() -> &'static [u8] {
    include_bytes!("../../assets/binary-fixed-fine.zstd")
}

pub(crate) struct Compressor {
    inner: zstd::bulk::Compressor<'static>,
    /// Packed frames emitted uncompressed because dictionary compression
    /// would not have shrunk them. The bare `RMB0` framing is the
    /// already-decoded signal, so no new header was needed.
    raw_fallbacks: u64,
}
impl Compressor {
    /// The dictionary compressor for packed `RMB0` checkpoints.
    pub(crate) fn new() -> Self {
        Self {
            inner: zstd::bulk::Compressor::with_dictionary(3, dictionary())
                .expect("embedded binary dictionary"),
            raw_fallbacks: 0,
        }
    }

    /// Compresses one packed `RMB0` checkpoint with the embedded dictionary,
    /// or returns it unchanged when the compressed frame would be no smaller.
    /// Both framings decode through [`crate::compression::decompress`]: `RMBZ`
    /// dictionary frames inflate, while a bare `RMB0` frame passes through as
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

    /// Packed frames returned uncompressed by [`Compressor::compress`].
    pub(crate) fn raw_fallbacks(&self) -> u64 {
        self.raw_fallbacks
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
            "9996f46a6bdb5718d08e53cdbf2cedee17645513c5966966000c978e1b7b9686"
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
    fn incompressible_frames_pass_through_as_bare_packed_checkpoints() {
        // Pseudo-random bytes defeat the dictionary, so the gate must return
        // the packed frame itself rather than a larger compressed one. The
        // shared decompressor already treats bare `RMB0` as inflated.
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
