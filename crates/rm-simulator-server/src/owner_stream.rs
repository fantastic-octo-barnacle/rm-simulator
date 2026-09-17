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

/// Anchor magic, `RMO5`, checked before any field is read. The fifth revision
/// keeps the `RMO4` header and configuration reference but quantizes every
/// dynamic float: translations to millimetres, quaternions to 1/32767,
/// velocities to 1 cm/s, rates to 1 mrad/s and aims to 0.1 mrad. Aims ride
/// 32-bit alongside wheel roll because both rotate without bound; every other
/// range fits 16 bits with room for violent motion (±327 m/s, ±32 rad/s). A
/// hostile or corrupt anchor still decodes to finite values, and
/// out-of-range or non-finite dynamics fail at encode time, so the datagram
/// size stays fixed whatever the motion.
pub const MAGIC: &[u8; 4] = b"RMO5";
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
    /// The header (snapshot, epoch, time, placement, chassis, flags and the
    /// configuration reference) keeps full width; every dynamic float is
    /// quantized (see [`MAGIC`]), so motion values cannot change the datagram
    /// size. Errors when a dynamic value is non-finite or outside its
    /// quantized range, or when the whole anchor exceeds [`MAX_BYTES`].
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
        for v in self.owner.pose.translation_m {
            bytes.extend(quantize_i32(v, POS_MM)?.to_le_bytes());
        }
        for v in self.owner.pose.rotation_wxyz {
            bytes.extend(quantize_i16(v, QUAT_SCALE)?.to_le_bytes());
        }
        for v in self.owner.turret.translation_m {
            bytes.extend(quantize_i32(v, POS_MM)?.to_le_bytes());
        }
        for v in self.owner.turret.rotation_wxyz {
            bytes.extend(quantize_i16(v, QUAT_SCALE)?.to_le_bytes());
        }
        for v in self.owner.velocity_m_s {
            bytes.extend(quantize_i16(v, VEL_SCALE)?.to_le_bytes());
        }
        for v in self.owner.angular_velocity_rad_s {
            bytes.extend(quantize_i16(v, RATE_SCALE)?.to_le_bytes());
        }
        let c = self.owner.command;
        bytes.extend(quantize_i16(c.forward_m_s, POS_MM)?.to_le_bytes());
        bytes.extend(quantize_i16(c.left_m_s, POS_MM)?.to_le_bytes());
        bytes.extend(quantize_i16(c.yaw_rate_rad_s, RATE_SCALE)?.to_le_bytes());
        bytes.extend(quantize_i32(c.aim_yaw_rad, AIM_SCALE)?.to_le_bytes());
        bytes.extend(quantize_i32(c.aim_pitch_rad, AIM_SCALE)?.to_le_bytes());
        for v in self.owner.held_aim_rad {
            bytes.extend(quantize_i32(v, AIM_SCALE)?.to_le_bytes());
        }
        for v in self.owner.gimbal_velocity_rad_s {
            bytes.extend(quantize_i16(v, RATE_SCALE)?.to_le_bytes());
        }
        bytes.push(u8::try_from(self.owner.wheels.len()).map_err(|_| invalid())?);
        for wheel in &self.owner.wheels {
            for v in wheel.hub_m {
                bytes.extend(quantize_i32(v, POS_MM)?.to_le_bytes());
            }
            bytes.extend(quantize_i32(wheel.spin_rad, AIM_SCALE)?.to_le_bytes());
            bytes.extend(quantize_i16(wheel.target_m_s, POS_MM)?.to_le_bytes());
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
            translation_m: reader.q32_array::<3>(POS_MM)?,
            rotation_wxyz: normalize(reader.q16_array::<4>(QUAT_SCALE)?)?,
        };
        let turret = Pose {
            translation_m: reader.q32_array::<3>(POS_MM)?,
            rotation_wxyz: normalize(reader.q16_array::<4>(QUAT_SCALE)?)?,
        };
        let velocity_m_s = reader.q16_array::<3>(VEL_SCALE)?;
        let angular_velocity_rad_s = reader.q16_array::<3>(RATE_SCALE)?;
        let command = ChassisCommand {
            forward_m_s: reader.q16(POS_MM)?,
            left_m_s: reader.q16(POS_MM)?,
            yaw_rate_rad_s: reader.q16(RATE_SCALE)?,
            aim_yaw_rad: reader.q32(AIM_SCALE)?,
            aim_pitch_rad: reader.q32(AIM_SCALE)?,
        };
        let held_aim_rad = reader.q32_array::<2>(AIM_SCALE)?;
        let gimbal_velocity_rad_s = reader.q16_array::<2>(RATE_SCALE)?;
        let count = reader.take::<1>()?[0] as usize;
        if count != config.wheel_hubs_m.len() || count > 16 {
            return Err(invalid());
        }
        let mut wheels = Vec::with_capacity(count);
        for _ in 0..count {
            wheels.push(WheelSnapshot {
                hub_m: reader.q32_array::<3>(POS_MM)?,
                spin_rad: reader.q32(AIM_SCALE)?,
                target_m_s: reader.q16(POS_MM)?,
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
/// Quantizer scales for the `RMO5` dynamic body, chosen to mirror the
/// checkpoint precisions: millimetre positions, 1/32767 quaternions,
/// centimetre-per-second velocities, milliradian-per-second rates and
/// 0.1 mrad aims. Wheel roll keeps 0.1 mrad in 32 bits because roll is
/// unbounded; every other range fits 16 bits with room for violent motion
/// (±327 m/s, ±32 rad/s, ±3.27 rad of aim).
const POS_MM: f64 = 1000.;
/// Quaternion component scale, matching the checkpoint wire grid.
const QUAT_SCALE: f64 = 32767.;
/// Body and wheel velocity scale, in units per m/s.
const VEL_SCALE: f64 = 100.;
/// Body, gimbal and yaw rate scale, in units per rad/s.
const RATE_SCALE: f64 = 1000.;
/// Aim, held aim and wheel roll scale, in units per radian.
const AIM_SCALE: f64 = 10000.;
/// The single error every anchor or configuration parsing failure produces.
fn invalid() -> io::Error {
    io::Error::other("invalid owner anchor")
}
/// Quantize one dynamic value to a signed wire integer, refusing a
/// non-finite value or one outside the integer range instead of wrapping it.
fn quantize_i16(value: f64, scale: f64) -> io::Result<i16> {
    if !value.is_finite() {
        return Err(invalid());
    }
    let quantized = (value * scale).round();
    if !(i16::MIN as f64..=i16::MAX as f64).contains(&quantized) {
        return Err(invalid());
    }
    Ok(quantized as i16)
}
/// Quantize one dynamic value to a signed 32-bit wire integer, refusing a
/// non-finite value or one outside the integer range instead of wrapping it.
fn quantize_i32(value: f64, scale: f64) -> io::Result<i32> {
    if !value.is_finite() {
        return Err(invalid());
    }
    let quantized = (value * scale).round();
    if !(i32::MIN as f64..=i32::MAX as f64).contains(&quantized) {
        return Err(invalid());
    }
    Ok(quantized as i32)
}
/// Normalize a decoded quaternion, rejecting a zero or grossly non-unit norm
/// the same way the checkpoint boundary does: integers always decode finite,
/// but a hostile anchor may still send a degenerate rotation.
fn normalize(rotation_wxyz: [f64; 4]) -> io::Result<[f64; 4]> {
    let norm = rotation_wxyz.iter().map(|v| v * v).sum::<f64>().sqrt();
    if !norm.is_finite() || norm < 0.5 || norm > 1.5 {
        return Err(invalid());
    }
    Ok(rotation_wxyz.map(|v| v / norm))
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
    /// Read one little-endian i16 and return its value in source units. The
    /// integer always decodes finite; range is enforced at encode time.
    fn q16(&mut self, scale: f64) -> io::Result<f64> {
        Ok(i16::from_le_bytes(self.take()?) as f64 / scale)
    }
    /// Read `N` little-endian i16 values, each in source units.
    fn q16_array<const N: usize>(&mut self, scale: f64) -> io::Result<[f64; N]> {
        let mut values = [0.; N];
        for value in &mut values {
            *value = self.q16(scale)?;
        }
        Ok(values)
    }
    /// Read one little-endian i32 and return its value in source units.
    fn q32(&mut self, scale: f64) -> io::Result<f64> {
        Ok(i32::from_le_bytes(self.take()?) as f64 / scale)
    }
    /// Read `N` little-endian i32 values, each in source units.
    fn q32_array<const N: usize>(&mut self, scale: f64) -> io::Result<[f64; N]> {
        let mut values = [0.; N];
        for value in &mut values {
            *value = self.q32(scale)?;
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

    /// The bytes the replaced layout spent on the configuration this anchor now
    /// references: the deflated JSON the old anchor repeated, measured with the
    /// current codec so the comparison uses one compressor.
    fn embedded_config_bytes(anchor: &OwnerAnchor) -> usize {
        2 + crate::compression::compress(&serde_json::to_vec(&anchor.owner.config).unwrap()).len()
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

    /// Dynamics decode within their quantization steps: millimetre
    /// positions, 1/32767 quaternions, 1 cm/s velocities, 1 mrad/s rates and
    /// 0.1 mrad aims. Wheel roll keeps 0.1 mrad in 32 bits.
    fn assert_quantized(expected: &OwnerAnchor, actual: &OwnerAnchor) {
        assert_eq!(expected.snapshot_id, actual.snapshot_id);
        assert_eq!(expected.input_epoch, actual.input_epoch);
        assert_eq!(expected.time_ns, actual.time_ns);
        assert_eq!(expected.paused, actual.paused);
        assert_eq!(expected.owner.id, actual.owner.id);
        assert_eq!(expected.owner.team, actual.owner.team);
        assert_eq!(expected.owner.defeated, actual.owner.defeated);
        assert_eq!(
            expected.owner.placement_revision,
            actual.owner.placement_revision
        );
        let close = |a: f64, b: f64, step: f64| {
            assert!((a - b).abs() <= step, "{a} != {b} within one {step} step");
        };
        for (a, b) in expected
            .owner
            .pose
            .translation_m
            .into_iter()
            .zip(actual.owner.pose.translation_m)
        {
            close(a, b, 0.001);
        }
        for (a, b) in expected
            .owner
            .pose
            .rotation_wxyz
            .into_iter()
            .zip(actual.owner.pose.rotation_wxyz)
        {
            close(a, b, 1. / 32767. + 1e-9);
        }
        for (a, b) in expected
            .owner
            .velocity_m_s
            .into_iter()
            .zip(actual.owner.velocity_m_s)
        {
            close(a, b, 0.01);
        }
        for (a, b) in expected
            .owner
            .angular_velocity_rad_s
            .into_iter()
            .zip(actual.owner.angular_velocity_rad_s)
        {
            close(a, b, 0.001);
        }
        let pairs = [
            (
                expected.owner.command.forward_m_s,
                actual.owner.command.forward_m_s,
                0.001,
            ),
            (
                expected.owner.command.left_m_s,
                actual.owner.command.left_m_s,
                0.001,
            ),
            (
                expected.owner.command.yaw_rate_rad_s,
                actual.owner.command.yaw_rate_rad_s,
                0.001,
            ),
            (
                expected.owner.command.aim_yaw_rad,
                actual.owner.command.aim_yaw_rad,
                0.0001,
            ),
            (
                expected.owner.command.aim_pitch_rad,
                actual.owner.command.aim_pitch_rad,
                0.0001,
            ),
        ];
        for (a, b, step) in pairs {
            close(a, b, step);
        }
        for (a, b) in expected
            .owner
            .held_aim_rad
            .into_iter()
            .zip(actual.owner.held_aim_rad)
        {
            close(a, b, 0.0001);
        }
        assert_eq!(expected.owner.wheels.len(), actual.owner.wheels.len());
        for (a, b) in expected.owner.wheels.iter().zip(&actual.owner.wheels) {
            for (x, y) in a.hub_m.into_iter().zip(b.hub_m) {
                close(x, y, 0.001);
            }
            close(a.spin_rad, b.spin_rad, 0.0001);
            close(a.target_m_s, b.target_m_s, 0.001);
        }
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
        assert_quantized(
            &anchor,
            &OwnerAnchor::decode(&bytes, &anchor.owner.config).unwrap(),
        );
        // The quantized layout is fixed: a noisy anchor costs the same bytes.
        let mut noisy = anchor.clone();
        noisy.owner.pose.translation_m = [1.12345678912345, -8.98765432123456, 0.12345678987654];
        noisy.owner.velocity_m_s = [0.3726741, 1.9982749813213, 0.0017218836524];
        for wheel in &mut noisy.owner.wheels {
            wheel.spin_rad = 200.197827635331;
            wheel.target_m_s = std::f64::consts::SQRT_2;
        }
        let packed = noisy.encode().unwrap();
        assert_eq!(
            packed.len(),
            bytes.len(),
            "motion precision cannot grow the datagram"
        );
        assert_quantized(
            &noisy,
            &OwnerAnchor::decode(&packed, &noisy.owner.config).unwrap(),
        );
        assert!(
            packed.len() < 230,
            "the quantized anchor must stay near 202 bytes, not 444"
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
        wrong_magic[..4].copy_from_slice(b"RMO4");
        assert!(OwnerAnchor::decode(&wrong_magic, &config).is_err());
        // Trailing bytes mean the sender and receiver disagree.
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(OwnerAnchor::decode(&trailing, &config).is_err());
        // A non-finite dynamic value never reaches the wire: encode refuses
        // it instead of silently sending a clamped integer.
        let mut nonfinite = anchor.clone();
        nonfinite.owner.pose.translation_m[0] = f64::NAN;
        assert!(nonfinite.encode().is_err());
        // An out-of-range dynamic value is refused the same way.
        let mut far = anchor.clone();
        far.owner.velocity_m_s[0] = 1e9;
        assert!(far.encode().is_err());
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
