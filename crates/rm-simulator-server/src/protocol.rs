// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Wire format between the simulation host and its clients: typed messages
//! carried in framed application payloads over GNS UDP. The client opens with
//! `Hello`; the server answers `Welcome` and then streams `Snapshot`s at its
//! own rate while accepting commands at any time. Everything a client can
//! do is a [`Command`], which the HTTP panel also posts.
use crate::simulation::SimulationState;
use rm_simulator_world::{ChassisCommand, ChassisConfig, Pose, RefereeCommand, Shot, Team};
use serde::{Deserialize, Serialize};

/// Version 22 drops the shooter-view fire path, the TCP snapshot delta chain
/// and the unread input acknowledgements.
/// Version 23 carries acceleration-limited gimbal motor state in owner anchors.
/// Version 25 adds host-enforced lobby passwords to Hello.
/// Version 26 adds per-pilot weapon updates, separate host caps, seeded angular
/// spread, speed variation and actual launch-speed feedback.
/// Version 27 delivers authoritative armor contacts independently of snapshots.
/// Version 28 references the owner chassis configuration by immutable identity
/// instead of repeating it in every anchor, with a dedicated reliable frame.
/// Version 29 combines referenced owner configurations with lossless RMI3 input
/// batches; experimental version 28 builds carried only one of these changes.
/// Version 30 carries projectile first-contact and dwell times for spent-ball
/// retirement. Version 31 names the shared physics tick in Hello and Welcome
/// so a host refuses a client that predicts at another rate.
/// Version 32 defaults periodic UDP checkpoints to packed fine fixed point
/// with its own embedded trained dictionary. Full confirmations stay exact.
/// Version 33 lets each pilot name the [`Robot`] it drives in Hello; the
/// chassis assignment and the roster repeat it, and the gun caliber follows
/// the robot instead of one host setting.
/// Version 34 removes the `ShotFinished` message, which no host ever produced:
/// a shot's end was only ever reported as a `ShotResult`.
/// Version 35 makes packed checkpoints the only periodic snapshot encoding and
/// ZSTD the only wire codec, removing the JSON checkpoint path, the DEFLATE
/// codec and every environment selector that chose between them.
/// Version 36 freezes the simulation at the 128 Hz tick and drops the rate from
/// the handshake.
/// Version 37 carries projectile timestamps as checkpoint-relative ages in
/// nanoseconds instead of absolute simulation times, which keeps every age
/// under 2^33 for a four-second ball. Reconstruction is exact, so prediction
/// replays retire the same balls on the same ticks. It also quantizes the
/// owner anchor (`RMO5`): millimetre positions, 1/32767 quaternions,
/// centimetre-per-second velocities, milliradian-per-second rates and
/// 0.1 mrad aims cut the anchor from 444 bytes toward 202.
/// Version 38 removes JSON from the gameplay wire. Checkpoints (`RMB1`) are a
/// positional, type-driven bitpack quantized while serializing, and every
/// other server (`RMM1`) and client (`RMQ1`) message, acknowledgement (`RMA2`)
/// and retirement (`RMR1`) uses the same codec.
/// Version 39 drops derived data from the checkpoint: the field clock, the
/// rune, outpost and referee views (rebuilt from the restore), wheel hubs and
/// tyre targets (rebuilt from pose, configuration and command). Every absolute
/// timestamp becomes a whole-tick code with an exact sub-tick remainder list,
/// and the dictionary is retrained.
/// Version 40 tightens the checkpoint's float coding: the field path implies
/// each fixed-point grid, so no grid index travels; delta step differences are
/// exponential-Golomb codes instead of a 6-bit width and raw bits; chassis
/// pose and turret rotations are smallest-three. The owner anchor (`RMO6`)
/// packs its two quaternions smallest-three too, and the dictionary is
/// retrained.
/// Version 41 lays the checkpoint out by change rate: byte-aligned slow
/// records (header, shot results, hits, bases, rules, chassis identity,
/// configuration and command) precede the dense chassis motion and
/// projectiles; chassis configurations and the projectile policy travel as a
/// preset index when they equal one, exactly otherwise. Deltas under 128
/// bytes skip compression, and the dictionary is retrained.
/// Version 42 differences each delta against its baseline dead-reckoned to the
/// frame's tick, carrying the tick lead in the delta header; a changed sequence
/// realigns on its elements' first field, so retired and spawned projectiles
/// no longer resend the whole array; delta Golomb orders are retuned for the
/// residuals and a baseline may rotate after 12 frames instead of 32.
/// Version 43 removes legacy surface: `ShotScheduled` no longer carries an
/// intended time, `Fire` and `FireAimed` no longer carry client timing
/// diagnostics, the checkpoint's rules always travel as the restore (a state
/// without a matching restore cannot be encoded), and the `RMI2` input batch
/// is no longer accepted. The protocol 41 dictionary is unchanged.
pub const PROTOCOL_VERSION: u32 = 43;

/// Explains incompatible host and client wire versions and how to resolve them.
///
/// ```
/// use rm_simulator_server::protocol::version_mismatch;
/// let message = version_mismatch(27, 26);
/// assert!(message.contains("host protocol 27, your protocol 26"));
/// assert!(message.contains("Update both games"));
/// ```
pub fn version_mismatch(host: u32, client: u32) -> String {
    format!(
        "Version mismatch: host protocol {host}, your protocol {client}. Update both games to the same version."
    )
}

/// Default listen port for the gameplay UDP transport.
pub const DEFAULT_PORT: u16 = 7700;
/// Default listen port for the referee HTTP panel.
pub const DEFAULT_HTTP_PORT: u16 = 7780;
/// Largest accepted message; snapshots of a busy field are a few tens of kB.
pub const MAX_LINE_BYTES: usize = 4 << 20;

/// Host caps, separate from the weapon settings a pilot starts with.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WeaponLimits {
    /// Maximum actual launch speed, in m/s, in (0, 40].
    pub max_speed_m_s: f64,
    /// Shortest permitted firing interval in ns; must be positive.
    pub min_interval_ns: u64,
}
impl Default for WeaponLimits {
    fn default() -> Self {
        Self {
            max_speed_m_s: 30.,
            min_interval_ns: 33_333_334,
        }
    }
}
impl WeaponLimits {
    /// Validate a pilot's configuration against the host's caliber and caps.
    ///
    /// ```
    /// use rm_simulator_server::protocol::{WeaponConfig, WeaponLimits};
    /// use rm_simulator_world::Caliber;
    /// let limits = WeaponLimits::default();
    /// assert!(limits.admit(Caliber::Mm17, WeaponConfig::default()).is_ok());
    /// ```
    pub fn admit(
        self,
        caliber: rm_simulator_world::Caliber,
        requested: WeaponConfig,
    ) -> Result<WeaponConfig, &'static str> {
        if !self.max_speed_m_s.is_finite()
            || !(0.0..=40.0).contains(&self.max_speed_m_s)
            || self.max_speed_m_s == 0.0
            || self.min_interval_ns == 0
        {
            return Err("host weapon limits require a positive interval and speed in (0, 40] m/s");
        }
        requested.validate()?;
        if requested.shot.caliber != caliber {
            return Err("caliber follows the robot");
        }
        if requested.shot.speed_m_s > self.max_speed_m_s {
            return Err("muzzle speed exceeds the host limit");
        }
        if requested.interval_ns < self.min_interval_ns {
            return Err("fire rate exceeds the host limit");
        }
        Ok(requested)
    }
}

/// The authoritative weapon offered by a host.
///
/// The host supplies defaults and caps for every pilot. Each pilot can choose
/// a lower rate or speed and any valid spread through `ConfigureWeapon`.
/// The host applies the stored settings when it launches a projectile.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WeaponConfig {
    /// Projectile template fired from the authoritative chassis muzzle.
    pub shot: Shot,
    /// Shortest simulation time between two launches, in nanoseconds. The
    /// default is 50,000,000 ns, or 20 Hz.
    pub interval_ns: u64,
    /// Maximum muzzle-speed deviation in m/s, in [0, 1]. The Gaussian standard
    /// deviation is one third of this value; zero disables speed variation.
    #[serde(default)]
    pub speed_variation_m_s: f64,
    /// Angular dispersion applied by the host and client prediction.
    #[serde(default)]
    pub spread: BulletSpread,
}
impl Default for WeaponConfig {
    fn default() -> Self {
        Self {
            shot: Shot {
                caliber: rm_simulator_world::Caliber::Mm17,
                speed_m_s: 25.0,
            },
            interval_ns: 50_000_000,
            speed_variation_m_s: 0.3,
            spread: BulletSpread {
                angle_rad: 0.3_f64.to_radians(),
                ..BulletSpread::default()
            },
        }
    }
}
impl WeaponConfig {
    /// Sample the launch speed for a configuration admitted by the host.
    /// `max_speed_m_s` is that host's speed cap, at least the nominal speed.
    /// The normal distribution is truncated at +/- three sigma and again at
    /// positive speed and the host cap. Its seed, chassis id and intended time
    /// in ns reproduce the sample independently of angular spread.
    ///
    /// ```
    /// use rm_simulator_server::protocol::WeaponConfig;
    /// let mut weapon = WeaponConfig::default();
    /// weapon.speed_variation_m_s = 1.0;
    /// let shot = weapon.sample_shot(4, 10_000_000, 30.0);
    /// assert!((24.0..=26.0).contains(&shot.speed_m_s));
    /// ```
    pub fn sample_shot(self, shooter: u32, intended_ns: u64, max_speed_m_s: f64) -> Shot {
        if self.speed_variation_m_s == 0.0 {
            return self.shot;
        }
        let mut bits = mix_bits(
            mix_bits(self.spread.seed ^ u64::from(shooter)) ^ intended_ns ^ 0x739d0383b683c057,
        );
        // Rejection sampling on the allowed interval avoids piling probability
        // onto the cap. The interval contains the Gaussian mode, even when a
        // very low speed cap leaves only a narrow portion of the distribution.
        let low = (-self.speed_variation_m_s).max(-self.shot.speed_m_s);
        let high = self
            .speed_variation_m_s
            .min(max_speed_m_s - self.shot.speed_m_s);
        for _ in 0..128 {
            let offset = low + unit_bits(bits) * (high - low);
            bits = mix_bits(bits);
            let acceptance = unit_bits(bits);
            bits = mix_bits(bits);
            let z = offset / (self.speed_variation_m_s / 3.0);
            let speed_m_s = self.shot.speed_m_s + offset;
            if acceptance <= (-0.5 * z * z).exp() && speed_m_s > 0.0 && speed_m_s <= max_speed_m_s {
                return Shot {
                    speed_m_s,
                    ..self.shot
                };
            }
        }
        // Keep runtime bounded for every seed; the admitted nominal speed is safe.
        self.shot
    }

    /// Check a configuration before a host adopts it.
    ///
    /// The muzzle speed must be finite and inside (0, 40] m/s, and the interval
    /// must be positive. These are simulator input bounds, not additional
    /// competition-rule enforcement. Invalid configurations are refused.
    pub fn validate(self) -> Result<Self, &'static str> {
        if !self.shot.speed_m_s.is_finite()
            || self.shot.speed_m_s <= 0.0
            || self.shot.speed_m_s > 40.0
        {
            return Err("muzzle speed must be in (0, 40] m/s");
        }
        if self.interval_ns == 0 {
            return Err("fire interval must be positive");
        }
        if !self.speed_variation_m_s.is_finite() || !(0.0..=1.0).contains(&self.speed_variation_m_s)
        {
            return Err("muzzle speed variation must be in [0, 1] m/s");
        }
        self.spread.validate()?;
        Ok(self)
    }
}

// SplitMix64 provides fixed integer mixing without mutable RNG state.
fn mix_bits(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}
fn unit_bits(x: u64) -> f64 {
    (x >> 11) as f64 / ((1_u64 << 53) as f64)
}

/// Radial distribution inside the configured spread cone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum SpreadDistribution {
    /// Uniform probability per solid angle inside the cone.
    Uniform,
    /// Gaussian angular offsets, truncated at three standard deviations.
    #[default]
    Gaussian,
}

/// Repeatable projectile dispersion. These are simulator settings, not rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BulletSpread {
    /// Maximum angular deviation from the muzzle axis in radians, in [0, pi/2].
    /// For Gaussian spread this is three times the angular standard deviation.
    pub angle_rad: f64,
    /// Probability distribution within the cone.
    pub distribution: SpreadDistribution,
    /// Seed combined with chassis id and intended launch time for each shot.
    pub seed: u64,
}
impl BulletSpread {
    /// Reject nonfinite angles and angles outside [0, pi/2] radians.
    ///
    /// ```
    /// use rm_simulator_server::protocol::BulletSpread;
    /// let spread = BulletSpread { angle_rad: -0.1, ..Default::default() };
    /// assert!(spread.validate().is_err());
    /// ```
    pub fn validate(self) -> Result<Self, &'static str> {
        if !self.angle_rad.is_finite()
            || !(0.0..=std::f64::consts::FRAC_PI_2).contains(&self.angle_rad)
        {
            return Err("spread angle must be in [0, 90] degrees");
        }
        Ok(self)
    }

    /// Rotate a muzzle within the spread cone without moving its origin.
    /// The same seed, chassis and intended time in ns produce the same offset,
    /// independent of delivery order and host time. Call after validation.
    ///
    /// ```
    /// use rm_simulator_server::protocol::BulletSpread;
    /// use rm_simulator_world::Pose;
    /// let muzzle = Pose::at([1.0, 2.0, 3.0]);
    /// assert_eq!(BulletSpread::default().apply(muzzle, 1, 0), muzzle);
    /// ```
    pub fn apply(self, mut muzzle: Pose, shooter: u32, intended_ns: u64) -> Pose {
        if self.angle_rad == 0.0 {
            return muzzle;
        }
        let bits = mix_bits(mix_bits(self.seed ^ u64::from(shooter)) ^ intended_ns);
        let u = unit_bits(bits);
        let azimuth = std::f64::consts::TAU * unit_bits(mix_bits(bits));
        let angle = match self.distribution {
            SpreadDistribution::Uniform => 2.0 * (u.sqrt() * (self.angle_rad * 0.5).sin()).asin(),
            SpreadDistribution::Gaussian => {
                self.angle_rad / 3.0 * (-2.0 * (1.0 - u * (1.0 - (-4.5_f64).exp())).ln()).sqrt()
            }
        };
        let (sin, cos) = (angle * 0.5).sin_cos();
        let offset = [cos, 0.0, -sin * azimuth.sin(), sin * azimuth.cos()];
        muzzle.rotation_wxyz = crate::math::quat_multiply(muzzle.rotation_wxyz, offset);
        muzzle
    }
}

/// What a client does on the field. A pilot drives a chassis and fires;
/// a spectator only watches; the referee watches and runs the match (the
/// `Referee`, `Pause` and `Step` commands are refused from other network roles).
/// The embedded owner has separate, in-process authority for local controls.
///
/// The host makes the real admission decision with [`Role::referees`]. Nothing
/// carries the role in the other direction: a command does not name the role
/// that is allowed to send it.
///
/// ```
/// use rm_simulator_server::protocol::Role;
///
/// // A missing role is the pilot, so an old Hello can never hand someone the
/// // match controls.
/// assert_eq!(Role::default(), Role::Pilot);
/// assert!(Role::Referee.referees());
/// assert!(!Role::Pilot.referees());
/// assert!(!Role::Spectator.referees());
/// assert_eq!(Role::Referee.name(), "referee");
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// Drives a chassis, aims and fires. The default for a Hello that omits
    /// the role.
    #[default]
    Pilot,
    /// Watches only. A spectator is on a team but has no chassis.
    Spectator,
    /// Watches on no team and may run the match. A host grants this to nobody
    /// else, and never gives the referee a robot.
    Referee,
}
impl Role {
    /// Lower-case name for notices and the roster. Stable for display and not
    /// a wire value, so nothing parses it back into a role.
    pub fn name(self) -> &'static str {
        match self {
            Role::Pilot => "pilot",
            Role::Spectator => "spectator",
            Role::Referee => "referee",
        }
    }
    /// May run the match clock and the referee commands.
    ///
    /// This is the only role check a host applies before a match control, and
    /// it is the reason a [`Command`] is refused from a pilot however the
    /// client labels itself.
    pub fn referees(self) -> bool {
        self == Role::Referee
    }
}

/// The robot a pilot asks to drive. Each robot fixes its chassis preset, its
/// gun caliber and the number printed on its armor, so a host never offers a
/// caliber of its own: the Hero fires 42 mm rounds from the mecanum chassis
/// and every infantry fires 17 mm rounds from the omni chassis. The two
/// infantry differ only in their number.
///
/// ```
/// use rm_simulator_server::protocol::Robot;
/// use rm_simulator_world::{Caliber, RobotKind};
///
/// assert_eq!(Robot::default(), Robot::Infantry3);
/// assert_eq!(Robot::Hero.caliber(), Caliber::Mm42);
/// assert_eq!(Robot::Infantry4.caliber(), Caliber::Mm17);
/// assert!(Robot::Hero.chassis_config().mecanum);
/// assert!(!Robot::Infantry3.chassis_config().mecanum);
/// assert_eq!(Robot::Infantry4.kind(), RobotKind::Infantry);
/// assert_eq!(Robot::Infantry4.number(), 4);
/// assert_eq!(Robot::parse("infantry-4"), Some(Robot::Infantry4));
/// assert_eq!(Robot::parse("infantry"), Some(Robot::Infantry3));
/// assert_eq!(Robot::parse(Robot::Hero.id()), Some(Robot::Hero));
/// assert_eq!(Robot::Hero.name(), "Hero");
/// ```
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum,
)]
pub enum Robot {
    /// Mecanum Hero with the 42 mm gun, armor number 1.
    Hero,
    /// Omni infantry number 3 with the 17 mm gun. The default for a Hello
    /// that names no robot.
    #[default]
    #[value(name = "infantry-3", alias = "infantry")]
    Infantry3,
    /// Omni infantry number 4 with the 17 mm gun, otherwise the same robot.
    #[value(name = "infantry-4")]
    Infantry4,
}
impl Robot {
    /// Every robot a pilot can choose, in menu order.
    pub const ALL: [Robot; 3] = [Robot::Hero, Robot::Infantry3, Robot::Infantry4];
    /// Display name for menus, notices and the roster.
    pub fn name(self) -> &'static str {
        match self {
            Robot::Hero => "Hero",
            Robot::Infantry3 => "Infantry 3",
            Robot::Infantry4 => "Infantry 4",
        }
    }
    /// The command-line and remembered-settings spelling: `hero`,
    /// `infantry-3` or `infantry-4`. [`Robot::parse`] reads it back.
    pub fn id(self) -> &'static str {
        match self {
            Robot::Hero => "hero",
            Robot::Infantry3 => "infantry-3",
            Robot::Infantry4 => "infantry-4",
        }
    }
    /// Read an [`Robot::id`] spelling; `infantry` alone means infantry 3.
    pub fn parse(text: &str) -> Option<Robot> {
        match text.trim() {
            "hero" => Some(Robot::Hero),
            "infantry" | "infantry-3" => Some(Robot::Infantry3),
            "infantry-4" => Some(Robot::Infantry4),
            _ => None,
        }
    }
    /// The caliber this robot's gun fires: 42 mm for the Hero, 17 mm for an
    /// infantry.
    pub fn caliber(self) -> rm_simulator_world::Caliber {
        match self {
            Robot::Hero => rm_simulator_world::Caliber::Mm42,
            Robot::Infantry3 | Robot::Infantry4 => rm_simulator_world::Caliber::Mm17,
        }
    }
    /// The robot class the referee records for it.
    pub fn kind(self) -> rm_simulator_world::RobotKind {
        match self {
            Robot::Hero => rm_simulator_world::RobotKind::Hero,
            Robot::Infantry3 | Robot::Infantry4 => rm_simulator_world::RobotKind::Infantry,
        }
    }
    /// The number printed on its armor: 1 for the Hero, 3 or 4 for an infantry.
    pub fn number(self) -> u8 {
        match self {
            Robot::Hero => 1,
            Robot::Infantry3 => 3,
            Robot::Infantry4 => 4,
        }
    }
    /// The chassis preset it drives: the mecanum Hero or the omni infantry.
    pub fn chassis_config(self) -> rm_simulator_world::ChassisConfig {
        match self {
            Robot::Hero => rm_simulator_world::ChassisConfig::hero(),
            Robot::Infantry3 | Robot::Infantry4 => rm_simulator_world::ChassisConfig::default(),
        }
    }
}

/// One seat as a short phrase for a join or leave notice: "red driving Hero
/// (chassis 3)", "blue, spectating" or "referee".
///
/// A chassis takes precedence over the role label, so a pilot is named by the
/// robot it drives. Without a team the role name stands alone, which is how
/// the referee reads. `team`, `chassis` and `robot` come from the roster
/// entry, so the text matches what [`PlayerInfo`] reports for the same seat.
///
/// ```
/// use rm_simulator_server::protocol::{Robot, Role, describe_seat};
/// use rm_simulator_world::Team;
///
/// assert_eq!(
///     describe_seat(Some(Team::Red), Role::Pilot, Some(3), Some(Robot::Hero)),
///     "red driving Hero (chassis 3)"
/// );
/// assert_eq!(describe_seat(Some(Team::Blue), Role::Spectator, None, None), "blue, spectating");
/// assert_eq!(describe_seat(None, Role::Referee, None, None), "referee");
/// ```
pub fn describe_seat(
    team: Option<Team>,
    role: Role,
    chassis: Option<u32>,
    robot: Option<Robot>,
) -> String {
    match (team, role, chassis) {
        (Some(team), _, Some(chassis)) => format!(
            "{} driving {} (chassis {chassis})",
            team.name(),
            robot.map_or("a robot", Robot::name)
        ),
        (Some(team), _, None) => format!("{}, spectating", team.name()),
        (None, role, _) => role.name().to_string(),
    }
}

/// Anything that changes the simulation.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Privileged training bots use ordinary chassis physics and never shoot.
    SpawnBot {
        /// Team the bot plays for.
        team: Team,
        /// Constant body yaw rate in rad/s. The host accepts a finite value
        /// within +/-20 and caps the field at 32 bots.
        spin_rad_s: f64,
    },
    /// Remove one training bot and its chassis.
    RemoveBot {
        /// Chassis id of the bot, as returned by an earlier [`Command::SpawnBot`].
        chassis: u32,
    },
    /// Remove every training bot.
    ClearBots,

    /// Refreshable pilot control state. Only the owning pilot may submit it.
    PilotInput {
        /// The sender's own chassis. The host refuses anyone else's.
        chassis: u32,
        /// One sampled control frame, carrying its own sequence, sample time
        /// and life revision. Stale copies refresh evidence without renewing
        /// the input lease.
        frame: crate::input_stream::InputFrame,
    },
    /// Embedded owner only: place their existing chassis using terrain ground lookup.
    /// Preserves its identity and referee record.
    PlaceChassis {
        /// The sender's own chassis.
        chassis: u32,
        /// New body position in world FLU metres. Must be finite.
        position_m: [f64; 3],
        /// New body heading in degrees, counter-clockwise from +x.
        yaw_deg: f64,
    },

    /// Desired body velocity and gun aim of a chassis; a host only accepts
    /// the sender's own.
    Chassis {
        /// The sender's own chassis.
        chassis: u32,
        /// Body-frame velocity in m/s and world-referenced gun aim. Every
        /// numeric field must be finite for the host to apply it.
        command: ChassisCommand,
    },
    /// Restore this defeated pilot's HP in place.
    Respawn {
        /// The sender's own chassis, which must currently be defeated.
        chassis: u32,
    },
    /// Debug recovery: restore HP and return this pilot to its initial spawn.
    ResetRobot {
        /// The sender's own chassis. Unlike [`Command::Respawn`] this needs no
        /// defeat.
        chassis: u32,
    },
    /// Buy one projectile for this pilot from team gold.
    BuyAmmo {
        /// The sender's own chassis.
        chassis: u32,
        /// Projectile class to buy.
        caliber: rm_simulator_world::Caliber,
    },
    /// Change this pilot's weapon settings for subsequent launches. Already
    /// flying projectiles retain their velocity and caliber. Queued fire
    /// requests use the settings current when the host executes them.
    ConfigureWeapon {
        /// The sender's own chassis.
        chassis: u32,
        /// Requested configuration within the host's rate and speed limits.
        weapon: WeaponConfig,
    },
    /// Fire the pilot's configured weapon from the authoritative chassis muzzle.
    /// A host only accepts the sender's own chassis.
    Fire {
        /// Chassis that fires; must be the sender's own.
        shooter: u32,
    },
    /// A deduplicated shot bound to an exact input/aim sample.
    ///
    /// The host keys the shot on `shooter` and `shot_id`, so a retransmission
    /// of the same id resolves to the first outcome instead of a second
    /// projectile. A sample that is still in the future is queued and fired at
    /// its tick; an expired one is refused. Retrying the same id with changed
    /// contents is an error.
    FireAimed {
        /// Chassis that fires; must be the sender's own.
        shooter: u32,
        /// Client-assigned identity, unique per shooter and never zero.
        shot_id: u64,
        /// The control and aim sample the shot is bound to. Its life revision
        /// and sample time decide whether the shot is still admissible.
        input: crate::input_stream::InputFrame,
    },
    /// Privileged training/free-camera projectile injection.
    SpawnProjectile {
        /// Muzzle pose in world FLU, with +x along the launch direction.
        muzzle: Pose,
        /// Projectile template and launch speed in m/s.
        shot: Shot,
    },
    /// Referee only on a host.
    Referee(RefereeCommand),
    /// Referee only on a host.
    Pause {
        /// True pauses the world clock, false resumes it.
        paused: bool,
    },
    /// Step the world by `ticks` physics ticks (also while paused). Referee
    /// only on a host.
    Step {
        /// Ticks to advance, from 1 to 60,000. Each tick is one fixed 128 Hz
        /// physics tick of world time.
        ticks: u64,
    },
}
impl Command {
    /// Runs the match rather than a robot: reserved for the referee.
    ///
    /// The host refuses any of these from a peer that is neither the referee
    /// nor the embedded owner, whatever the peer calls itself. Firing and
    /// driving are excluded, so an ordinary command needs no match privilege.
    ///
    /// ```
    /// use rm_simulator_server::protocol::Command;
    /// use rm_simulator_world::{ChassisCommand, RefereeCommand, Team};
    ///
    /// // The three match controls a referee holds.
    /// assert!(Command::Referee(RefereeCommand::StartMatch).is_match_control());
    /// assert!(Command::Pause { paused: true }.is_match_control());
    /// assert!(Command::Step { ticks: 16 }.is_match_control());
    /// // Training bots are privileged too, because they add and remove
    /// // chassis that no pilot owns.
    /// assert!(Command::SpawnBot { team: Team::Red, spin_rad_s: 1.0 }.is_match_control());
    /// assert!(Command::ClearBots.is_match_control());
    ///
    /// // Driving, aiming and firing are not.
    /// assert!(
    ///     !Command::Chassis {
    ///         chassis: 1,
    ///         command: ChassisCommand::default(),
    ///     }
    ///     .is_match_control()
    /// );
    /// assert!(!Command::Fire { shooter: 1 }.is_match_control());
    ///
    /// // The panel posts the same JSON the wire carries, and omitted fields
    /// // default: a chassis command without an aim points the gun forward.
    /// let command: Command = serde_json::from_str(
    ///     r#"{"Chassis":{"chassis":1,"command":{"forward_m_s":1.0,"left_m_s":0.0,"yaw_rate_rad_s":0.0}}}"#,
    /// )
    /// .unwrap();
    /// assert_eq!(
    ///     command,
    ///     Command::Chassis {
    ///         chassis: 1,
    ///         command: ChassisCommand {
    ///             forward_m_s: 1.0,
    ///             ..Default::default()
    ///         },
    ///     }
    /// );
    /// ```
    pub fn is_match_control(&self) -> bool {
        matches!(
            self,
            Command::Referee(_)
                | Command::Pause { .. }
                | Command::Step { .. }
                | Command::SpawnBot { .. }
                | Command::RemoveBot { .. }
                | Command::ClearBots
        )
    }
}

/// Everything a client sends, in the transport's framed payloads.
///
/// A session is `Hello` first, then any number of `Command`s, `Ping`s and
/// `TimeProbe`s. `Ping` is the confirmation barrier: the host applies the
/// commands that arrived before it, then returns the resulting snapshot and
/// only afterwards the Pong, so the pair stays in application order on
/// whatever lanes carry them. `TimeProbe` measures the clock without waiting
/// for any command.
///
/// ```
/// use rm_simulator_server::protocol::{ClientMessage, Command, PROTOCOL_VERSION, Role};
///
/// // A client opens with Hello and then sends commands and a barrier.
/// let hello = ClientMessage::Hello {
///     protocol: PROTOCOL_VERSION,
///     password: String::new(),
///     name: "pilot".into(),
///     team: None,
///     role: Role::Pilot,
///     robot: Default::default(),
/// };
/// let start = ClientMessage::Command(Command::Referee(
///     rm_simulator_world::RefereeCommand::StartMatch,
/// ));
/// let ping = ClientMessage::Ping { nonce: 7 };
///
/// // Each control message is one JSON document in its own framed payload.
/// for message in [&hello, &start, &ping] {
///     let bytes = serde_json::to_vec(message).unwrap();
///     assert!(!bytes.contains(&b'\n'));
///     assert_eq!(
///         &serde_json::from_slice::<ClientMessage>(&bytes).unwrap(),
///         message
///     );
/// }
///
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
// Keep small fixed-size control records inline in bounded transport queues.
#[allow(clippy::large_enum_variant)]
pub enum ClientMessage {
    /// Clock sample independent of command confirmations.
    ///
    /// The answer is a [`ServerMessage::TimeSample`], which carries the host's
    /// simulation time and paused flag. It is not ordered against commands and
    /// confirms none of them, so timing probes never interfere with the
    /// `Ping` barrier.
    TimeProbe {
        /// Echoed back unchanged so the client can match the round trip. Any
        /// value is legal and the host does not interpret it.
        nonce: u64,
    },
    /// The opening message. The host answers [`ServerMessage::Welcome`] or
    /// closes the connection.
    ///
    /// A host checks `protocol` and `password` before admitting the client, so
    /// nothing later in the session is read from a peer that failed either.
    Hello {
        /// Protocol version the client speaks, which must equal
        /// [`PROTOCOL_VERSION`] exactly.
        protocol: u32,
        /// Shared password, empty when the host set none.
        password: String,
        /// Display name shown in the roster and notices.
        name: String,
        /// Preferred team; without one the host balances the teams. The
        /// referee has none.
        team: Option<Team>,
        /// Requested role. A host grants the referee only to the host's own
        /// operator, so this is a request rather than an assignment.
        role: Role,
        /// The robot a pilot asks to drive, which fixes its chassis preset
        /// and gun caliber. Ignored for a spectator or the referee.
        robot: Robot,
    },
    /// One action on the field, addressed to a chassis or to the match.
    Command(Command),
    /// After preceding commands, return their resulting Snapshot then Pong.
    Ping {
        /// Echoed back in [`ServerMessage::Pong`]. A client treats a Pong as
        /// covering every command it sent with a smaller nonce.
        nonce: u64,
    },
}

/// The chassis a client drives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChassisAssignment {
    /// Field-assigned chassis id. Ids are never reused, so a stale command can
    /// only fail, never hit a later robot.
    pub id: u32,
    /// Its configuration, for the client's visuals and camera.
    pub config: ChassisConfig,
    /// The robot it is, which the caliber in the Welcome's weapon follows.
    pub robot: Robot,
}
/// The host's answer to [`ClientMessage::Hello`].
///
/// It fixes the seat for the rest of the session: the role, the team and the
/// chassis id the client may command. A client keeps them, so a later
/// re-welcome is a new session rather than a role change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Welcome {
    /// How to build the shared prediction geometry, or `None` when this host
    /// offers no client-side prediction.
    pub prediction_scene: Option<crate::prediction::PredictionScene>,
    /// Protocol version the host speaks, for the client's compatibility check.
    pub protocol: u32,
    /// Identity of this connection in the host roster, never reused.
    pub client_id: u32,
    /// The referee has no team.
    pub team: Option<Team>,
    /// Seat granted to this client, which may differ from the one requested.
    pub role: Role,
    /// Absent unless a pilot, or when the host offers no chassis.
    pub chassis: Option<ChassisAssignment>,
    /// Host starting settings for this pilot, with the caliber of its robot;
    /// a spectator sees the host's 17 mm defaults.
    pub weapon: WeaponConfig,
    /// Host caps, independent of the starting settings.
    pub weapon_limits: WeaponLimits,
}
/// One connected client, as listed in the roster.
///
/// A host sends the whole roster whenever it changes, so a client replaces its
/// copy rather than merging entries. `client_id` is the connection; `chassis`
/// is the robot, and only a pilot has one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerInfo {
    /// Identity of this connection in the host roster, never reused.
    pub client_id: u32,
    /// Display name the client gave in its Hello.
    pub name: String,
    /// The referee has no team.
    pub team: Option<Team>,
    /// Seat this client holds.
    pub role: Role,
    /// The chassis this player drives; only a pilot has one.
    pub chassis: Option<u32>,
    /// The robot that chassis is; `Some` exactly when `chassis` is.
    pub robot: Option<Robot>,
}

/// The host's verdict on one deduplicated shot, keyed by shooter and shot id.
///
/// It is also the deduplication key: the host remembers the first result for a
/// `(shooter, shot_id)` pair and repeats it to any retry, so a shot cannot
/// spawn twice under loss.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShotResult {
    /// Simulation time the projectile launched, in nanoseconds, or `None` when
    /// the shot was refused.
    pub executed_time_ns: Option<u64>,
    /// Actual initial speed in m/s after sampling, or `None` on rejection.
    pub launch_speed_m_s: Option<f64>,
    /// Chassis that fired.
    pub shooter: u32,
    /// Client-assigned shot identity this result answers.
    pub shot_id: u64,
    /// Projectile id on success, or the reason the shot was refused.
    pub result: Result<u64, String>,
}

/// Everything a host sends, in the transport's framed payloads.
///
/// The reader keeps only the newest snapshot and roster and coalesces the rest
/// into ordered queues, so nothing here is a revision chain. A
/// [`ServerMessage::Notice`] or [`ServerMessage::Rejected`] is display text and
/// never carries a rule.
///
/// ```
/// use rm_simulator_server::protocol::{PlayerInfo, Role, ServerMessage};
///
/// // A host can send several kinds on one connection.
/// let roster = ServerMessage::Roster(vec![PlayerInfo {
///     client_id: 1,
///     name: "pilot".into(),
///     team: None,
///     role: Role::Pilot,
///     chassis: None,
///     robot: None,
/// }]);
/// let rejected = ServerMessage::Rejected {
///     reason: "that chassis is not yours".into(),
/// };
/// let pong = ServerMessage::Pong { nonce: 7 };
///
/// let bytes = serde_json::to_vec(&roster).unwrap();
/// assert!(matches!(
///     serde_json::from_slice::<ServerMessage>(&bytes).unwrap(),
///     ServerMessage::Roster(players) if players.len() == 1
/// ));
/// // A rejection is a plain reason string, not a rule outcome.
/// let text = serde_json::to_string(&rejected).unwrap();
/// assert!(text.contains("that chassis is not yours"));
/// // A Pong echoes the Ping nonce so the client can retire its commands.
/// assert!(matches!(pong, ServerMessage::Pong { nonce: 7 }));
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Host downstream application queues sampled independently of input execution.
    DeliveryStats(crate::pacing::QueueStats),
    /// Ordered authoritative contact feedback. Native reliable delivery retries
    /// this event independently of replaceable world snapshots.
    Hit {
        /// Input epoch at detection; events cannot cross pause/reset boundaries.
        epoch: u64,
        /// Monotonic event identity for this host connection, never reused.
        event_id: u64,
        /// Scored contact, including projectile, shooter, target and simulation time.
        hit: rm_simulator_world::ArmorHit,
    },
    /// Local diagnostics for the sender's connection. Missing native
    /// measurements stay missing rather than being reported as zero.
    Telemetry(crate::network_stats::HostTelemetry),
    /// Admission receipt only. No projectile or ammunition change is confirmed.
    ShotScheduled {
        /// Chassis that will fire.
        shooter: u32,
        /// Client-assigned shot identity.
        shot_id: u64,
    },
    /// The host's verdict on one deduplicated shot.
    ShotResult(ShotResult),
    /// The answer to a [`ClientMessage::TimeProbe`].
    TimeSample {
        /// The probe's nonce, echoed back.
        nonce: u64,
        /// Host simulation time at the sample, in nanoseconds.
        time_ns: u64,
        /// Whether the world was paused at the sample.
        paused: bool,
    },
    /// The answer to a [`ClientMessage::Hello`]. Boxed because it is large
    /// relative to the other variants.
    Welcome(Box<Welcome>),
    /// One owner chassis configuration on the reliable control lane. An owner
    /// anchor references it by [`crate::owner_stream::ConfigRevision`] rather
    /// than repeating the configuration, so it travels once per configuration
    /// change and only after the peer acknowledges it may an anchor name it.
    /// Boxed because a configuration is large relative to the other variants.
    OwnerConfig(Box<crate::owner_stream::OwnerConfig>),
    /// One authoritative world state. Boxed because it dominates the other
    /// variants and is replaced rather than queued.
    Snapshot(Box<SimulationState>),
    /// A command was not applied.
    Rejected {
        /// Why the host refused the command. Not a rule outcome, so a client
        /// may show it without changing its simulation.
        reason: String,
    },
    /// The answer to a [`ClientMessage::Ping`], sent after the snapshot that
    /// covers the commands which preceded the Ping.
    Pong {
        /// The Ping's nonce, echoed back.
        nonce: u64,
    },
    /// Human-readable news (another player joined or left).
    Notice(String),
    /// Everyone connected, sent whenever it changes.
    Roster(Vec<PlayerInfo>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_simulator_world::{Caliber, Field, FieldConfig, RefereeConfig};

    #[test]
    fn snapshots_survive_the_wire() {
        let config = FieldConfig {
            referee: Some(RefereeConfig::alternating(1, 2)),
            ..FieldConfig::default()
        };
        let mut field = Field::new(&config).unwrap();
        field.referee_command(RefereeCommand::StartMatch).unwrap();
        field.step(5_500).unwrap();
        field
            .fire(
                Pose::at([0.0, 0.0, 1.0]),
                Shot::at_limit(Caliber::Mm17),
                None,
            )
            .unwrap();
        field.step(200).unwrap();
        let state = SimulationState {
            bots: vec![],
            input_epoch: 0,
            shot_results: Vec::new(),
            snapshot_id: 0,
            paused: false,
            field: field.snapshot(),
        };
        let bytes = serde_json::to_vec(&ServerMessage::Snapshot(Box::new(state.clone()))).unwrap();
        let back = serde_json::from_slice::<ServerMessage>(&bytes).unwrap();
        assert_eq!(back, ServerMessage::Snapshot(Box::new(state)));
        // The panel's shorthand for commands is the same JSON.
        let command: Command =
            serde_json::from_str(r#"{"Referee":{"ActivateRune":{"team":"Red"}}}"#).unwrap();
        assert_eq!(
            command,
            Command::Referee(RefereeCommand::ActivateRune { team: Team::Red })
        );
        let command: Command = serde_json::from_str(r#"{"Referee":"StartMatch"}"#).unwrap();
        assert_eq!(command, Command::Referee(RefereeCommand::StartMatch));
        assert!(command.is_match_control());
        assert!(Command::Pause { paused: true }.is_match_control());
        assert!(!fire_command().is_match_control());
        // A chassis command without an aim points the gun forward.
        let command: Command = serde_json::from_str(
            r#"{"Chassis":{"chassis":1,"command":{"forward_m_s":1.0,"left_m_s":0.0,"yaw_rate_rad_s":0.0}}}"#,
        )
        .unwrap();
        assert_eq!(
            command,
            Command::Chassis {
                chassis: 1,
                command: rm_simulator_world::ChassisCommand {
                    forward_m_s: 1.0,
                    ..Default::default()
                }
            }
        );
    }
    fn fire_command() -> Command {
        Command::SpawnProjectile {
            muzzle: Pose::default(),
            shot: Shot::at_limit(Caliber::Mm17),
        }
    }
}

#[cfg(test)]
mod spread_tests {
    use super::*;
    #[test]
    fn spread_is_repeatable_bounded_and_gaussian_is_more_concentrated() {
        let pose = Pose::at([1., 2., 3.]);
        let mut means = Vec::new();
        for distribution in [SpreadDistribution::Uniform, SpreadDistribution::Gaussian] {
            let spread = BulletSpread {
                angle_rad: 0.2,
                distribution,
                seed: 17,
            };
            let mut squared = 0.;
            let mut transverse = [0.; 2];
            for tick in 0..10_000 {
                let sampled = spread.apply(pose, 4, tick * 1_000_000);
                assert_eq!(sampled, spread.apply(pose, 4, tick * 1_000_000));
                assert_eq!(sampled.translation_m, pose.translation_m);
                let direction = crate::math::rotate(sampled.rotation_wxyz, [1., 0., 0.]);
                let angle = direction[0].clamp(-1., 1.).acos();
                assert!(angle <= 0.2 + 1e-12);
                assert!((direction.iter().map(|v| v * v).sum::<f64>() - 1.).abs() < 1e-12);
                squared += angle * angle;
                transverse[0] += direction[1];
                transverse[1] += direction[2];
            }
            assert!(transverse.iter().all(|v| (v / 10_000.).abs() < 0.003));
            means.push(squared / 10_000.);
            assert_ne!(spread.apply(pose, 4, 0), spread.apply(pose, 5, 0));
            assert_ne!(
                spread.apply(pose, 4, 0),
                BulletSpread { seed: 18, ..spread }.apply(pose, 4, 0)
            );
        }
        assert!(means[1] < means[0] * 0.5);
        assert_eq!(BulletSpread::default().apply(pose, 1, 2), pose);
    }
    #[test]
    fn speed_variation_is_normal_repeatable_bounded_and_respects_caps() {
        let mut weapon = WeaponConfig::default();
        weapon.shot.speed_m_s = 25.;
        weapon.speed_variation_m_s = 0.;
        assert_eq!(weapon.sample_shot(1, 2, 30.), weapon.shot);
        weapon.speed_variation_m_s = 1.;
        let mut sum = 0.;
        let mut squared = 0.;
        for time in 0..10_000 {
            let shot = weapon.sample_shot(1, time, 30.);
            assert_eq!(shot, weapon.sample_shot(1, time, 30.));
            assert!((24.0..=26.0).contains(&shot.speed_m_s));
            let offset = shot.speed_m_s - 25.;
            sum += offset;
            squared += offset * offset;
        }
        assert!((sum / 10_000.).abs() < 0.015);
        assert!((0.10..0.12).contains(&(squared / 10_000.)));
        for nominal in [30., 0.1, 1e-9] {
            weapon.shot.speed_m_s = nominal;
            for time in 0..1000 {
                let speed = weapon.sample_shot(1, time, nominal).speed_m_s;
                assert!(speed > 0. && speed <= nominal);
                assert!((speed - nominal).abs() <= 1.);
            }
        }
        for value in [-0.1, 1.01, f64::NAN, f64::INFINITY] {
            weapon.speed_variation_m_s = value;
            assert!(weapon.validate().is_err());
        }
    }

    #[test]
    fn invalid_settings_are_rejected_and_legacy_defaults_are_exact() {
        for angle_rad in [f64::NAN, f64::INFINITY, -0.1, 2.] {
            assert!(
                BulletSpread {
                    angle_rad,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
        let host = WeaponConfig::default();
        let mut requested = host;
        requested.shot.speed_m_s = 31.;
        assert!(
            WeaponLimits::default()
                .admit(host.shot.caliber, requested)
                .is_err()
        );
        requested = host;
        requested.interval_ns = 1;
        assert!(
            WeaponLimits::default()
                .admit(host.shot.caliber, requested)
                .is_err()
        );
        requested = host;
        requested.shot.caliber = rm_simulator_world::Caliber::Mm42;
        assert!(
            WeaponLimits::default()
                .admit(host.shot.caliber, requested)
                .is_err()
        );
        let host = WeaponConfig {
            spread: BulletSpread::default(),
            speed_variation_m_s: 0.,
            ..host
        };
        let mut legacy = serde_json::to_value(host).unwrap();
        legacy.as_object_mut().unwrap().remove("spread");
        legacy
            .as_object_mut()
            .unwrap()
            .remove("speed_variation_m_s");
        assert_eq!(
            serde_json::from_value::<WeaponConfig>(legacy).unwrap(),
            host
        );
    }
}
