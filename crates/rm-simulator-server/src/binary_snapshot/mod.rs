// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Packed periodic checkpoints with fine projectile fixed point and an embedded
//! dictionary trained on those exact bytes. The UDP baseline state machine owns
//! acknowledgement, retention, recovery and ordering; this module only encodes.
//! `RM_NET_SNAPSHOT=json` selects the previous JSON format for comparison.
//! Confirmations, owner anchors, commands and TCP retain their existing paths.

pub mod bitpack;
pub mod fixed_point;

/// Binary dictionary frame marker, distinct from the JSON dictionary's RMZ2.
pub const MAGIC: &[u8; 4] = b"RMBZ";

/// The fine fixed-point dictionary embedded in protocol 32. Replacing these
/// bytes requires a protocol version bump and held-out bandwidth evaluation.
pub fn dictionary() -> &'static [u8] {
    include_bytes!("../../assets/binary-fixed-fine.zstd")
}

pub(crate) fn selected() -> bool {
    static BINARY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *BINARY.get_or_init(|| std::env::var("RM_NET_SNAPSHOT").as_deref() != Ok("json"))
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
            include_bytes!(
                "../../../../docs/bandwidth-results/binary-protocol/dictionaries/fixed-fine.zstd"
            )
        );
    }
}
