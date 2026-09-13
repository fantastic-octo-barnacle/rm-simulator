// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Independently usable owner state. Full snapshots remain coherent world/context checkpoints.
use crate::simulation::SimulationState;
use rm_simulator_world::ChassisSnapshot;
use serde::{Deserialize, Serialize};
use std::io;

/// Anchor magic, `RMO3`, checked before any field is read.
pub const MAGIC: &[u8; 4] = b"RMO3";
/// Largest accepted anchor datagram in bytes. An encoding past this fails
/// rather than fragmenting, because one anchor must fit one datagram.
pub const MAX_BYTES: usize = 1000;
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
    /// Encode the anchor as a little-endian datagram.
    ///
    /// Every f64 keeps full precision at a fixed eight bytes, so motion values
    /// cannot change the datagram size. The chassis config is deflated JSON
    /// behind a u16 length. Errors when that length does not fit or the whole
    /// anchor exceeds [`MAX_BYTES`].
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
        let config =
            miniz_oxide::deflate::compress_to_vec(&serde_json::to_vec(&self.owner.config)?, 1);
        let length = u16::try_from(config.len()).map_err(|_| invalid())?;
        bytes.extend(length.to_le_bytes());
        bytes.extend(config);
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
    /// Decode an anchor produced by [`OwnerAnchor::encode`].
    ///
    /// Rejects a short or oversized buffer, a wrong magic, a non-boolean flag, a
    /// non-finite float, trailing bytes and a zero snapshot id. The wheel count
    /// must equal the length in the encoded config, so the anchor is decoded
    /// only against the exact revision it names. The config must also pass its
    /// own validation.
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
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
        let length = u16::from_le_bytes(reader.take()?) as usize;
        let compressed = reader.0.get(..length).ok_or_else(invalid)?;
        let json = miniz_oxide::inflate::decompress_to_vec_with_limit(compressed, 8192)
            .map_err(|_| invalid())?;
        let config: rm_simulator_world::ChassisConfig = serde_json::from_slice(&json)?;
        config.validate().map_err(|_| invalid())?;
        reader.0 = &reader.0[length..];
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
                config,
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
/// The single error every anchor parsing failure produces.
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
    #[test]
    fn moving_owner_roundtrips_without_world_or_projectiles() {
        let mut field = Field::new(&FieldConfig::default()).unwrap();
        let chassis = field
            .add_chassis(&ChassisPlacement {
                team: Team::Red,
                spawn: Pose::at([0., 0., 0.3]),
                config: ChassisConfig::default(),
            })
            .unwrap();
        field
            .command_chassis(
                chassis,
                ChassisCommand {
                    forward_m_s: 2.,
                    aim_yaw_rad: 0.37,
                    ..Default::default()
                },
            )
            .unwrap();
        field.step(177).unwrap();
        let mut state = crate::simulation::Simulation::new(field, false).state();
        state.snapshot_id = 5;
        let anchor = OwnerAnchor::from_state(&state, chassis).unwrap();
        let bytes = anchor.encode().unwrap();
        eprintln!("owner anchor application bytes: {}", bytes.len());
        assert_eq!(OwnerAnchor::decode(&bytes).unwrap(), anchor);
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
        assert_eq!(OwnerAnchor::decode(&packed).unwrap(), noisy);
        for length in [0, 3, 30, packed.len() - 1] {
            assert!(OwnerAnchor::decode(&packed[..length]).is_err());
        }
        assert!(OwnerAnchor::decode(&vec![0; MAX_BYTES + 1]).is_err());
        assert!(OwnerAnchor::from_state(&state, chassis + 1).is_none());
    }
}
