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

pub(crate) struct Compressor(zstd::bulk::Compressor<'static>);
impl Compressor {
    pub(crate) fn new() -> Self {
        Self(
            zstd::bulk::Compressor::with_dictionary(3, dictionary())
                .expect("embedded binary dictionary"),
        )
    }

    pub(crate) fn compress(&mut self, raw: &[u8]) -> Vec<u8> {
        let mut frame = MAGIC.to_vec();
        frame.extend(self.0.compress(raw).expect("binary checkpoint compression"));
        frame
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
}
