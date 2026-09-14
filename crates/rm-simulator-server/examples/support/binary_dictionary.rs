// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Dictionary framing for experiments only. These frames must never masquerade
//! as the production RMZ2 frames, which require the embedded JSON dictionary.
use super::fixed_point::Quantization;
use std::io;

const MAGIC: &[u8; 4] = b"RMBZ";

/// File stem identifying a binary layout and its numeric precision policy.
pub fn name(packed: bool, mode: Quantization) -> &'static str {
    match mode {
        Quantization::None if packed => "bit-lossless",
        Quantization::None => "byte-lossless",
        Quantization::Coarse => "fixed-coarse",
        Quantization::Fine => "fixed-fine",
        Quantization::Chassis => "fixed-chassis",
    }
}

/// Reused ZSTD level 3 contexts for one trained experimental dictionary.
pub struct Codec {
    compressor: zstd::bulk::Compressor<'static>,
    decompressor: zstd::bulk::Decompressor<'static>,
}
impl Codec {
    /// Compile a trained dictionary into both contexts. ZSTD carries its
    /// dictionary id in each frame and refuses a different dictionary at decode.
    pub fn new(dictionary: &[u8]) -> io::Result<Self> {
        if !dictionary.starts_with(&rm_simulator_server::compression::TRAINED_DICTIONARY_MAGIC) {
            return Err(io::Error::other("expected a trained ZSTD dictionary"));
        }
        Ok(Self {
            compressor: zstd::bulk::Compressor::with_dictionary(3, dictionary)?,
            decompressor: zstd::bulk::Decompressor::with_dictionary(dictionary)?,
        })
    }

    /// Compress with a distinct four-byte prefix, counted in bandwidth totals.
    pub fn compress(&mut self, raw: &[u8]) -> Vec<u8> {
        let mut frame = MAGIC.to_vec();
        frame.extend(
            self.compressor
                .compress(raw)
                .expect("binary dictionary compression"),
        );
        frame
    }

    /// Decode this experiment's framing with a strict inflated byte ceiling.
    pub fn decompress(&mut self, frame: &[u8], limit: usize) -> io::Result<Vec<u8>> {
        if !frame.starts_with(MAGIC) {
            return Err(io::Error::other("not an experimental dictionary frame"));
        }
        self.decompressor.decompress(&frame[MAGIC.len()..], limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_identity_framing_and_inflate_limit_are_enforced() {
        let samples: Vec<Vec<u8>> = (0..128)
            .map(|i| {
                format!("a bounded frame with repeated dictionary words and a varying id {i}")
                    .into_bytes()
            })
            .collect();
        let a = zstd::dict::from_samples(&samples, 1024).unwrap();
        let other: Vec<Vec<u8>> = samples
            .iter()
            .map(|s| s.iter().rev().copied().collect())
            .collect();
        let b = zstd::dict::from_samples(&other, 1024).unwrap();
        let mut codec = Codec::new(&a).unwrap();
        let frame = codec.compress(&samples[0]);
        assert_eq!(codec.decompress(&frame, 1024).unwrap(), samples[0]);
        assert!(codec.decompress(&frame, 2).is_err());
        assert!(Codec::new(&b).unwrap().decompress(&frame, 1024).is_err());
        assert!(codec.decompress(b"RMZ2wrong framing", 1024).is_err());
        assert!(Codec::new(b"raw content is not a trained dictionary").is_err());
    }
}
