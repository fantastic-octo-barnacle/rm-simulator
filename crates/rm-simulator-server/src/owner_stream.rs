// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Independently usable owner state. Full snapshots remain coherent world/context checkpoints.
//!
//! An anchor carries the owner chassis' dynamic state and an immutable
//! [`ConfigRevision`] naming its [`ChassisConfig`]. The configuration itself
//! travels once as an [`OwnerConfig`] on the reliable control lane
//! (`ServerMessage::OwnerConfig`), and an anchor may name a revision only after
//! the peer acknowledged it. A peer that meets an unknown reference drops the
//! anchor and asks for the configuration again; it never applies a reference it
//! cannot resolve and never fails the connection over one.
use crate::simulation::SimulationState;
use rm_simulator_world::{ChassisConfig, ChassisSnapshot};
use serde::{Deserialize, Serialize};
use std::io;

/// Anchor magic, `RMO4`, checked before any field is read. The fourth revision
/// replaces the deflated configuration that `RMO3` repeated in every anchor with
/// a [`ConfigRevision`] reference, so the two layouts are not interchangeable.
pub const MAGIC: &[u8; 4] = b"RMO4";
/// Largest accepted anchor datagram in bytes. An encoding past this fails
/// rather than fragmenting, because one anchor must fit one datagram.
pub const MAX_BYTES: usize = 1000;

/// Immutable identity of one owner [`ChassisConfig`].
///
/// The host and the client derive it the same way from the same canonical JSON,
/// so it is a pure function of the configuration: the same values always name
/// the same revision, two different configurations never share one (barring a
/// 64-bit FNV-1a collision), and a revision is never reused for changed values.
/// It is explicit rather than inferred from "the host already sent it": an
/// anchor may carry it only after the peer acknowledged it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConfigRevision(u64);

impl ConfigRevision {
    /// The revision of `config`.
    ///
    /// Zero is remapped to one so a decoded zero can never be confused with an
    /// absent reference.
    pub fn of(config: &ChassisConfig) -> Self {
        // FNV-1a over the exact JSON both sides serialize. That encoding is
        // deterministic (fixed struct field order, shortest round-trip floats),
        // so it is a stable canonical form; the deflated form is not, because a
        // different compressor version could change it.
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in serde_json::to_vec(config).expect("chassis config serializes") {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Self(if hash == 0 { 1 } else { hash })
    }
    /// The wire value, which is the whole identity. It carries no order or
    /// arithmetic meaning and a peer must not increment or compare it.
    pub const fn raw(self) -> u64 {
        self.0
    }
    /// Rebuilds an identity from its wire value, for feedback that carries the
    /// raw number rather than the typed field.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

/// One owner chassis configuration, carried once on the reliable control lane.
///
/// The host queues it before it may reference it, and keeps resending it on a
/// bounded schedule until the peer's
/// [`crate::udp_snapshot::Feedback::ConfigStored`] answer arrives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OwnerConfig {
    /// The immutable identity of `config`.
    pub revision: ConfigRevision,
    /// The configuration every anchor that names `revision` decodes against.
    pub config: ChassisConfig,
}

impl OwnerConfig {
    /// The frame for `config`, with its content-derived identity.
    pub fn new(config: ChassisConfig) -> Self {
        Self {
            revision: ConfigRevision::of(&config),
            config,
        }
    }
    /// Checks that `revision` names exactly `config` and that the configuration
    /// passes its own validation. A frame that fails this is refused rather
    /// than cached, so corrupted or mismatched values cannot be applied.
    pub fn validate(&self) -> io::Result<()> {
        if ConfigRevision::of(&self.config) != self.revision {
            return Err(invalid());
        }
        self.config.validate().map_err(|_| invalid())
    }
}

/// One independently usable owner state, tagged with the exact host revision it
/// was cut from. A decoder applies it only against the input revision it names,
/// because a full snapshot remains the coherent world checkpoint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OwnerAnchor {
    /// Host snapshot id the anchor was cut from. Zero is invalid on the wire.
    pub snapshot_id: u64,
    /// Host control epoch the anchor belongs to. A client that sees this change
    /// resets its input timeline rather than blending across the gap.
    pub input_epoch: u64,
    /// Host simulation time the anchor was taken at, in ns.
    pub time_ns: u64,
    /// Host pause state at that time. A paused anchor carries presentation, not
    /// live motion.
    pub paused: bool,
    /// The owner chassis state. Wheel contacts are stripped because a contact
    /// is solver state that a restore rebuilds.
    pub owner: ChassisSnapshot,
}
impl OwnerAnchor {
    /// Cut an anchor for `chassis` from `state`, or `None` when the field holds
    /// no such chassis.
    ///
    /// Wheel contacts are cleared, so a decoded anchor never claims ground
    /// contact the receiving solver did not observe.
    pub fn from_state(state: &SimulationState, chassis: u32) -> Option<Self> {
        let mut owner = state
            .field
            .chassis
            .iter()
            .find(|c| c.id == chassis)?
            .clone();
        for wheel in &mut owner.wheels {
            wheel.contact = None;
        }
        Some(Self {
            snapshot_id: state.snapshot_id,
            input_epoch: state.input_epoch,
            time_ns: state.field.time_ns,
            paused: state.paused,
            owner,
        })
    }
    /// The immutable revision this anchor's configuration would be encoded as.
    pub fn config_revision(&self) -> ConfigRevision {
        ConfigRevision::of(&self.owner.config)
    }
    /// Encode the anchor as a little-endian datagram.
    ///
    /// Every f64 keeps full precision at a fixed eight bytes, so motion values
    /// cannot change the datagram size. The chassis configuration is replaced by
    /// its eight-byte [`ConfigRevision`], which the receiver must already hold
    /// (see [`OwnerAnchor::decode`]). Errors only when the whole anchor exceeds
    /// [`MAX_BYTES`].
    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut bytes = MAGIC.to_vec();
        for n in [
            self.snapshot_id,
            self.input_epoch,
            self.time_ns,
            self.owner.placement_revision,
        ] {
            bytes.extend(n.to_le_bytes());
        }
        bytes.extend(self.owner.id.to_le_bytes());
        bytes.push(u8::from(self.paused));
        bytes.push(u8::from(self.owner.defeated));
        bytes.push(u8::from(self.owner.team == rm_simulator_world::Team::Blue));
        bytes.extend(self.config_revision().raw().to_le_bytes());
        let c = self.owner.command;
        for n in self
            .owner
            .pose
            .translation_m
            .into_iter()
            .chain(self.owner.pose.rotation_wxyz)
            .chain(self.owner.turret.translation_m)
            .chain(self.owner.turret.rotation_wxyz)
            .chain(self.owner.velocity_m_s)
            .chain(self.owner.angular_velocity_rad_s)
            .chain([
                c.forward_m_s,
                c.left_m_s,
                c.yaw_rate_rad_s,
                c.aim_yaw_rad,
                c.aim_pitch_rad,
            ])
            .chain(self.owner.held_aim_rad)
            .chain(self.owner.gimbal_velocity_rad_s)
        {
            bytes.extend(n.to_le_bytes());
        }
        bytes.push(u8::try_from(self.owner.wheels.len()).map_err(|_| invalid())?);
        for wheel in &self.owner.wheels {
            for n in wheel
                .hub_m
                .into_iter()
                .chain([wheel.spin_rad, wheel.target_m_s])
            {
                bytes.extend(n.to_le_bytes());
            }
        }
        if bytes.len() > MAX_BYTES {
            return Err(io::Error::other("owner anchor exceeds datagram budget"));
        }
        Ok(bytes)
    }
    /// Read only the configuration reference an encoded anchor names.
    ///
    /// A receiver looks the configuration up before [`OwnerAnchor::decode`],
    /// because decoding needs it. Only the fixed header is parsed, so this
    /// cannot be tricked into a large allocation. Errors on a short buffer, a
    /// wrong magic or an oversized datagram.
    pub fn config_revision_of(bytes: &[u8]) -> io::Result<ConfigRevision> {
        if bytes.len() > MAX_BYTES || !bytes.starts_with(MAGIC) {
            return Err(invalid());
        }
        let mut reader = Reader(&bytes[4..]);
        reader.u64()?; // snapshot id
        reader.u64()?; // input epoch
        reader.u64()?; // simulation time
        reader.u64()?; // placement revision
        reader.take::<4>()?; // chassis id
        reader.boolean()?; // paused
        reader.boolean()?; // defeated
        reader.boolean()?; // team
        Ok(ConfigRevision(reader.u64()?))
    }
    /// Decode an anchor produced by [`OwnerAnchor::encode`] against the exact
    /// configuration it names.
    ///
    /// `config` must be the configuration the caller holds for the anchor's
    /// [`ConfigRevision`]; a mismatch is refused rather than applied, so a
    /// changed configuration can never be confused with the old one. Rejects a
    /// short or oversized buffer, a wrong magic, a non-boolean flag, a
    /// non-finite float, trailing bytes and a zero snapshot id. The wheel count
    /// must equal the length in `config`, and the configuration must pass its
    /// own validation.
    pub fn decode(bytes: &[u8], config: &ChassisConfig) -> io::Result<Self> {
        use rm_simulator_world::{ChassisCommand, Pose, Team, WheelSnapshot};
        if bytes.len() > MAX_BYTES || !bytes.starts_with(MAGIC) {
            return Err(invalid());
        }
        let mut reader = Reader(&bytes[4..]);
        let snapshot_id = reader.u64()?;
        let input_epoch = reader.u64()?;
        let time_ns = reader.u64()?;
        let placement_revision = reader.u64()?;
        let id = u32::from_le_bytes(reader.take()?);
        let paused = reader.boolean()?;
        let defeated = reader.boolean()?;
        let team = if reader.boolean()? {
            Team::Blue
        } else {
            Team::Red
        };
        let revision = ConfigRevision(reader.u64()?);
        if revision != ConfigRevision::of(config) || config.validate().is_err() {
            return Err(invalid());
        }
        let pose = Pose {
            translation_m: reader.array()?,
            rotation_wxyz: reader.array()?,
        };
        let turret = Pose {
            translation_m: reader.array()?,
            rotation_wxyz: reader.array()?,
        };
        let velocity_m_s = reader.array()?;
        let angular_velocity_rad_s = reader.array()?;
        let [
            forward_m_s,
            left_m_s,
            yaw_rate_rad_s,
            aim_yaw_rad,
            aim_pitch_rad,
        ] = reader.array()?;
        let command = ChassisCommand {
            forward_m_s,
            left_m_s,
            yaw_rate_rad_s,
            aim_yaw_rad,
            aim_pitch_rad,
        };
        let held_aim_rad = reader.array()?;
        let gimbal_velocity_rad_s = reader.array()?;
        let count = reader.take::<1>()?[0] as usize;
        if count != config.wheel_hubs_m.len() || count > 16 {
            return Err(invalid());
        }
        let mut wheels = Vec::with_capacity(count);
        for _ in 0..count {
            wheels.push(WheelSnapshot {
                hub_m: reader.array()?,
                spin_rad: reader.array::<1>()?[0],
                target_m_s: reader.array::<1>()?[0],
                contact: None,
            });
        }
        if !reader.0.is_empty() || snapshot_id == 0 {
            return Err(invalid());
        }
        Ok(Self {
            snapshot_id,
            input_epoch,
            time_ns,
            paused,
            owner: ChassisSnapshot {
                id,
                team,
                config: config.clone(),
                placement_revision,
                pose,
                turret,
                velocity_m_s,
                angular_velocity_rad_s,
                command,
                held_aim_rad,
                gimbal_velocity_rad_s,
                wheels,
                defeated,
            },
        })
    }
}
/// The single error every anchor or configuration parsing failure produces.
fn invalid() -> io::Error {
    io::Error::other("invalid owner anchor")
}
/// A cursor over the anchor body after the four magic bytes.
struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    /// Consume the next `N` bytes, leaving the cursor after them, or fail on a
    /// short buffer.
    fn take<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let value = self
            .0
            .get(..N)
            .ok_or_else(invalid)?
            .try_into()
            .map_err(|_| invalid())?;
        self.0 = &self.0[N..];
        Ok(value)
    }
    /// Read one little-endian u64.
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take()?))
    }
    /// Read one strict flag: 0 is false and 1 is true, any other byte is invalid.
    fn boolean(&mut self) -> io::Result<bool> {
        match self.take::<1>()?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid()),
        }
    }
    /// Read `N` little-endian f64 values, rejecting any non-finite one because a
    /// non-finite pose would poison the restored body.
    fn array<const N: usize>(&mut self) -> io::Result<[f64; N]> {
        let mut values = [0.; N];
        for value in &mut values {
            *value = f64::from_le_bytes(self.take()?);
            if !value.is_finite() {
                return Err(invalid());
            }
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_simulator_world::*;

    /// One moving chassis at `ticks`, with a host snapshot id and input epoch.
    fn anchored(ticks: u64, snapshot_id: u64, epoch: u64) -> (SimulationState, u32) {
        let mut field = Field::new(&FieldConfig::default()).unwrap();
        let chassis = field
            .add_chassis(&ChassisPlacement {
                team: Team::Red,
                kind: rm_simulator_world::RobotKind::Infantry,
                spawn: Pose::at([0., 0., 0.3]),
                config: ChassisConfig::default(),
            })
            .unwrap();
        let mut simulation = crate::simulation::Simulation::new(field, false);
        simulation.step(ticks).unwrap();
        let mut state = simulation.state();
        state.snapshot_id = snapshot_id;
        state.input_epoch = epoch;
        (state, chassis)
    }

    /// The bytes `RMO3` spent on the configuration this anchor now references.
    fn embedded_config_bytes(anchor: &OwnerAnchor) -> usize {
        2 + miniz_oxide::deflate::compress_to_vec(
            &serde_json::to_vec(&anchor.owner.config).unwrap(),
            1,
        )
        .len()
    }

    #[test]
    fn config_identity_is_content_derived_and_never_reused_for_changed_values() {
        let base = ChassisConfig::default();
        // The same values always name the same revision, regardless of how many
        // times the host has sent them or whether it ever did.
        assert_eq!(ConfigRevision::of(&base), ConfigRevision::of(&base.clone()));
        assert_ne!(ConfigRevision::of(&base).raw(), 0);
        // A changed configuration must never be silently confused with the old
        // identity, however small the change.
        let mut changed = base.clone();
        changed.mass_kg += 0.000_001;
        assert_ne!(ConfigRevision::of(&base), ConfigRevision::of(&changed));
        let mut wheels = base.clone();
        wheels.wheel_hubs_m.push([0.1, 0.1]);
        assert_ne!(ConfigRevision::of(&base), ConfigRevision::of(&wheels));
        // A configuration frame must name exactly the configuration it carries.
        let frame = OwnerConfig::new(changed);
        frame.validate().unwrap();
        let mismatched = OwnerConfig {
            config: base,
            ..frame
        };
        assert!(mismatched.validate().is_err());
    }

    #[test]
    fn anchor_round_trips_against_its_named_configuration_only() {
        let (state, chassis) = anchored(177, 5, 3);
        let anchor = OwnerAnchor::from_state(&state, chassis).unwrap();
        let bytes = anchor.encode().unwrap();
        assert_eq!(
            OwnerAnchor::config_revision_of(&bytes).unwrap(),
            anchor.config_revision()
        );
        assert_eq!(
            OwnerAnchor::decode(&bytes, &anchor.owner.config).unwrap(),
            anchor
        );
        // The dynamic values stay lossless f64: a noisy anchor decodes exactly.
        let mut noisy = anchor.clone();
        noisy.owner.pose.translation_m = [1.12345678912345, -8.98765432123456, 0.12345678987654];
        noisy.owner.velocity_m_s = [0.0000000000003726741, 1.9982749813213, 0.0017218836524];
        for wheel in &mut noisy.owner.wheels {
            wheel.spin_rad = 2.197827635331;
            wheel.target_m_s = std::f64::consts::SQRT_2;
        }
        let packed = noisy.encode().unwrap();
        assert_eq!(
            packed.len(),
            bytes.len(),
            "motion precision cannot grow the datagram"
        );
        assert_eq!(
            OwnerAnchor::decode(&packed, &noisy.owner.config).unwrap(),
            noisy
        );
        // A different configuration must not decode an anchor that names the
        // old one, even when its wheel count happens to agree.
        let mut changed = anchor.owner.config.clone();
        changed.mass_kg += 1.;
        assert!(OwnerAnchor::decode(&bytes, &changed).is_err());
        assert!(OwnerAnchor::from_state(&state, chassis + 1).is_none());
    }

    #[test]
    fn anchor_still_rejects_malformed_input() {
        let (state, chassis) = anchored(20, 7, 0);
        let anchor = OwnerAnchor::from_state(&state, chassis).unwrap();
        let bytes = anchor.encode().unwrap();
        let config = anchor.owner.config.clone();
        for length in [0, 3, 30, bytes.len() - 1] {
            assert!(OwnerAnchor::decode(&bytes[..length], &config).is_err());
        }
        assert!(OwnerAnchor::decode(&vec![0; MAX_BYTES + 1], &config).is_err());
        // A wrong magic is not an anchor at all.
        let mut wrong_magic = bytes.clone();
        wrong_magic[..4].copy_from_slice(b"RMO3");
        assert!(OwnerAnchor::decode(&wrong_magic, &config).is_err());
        // Trailing bytes mean the sender and receiver disagree.
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(OwnerAnchor::decode(&trailing, &config).is_err());
        // A non-finite dynamic value would poison the restored body.
        let mut nonfinite = anchor.clone();
        nonfinite.owner.pose.translation_m[0] = f64::NAN;
        assert!(OwnerAnchor::decode(&nonfinite.encode().unwrap(), &config).is_err());
        // A zero snapshot id is invalid on the wire.
        let mut zero = anchor;
        zero.snapshot_id = 0;
        assert!(OwnerAnchor::decode(&zero.encode().unwrap(), &config).is_err());
        // Peeking a reference must reject the same malformed headers.
        assert!(OwnerAnchor::config_revision_of(&bytes[..10]).is_err());
        assert!(OwnerAnchor::config_revision_of(&wrong_magic).is_err());
    }

    #[test]
    fn reference_replaces_the_embedded_configuration_for_fewer_bytes() {
        let (state, chassis) = anchored(40, 9, 0);
        let anchor = OwnerAnchor::from_state(&state, chassis).unwrap();
        let reference = anchor.encode().unwrap();
        let embedded = reference.len() - 8 + embedded_config_bytes(&anchor);
        eprintln!(
            "owner anchor embedded/reference bytes: {embedded}/{}",
            reference.len()
        );
        assert!(
            reference.len() < embedded,
            "the reference must remove the repeated configuration"
        );
        // The dynamic bytes are unchanged, and one anchor still fits one datagram.
        assert_eq!(
            embedded - reference.len(),
            embedded_config_bytes(&anchor) - 8
        );
        assert!(reference.len() <= MAX_BYTES);
    }
}
