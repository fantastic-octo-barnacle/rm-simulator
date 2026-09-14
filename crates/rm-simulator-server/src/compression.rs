// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Selectable wire compression: DEFLATE, the production default, or ZSTD with a
//! trained checkpoint dictionary.
//!
//! This module is the one place that knows how a wire frame becomes bytes. The
//! UDP codec, the snapshot envelope, the owner configuration and the client's
//! input batches all call [`compress`] and [`decompress`], so a development run
//! can compare codecs without touching the framing above them.
//!
//! Every frame is self-describing, so a decoder accepts whatever its peer
//! selected even when the two processes selected differently:
//!
//! - a ZSTD frame carries [`ZSTD_MAGIC`] or [`ZSTD_DICTIONARY_MAGIC`] in front
//!   of it;
//! - a DEFLATE frame carries no prefix, exactly as it did before this module
//!   existed, so old and new builds still interoperate on that codec.
//!
//! The dictionary is compiled into both peers from
//! `assets/checkpoint-dictionary.zstd`, so it costs no wire bytes and cannot go
//! missing at run time. It is trained by the `train_checkpoint_dictionary`
//! example; see `assets/README.md` for the recorded hash. Retrain it whenever
//! the checkpoint schema changes, because a dictionary tuned to the old schema
//! is at best dead weight. A dictionary replacement must bump
//! `protocol::PROTOCOL_VERSION` so the handshake rejects incompatible peers.
//!
//! `RM_NET_CODEC` selects the codec for a whole process: `deflate` (the
//! default), `zstd` (no dictionary) or `zstd-dict`. `RM_NET_DEFLATE_LEVEL` and
//! `RM_NET_ZSTD_LEVEL` override the effort level. Like the physics rate, the
//! choice is frozen on first use, and both peers of a connection must run the
//! same build because they must embed the same dictionary.
use std::{cell::RefCell, env, io, sync::OnceLock};

/// First four bytes of a ZSTD frame compressed without a dictionary.
pub const ZSTD_MAGIC: &[u8; 4] = b"RMZ1";
/// First four bytes of a ZSTD frame compressed with the trained dictionary.
pub const ZSTD_DICTIONARY_MAGIC: &[u8; 4] = b"RMZ2";
/// Effort level used for DEFLATE when `RM_NET_DEFLATE_LEVEL` is unset. This is
/// the level the live wire used before the codec was selectable.
pub const DEFAULT_DEFLATE_LEVEL: i32 = 1;
/// Effort level used for ZSTD when `RM_NET_ZSTD_LEVEL` is unset. ZSTD's own
/// default; the comparison example sweeps the alternatives.
pub const DEFAULT_ZSTD_LEVEL: i32 = 3;
/// Highest ZSTD effort level `zstd` accepts.
const ZSTD_MAX_LEVEL: i32 = 22;
/// Highest DEFLATE effort level `miniz_oxide` accepts.
const DEFLATE_MAX_LEVEL: i32 = 10;
/// The trained checkpoint dictionary, shared by both peers.
static DICTIONARY: &[u8] = include_bytes!("../assets/checkpoint-dictionary.zstd");
/// Bytes that begin a trained ZSTD dictionary. [`dictionary`] starts with them;
/// an asset that does not came from the trainer's raw-content fallback and
/// would silently degrade every frame it compresses.
pub const TRAINED_DICTIONARY_MAGIC: [u8; 4] = [0x37, 0xa4, 0x30, 0xec];

thread_local! {
    /// One compressor per thread. Building a ZSTD context and compiling the
    /// dictionary into it costs far more than a frame, so a thread reuses both.
    static COMPRESSOR: RefCell<Option<Compressor>> = const { RefCell::new(None) };
    /// One decompressor per thread, holding the contexts of both ZSTD modes.
    static DECOMPRESSOR: RefCell<Option<Decompressor>> = const { RefCell::new(None) };
}

/// The compression algorithm a build selects for the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// DEFLATE (`miniz_oxide`). The production default and the only codec a
    /// peer built before this module understands.
    Deflate,
    /// ZSTD without a dictionary, which isolates the dictionary's contribution.
    Zstd,
    /// ZSTD with the embedded trained checkpoint dictionary.
    ZstdDictionary,
}

impl Mode {
    /// Parses one `RM_NET_CODEC` value. An unknown value is [`Mode::Deflate`],
    /// so a typo can never silently change the wire format.
    ///
    /// ```
    /// use rm_simulator_server::compression::Mode;
    ///
    /// assert_eq!(Mode::parse("zstd-dict"), Mode::ZstdDictionary);
    /// assert_eq!(Mode::parse("zstd"), Mode::Zstd);
    /// assert_eq!(Mode::parse("deflate"), Mode::Deflate);
    /// assert_eq!(Mode::parse("lz4"), Mode::Deflate);
    /// ```
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "zstd" => Self::Zstd,
            "zstd-dict" => Self::ZstdDictionary,
            _ => Self::Deflate,
        }
    }

    /// The prefix a frame compressed by this mode carries, or `None` for
    /// DEFLATE, whose frames stay unprefixed and byte-identical to the
    /// historical wire.
    pub fn magic(self) -> Option<&'static [u8; 4]> {
        match self {
            Self::Deflate => None,
            Self::Zstd => Some(ZSTD_MAGIC),
            Self::ZstdDictionary => Some(ZSTD_DICTIONARY_MAGIC),
        }
    }

    /// `level` clamped to the range the algorithm accepts, so a stale
    /// environment value cannot fail a frame.
    fn clamp(self, level: i32) -> i32 {
        match self {
            Self::Deflate => level.clamp(0, DEFLATE_MAX_LEVEL),
            Self::Zstd | Self::ZstdDictionary => level.clamp(1, ZSTD_MAX_LEVEL),
        }
    }
}

/// One wire codec and its effort level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Codec {
    /// Which algorithm compresses a frame.
    pub mode: Mode,
    /// Effort level, already clamped to the range `mode` accepts.
    pub level: i32,
}

impl Codec {
    /// DEFLATE at `level`, clamped to `0..=10`.
    pub fn deflate(level: i32) -> Self {
        Self {
            mode: Mode::Deflate,
            level: Mode::Deflate.clamp(level),
        }
    }

    /// ZSTD at `level`, clamped to `1..=22`, with no dictionary.
    pub fn zstd(level: i32) -> Self {
        Self {
            mode: Mode::Zstd,
            level: Mode::Zstd.clamp(level),
        }
    }

    /// ZSTD at `level`, clamped to `1..=22`, with the embedded dictionary.
    pub fn zstd_dictionary(level: i32) -> Self {
        Self {
            mode: Mode::ZstdDictionary,
            level: Mode::ZstdDictionary.clamp(level),
        }
    }

    /// The codec this process selected from `RM_NET_CODEC`,
    /// `RM_NET_DEFLATE_LEVEL` and `RM_NET_ZSTD_LEVEL`.
    ///
    /// ```
    /// use rm_simulator_server::compression::{Codec, DEFAULT_DEFLATE_LEVEL, Mode};
    ///
    /// // With no environment override the production codec is DEFLATE level 1.
    /// let codec = Codec::from_env();
    /// assert!(codec.level >= 0 && codec.level <= 22);
    /// assert!(matches!(codec.mode, Mode::Deflate | Mode::Zstd | Mode::ZstdDictionary));
    /// assert_eq!(Codec::deflate(DEFAULT_DEFLATE_LEVEL).level, 1);
    /// ```
    pub fn from_env() -> Self {
        let mode = Mode::parse(&env::var("RM_NET_CODEC").unwrap_or_default());
        let level = match mode {
            Mode::Deflate => env_level("RM_NET_DEFLATE_LEVEL", DEFAULT_DEFLATE_LEVEL),
            Mode::Zstd | Mode::ZstdDictionary => env_level("RM_NET_ZSTD_LEVEL", DEFAULT_ZSTD_LEVEL),
        };
        Self {
            mode,
            level: mode.clamp(level),
        }
    }

    /// Compresses one frame with a throwaway context. Hot paths use a
    /// [`Compressor`] instead, which reuses its context across frames.
    pub fn compress(self, bytes: &[u8]) -> Vec<u8> {
        Compressor::new(self).compress(bytes)
    }
}

/// One reusable compressor. A frame it cannot compress with ZSTD falls back to
/// DEFLATE, which every decoder reads, so no frame can be lost to a compression
/// failure; the fallback is unprefixed and therefore self-describing.
pub struct Compressor {
    codec: Codec,
    zstd: Option<zstd::bulk::Compressor<'static>>,
}

impl Compressor {
    /// Builds a compressor for `codec`. An unusable ZSTD configuration (a
    /// corrupt embedded dictionary, say) leaves `self` falling back to DEFLATE
    /// at this codec's level, clamped into DEFLATE's own range; the test suite
    /// asserts the dictionary is valid so that path stays a defensive fallback.
    pub fn new(codec: Codec) -> Self {
        let zstd = match codec.mode {
            Mode::Deflate => None,
            Mode::Zstd => zstd::bulk::Compressor::new(codec.level).ok(),
            Mode::ZstdDictionary => {
                zstd::bulk::Compressor::with_dictionary(codec.level, DICTIONARY).ok()
            }
        };
        debug_assert!(
            codec.mode == Mode::Deflate || zstd.is_some(),
            "ZSTD compressor unavailable for {:?}",
            codec.mode
        );
        Self { codec, zstd }
    }

    /// The codec this compressor was built for. A per-frame fallback does not
    /// change it; the emitted frame says which algorithm actually ran.
    pub fn codec(&self) -> Codec {
        self.codec
    }

    /// Compresses one frame, falling back to DEFLATE on any ZSTD error.
    pub fn compress(&mut self, bytes: &[u8]) -> Vec<u8> {
        if let Some(magic) = self.codec.mode.magic()
            && let Some(frame) = self
                .zstd
                .as_mut()
                .and_then(|zstd| zstd.compress(bytes).ok())
        {
            let mut out = Vec::with_capacity(magic.len() + frame.len());
            out.extend_from_slice(magic);
            out.extend_from_slice(&frame);
            return out;
        }
        deflate(bytes, self.codec.level)
    }
}

/// One reusable decompressor for every codec this module writes. The frame's
/// prefix picks the algorithm, so one decoder serves a mixed pair of peers.
pub struct Decompressor {
    zstd: Option<zstd::bulk::Decompressor<'static>>,
    zstd_dictionary: Option<zstd::bulk::Decompressor<'static>>,
}

impl Default for Decompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Decompressor {
    /// Builds the decoder contexts. Both ZSTD modes share the same underlying
    /// library, so only the dictionary-carrying context can fail to build.
    pub fn new() -> Self {
        Self {
            zstd: zstd::bulk::Decompressor::new().ok(),
            zstd_dictionary: zstd::bulk::Decompressor::with_dictionary(DICTIONARY).ok(),
        }
    }

    /// Inflates one frame, refusing to produce more than `limit` bytes. Both
    /// decoders stop at the limit instead of allocating, so the bound protects
    /// the process from a hostile peer as well as from a decoding bug.
    pub fn decompress(&mut self, bytes: &[u8], limit: usize) -> io::Result<Vec<u8>> {
        if let Some(body) = bytes.strip_prefix(ZSTD_MAGIC) {
            let context = self
                .zstd
                .as_mut()
                .ok_or_else(|| io::Error::other("ZSTD decompressor unavailable"))?;
            return context.decompress(body, limit);
        }
        if let Some(body) = bytes.strip_prefix(ZSTD_DICTIONARY_MAGIC) {
            let context = self
                .zstd_dictionary
                .as_mut()
                .ok_or_else(|| io::Error::other("ZSTD dictionary decompressor unavailable"))?;
            return context.decompress(body, limit);
        }
        miniz_oxide::inflate::decompress_to_vec_with_limit(bytes, limit).map_err(invalid)
    }
}

/// The trained dictionary both peers embed. Exposed so a benchmark can report
/// its size and reproduce the training input.
pub fn dictionary() -> &'static [u8] {
    DICTIONARY
}

/// The process-wide codec, parsed from the environment on first use.
pub fn selected() -> Codec {
    static SELECTED: OnceLock<Codec> = OnceLock::new();
    *SELECTED.get_or_init(Codec::from_env)
}

// Each deterministic experiment replay starts with fresh reusable contexts,
// matching a fresh process without changing the selected codec or live behavior.
#[cfg(all(test, feature = "section-topics"))]
pub(crate) fn reset_test_contexts() {
    COMPRESSOR.with(|cell| *cell.borrow_mut() = None);
    DECOMPRESSOR.with(|cell| *cell.borrow_mut() = None);
}

/// Compresses one frame with the process-wide codec.
pub fn compress(bytes: &[u8]) -> Vec<u8> {
    let codec = selected();
    COMPRESSOR.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.as_ref().is_none_or(|held| held.codec() != codec) {
            *slot = Some(Compressor::new(codec));
        }
        slot.as_mut().expect("just filled").compress(bytes)
    })
}

/// Decompresses one frame whichever codec produced it, producing at most
/// `limit` bytes.
pub fn decompress(bytes: &[u8], limit: usize) -> io::Result<Vec<u8>> {
    DECOMPRESSOR.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(Decompressor::new());
        }
        slot.as_mut().expect("just filled").decompress(bytes, limit)
    })
}

/// DEFLATE with `miniz_oxide`, clamping the level into its accepted range.
fn deflate(bytes: &[u8], level: i32) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(bytes, level.clamp(0, DEFLATE_MAX_LEVEL) as u8)
}

/// An invalid-data error carrying the inflater's message, which is what every
/// other wire decoder in the crate reports.
fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

/// One integer environment override, or `default` when unset or unparsable.
fn env_level(name: &str, default: i32) -> i32 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative frame: JSON keys, repeated structure and one float
    /// array, so both algorithms have something to find.
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
    fn both_codecs_round_trip_the_same_bytes() {
        let source = frame();
        for codec in [
            Codec::deflate(DEFAULT_DEFLATE_LEVEL),
            Codec::deflate(4),
            Codec::zstd(1),
            Codec::zstd(DEFAULT_ZSTD_LEVEL),
            Codec::zstd_dictionary(1),
            Codec::zstd_dictionary(DEFAULT_ZSTD_LEVEL),
        ] {
            let mut encoder = Compressor::new(codec);
            let mut decoder = Decompressor::new();
            let compressed = encoder.compress(&source);
            let restored = decoder
                .decompress(&compressed, source.len() + 1)
                .unwrap_or_else(|error| panic!("{codec:?} failed to decode: {error}"));
            assert_eq!(restored, source, "{codec:?} changed the frame");
        }
    }

    #[test]
    fn empty_and_incompressible_frames_round_trip() {
        // A pseudo-random body has no structure; neither codec may corrupt it.
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
            for codec in [Codec::deflate(1), Codec::zstd(3), Codec::zstd_dictionary(3)] {
                let compressed = codec.compress(&source);
                let restored = Decompressor::new()
                    .decompress(&compressed, source.len().max(1) + 1)
                    .unwrap();
                assert_eq!(restored, source, "{codec:?}");
            }
        }
    }

    #[test]
    fn zstd_frames_carry_their_magic_and_deflate_frames_do_not() {
        let source = frame();
        let zstd = Codec::zstd(3).compress(&source);
        let zstd_dictionary = Codec::zstd_dictionary(3).compress(&source);
        let deflate = Codec::deflate(1).compress(&source);
        assert!(zstd.starts_with(ZSTD_MAGIC));
        assert!(zstd_dictionary.starts_with(ZSTD_DICTIONARY_MAGIC));
        // The historical DEFLATE wire is unchanged, which is what lets an old
        // peer keep decoding it.
        assert!(!deflate.starts_with(ZSTD_MAGIC));
        assert!(!deflate.starts_with(ZSTD_DICTIONARY_MAGIC));
        assert_eq!(deflate, miniz_oxide::deflate::compress_to_vec(&source, 1));
    }

    #[test]
    fn one_decoder_reads_every_codec_without_being_told_which() {
        let source = frame();
        let mut decoder = Decompressor::new();
        for compressed in [
            Codec::deflate(6).compress(&source),
            Codec::zstd(9).compress(&source),
            Codec::zstd_dictionary(9).compress(&source),
        ] {
            assert_eq!(
                decoder.decompress(&compressed, source.len() + 1).unwrap(),
                source
            );
        }
    }

    #[test]
    fn the_limit_stops_a_bomb_from_either_codec() {
        let limit = 4096;
        let bomb = vec![0u8; limit + 1];
        let zstd = Codec::zstd(1).compress(&bomb);
        let zstd_dictionary = Codec::zstd_dictionary(1).compress(&bomb);
        let deflate = Codec::deflate(1).compress(&bomb);
        assert!(zstd.len() < 64 && deflate.len() < 64, "the bombs are tiny");
        for compressed in [zstd, zstd_dictionary, deflate] {
            assert!(
                Decompressor::new().decompress(&compressed, limit).is_err(),
                "a frame past the limit must fail, not truncate"
            );
        }
    }

    #[test]
    fn the_embedded_dictionary_is_a_valid_trained_dictionary() {
        assert!(!dictionary().is_empty(), "the asset must be embedded");
        assert_eq!(
            dictionary().get(..4),
            Some(TRAINED_DICTIONARY_MAGIC.as_slice()),
            "the asset is not a trained ZSTD dictionary; retrain it"
        );
        // A dictionary that cannot build a context would silently disable the
        // codec, so prove both directions work with it.
        let source = frame();
        let compressed = Codec::zstd_dictionary(3).compress(&source);
        assert!(compressed.starts_with(ZSTD_DICTIONARY_MAGIC));
        assert_eq!(
            Decompressor::new()
                .decompress(&compressed, source.len() + 1)
                .unwrap(),
            source
        );
    }

    #[test]
    fn the_dictionary_serves_real_checkpoints() {
        // The dictionary only has to decode what it was trained for, and the
        // frames it exists to shrink are real checkpoints, not synthetic ones.
        // Assert both directions here: a corrupt or stale asset that merely
        // round-trips would otherwise go unnoticed.
        use crate::protocol::ServerMessage;
        use crate::snapshot_codec::{difference, encode_player_message};
        use crate::udp_snapshot::{Wire, envelope_bytes};
        use rm_simulator_world::{Field, FieldConfig};
        let mut simulation =
            crate::simulation::Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false);
        let mut checkpoints = Vec::new();
        for tick in 1..=2 {
            simulation.step(16).unwrap();
            let mut state = simulation.state();
            state.snapshot_id = tick;
            let compact = encode_player_message(&ServerMessage::Snapshot(Box::new(state)));
            checkpoints.push(serde_json::from_slice::<serde_json::Value>(&compact).unwrap());
        }
        let independent = envelope_bytes(&Wire::Independent {
            epoch: 0,
            state: checkpoints[1].clone(),
        });
        let deflate = Codec::deflate(1).compress(&independent);
        let trained = Codec::zstd_dictionary(3).compress(&independent);
        assert!(
            trained.len() * 2 < deflate.len(),
            "the dictionary must beat plain DEFLATE on a real checkpoint: {} vs {}",
            trained.len(),
            deflate.len()
        );
        for raw in [&independent] {
            for compressed in [deflate.clone(), trained.clone()] {
                assert_eq!(
                    Decompressor::new()
                        .decompress(&compressed, raw.len() + 1)
                        .unwrap(),
                    *raw
                );
            }
        }
        let delta = envelope_bytes(&Wire::Delta {
            epoch: 0,
            base: 1,
            patch: difference(&checkpoints[0], &checkpoints[1]),
        });
        let compressed = Codec::zstd_dictionary(3).compress(&delta);
        assert!(compressed.starts_with(ZSTD_DICTIONARY_MAGIC));
        assert_eq!(
            Decompressor::new()
                .decompress(&compressed, delta.len() + 1)
                .unwrap(),
            delta
        );
    }

    #[test]
    fn levels_are_clamped_into_the_range_each_codec_accepts() {
        assert_eq!(Codec::deflate(99).level, DEFLATE_MAX_LEVEL);
        assert_eq!(Codec::deflate(-3).level, 0);
        assert_eq!(Codec::zstd(99).level, ZSTD_MAX_LEVEL);
        assert_eq!(Codec::zstd(0).level, 1);
        assert_eq!(Codec::zstd_dictionary(-9).level, 1);
        // A clamped compressor still round-trips, which is the point of the
        // clamp: a stale environment value must not fail a frame.
        let source = frame();
        let compressed = Codec::zstd(99).compress(&source);
        assert_eq!(
            Decompressor::new().decompress(&compressed, 65536).unwrap(),
            source
        );
    }
}
