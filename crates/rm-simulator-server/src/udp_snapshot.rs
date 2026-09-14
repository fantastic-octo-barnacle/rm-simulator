// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! UDP deltas reference decoded, pinned baselines; never the last transmitted frame.
//! The decoder pins at most two baselines, so a new one may only be proposed once
//! the previous one is retired. `Retire` is reliable but its `Retired` answer is
//! not, so an unanswered retirement is resent on a bounded schedule rather than
//! abandoned: dropping it locally would propose a third baseline the decoder must
//! refuse.
use crate::{
    protocol::ServerMessage,
    snapshot_codec::{Patch, apply, difference},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, io};
/// First four bytes of every baseline or owner-configuration feedback payload,
/// so a sender can tell feedback from snapshot traffic on the same lane.
pub const ACK_MAGIC: &[u8; 4] = b"RMA1";
const BASE_LIMIT: usize = 1024 * 1024;
/// One application frame on the delta lane, always tagged by the state epoch.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Wire {
    /// A complete state that needs no baseline, used when no baseline is
    /// acknowledged yet and when a delta would be larger than the state.
    Independent {
        /// Input epoch the state belongs to; a decoder drops frames from an
        /// older epoch.
        epoch: u64,
        /// The encoded player checkpoint.
        state: Value,
    },
    /// A candidate baseline the decoder must store before deltas may reference
    /// it. Re-sending the same `id` with different state is an error.
    Full {
        /// Input epoch the state belongs to.
        epoch: u64,
        /// Baseline id, allocated in increasing order and returned in
        /// [`Feedback::Stored`].
        id: u64,
        /// The encoded player checkpoint to pin.
        state: Value,
    },
    /// A patch against a baseline the decoder already pinned. `None` means the
    /// state is unchanged from that baseline.
    Delta {
        /// Input epoch the state belongs to.
        epoch: u64,
        /// Id of the pinned baseline this patch applies to.
        base: u64,
        /// The structural change, or `None` for an unchanged state.
        patch: Option<Patch>,
    },
    /// Asks the decoder to drop baseline `id` so the encoder can propose a new
    /// one. The decoder pins at most two baselines, so retirement is what keeps
    /// the rotation moving.
    Retire {
        /// Input epoch the baseline belongs to.
        epoch: u64,
        /// Id of the baseline to drop.
        id: u64,
    },
}
#[derive(Serialize, Deserialize)]
struct Envelope<T = Wire> {
    #[serde(rename = "UdpSnapshot")]
    message: T,
}
/// The decoder's answer about one baseline, or the owner codec's answer about
/// one owner configuration. `Stored` and `Retired` confirm a state transition;
/// `Missing` tells the encoder a delta referenced a baseline the decoder never
/// pinned, so the next frame must be independent again; `ConfigStored` and
/// `ConfigMissing` run the owner-configuration acknowledgement handshake.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Feedback {
    /// The candidate baseline was pinned under `id`.
    Stored {
        /// Input epoch the baseline belongs to.
        epoch: u64,
        /// Id of the pinned baseline.
        id: u64,
    },
    /// Baseline `id` is gone and will not be referenced again.
    Retired {
        /// Input epoch the baseline belongs to.
        epoch: u64,
        /// Id of the dropped baseline.
        id: u64,
    },
    /// A delta referenced baseline `id`, which was not pinned.
    Missing {
        /// Input epoch the delta claimed.
        epoch: u64,
        /// Id of the baseline that was needed.
        id: u64,
    },
    /// The client stored the owner configuration named by `revision`, so the
    /// host may now reference it from an anchor.
    ConfigStored {
        /// Wire value of the acknowledged configuration revision.
        revision: u64,
    },
    /// An anchor named a configuration `revision` the client does not hold. The
    /// host should resend it; the client dropped that anchor rather than
    /// applying a reference it could not decode.
    ConfigMissing {
        /// Wire value of the configuration revision the client needs.
        revision: u64,
    },
}
impl Feedback {
    /// Encodes the feedback as [`ACK_MAGIC`] followed by JSON, which keeps it
    /// distinguishable from a snapshot frame on the same unreliable lane.
    ///
    /// ```
    /// use rm_simulator_server::udp_snapshot::{ACK_MAGIC, Feedback};
    ///
    /// let stored = Feedback::Stored { epoch: 4, id: 2 };
    /// let bytes = stored.encode();
    /// assert!(bytes.starts_with(ACK_MAGIC));
    /// assert_eq!(Feedback::decode(&bytes).unwrap(), stored);
    /// // A payload that is not baseline feedback is refused, not guessed at.
    /// assert!(Feedback::decode(b"RMG1nonsense").is_err());
    /// ```
    pub fn encode(self) -> Vec<u8> {
        let mut bytes = ACK_MAGIC.to_vec();
        bytes.extend(serde_json::to_vec(&self).unwrap());
        bytes
    }
    /// Decodes [`Feedback::encode`] output. Errors on a missing magic prefix or
    /// a payload longer than 256 bytes.
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > 256 || !bytes.starts_with(ACK_MAGIC) {
            return Err(io::Error::other("invalid baseline feedback"));
        }
        serde_json::from_slice(&bytes[4..]).map_err(Into::into)
    }
}
/// The uncompressed envelope JSON for one [`Wire`] frame. [`encode`] compresses
/// exactly these bytes, so the dictionary trainer sees the framing the wire
/// does instead of re-deriving it.
pub fn envelope_bytes(wire: &Wire) -> Vec<u8> {
    serde_json::to_vec(&Envelope { message: wire }).unwrap()
}
/// Frames a [`Wire`] under the `UdpSnapshot` key and compresses it with the
/// process-wide codec. The envelope bytes themselves are codec-independent, so
/// a delta choice made under one codec stays readable under another.
pub fn encode(wire: Wire) -> Vec<u8> {
    crate::compression::compress(&envelope_bytes(&wire))
}
/// Reads an inflated frame. `Ok(None)` means the JSON carried no `UdpSnapshot`
/// key, so a caller can leave other message kinds on the same stream alone
/// instead of failing on them.
///
/// ```
/// use rm_simulator_server::udp_snapshot::{Wire, parse};
///
/// let frame = br#"{"UdpSnapshot":{"Independent":{"epoch":7,"state":{"snapshot_id":1}}}}"#;
/// let wire = parse(frame).unwrap().unwrap();
/// assert!(matches!(wire, Wire::Independent { epoch: 7, .. }));
/// // A JSON message of another kind is not an error here.
/// assert!(parse(br#"{"Pong":{"nonce":1}}"#).unwrap().is_none());
/// ```
pub fn parse(bytes: &[u8]) -> io::Result<Option<Wire>> {
    let value: Value = serde_json::from_slice(bytes)?;
    if value.get("UdpSnapshot").is_none() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_value::<Envelope>(value)?.message))
}
/// Server-side delta encoder for one peer.
///
/// A new baseline is proposed only while no proposal and no retirement are
/// outstanding, so the decoder never has to pin a third baseline. Every frame
/// also carries an independent encoding of the same state, and the encoder falls
/// back to it whenever the delta would be larger, which keeps a single lost
/// baseline from stalling delivery.
pub struct Encoder {
    epoch: Option<u64>,
    next_id: u64,
    active: Option<(u64, Value)>,
    pending: Option<(u64, Value)>,
    retiring: Option<(u64, Value)>,
    /// Frames since the outstanding `Retire` was last sent.
    retire_age: u64,
    count: u64,
    recover: bool,
    /// Compressor for this encoder's codec. A benchmark builds one encoder per
    /// candidate so each reuses its own context and dictionary.
    compressor: crate::compression::Compressor,
    /// Bytes the same states would have taken as independent frames.
    pub full_bytes: u64,
    /// Bytes actually produced, deltas and full frames together.
    pub sent_bytes: u64,
    /// Frames emitted as [`Wire::Delta`].
    pub deltas: u64,
}
impl Default for Encoder {
    fn default() -> Self {
        Self::with_codec(crate::compression::selected())
    }
}
/// Resend an unanswered `Retire` after this many frames; at the 32 ms broadcast
/// period that is roughly half a second between attempts.
const RETIRE_RESEND_FRAMES: u64 = 16;
impl Encoder {
    /// A baseline encoder that compresses every frame with `codec`. The
    /// production path uses [`Encoder::default`], which takes the process-wide
    /// [`crate::compression::selected`] codec.
    pub fn with_codec(codec: crate::compression::Codec) -> Self {
        Self {
            epoch: None,
            next_id: 0,
            active: None,
            pending: None,
            retiring: None,
            retire_age: 0,
            count: 0,
            recover: false,
            compressor: crate::compression::Compressor::new(codec),
            full_bytes: 0,
            sent_bytes: 0,
            deltas: 0,
        }
    }
    /// The `Retire` to resend, if its `Retired` answer has not arrived in time.
    /// Callers send it on the same reliable lane as the original.
    pub fn resend_retire(&mut self) -> Option<Wire> {
        let epoch = self.epoch?;
        let (id, _) = self.retiring.as_ref()?;
        if self.retire_age < RETIRE_RESEND_FRAMES {
            return None;
        }
        self.retire_age = 0;
        Some(Wire::Retire { epoch, id: *id })
    }
    /// Encodes one frame from an independent player checkpoint, compressed for
    /// the wire. `epoch` selects the state generation: a change discards every
    /// baseline and restarts the rotation.
    ///
    /// ```
    /// use rm_simulator_server::compression::decompress;
    /// use rm_simulator_server::protocol::ServerMessage;
    /// use rm_simulator_server::simulation::Simulation;
    /// use rm_simulator_server::snapshot_codec::encode_player_message;
    /// use rm_simulator_server::udp_snapshot::{Decoder, Encoder, Wire, parse};
    /// use rm_simulator_world::{Field, FieldConfig};
    ///
    /// let simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false);
    /// let mut state = simulation.state();
    /// // The decoder rejects a frame whose state epoch does not match the wire.
    /// state.input_epoch = 4;
    /// let state = encode_player_message(&ServerMessage::Snapshot(Box::new(state)));
    /// let inflate = |bytes: &[u8]| {
    ///     parse(&decompress(bytes, 4 << 20).unwrap()).unwrap().unwrap()
    /// };
    /// let mut encoder = Encoder::default();
    /// let mut decoder = Decoder::default();
    /// // The first frame proposes a baseline, since none is pinned yet.
    /// let first = inflate(&encoder.snapshot(4, &state).unwrap());
    /// assert!(matches!(first, Wire::Full { id: 1, .. }));
    /// let (message, feedback) = decoder.receive(first).unwrap();
    /// assert!(message.is_some());
    /// assert_eq!(decoder.pinned(), 1);
    /// // The answer promotes the candidate, so the next frame is a delta.
    /// assert!(encoder.feedback(feedback.unwrap()).is_none());
    /// let second = inflate(&encoder.snapshot(4, &state).unwrap());
    /// assert!(matches!(second, Wire::Delta { base: 1, patch: None, .. }));
    /// assert!(encoder.deltas > 0);
    /// ```
    pub fn snapshot(&mut self, epoch: u64, bytes: &[u8]) -> io::Result<Vec<u8>> {
        let state: Value = serde_json::from_slice(bytes)?;
        if self.epoch != Some(epoch) {
            self.epoch = Some(epoch);
            self.active = None;
            self.pending = None;
            self.retiring = None;
            self.retire_age = 0;
            self.count = 0;
            self.recover = false;
        }
        self.count += 1;
        if self.retiring.is_some() {
            self.retire_age += 1;
        }
        let independent = self
            .compressor
            .compress(&envelope_bytes(&Wire::Independent {
                epoch,
                state: state.clone(),
            }));
        self.full_bytes += independent.len() as u64;
        if bytes.len() > BASE_LIMIT {
            self.sent_bytes += independent.len() as u64;
            return Ok(independent);
        }
        if self.pending.is_none()
            && self.retiring.is_none()
            && (self.active.is_none() || self.count >= 32)
        {
            self.next_id = self
                .next_id
                .checked_add(1)
                .ok_or_else(|| io::Error::other("baseline id exhausted"))?;
            self.pending = Some((self.next_id, state.clone()));
            self.count = 0;
        }
        let encoded = if self.recover
            && let Some((id, baseline)) = self.active.as_ref()
        {
            self.recover = false;
            self.compressor.compress(&envelope_bytes(&Wire::Full {
                epoch,
                id: *id,
                state: baseline.clone(),
            }))
        } else if let Some((id, baseline)) = &self.pending
            && self.count.is_multiple_of(8)
        {
            self.compressor.compress(&envelope_bytes(&Wire::Full {
                epoch,
                id: *id,
                state: baseline.clone(),
            }))
        } else if let Some((id, baseline)) = &self.active {
            let patch = difference(baseline, &state);
            let delta = self.compressor.compress(&envelope_bytes(&Wire::Delta {
                epoch,
                base: *id,
                patch,
            }));
            if delta.len() < independent.len() {
                self.deltas += 1;
                delta
            } else {
                independent
            }
        } else {
            independent
        };
        self.sent_bytes += encoded.len() as u64;
        Ok(encoded)
    }
    /// Applies one decoder answer. Returns a [`Wire::Retire`] to send when the
    /// stored baseline replaced an older active one, which is the only moment a
    /// baseline may be retired. Answers for other epochs or other ids are
    /// ignored, so a delayed duplicate cannot disturb the current rotation.
    pub fn feedback(&mut self, feedback: Feedback) -> Option<Wire> {
        match feedback {
            Feedback::Stored { epoch, id }
                if Some(epoch) == self.epoch
                    && self.pending.as_ref().is_some_and(|(p, _)| *p == id) =>
            {
                self.retiring = self.active.take();
                self.active = self.pending.take();
                self.retire_age = 0;
                self.count = 0;
                self.retiring
                    .as_ref()
                    .map(|(id, _)| Wire::Retire { epoch, id: *id })
            }
            Feedback::Retired { epoch, id }
                if Some(epoch) == self.epoch
                    && self.retiring.as_ref().is_some_and(|(p, _)| *p == id) =>
            {
                self.retiring = None;
                self.retire_age = 0;
                None
            }
            Feedback::Missing { epoch, id }
                if Some(epoch) == self.epoch
                    && self.active.as_ref().is_some_and(|(p, _)| *p == id) =>
            {
                self.recover = true;
                None
            }
            _ => None,
        }
    }
}
/// Client-side delta decoder for one connection.
///
/// Pins at most two baselines, the active one and the candidate replacing it.
/// A third [`Wire::Full`] is refused rather than evicting a baseline a delta
/// might still reference, which is why the encoder retires before proposing.
#[derive(Default)]
pub struct Decoder {
    epoch: Option<u64>,
    baselines: BTreeMap<u64, Value>,
    retired_through: u64,
    /// Deltas refused because their baseline was not pinned.
    pub missing: u64,
    /// Deltas applied to a pinned baseline.
    pub deltas: u64,
}
impl Decoder {
    /// Baselines held for future deltas. The wire contract caps this at two:
    /// a third can only be proposed once the encoder retires one.
    pub fn pinned(&self) -> usize {
        self.baselines.len()
    }
    /// Applies one frame, returning the decoded message when it produced one and
    /// the feedback the encoder needs. A delta for an unpinned baseline yields
    /// `Feedback::Missing` and no message rather than a partially applied state.
    pub fn receive(&mut self, wire: Wire) -> io::Result<(Option<ServerMessage>, Option<Feedback>)> {
        let epoch = match &wire {
            Wire::Independent { epoch, .. }
            | Wire::Full { epoch, .. }
            | Wire::Delta { epoch, .. }
            | Wire::Retire { epoch, .. } => *epoch,
        };
        if self.epoch.is_some_and(|old| epoch < old) {
            return Ok((None, None));
        }
        if self.epoch != Some(epoch) {
            self.epoch = Some(epoch);
            self.baselines.clear();
            self.retired_through = 0;
        }
        let (value, feedback) = match wire {
            Wire::Independent { state, .. } => (state, None),
            Wire::Full { id, state, .. } => {
                if id <= self.retired_through {
                    return Ok((None, None));
                }
                if !self.baselines.contains_key(&id) && self.baselines.len() >= 2 {
                    return Err(io::Error::other("baseline cache overflow"));
                }
                let bytes = serde_json::to_vec(&state)?;
                if bytes.len() > BASE_LIMIT {
                    return Err(io::Error::other("baseline exceeds byte cap"));
                }
                validate(&bytes, epoch)?;
                if self.baselines.get(&id).is_some_and(|old| old != &state) {
                    return Err(io::Error::other("baseline identity changed"));
                }
                self.baselines.insert(id, state.clone());
                (state, Some(Feedback::Stored { epoch, id }))
            }
            Wire::Delta { base, patch, .. } => {
                let Some(mut value) = self.baselines.get(&base).cloned() else {
                    self.missing += 1;
                    return Ok((None, Some(Feedback::Missing { epoch, id: base })));
                };
                if let Some(patch) = patch {
                    apply(&mut value, patch, 0)?;
                }
                self.deltas += 1;
                (value, None)
            }
            Wire::Retire { id, .. } => {
                self.baselines.retain(|base, _| *base > id);
                self.retired_through = self.retired_through.max(id);
                return Ok((None, Some(Feedback::Retired { epoch, id })));
            }
        };
        let bytes = serde_json::to_vec(&value)?;
        if bytes.len() > crate::protocol::MAX_LINE_BYTES {
            return Err(io::Error::other("decoded state exceeds cap"));
        }
        Ok((Some(validate(&bytes, epoch)?), feedback))
    }
}
fn validate(bytes: &[u8], epoch: u64) -> io::Result<ServerMessage> {
    let message = crate::snapshot_codec::decode_player_message(bytes)?;
    if !matches!(&message, ServerMessage::Snapshot(state) if state.input_epoch == epoch) {
        return Err(io::Error::other("invalid baseline state/epoch"));
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::Simulation;
    use rm_simulator_world::{Field, FieldConfig};
    fn source(tick: u64, epoch: u64) -> Vec<u8> {
        let mut simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false);
        simulation.step(tick).unwrap();
        let mut state = simulation.state();
        state.snapshot_id = tick + 1;
        state.input_epoch = epoch;
        crate::snapshot_codec::encode_player_message(&ServerMessage::Snapshot(Box::new(state)))
    }
    fn wire(bytes: &[u8]) -> Wire {
        parse(&crate::compression::decompress(bytes, 4 << 20).unwrap())
            .unwrap()
            .unwrap()
    }
    fn decoded(message: Option<ServerMessage>) -> crate::simulation::SimulationState {
        let Some(ServerMessage::Snapshot(state)) = message else {
            panic!("missing snapshot")
        };
        *state
    }
    #[test]
    fn missing_ack_still_delivers_current_independent_state() {
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        let mut newest = 0;
        for tick in 0..30 {
            let bytes = encoder.snapshot(0, &source(tick, 0)).unwrap();
            let (message, _) = decoder.receive(wire(&bytes)).unwrap(); // Drop every application ACK.
            newest = newest.max(decoded(message).field.tick);
        }
        assert_eq!(newest, 29);
        assert_eq!(decoder.baselines.len(), 1);
        assert_eq!(encoder.deltas, 0);
    }
    #[test]
    fn deltas_survive_loss_reordering_and_duplicates_without_chaining() {
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        let (_, ack) = decoder
            .receive(wire(&encoder.snapshot(0, &source(0, 0)).unwrap()))
            .unwrap();
        encoder.feedback(ack.unwrap());
        let lost = encoder.snapshot(0, &source(1, 0)).unwrap();
        let next = encoder.snapshot(0, &source(2, 0)).unwrap();
        for bytes in [&next, &next, &lost] {
            let actual = decoded(decoder.receive(wire(bytes)).unwrap().0);
            let expected = decoded(Some(
                crate::snapshot_codec::decode_player_message(&source(actual.field.tick, 0))
                    .unwrap(),
            ));
            assert_eq!(actual, expected);
        }
        assert_eq!(decoder.baselines.len(), 1);
        assert!(encoder.deltas > 0);
    }
    #[test]
    fn retirement_pins_two_baselines_until_ack_and_old_full_cannot_resurrect() {
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        let old = encoder.snapshot(0, &source(0, 0)).unwrap();
        let (_, ack) = decoder.receive(wire(&old)).unwrap();
        encoder.feedback(ack.unwrap());
        let mut retirement = None;
        for tick in 1..40 {
            let (_, ack) = decoder
                .receive(wire(&encoder.snapshot(0, &source(tick, 0)).unwrap()))
                .unwrap();
            if let Some(ack) = ack
                && let Some(retire) = encoder.feedback(ack)
            {
                retirement = Some(retire);
                break;
            }
        }
        assert!(retirement.is_some());
        assert_eq!(decoder.baselines.len(), 2);
        for tick in 50..100 {
            decoder
                .receive(wire(&encoder.snapshot(0, &source(tick, 0)).unwrap()))
                .unwrap();
            assert!(encoder.pending.is_none());
        }
        let (_, ack) = decoder.receive(retirement.unwrap()).unwrap();
        assert_eq!(decoder.baselines.len(), 1);
        assert!(decoder.receive(wire(&old)).unwrap().0.is_none());
        encoder.feedback(ack.unwrap());
        assert!(encoder.retiring.is_none());
        decoder
            .receive(wire(&encoder.snapshot(0, &source(101, 0)).unwrap()))
            .unwrap();
        assert!(encoder.pending.is_some());
    }
    #[test]
    fn a_lost_retired_answer_is_resent_and_rotation_resumes() {
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        let (_, ack) = decoder
            .receive(wire(&encoder.snapshot(0, &source(0, 0)).unwrap()))
            .unwrap();
        encoder.feedback(ack.unwrap());
        let mut retirement = None;
        for tick in 1..40 {
            let (_, ack) = decoder
                .receive(wire(&encoder.snapshot(0, &source(tick, 0)).unwrap()))
                .unwrap();
            if let Some(ack) = ack
                && let Some(retire) = encoder.feedback(ack)
            {
                retirement = Some(retire);
                break;
            }
        }
        // The decoder retires the old baseline but its answer never arrives.
        let (_, retired) = decoder.receive(retirement.unwrap()).unwrap();
        assert!(matches!(retired, Some(Feedback::Retired { .. })));
        assert_eq!(decoder.baselines.len(), 1);
        // Without a retry the encoder would stay pinned for the connection's life.
        let mut resent = None;
        for tick in 100..100 + RETIRE_RESEND_FRAMES + 1 {
            decoder
                .receive(wire(&encoder.snapshot(0, &source(tick, 0)).unwrap()))
                .unwrap();
            assert!(
                encoder.pending.is_none(),
                "rotation is blocked until retired"
            );
            if let Some(retire) = encoder.resend_retire() {
                resent = Some(retire);
                break;
            }
        }
        let (_, retired) = decoder.receive(resent.expect("retire resent")).unwrap();
        encoder.feedback(retired.unwrap());
        assert!(encoder.retiring.is_none());
        // A fresh baseline can be proposed and stored again.
        let mut rotated = false;
        for tick in 200..260 {
            let (_, ack) = decoder
                .receive(wire(&encoder.snapshot(0, &source(tick, 0)).unwrap()))
                .unwrap();
            if let Some(ack) = ack {
                encoder.feedback(ack);
                rotated = true;
                break;
            }
        }
        assert!(rotated, "a new baseline must be storable after retirement");
        assert!(decoder.baselines.len() <= 2);
    }
    #[test]
    fn missing_baseline_recovers_and_epoch_change_rejects_old_traffic() {
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        let first = encoder.snapshot(1, &source(0, 1)).unwrap();
        let (_, ack) = decoder.receive(wire(&first)).unwrap();
        encoder.feedback(ack.unwrap());
        decoder.baselines.clear();
        let delta = encoder.snapshot(1, &source(2, 1)).unwrap();
        let (message, missing) = decoder.receive(wire(&delta)).unwrap();
        assert!(message.is_none());
        encoder.feedback(missing.unwrap());
        let (_, ack) = decoder
            .receive(wire(&encoder.snapshot(1, &source(3, 1)).unwrap()))
            .unwrap();
        assert!(ack.is_some());
        let new_epoch = encoder.snapshot(2, &source(4, 2)).unwrap();
        decoder.receive(wire(&new_epoch)).unwrap();
        assert!(decoder.receive(wire(&delta)).unwrap().0.is_none());
        assert!(decoder.receive(wire(&first)).unwrap().0.is_none());
        assert_eq!(decoder.baselines.len(), 1);
    }
    #[test]
    fn sustained_updates_save_encoded_bytes_and_caches_stay_bounded() {
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        for tick in 0..160 {
            let (_, ack) = decoder
                .receive(wire(&encoder.snapshot(0, &source(tick, 0)).unwrap()))
                .unwrap();
            if let Some(ack) = ack
                && let Some(retire) = encoder.feedback(ack)
            {
                let (_, ack) = decoder.receive(retire).unwrap();
                encoder.feedback(ack.unwrap());
            }
            assert!(decoder.baselines.len() <= 2);
        }
        eprintln!(
            "UDP independent/delta encoded bytes: {}/{} ({} deltas)",
            encoder.full_bytes, encoder.sent_bytes, encoder.deltas
        );
        // The delta scheme always beats independent frames here. How far is
        // codec-dependent: a strong dictionary makes an independent frame small
        // enough to win the size guard often, so the margin is narrower than
        // DEFLATE's. Keep the original DEFLATE bound as the production guard.
        assert!(encoder.sent_bytes < encoder.full_bytes);
        if crate::compression::selected().mode == crate::compression::Mode::Deflate {
            assert!(encoder.sent_bytes * 2 < encoder.full_bytes);
            assert!(encoder.deltas > 100);
        }
    }
}
