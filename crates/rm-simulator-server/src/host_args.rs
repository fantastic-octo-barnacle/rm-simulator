// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! The options a host accepts, and the values they derive.
//!
//! The headless `rm-simulator-server` binary and the app's local-host path offer
//! the same field, rune, weapon and chassis options, so they are declared once
//! here instead of once per binary. A binary that embeds [`HostArgs`] only has to
//! decide how it presents them; the derivations ([`HostArgs::layout_options`],
//! [`HostArgs::weapon`], [`HostArgs::weapon_limits`],
//! [`HostArgs::chassis_config`]) are shared, so a host cannot disagree with the
//! app about what its own command line meant.
//!
//! The two binaries interpret two options differently on purpose, and the
//! accessors below hold that difference in one place: an absent `--listen`
//! means "run in process" to the app and "bind the default address" to the
//! headless host, and an absent `--http` means "serve no panel" to the app and
//! "serve the default loopback panel" to the headless host.
use crate::layout::LayoutOptions;
use crate::protocol::{BulletSpread, DEFAULT_HTTP_PORT, DEFAULT_PORT, WeaponConfig, WeaponLimits};
use clap::Args;
use rm_simulator_world::{Caliber, RuneKind, Shot, outpost, projectile::ProjectilePolicy};

/// Field, rune, weapon and chassis options shared by every host.
///
/// Both binaries that embed this struct expose every field as a `--flag` with
/// the same name, help text and default, so a command line written for one is
/// accepted by the other.
#[derive(Args, Debug, Clone)]
pub struct HostArgs {
    /// Directory holding the extracted RMUC2026 CAD (`manifest.json`, `*.glb`, `equipment/`).
    #[arg(long, default_value_os_t = crate::cad_assets::default_cad_assets())]
    pub cad_assets: std::path::PathBuf,
    /// Use the Big Rune sinusoidal motion and two-target groups instead of the Small Rune.
    #[arg(long)]
    pub big_rune: bool,
    /// Omit the rune rules; the CAD rune stays static.
    #[arg(long, conflicts_with = "big_rune")]
    pub no_rune: bool,
    /// Keep spent projectiles until the four-second flight limit instead of
    /// retiring a ball that has rested slowly on scenery.
    #[arg(long)]
    pub no_projectile_retirement: bool,
    /// Outpost rotor speed in rad/s; defaults to the section 5.5.1 value of 0.8π.
    #[arg(long, default_value_t = outpost::DEFAULT_SPEED_RAD_S)]
    pub outpost_speed_rad_s: f64,
    /// Open with the world clock paused.
    #[arg(long)]
    pub start_paused: bool,
    /// Starting muzzle speed; defaults to 25 m/s. Each pilot's caliber follows
    /// the robot it chose: 42 mm for the Hero, 17 mm for an infantry.
    #[arg(long)]
    pub muzzle_speed_m_s: Option<f64>,
    /// Starting shots per second while the trigger is held.
    #[arg(long, default_value_t = 20.0)]
    pub fire_rate_hz: f64,
    /// Host maximum firing rate in Hz, independent of the starting rate.
    #[arg(long, default_value_t = 30.0)]
    pub max_fire_rate_hz: f64,
    /// Host maximum actual launch speed in m/s.
    #[arg(long, default_value_t = 30.0)]
    pub max_muzzle_speed_m_s: f64,
    /// Maximum Gaussian muzzle-speed deviation in m/s, from 0 to 1 (three sigma).
    #[arg(long, default_value_t = 0.3)]
    pub muzzle_speed_variation_m_s: f64,
    /// Maximum bullet deviation in degrees; Gaussian uses this as three sigma.
    #[arg(long, default_value_t = 0.3)]
    pub spread_deg: f64,
    /// Bullet distribution inside the spread cone.
    #[arg(long, value_enum, default_value = "gaussian")]
    pub spread_distribution: crate::protocol::SpreadDistribution,
    /// Repeatable bullet spread seed.
    #[arg(long, default_value_t = 0)]
    pub spread_seed: u64,
    /// Skip the field CAD collision proxies; only a flat floor at height zero and armor collide.
    #[arg(long)]
    pub no_field_collision: bool,
    /// Run without a referee: the runes keep their training policy and the
    /// referee match keys do nothing.
    #[arg(long)]
    pub no_referee: bool,
    /// Host the local simulation for other players on this address
    /// (default 0.0.0.0:7700 when given without a value). Absent means the app
    /// simulates in process and the headless host binds the default address.
    #[arg(long, num_args = 0..=1, default_missing_value = "0.0.0.0:7700")]
    pub listen: Option<String>,
    /// Serve the referee panel and JSON API on this address
    /// (default 127.0.0.1:7780 when given without a value); the headless host
    /// serves no panel for `none`. Absent means the app serves none.
    #[arg(long, num_args = 0..=1, default_missing_value = "127.0.0.1:7780")]
    pub http: Option<String>,
}

impl HostArgs {
    /// The host's default caliber: 17 mm. Every pilot's own robot overrides it
    /// with its caliber, so this is only what a chassis with no named robot
    /// (a training bot) fires.
    pub fn caliber(&self) -> Caliber {
        Caliber::Mm17
    }

    /// The round this host's pilots start with. The default starting speed is
    /// 25 m/s for either caliber; `--muzzle-speed-m-s` overrides it.
    pub fn shot(&self) -> Shot {
        let caliber = self.caliber();
        Shot {
            caliber,
            speed_m_s: self.muzzle_speed_m_s.unwrap_or(25.0),
        }
    }

    /// The minimum gap between shots in ns, from `--fire-rate-hz`, which is
    /// clamped to 0.1..=1000 Hz.
    pub fn fire_interval_ns(&self) -> u64 {
        (1e9 / self.fire_rate_hz.clamp(0.1, 1000.0)).ceil() as u64
    }

    /// Host caps independent of a pilot's starting weapon.
    pub fn weapon_limits(&self) -> WeaponLimits {
        WeaponLimits {
            max_speed_m_s: self.max_muzzle_speed_m_s,
            min_interval_ns: (1e9 / self.max_fire_rate_hz.clamp(0.1, 1000.0)).ceil() as u64,
        }
    }

    /// The weapon configuration the host enforces for each pilot: the caliber,
    /// the launch speed, the firing cadence and the spread.
    pub fn weapon(&self) -> WeaponConfig {
        WeaponConfig {
            shot: self.shot(),
            interval_ns: self.fire_interval_ns(),
            speed_variation_m_s: self.muzzle_speed_variation_m_s,
            spread: BulletSpread {
                angle_rad: self.spread_deg.to_radians(),
                distribution: self.spread_distribution,
                seed: self.spread_seed,
            },
        }
    }

    /// The field, rune, referee and projectile layout these options describe.
    pub fn layout_options(&self) -> LayoutOptions {
        LayoutOptions {
            rune: (!self.no_rune).then_some(if self.big_rune {
                RuneKind::Big
            } else {
                RuneKind::Small
            }),
            outpost_speed_rad_s: self.outpost_speed_rad_s,
            terrain: !self.no_field_collision,
            referee: !self.no_referee,
            projectile_policy: if self.no_projectile_retirement {
                ProjectilePolicy::default().without_retirement()
            } else {
                ProjectilePolicy::default()
            },
        }
    }

    /// The gameplay address a headless host binds. `--listen` wins when given,
    /// bare or with a value; otherwise the default gameplay port.
    pub fn listen_address(&self) -> String {
        self.listen
            .clone()
            .unwrap_or_else(|| format!("0.0.0.0:{DEFAULT_PORT}"))
    }

    /// The referee-panel address a headless host binds. `--http` wins when
    /// given, bare or with a value; otherwise the default loopback port.
    pub fn http_address(&self) -> String {
        self.http
            .clone()
            .unwrap_or_else(|| format!("127.0.0.1:{DEFAULT_HTTP_PORT}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The wrapper below stands in for either binary: it flattens the shared
    /// options exactly as they do, so the parse outcomes under test are the ones
    /// both binaries see.
    #[derive(Parser, Debug)]
    #[command(name = "host-under-test")]
    struct Host {
        #[command(flatten)]
        host: HostArgs,
    }

    fn parse(arguments: &[&str]) -> Result<Host, clap::Error> {
        let mut argv = vec!["host-under-test"];
        argv.extend_from_slice(arguments);
        Host::try_parse_from(argv)
    }

    #[test]
    fn defaults_match_both_binaries() {
        let host = parse(&[]).unwrap().host;
        assert_eq!(
            host.spread_distribution,
            crate::protocol::SpreadDistribution::Gaussian
        );
        assert!(host.listen.is_none() && host.http.is_none());
        assert_eq!(host.weapon(), WeaponConfig::default());
        assert_eq!(host.weapon_limits(), WeaponLimits::default());
        assert_eq!(host.caliber(), Caliber::Mm17);
        let options = host.layout_options();
        // A rune is on by default; `--no-rune` is what removes it.
        assert_eq!(options.rune, Some(RuneKind::Small));
        assert_eq!(options.outpost_speed_rad_s, outpost::DEFAULT_SPEED_RAD_S);
        assert!(options.terrain && options.referee);
    }

    #[test]
    fn a_bare_address_flag_takes_the_documented_default() {
        let host = parse(&["--listen", "--http"]).unwrap().host;
        assert_eq!(
            host.listen.as_deref(),
            Some(format!("0.0.0.0:{DEFAULT_PORT}").as_str())
        );
        assert_eq!(
            host.http.as_deref(),
            Some(format!("127.0.0.1:{DEFAULT_HTTP_PORT}").as_str())
        );
        assert_eq!(host.listen_address(), format!("0.0.0.0:{DEFAULT_PORT}"));
        assert_eq!(
            host.http_address(),
            format!("127.0.0.1:{DEFAULT_HTTP_PORT}")
        );
    }

    #[test]
    fn an_absent_address_is_resolved_per_binary() {
        let host = parse(&[]).unwrap().host;
        // The headless host always binds something; the app reads the `None`.
        assert!(host.listen.is_none());
        assert_eq!(host.listen_address(), format!("0.0.0.0:{DEFAULT_PORT}"));
        assert_eq!(
            host.http_address(),
            format!("127.0.0.1:{DEFAULT_HTTP_PORT}")
        );
        let explicit = parse(&["--listen", "127.0.0.1:9000", "--http", "none"])
            .unwrap()
            .host;
        assert_eq!(explicit.listen_address(), "127.0.0.1:9000");
        assert_eq!(explicit.http_address(), "none");
    }

    #[test]
    fn the_host_offers_no_robot_or_caliber_option() {
        // A pilot's robot is its own choice and travels in `Hello`; a host's
        // caliber is the 17 mm default every pilot's robot overrides.
        let host = parse(&[]).unwrap().host;
        assert_eq!(host.caliber(), Caliber::Mm17);
        assert!(parse(&["--robot", "hero"]).is_err());
        assert!(parse(&["--projectile-mm", "42"]).is_err());
    }

    #[test]
    fn starting_weapon_defaults_are_independent_of_the_caps() {
        let host = parse(&[]).unwrap().host;
        assert_eq!(
            host.shot(),
            Shot {
                caliber: Caliber::Mm17,
                speed_m_s: 25.0
            }
        );
        assert_eq!(host.fire_interval_ns(), 50_000_000);
        let tuned = parse(&["--muzzle-speed-m-s", "10", "--fire-rate-hz", "2"])
            .unwrap()
            .host;
        assert_eq!(tuned.shot().speed_m_s, 10.0);
        assert_eq!(tuned.fire_interval_ns(), 500_000_000);
    }

    #[test]
    fn rune_and_field_options_agree_between_binaries() {
        assert!(parse(&["--big-rune", "--no-rune"]).is_err());
        let host = parse(&["--no-rune", "--no-referee", "--no-field-collision"])
            .unwrap()
            .host;
        let options = host.layout_options();
        assert!(options.rune.is_none() && !options.referee && !options.terrain);
        let big = parse(&["--big-rune"]).unwrap().host.layout_options();
        assert_eq!(big.rune, Some(RuneKind::Big));
    }
}
