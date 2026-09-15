// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Command-line arguments.
use bevy::prelude::Resource;
use clap::Parser;
use rm_simulator_server::protocol::{Robot, Role, WeaponConfig};
use rm_simulator_world::{Caliber, Shot, Team, outpost as outpost_rules};

use crate::debug::CollisionView;

/// Struck light bars show grey for this long after a detected strike
/// (`--hit-flash-ms`). The rule manual gives no figure; this is short enough
/// to read as a blink at the outpost's fastest detection interval.
pub const DEFAULT_HIT_FLASH_MS: u64 = 50;
/// Blink rate of an Activated rune's arms (`--rune-flash-hz`). Not in the
/// rule text, which shows the activated rune fully lit (Figure 5-23); a
/// visual aid, off with 0.
pub const DEFAULT_RUNE_FLASH_HZ: f64 = 2.0;
/// How many times an activated rune blinks before its arms stay lit.
pub const DEFAULT_RUNE_FLASHES: u32 = 3;

/// One parsed launch. The title screen keeps the instance as `BaseArgs` and
/// copies it into each `JoinRequest`, so a menu choice starts the same match the
/// equivalent command line would.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "rm-simulator",
    about = "First-person RoboMaster field simulator"
)]
pub struct Args {
    /// Lobby directory as HOST:PORT.
    #[arg(long, default_value = rm_simulator_server::lobby::DEFAULT_HOST)]
    pub lobby_host: String,
    /// Advertise a named lobby while hosting.
    #[arg(long, requires = "listen")]
    pub lobby_name: Option<String>,
    /// List this lobby publicly; otherwise discover it on the LAN only.
    #[arg(long, requires = "lobby_name")]
    pub public_lobby: bool,
    /// Public game IP:PORT override; empty uses the directory-observed IP.
    #[arg(long, default_value = "")]
    pub advertise_address: String,
    /// Password required by a hosted match, or supplied when joining.
    #[arg(long, default_value = "")]
    pub password: String,

    /// Skip the title screen and start a local practice match at once.
    #[arg(long, conflicts_with_all = ["connect", "listen"])]
    pub play: bool,
    /// Disable local prediction and use host-driven chassis, aim and firing.
    #[arg(long)]
    pub no_prediction: bool,
    /// App automation console, JSON lines over TCP (localhost only).
    #[arg(long, num_args = 0..=1, default_missing_value = "127.0.0.1:7790")]
    pub console: Option<std::net::SocketAddr>,
    /// Window presentation; headless still renders on the GPU.
    #[arg(long, value_enum, default_value_t = WindowMode::Normal)]
    pub window_mode: WindowMode,

    /// Robot this pilot drives: the mecanum Hero fires 42 mm, the omni
    /// infantries fire 17 mm. Every host, local or remote, honours the pick.
    #[arg(long, value_enum, default_value_t = Robot::default())]
    pub robot: Robot,
    /// Directory holding the extracted RMUC2026 CAD (`manifest.json`, `*.glb`, `equipment/`).
    #[arg(long, default_value_os_t = default_cad_assets())]
    pub cad_assets: std::path::PathBuf,
    /// Use the Big Rune sinusoidal motion and two-target groups instead of the Small Rune.
    #[arg(long)]
    pub big_rune: bool,
    /// Omit the rune rules; the CAD rune stays static.
    #[arg(long, conflicts_with = "big_rune")]
    pub no_rune: bool,
    /// Keep spent projectiles until the four-second flight limit instead of
    /// retiring a ball that has rested slowly on scenery.
    #[arg(long, conflicts_with = "connect")]
    pub no_projectile_retirement: bool,
    /// Outpost rotor speed in rad/s; defaults to the section 5.5.1 value of 0.8π.
    #[arg(long, default_value_t = outpost_rules::DEFAULT_SPEED_RAD_S)]
    pub outpost_speed_rad_s: f64,
    /// Start position in world FLU metres, `x,y,z`. Driving, the chassis is set
    /// down on the highest ground below `z`. Defaults to the red side before the centre-line plateau.
    #[arg(long, value_parser = parse_vec3, allow_hyphen_values = true)]
    pub spawn: Option<[f64; 3]>,
    /// Start heading in degrees, counter-clockwise from FLU forward (+x);
    /// defaults to facing the field from your team's side.
    #[arg(long, allow_hyphen_values = true)]
    pub spawn_yaw_deg: Option<f32>,
    /// Start pitch in degrees, positive upward.
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    pub spawn_pitch_deg: f32,
    /// Open with the world clock paused.
    #[arg(long)]
    pub start_paused: bool,
    /// Save the window to this PNG once the scene has loaded and settled, then exit.
    #[arg(long)]
    pub screenshot: Option<std::path::PathBuf>,
    /// Open the developer panel at startup.
    #[arg(long)]
    pub debug_panel: bool,
    /// Show frame and render pass statistics at startup.
    #[arg(long)]
    pub render_stats: bool,
    /// Draw visual meshes with wireframe lines when supported by the GPU.
    #[arg(long)]
    pub wireframe: bool,
    /// Starting muzzle speed; defaults to 25 m/s.
    #[arg(long, conflicts_with = "connect")]
    pub muzzle_speed_m_s: Option<f64>,
    /// Shared physics rate in Hz: 1000 (the default), 500, 250 or 128. It sets
    /// the tick length the whole process integrates at, 128 Hz meaning exactly
    /// 7,812,500 ns, and any other value is refused. On a remote host it states
    /// the rate this client predicts at, and the host refuses a mismatch.
    #[arg(long, default_value_t = 1000, value_parser = parse_physics_rate_hz)]
    pub physics_rate_hz: u32,
    /// Shots per second while the trigger is held.
    #[arg(long, default_value_t = 20.0, conflicts_with = "connect")]
    pub fire_rate_hz: f64,
    /// Host maximum firing rate in Hz, independent of the starting rate.
    #[arg(long, default_value_t = 30.0, conflicts_with = "connect")]
    pub max_fire_rate_hz: f64,
    /// Host maximum actual launch speed in m/s.
    #[arg(long, default_value_t = 30.0, conflicts_with = "connect")]
    pub max_muzzle_speed_m_s: f64,
    /// Maximum Gaussian muzzle-speed deviation in m/s, from 0 to 1 (three sigma).
    #[arg(long, default_value_t = 0.3, conflicts_with = "connect")]
    pub muzzle_speed_variation_m_s: f64,
    /// Maximum bullet deviation in degrees; Gaussian uses this as three sigma.
    #[arg(long, default_value_t = 0.3, conflicts_with = "connect")]
    pub spread_deg: f64,
    /// Bullet distribution inside the spread cone.
    #[arg(
        long,
        value_enum,
        default_value = "gaussian",
        conflicts_with = "connect"
    )]
    pub spread_distribution: rm_simulator_server::protocol::SpreadDistribution,
    /// Repeatable bullet spread seed.
    #[arg(long, default_value_t = 0, conflicts_with = "connect")]
    pub spread_seed: u64,
    /// Skip the field CAD collision proxies; only a flat floor at height zero and armor collide.
    #[arg(long)]
    pub no_field_collision: bool,
    /// Free-flying camera without a chassis; on a remote host, spectate.
    #[arg(long)]
    pub fly: bool,
    /// Join as the referee: no chassis, the free camera, and the match
    /// keys (`P`, `N`, `M`, `F`), which a remote host grants to nobody else.
    #[arg(long, conflicts_with = "third_person")]
    pub referee: bool,
    /// Start with the physics geometry drawn as a green wireframe over the
    /// scenery (`overlay`) or on its own (`alone`); choose a view in the F3 panel.
    #[arg(long, value_enum, default_value_t = CollisionView::Hidden)]
    pub collision_view: CollisionView,
    /// Start driving in the third-person view (`V` toggles).
    #[arg(long, conflicts_with = "fly")]
    pub third_person: bool,
    /// Play on a remote host (`HOST:PORT`) instead of simulating locally; the
    /// host decides the field, the rune and who drives the chassis.
    #[arg(long, conflicts_with_all = ["listen", "http", "big_rune", "no_rune", "no_referee", "start_paused", "no_field_collision"])]
    pub connect: Option<String>,
    /// Gameplay transport; GNS is Valve's standalone UDP networking library.
    #[arg(long, value_enum, default_value = "gns")]
    pub transport: rm_simulator_server::net::Transport,
    /// Persistent local network diagnostics overlay.
    #[arg(long, value_enum, default_value = "off")]
    pub network_stats: crate::network_hud::NetworkStatsMode,
    /// Host the local simulation for other players on this address
    /// (default 0.0.0.0:7700 when given without a value).
    #[arg(long, num_args = 0..=1, default_missing_value = "0.0.0.0:7700")]
    pub listen: Option<String>,
    /// Serve the referee panel and JSON API on this address
    /// (default 127.0.0.1:7780 when given without a value).
    #[arg(long, num_args = 0..=1, default_missing_value = "127.0.0.1:7780")]
    pub http: Option<String>,
    /// Team whose rune `F` activates (and requested from a remote host; the
    /// referee spends `F` for this team but joins on neither).
    #[arg(long, value_enum, default_value_t = TeamArg::Red)]
    pub team: TeamArg,
    /// Player name shown to other clients.
    #[arg(long, default_value = "pilot")]
    pub name: String,
    /// Run the local simulation without a referee: the runes keep their
    /// training policy and `M`/`F` do nothing.
    #[arg(long)]
    pub no_referee: bool,
    /// Blink rate of an activated rune's arms; 0 keeps them lit.
    #[arg(long, default_value_t = DEFAULT_RUNE_FLASH_HZ)]
    pub rune_flash_hz: f64,
    /// Number of blinks after activation before the arms stay lit (0 blinks forever).
    #[arg(long, default_value_t = DEFAULT_RUNE_FLASHES)]
    pub rune_flashes: u32,
    /// How long a struck light bar shows grey.
    #[arg(long, default_value_t = DEFAULT_HIT_FLASH_MS)]
    pub hit_flash_ms: u64,
}
/// Whether the launch owns an OS window. All three modes render on the GPU and
/// process input; only the event loop and the camera's render target differ.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum, Resource)]
pub enum WindowMode {
    /// A visible window that takes focus on creation.
    #[default]
    Normal,
    /// A visible window that asks for no focus, so it can run beside another one.
    Unfocused,
    /// No OS window and no winit plugin; the gameplay camera renders to an image.
    Headless,
}

/// The team a pilot drives for, named on the command line. It is also the team
/// whose rune `F` activates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum TeamArg {
    /// Red, whose half is +x.
    Red,
    /// Blue, whose half is -x.
    Blue,
}
impl From<TeamArg> for Team {
    fn from(team: TeamArg) -> Team {
        match team {
            TeamArg::Red => Team::Red,
            TeamArg::Blue => Team::Blue,
        }
    }
}

/// Accept only a measured physics rate. 128 Hz must mean exactly 7,812,500 ns
/// per tick, so a rate that does not divide one second exactly is refused
/// rather than rounded into a clock nobody asked for.
fn parse_physics_rate_hz(text: &str) -> Result<u32, String> {
    let offered = || {
        rm_simulator_world::OFFERED_RATES_HZ
            .iter()
            .map(|(hz, _)| hz.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let hz: u32 = text
        .trim()
        .parse()
        .map_err(|_| format!("`{text}` is not a physics rate in Hz; use {}", offered()))?;
    if rm_simulator_world::tick_ns_for_hz(hz).is_some() {
        Ok(hz)
    } else {
        Err(format!(
            "unsupported physics rate `{hz}` Hz; use {}",
            offered()
        ))
    }
}

impl Args {
    /// Whether the command line already names a match to enter. Bare launches
    /// show the title screen; automation (a screenshot, the console) and any
    /// explicit connect, host or practice request go straight in.
    pub fn auto_join(&self) -> bool {
        self.play
            || self.connect.is_some()
            || self.listen.is_some()
            || self.screenshot.is_some()
            || self.console.is_some()
    }
    /// CLI-relative paths are based on the launch working directory, including
    /// when Bevy would otherwise choose the executable's directory.
    pub fn resolve_asset_path(&mut self) -> std::io::Result<()> {
        self.cad_assets = std::path::absolute(&self.cad_assets)?;
        Ok(())
    }

    /// What this player is to a host: the referee, a spectator when flying,
    /// else a pilot.
    pub fn role(&self) -> Role {
        if self.referee {
            Role::Referee
        } else if self.fly {
            Role::Spectator
        } else {
            Role::Pilot
        }
    }
    /// The caliber the chosen robot fires: 42 mm for a Hero, else 17 mm.
    pub fn caliber(&self) -> Caliber {
        self.robot.caliber()
    }
    /// The round this player fires. The default starting speed is 25 m/s for
    /// either caliber; `--muzzle-speed-m-s` overrides it.
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
    /// Host caps independent of initial pilot settings.
    pub fn weapon_limits(&self) -> rm_simulator_server::protocol::WeaponLimits {
        rm_simulator_server::protocol::WeaponLimits {
            max_speed_m_s: self.max_muzzle_speed_m_s,
            min_interval_ns: (1e9 / self.max_fire_rate_hz.clamp(0.1, 1000.0)).ceil() as u64,
        }
    }
    /// The weapon configuration the host enforces for this player: the caliber,
    /// the launch speed and the firing cadence.
    pub fn weapon(&self) -> WeaponConfig {
        WeaponConfig {
            shot: self.shot(),
            interval_ns: self.fire_interval_ns(),
            speed_variation_m_s: self.muzzle_speed_variation_m_s,
            spread: rm_simulator_server::protocol::BulletSpread {
                angle_rad: self.spread_deg.to_radians(),
                distribution: self.spread_distribution,
                seed: self.spread_seed,
            },
        }
    }
}

fn parse_vec3(text: &str) -> Result<[f64; 3], String> {
    let parts: Vec<f64> = text
        .split(',')
        .map(|part| part.trim().parse::<f64>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    match parts[..] {
        [x, y, z] if parts.iter().all(|v| v.is_finite()) => Ok([x, y, z]),
        _ => Err("expected three finite numbers `x,y,z`".to_string()),
    }
}

/// The CAD package used when `--cad-assets` is absent: `field/` beside an
/// installed `bin/` directory, else the checkout's `local-assets/field`, then
/// its Git LFS `field/` package, else
/// `~/dev/RM/assets/rm2026-field`.
pub fn default_cad_assets() -> std::path::PathBuf {
    rm_simulator_server::cad_assets::default_cad_assets()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_paths_are_absolute_before_manifest_and_bevy_loading() {
        let mut args = Args::try_parse_from(["rm-simulator", "--cad-assets", "field"]).unwrap();
        args.resolve_asset_path().unwrap();
        assert_eq!(
            args.cad_assets,
            std::env::current_dir().unwrap().join("field")
        );
        let resolved = args.cad_assets.clone();
        args.resolve_asset_path().unwrap();
        assert_eq!(args.cad_assets, resolved);
    }

    #[test]
    fn prediction_defaults_allow_an_explicit_host_presentation_override() {
        let args = Args::try_parse_from(["rm-simulator"]).unwrap();
        assert!(!args.no_prediction);
        assert!(
            Args::try_parse_from(["rm-simulator", "--no-prediction"])
                .unwrap()
                .no_prediction
        );
    }

    #[test]
    fn automation_modes_and_optional_console_address_parse() {
        let args = Args::parse_from(["rm-simulator", "--console"]);
        assert_eq!(args.console.unwrap().to_string(), "127.0.0.1:7790");
        assert_eq!(args.window_mode, WindowMode::Normal);
        for (name, mode) in [
            ("normal", WindowMode::Normal),
            ("unfocused", WindowMode::Unfocused),
            ("headless", WindowMode::Headless),
        ] {
            let args = Args::parse_from([
                "rm-simulator",
                "--console",
                "127.0.0.1:0",
                "--window-mode",
                name,
            ]);
            assert_eq!(args.window_mode, mode);
            assert_eq!(args.console.unwrap().port(), 0);
        }
        assert!(Args::try_parse_from(["rm-simulator", "--window-mode", "hidden"]).is_err());
    }
    #[test]
    fn only_the_measured_physics_rates_parse() {
        assert_eq!(Args::parse_from(["rm-simulator"]).physics_rate_hz, 1000);
        for hz in [1000, 500, 250, 128] {
            let args = Args::parse_from(["rm-simulator", "--physics-rate-hz", &hz.to_string()]);
            assert_eq!(args.physics_rate_hz, hz);
            // 128 Hz must mean exactly 7,812,500 ns, not a rounded division.
            assert_eq!(
                rm_simulator_world::tick_ns_for_hz(args.physics_rate_hz).unwrap() * u64::from(hz),
                1_000_000_000
            );
        }
        for bad in ["0", "60", "333", "1001", "128.0", "many"] {
            let error = Args::try_parse_from(["rm-simulator", "--physics-rate-hz", bad])
                .expect_err(bad)
                .to_string();
            assert!(error.contains("1000, 500, 250, 128"), "{bad}: {error}");
        }
        // A remote client states its rate too, so the option survives --connect.
        assert!(
            Args::try_parse_from([
                "rm-simulator",
                "--connect",
                "localhost:7700",
                "--physics-rate-hz",
                "128",
            ])
            .is_ok()
        );
    }

    #[test]
    fn the_robot_fixes_the_caliber_and_travels_to_remote_hosts_too() {
        let args = Args::parse_from(["rm-simulator"]);
        assert_eq!(args.robot, Robot::Infantry3);
        assert_eq!(args.caliber(), Caliber::Mm17);
        let hero = Args::parse_from(["rm-simulator", "--robot", "hero"]);
        assert_eq!(hero.robot, Robot::Hero);
        assert_eq!(hero.caliber(), Caliber::Mm42);
        assert_eq!(hero.shot().caliber, Caliber::Mm42);
        assert_eq!(hero.weapon().shot.caliber, Caliber::Mm42);
        let four = Args::parse_from(["rm-simulator", "--robot", "infantry-4"]);
        assert_eq!(four.robot, Robot::Infantry4);
        assert_eq!(four.caliber(), Caliber::Mm17);
        assert_eq!(
            Args::parse_from(["rm-simulator", "--robot", "infantry"]).robot,
            Robot::Infantry3
        );
        let remote = Args::parse_from([
            "rm-simulator",
            "--connect",
            "localhost:7700",
            "--robot",
            "hero",
        ]);
        assert_eq!(remote.robot, Robot::Hero);
        assert!(Args::try_parse_from(["rm-simulator", "--projectile-mm", "42"]).is_err());
        assert!(Args::try_parse_from(["rm-simulator", "--robot", "sentry"]).is_err());
        for option in ["--muzzle-speed-m-s", "--fire-rate-hz"] {
            assert!(
                Args::try_parse_from(["rm-simulator", "--connect", "localhost:7700", option, "17"])
                    .is_err()
            );
        }
    }
    #[test]
    fn starting_weapon_defaults_are_independent_of_the_30_hz_and_30_m_s_caps() {
        let args = Args::parse_from(["rm-simulator"]);
        assert_eq!(
            args.shot(),
            Shot {
                caliber: Caliber::Mm17,
                speed_m_s: 25.0
            }
        );
        assert_eq!(args.fire_interval_ns(), 50_000_000);
        assert_eq!(args.weapon(), WeaponConfig::default());
        assert_eq!(
            args.weapon_limits(),
            rm_simulator_server::protocol::WeaponLimits::default()
        );
        let args = Args::parse_from([
            "rm-simulator",
            "--robot",
            "hero",
            "--muzzle-speed-m-s",
            "10",
            "--fire-rate-hz",
            "2",
        ]);
        assert_eq!(
            args.shot(),
            Shot {
                caliber: Caliber::Mm42,
                speed_m_s: 10.0
            }
        );
        assert_eq!(args.fire_interval_ns(), 500_000_000);
    }
    #[test]
    fn spawn_argument_parses_three_finite_numbers() {
        assert_eq!(parse_vec3("1, -2.5,3").unwrap(), [1.0, -2.5, 3.0]);
        assert!(parse_vec3("1,2").is_err());
        assert!(parse_vec3("1,2,nan").is_err());
        let args = Args::parse_from([
            "rm-simulator",
            "--spawn",
            "-1,2,3",
            "--spawn-yaw-deg",
            "-90",
        ]);
        assert_eq!(args.spawn, Some([-1.0, 2.0, 3.0]));
        assert_eq!(args.spawn_yaw_deg, Some(-90.0));
        assert!(Args::try_parse_from(["rm-simulator", "--fly", "--third-person"]).is_err());
        assert!(Args::try_parse_from(["rm-simulator", "--referee", "--third-person"]).is_err());
    }
    #[test]
    fn the_role_follows_the_referee_and_fly_flags() {
        assert_eq!(Args::parse_from(["rm-simulator"]).role(), Role::Pilot);
        assert_eq!(
            Args::parse_from(["rm-simulator", "--fly"]).role(),
            Role::Spectator
        );
        assert_eq!(
            Args::parse_from(["rm-simulator", "--referee"]).role(),
            Role::Referee
        );
        assert_eq!(
            Args::parse_from(["rm-simulator", "--referee", "--fly"]).role(),
            Role::Referee
        );
    }
    #[test]
    fn session_arguments_parse_and_exclude_each_other() {
        let args = Args::try_parse_from(["rm-simulator"]).unwrap();
        assert!(args.connect.is_none() && args.listen.is_none() && args.http.is_none());
        assert_eq!(args.team, TeamArg::Red);
        assert_eq!(args.rune_flash_hz, DEFAULT_RUNE_FLASH_HZ);
        assert_eq!(args.rune_flashes, DEFAULT_RUNE_FLASHES);
        assert_eq!(args.hit_flash_ms, DEFAULT_HIT_FLASH_MS);
        let host =
            Args::try_parse_from(["rm-simulator", "--listen", "--http", "--team", "blue"]).unwrap();
        assert_eq!(
            host.listen.as_deref(),
            Some(format!("0.0.0.0:{}", rm_simulator_server::protocol::DEFAULT_PORT).as_str())
        );
        assert_eq!(
            host.http.as_deref(),
            Some(
                format!(
                    "127.0.0.1:{}",
                    rm_simulator_server::protocol::DEFAULT_HTTP_PORT
                )
                .as_str()
            )
        );
        assert_eq!(Team::from(host.team), Team::Blue);
        let host = Args::try_parse_from(["rm-simulator", "--listen", "127.0.0.1:9000"]).unwrap();
        assert_eq!(host.listen.as_deref(), Some("127.0.0.1:9000"));
        let client =
            Args::try_parse_from(["rm-simulator", "--connect", "host:7700", "--fly"]).unwrap();
        assert_eq!(client.connect.as_deref(), Some("host:7700"));
        assert!(Args::try_parse_from(["rm-simulator", "--connect", "h:1", "--listen"]).is_err());
        assert!(Args::try_parse_from(["rm-simulator", "--connect", "h:1", "--big-rune"]).is_err());
        assert!(
            Args::try_parse_from(["rm-simulator", "--connect", "h:1", "--no-referee"]).is_err()
        );
    }
}
