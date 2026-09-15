// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Self-identifying wire framing for every message that is not a periodic
//! checkpoint.
//!
//! This module is the one place that knows how a wire frame becomes bytes. The
//! UDP codec, the owner configuration, the confirmation snapshots and the
//! client's input batches all call [`encode`], [`compress`] and [`decompress`],
//! so the framing above them never names an algorithm.
//!
//! Periodic checkpoints do not use this module's ZSTD path: they are bitpacked
//! and compressed with the checkpoint dictionary in
//! [`crate::binary_snapshot`], whose frames carry their own
//! [`crate::binary_snapshot::MAGIC`]. An *uncompressed* periodic checkpoint is
//! simply the packed `RMB0` frame itself.
//!
//! Every frame this module writes is self-describing: [`compress`] and
//! [`encode(false, _)`][encode] carry [`MAGIC`] in front of the ZSTD body, while
//! [`encode(true, _)`][encode] carries [`RAW_MAGIC`] in front of the unchanged
//! body. [`decompress`] auto-detects all of them plus a bare packed `RMB0`
//! frame, which is what lets one decoder serve a whole connection without a
//! decode-side flag: the loopback transport sets `raw` so a local session runs
//! the same framing without paying for ZSTD.
use std::{cell::RefCell, io};

/// First four bytes of every frame [`compress`] writes.
pub const MAGIC: &[u8; 4] = b"RMZ1";
/// First four bytes of every frame [`encode`] writes with `raw = true`: the
/// payload follows unchanged. The loopback transport sets it so a local session
/// keeps the whole codec — framing, delta baselines and input batches — while
/// skipping ZSTD entirely.
pub const RAW_MAGIC: &[u8; 4] = b"RMRW";
/// Compression effort. ZSTD's own default, and the only level the wire uses; a
/// caller that wants less CPU for a frame does not compress it at all.
pub const LEVEL: i32 = 3;

thread_local! {
    /// One compressor per thread. Building a ZSTD context costs far more than a
    /// frame, so a thread reuses it.
    static COMPRESSOR: RefCell<Option<zstd::bulk::Compressor<'static>>> =
        const { RefCell::new(None) };
    /// One decompressor per thread, holding the plain context and the checkpoint
    /// dictionary context.
    static DECOMPRESSOR: RefCell<Option<Decompressor>> = const { RefCell::new(None) };
}

/// One reusable decompressor for every frame kind this module's `decompress`
/// accepts. The frame's prefix picks the context: a plain ZSTD message, a
/// dictionary-compressed checkpoint, a raw body or a bare packed checkpoint.
struct Decompressor {
    zstd: zstd::bulk::Decompressor<'static>,
    binary_dictionary: zstd::bulk::Decompressor<'static>,
}

impl Decompressor {
    /// Builds both contexts. The embedded dictionary is checked by the test
    /// suite, so a failure here is a build problem rather than a runtime state.
    fn new() -> io::Result<Self> {
        Ok(Self {
            zstd: zstd::bulk::Decompressor::new()?,
            binary_dictionary: zstd::bulk::Decompressor::with_dictionary(
                crate::binary_snapshot::dictionary(),
            )?,
        })
    }

    /// Inflates one frame, refusing to produce more than `limit` bytes. Every
    /// decoder stops at the limit instead of allocating, so the bound protects
    /// the process from a hostile peer as well as from a decoding bug. The
    /// prefix picks the kind: a dictionary checkpoint, a plain ZSTD frame, an
    /// uncompressed [`RAW_MAGIC`] body or a bare packed `RMB0` frame, which is
    /// already the inflated checkpoint and passes through unchanged.
    fn decompress(&mut self, bytes: &[u8], limit: usize) -> io::Result<Vec<u8>> {
        if let Some(body) = bytes.strip_prefix(crate::binary_snapshot::MAGIC) {
            return self.binary_dictionary.decompress(body, limit);
        }
        if let Some(body) = bytes.strip_prefix(MAGIC) {
            return self.zstd.decompress(body, limit);
        }
        if let Some(body) = bytes.strip_prefix(RAW_MAGIC) {
            return bounded(body, limit);
        }
        if bytes.starts_with(crate::binary_snapshot::bitpack::MAGIC) {
            return bounded(bytes, limit);
        }
        Err(invalid("not a compressed frame"))
    }
}

/// A frame that arrived already inflated: return it unchanged, refusing one past
/// `limit` instead of truncating it.
fn bounded(bytes: &[u8], limit: usize) -> io::Result<Vec<u8>> {
    if bytes.len() > limit {
        return Err(invalid("inflated frame exceeds limit"));
    }
    Ok(bytes.to_vec())
}

/// Frames one application payload. When `raw` is true the body follows
/// [`RAW_MAGIC`] unchanged, so a local session keeps every framing decision
/// above this module while skipping ZSTD; otherwise the body is
/// [`compress`]ed under [`MAGIC`].
///
/// ```
/// use rm_simulator_server::compression::{RAW_MAGIC, decompress, encode};
///
/// let frame = encode(true, b"{\"tick\":7}");
/// assert!(frame.starts_with(RAW_MAGIC));
/// assert_eq!(decompress(&frame, 64).unwrap(), b"{\"tick\":7}");
/// // The compressed form stays exactly `compress`.
/// assert!(encode(false, b"{\"tick\":7}").starts_with(b"RMZ1"));
/// ```
pub fn encode(raw: bool, bytes: &[u8]) -> Vec<u8> {
    if !raw {
        return compress(bytes);
    }
    let mut frame = Vec::with_capacity(RAW_MAGIC.len() + bytes.len());
    frame.extend_from_slice(RAW_MAGIC);
    frame.extend_from_slice(bytes);
    frame
}

/// Compresses one frame with the process-wide effort level.
pub fn compress(bytes: &[u8]) -> Vec<u8> {
    COMPRESSOR.with(|cell| {
        let mut slot = cell.borrow_mut();
        let compressor = slot.get_or_insert_with(|| {
            zstd::bulk::Compressor::new(LEVEL).expect("ZSTD compressor level 3")
        });
        let mut frame = Vec::with_capacity(MAGIC.len() + bytes.len() / 2);
        frame.extend_from_slice(MAGIC);
        frame.extend(compressor.compress(bytes).expect("ZSTD frame compression"));
        frame
    })
}

/// Decompresses one frame of any kind this module or the checkpoint codec
/// writes, producing at most `limit` bytes: a dictionary checkpoint, a plain
/// ZSTD frame, an uncompressed [`RAW_MAGIC`] body, or a bare packed `RMB0`
/// frame, which is returned unchanged because it is already inflated.
///
/// ```
/// use rm_simulator_server::compression::{compress, decompress, encode};
///
/// let frame = compress(b"{\"tick\":7}");
/// assert_eq!(decompress(&frame, 64).unwrap(), b"{\"tick\":7}");
/// let raw = encode(true, b"{\"tick\":7}");
/// assert_eq!(decompress(&raw, 64).unwrap(), b"{\"tick\":7}");
/// // A frame that was never framed is refused rather than guessed at.
/// assert!(decompress(b"{\"tick\":7}", 64).is_err());
/// ```
pub fn decompress(bytes: &[u8], limit: usize) -> io::Result<Vec<u8>> {
    DECOMPRESSOR.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(Decompressor::new()?);
        }
        slot.as_mut().expect("just filled").decompress(bytes, limit)
    })
}

/// An invalid-data error carrying the reason, which is what every other wire
/// decoder in the crate reports.
fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative frame: JSON keys, repeated structure and one float
    /// array, so ZSTD has something to find.
    fn frame() -> Vec<u8> {
        let mut bytes = Vec::new();
        for tick in 0..64 {
            bytes.extend_from_slice(
                format!(
                    "{{\"tick\":{tick},\"projectiles\":[{{\"id\":7,\"position\":[{}.5,0.25,1.75],\"velocity\":[-3.5,0.0,9.25]}}],\"hp\":[200,200,150]}}",
                    tick % 17
                )
                .as_bytes(),
            );
        }
        bytes
    }

    #[test]
    fn frames_round_trip_and_carry_their_magic() {
        let source = frame();
        let compressed = compress(&source);
        assert!(compressed.starts_with(MAGIC));
        assert!(
            compressed.len() < source.len(),
            "ZSTD must shrink this frame"
        );
        assert_eq!(decompress(&compressed, source.len() + 1).unwrap(), source);
    }

    #[test]
    fn empty_and_incompressible_frames_round_trip() {
        // A pseudo-random body has no structure; ZSTD may grow it, but it may
        // not corrupt it.
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let noisy: Vec<u8> = (0..4096)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        for source in [Vec::new(), noisy] {
            let compressed = compress(&source);
            assert_eq!(
                decompress(&compressed, source.len().max(1) + 1).unwrap(),
                source
            );
        }
    }

    #[test]
    fn the_limit_stops_a_bomb() {
        let limit = 4096;
        let compressed = compress(&vec![0u8; limit + 1]);
        assert!(compressed.len() < 64, "the bomb is tiny");
        assert!(
            decompress(&compressed, limit).is_err(),
            "a frame past the limit must fail, not truncate"
        );
    }

    #[test]
    fn an_uncompressed_or_foreign_frame_is_refused() {
        assert!(decompress(b"", 64).is_err());
        assert!(decompress(b"{\"tick\":1}", 64).is_err());
        assert!(decompress(b"RMBXnonsense", 64).is_err());
        assert!(
            decompress(b"RMRW", 64).is_ok(),
            "an empty raw body is legal"
        );
    }

    #[test]
    fn a_raw_frame_round_trips_and_respects_the_limit() {
        let source = frame();
        let raw = encode(true, &source);
        assert!(raw.starts_with(RAW_MAGIC));
        assert_eq!(raw.len(), RAW_MAGIC.len() + source.len());
        assert_eq!(decompress(&raw, source.len()).unwrap(), source);
        // One byte past the limit fails rather than truncating.
        assert!(decompress(&raw, source.len() - 1).is_err());
        // The compressed form is exactly `compress`, so the flag is the only
        // difference between the two framings.
        assert_eq!(encode(false, &source), compress(&source));
    }

    #[test]
    fn a_bare_packed_checkpoint_passes_through_unchanged() {
        // A packed `RMB0` frame is already the inflated checkpoint, so the
        // decoder returns it whole for `udp_snapshot::parse` to inspect.
        let mut packed = crate::binary_snapshot::bitpack::MAGIC.to_vec();
        packed.extend_from_slice(&[1u8; 64]);
        assert_eq!(decompress(&packed, 128).unwrap(), packed);
        assert!(decompress(&packed, packed.len() - 1).is_err());
    }

    #[test]
    fn the_embedded_checkpoint_dictionary_serves_real_checkpoints() {
        // The checkpoint dictionary is trained on real checkpoints, so both
        // directions must work on one. A corrupt or stale asset that merely
        // round-trips a synthetic frame would otherwise go unnoticed.
        use crate::protocol::ServerMessage;
        use crate::snapshot_codec::encode_player_message;
        use rm_simulator_world::{Field, FieldConfig};
        let mut simulation =
            crate::simulation::Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false);
        simulation.step(16).unwrap();
        let mut state = simulation.state();
        state.snapshot_id = 1;
        let checkpoint = encode_player_message(&ServerMessage::Snapshot(Box::new(state)));
        let packed = crate::binary_snapshot::bitpack::encode(
            &serde_json::from_slice(&checkpoint).unwrap(),
            None,
            0,
            0,
            true,
            true,
        )
        .unwrap();
        let mut encoder = crate::binary_snapshot::Compressor::new();
        let compressed = encoder.compress(&packed);
        assert!(compressed.starts_with(crate::binary_snapshot::MAGIC));
        assert_eq!(
            decompress(&compressed, checkpoint.len() + 1).unwrap(),
            packed
        );
    }
}
