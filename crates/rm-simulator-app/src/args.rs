// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Command-line arguments.
use bevy::prelude::Resource;
use clap::Parser;
use rm_simulator_server::host_args::HostArgs;
use rm_simulator_server::protocol::{Robot, Role, WeaponConfig};
use rm_simulator_world::{Caliber, Shot, Team};

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

/// A performance type named on the command line (Tables 5-12 to 5-14).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum PerformanceChoice {
    /// Long-range attack-focused Hero.
    LongRange,
    /// Melee-focused Hero.
    Melee,
    /// HP-focused chassis with a cooling-focused launcher.
    HpCooling,
    /// HP-focused chassis with a burst-focused launcher.
    HpBurst,
    /// Power-focused chassis with a cooling-focused launcher.
    PowerCooling,
    /// Power-focused chassis with a burst-focused launcher.
    PowerBurst,
}
impl PerformanceChoice {
    /// The rules' performance value for this choice.
    pub fn performance(self) -> rm_simulator_world::gameplay::Performance {
        use rm_simulator_world::gameplay::{
            HeroType, InfantryChassis, InfantryLauncher, Performance,
        };
        let infantry = |chassis, launcher| Performance::Infantry { chassis, launcher };
        match self {
            Self::LongRange => Performance::Hero(HeroType::LongRangeFocused),
            Self::Melee => Performance::Hero(HeroType::MeleeFocused),
            Self::HpCooling => {
                infantry(InfantryChassis::HpFocused, InfantryLauncher::CoolingFocused)
            }
            Self::HpBurst => infantry(InfantryChassis::HpFocused, InfantryLauncher::BurstFocused),
            Self::PowerCooling => infantry(
                InfantryChassis::PowerFocused,
                InfantryLauncher::CoolingFocused,
            ),
            Self::PowerBurst => infantry(
                InfantryChassis::PowerFocused,
                InfantryLauncher::BurstFocused,
            ),
        }
    }
}

/// One parsed launch. The title screen keeps the instance as `BaseArgs` and
/// copies it into each `JoinRequest`, so a menu choice starts the same match the
/// equivalent command line would.
///
/// The options a host shares with the headless binary live in the flattened
/// [`HostArgs`] field, so the two cannot disagree about a field, rune, weapon or
/// chassis default. Everything else here is app-only: how to reach a host, what
/// this client is, and how the window and overlays present the match.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "rm-simulator",
    about = "First-person RoboMaster field simulator"
)]
pub struct Args {
    #[command(flatten)]
    pub host: HostArgs,
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
    /// Section 5.4.2 performance type to request after joining: `long-range`
    /// or `melee` for a Hero, `hp-cooling`, `hp-burst`, `power-cooling` or
    /// `power-burst` for an infantry. Unset keeps the rulebook default; a host
    /// refuses a type for the other robot class or during a running round.
    #[arg(long, value_enum)]
    pub performance: Option<PerformanceChoice>,
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
    /// Free-flying camera without a chassis; on a remote host, spectate.
    #[arg(long)]
    pub fly: bool,
    /// Join as the referee: no chassis, the free camera, and the match keys a
    /// remote host grants to nobody else.
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
    /// host decides the field, the rune and who drives the chassis. A remote host
    /// also decides every shared host option below, so naming one with
    /// `--connect` is refused rather than silently ignored. `--robot` is the
    /// exception: it names the pilot, not the host, and travels with `Hello`.
    #[arg(
        long,
        conflicts_with_all = [
            "outpost_speed_rad_s",
            "muzzle_speed_m_s",
            "fire_rate_hz",
            "max_fire_rate_hz",
            "max_muzzle_speed_m_s",
            "muzzle_speed_variation_m_s",
            "spread_deg",
            "spread_distribution",
            "spread_seed",
            "no_projectile_retirement",
            "big_rune",
            "no_rune",
            "no_field_collision",
            "no_referee",
            "listen",
            "http",
            "start_paused",
        ]
    )]
    pub connect: Option<String>,
    /// Persistent local network diagnostics overlay.
    #[arg(long, value_enum, default_value = "off")]
    pub network_stats: crate::network_hud::NetworkStatsMode,
    /// Team whose rune `F` activates (and requested from a remote host; the
    /// referee spends `F` for this team but joins on neither).
    #[arg(long, value_enum, default_value_t = TeamArg::Red)]
    pub team: TeamArg,
    /// Player name shown to other clients.
    #[arg(long, default_value = "pilot")]
    pub name: String,
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

impl Args {
    /// Whether the command line already names a match to enter. Bare launches
    /// show the title screen; automation (a screenshot, the console) and any
    /// explicit connect, host or practice request go straight in.
    pub fn auto_join(&self) -> bool {
        self.play
            || self.connect.is_some()
            || self.host.listen.is_some()
            || self.screenshot.is_some()
            || self.console.is_some()
    }
    /// CLI-relative paths are based on the launch working directory, including
    /// when Bevy would otherwise choose the executable's directory.
    pub fn resolve_asset_path(&mut self) -> std::io::Result<()> {
        self.host.cad_assets = std::path::absolute(&self.host.cad_assets)?;
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
    /// The round this player fires: the Hero's `--hero-muzzle-speed-m-s`
    /// (12 m/s by default) for 42 mm, else `--muzzle-speed-m-s` (25 m/s).
    pub fn shot(&self) -> Shot {
        self.weapon().shot
    }
    /// The minimum gap between this player's shots in ns: the Hero's
    /// `--hero-fire-rate-hz` for 42 mm, else `--fire-rate-hz`, clamped to
    /// 0.1..=1000 Hz.
    pub fn fire_interval_ns(&self) -> u64 {
        self.weapon().interval_ns
    }
    /// The weapon configuration the host starts this player with: the Hero's
    /// own settings for 42 mm, else the shared 17 mm settings.
    pub fn weapon(&self) -> WeaponConfig {
        match self.caliber() {
            Caliber::Mm42 => self.host.hero_weapon(),
            Caliber::Mm17 => self.host.weapon(),
        }
    }
    /// The host caps for this player's caliber.
    pub fn weapon_limits(&self) -> rm_simulator_server::protocol::WeaponLimits {
        match self.caliber() {
            Caliber::Mm42 => self.host.hero_weapon_limits(),
            Caliber::Mm17 => self.host.weapon_limits(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use rm_simulator_server::protocol::WeaponConfig;
    use rm_simulator_world::{Caliber, Shot};

    #[test]
    fn asset_paths_are_absolute_before_manifest_and_bevy_loading() {
        let mut args = Args::try_parse_from(["rm-simulator", "--cad-assets", "field"]).unwrap();
        args.resolve_asset_path().unwrap();
        assert_eq!(
            args.host.cad_assets,
            std::env::current_dir().unwrap().join("field")
        );
        let resolved = args.host.cad_assets.clone();
        args.resolve_asset_path().unwrap();
        assert_eq!(args.host.cad_assets, resolved);
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
    fn a_performance_choice_names_the_rules_type() {
        use rm_simulator_world::gameplay::{HeroType, Performance, RobotKind};
        assert_eq!(Args::parse_from(["rm-simulator"]).performance, None);
        let hero = Args::parse_from(["rm-simulator", "--robot", "hero", "--performance", "melee"]);
        assert_eq!(
            hero.performance.unwrap().performance(),
            Performance::Hero(HeroType::MeleeFocused)
        );
        let infantry = Args::parse_from(["rm-simulator", "--performance", "hp-cooling"]);
        assert_eq!(
            Some(infantry.performance.unwrap().performance()),
            Performance::default_for(RobotKind::Infantry)
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
        // The robot is the pilot's, so it travels to a remote host that decides
        // everything else.
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
            args.host.shot(),
            Shot {
                caliber: Caliber::Mm17,
                speed_m_s: 25.0
            }
        );
        assert_eq!(args.host.fire_interval_ns(), 50_000_000);
        assert_eq!(args.host.weapon(), WeaponConfig::default());
        assert_eq!(
            args.host.weapon_limits(),
            rm_simulator_server::protocol::WeaponLimits::default()
        );
        let args = Args::parse_from([
            "rm-simulator",
            "--robot",
            "hero",
            "--hero-muzzle-speed-m-s",
            "10",
            "--hero-fire-rate-hz",
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
        assert_eq!(args.weapon().shot.caliber, Caliber::Mm42);
        assert_eq!(args.weapon().interval_ns, 500_000_000);
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
        assert!(args.connect.is_none() && args.host.listen.is_none() && args.host.http.is_none());
        assert_eq!(args.team, TeamArg::Red);
        assert_eq!(args.rune_flash_hz, DEFAULT_RUNE_FLASH_HZ);
        assert_eq!(args.rune_flashes, DEFAULT_RUNE_FLASHES);
        assert_eq!(args.hit_flash_ms, DEFAULT_HIT_FLASH_MS);
        let hosting =
            Args::try_parse_from(["rm-simulator", "--listen", "--http", "--team", "blue"]).unwrap();
        assert_eq!(
            hosting.host.listen.as_deref(),
            Some(format!("0.0.0.0:{}", rm_simulator_server::protocol::DEFAULT_PORT).as_str())
        );
        assert_eq!(
            hosting.host.http.as_deref(),
            Some(
                format!(
                    "127.0.0.1:{}",
                    rm_simulator_server::protocol::DEFAULT_HTTP_PORT
                )
                .as_str()
            )
        );
        assert_eq!(Team::from(hosting.team), Team::Blue);
        let hosting = Args::try_parse_from(["rm-simulator", "--listen", "127.0.0.1:9000"]).unwrap();
        assert_eq!(hosting.host.listen.as_deref(), Some("127.0.0.1:9000"));
        let client =
            Args::try_parse_from(["rm-simulator", "--connect", "host:7700", "--fly"]).unwrap();
        assert_eq!(client.connect.as_deref(), Some("host:7700"));
        assert!(Args::try_parse_from(["rm-simulator", "--connect", "h:1", "--listen"]).is_err());
        assert!(Args::try_parse_from(["rm-simulator", "--connect", "h:1", "--big-rune"]).is_err());
        assert!(
            Args::try_parse_from(["rm-simulator", "--connect", "h:1", "--no-referee"]).is_err()
        );
        // Every host-owned option is refused rather than silently ignored:
        // the remote host decides the rune motion too.
        assert!(
            Args::try_parse_from([
                "rm-simulator",
                "--connect",
                "h:1",
                "--outpost-speed-rad-s",
                "0.4"
            ])
            .is_err()
        );
        assert!(
            Args::try_parse_from(["rm-simulator", "--connect", "h:1", "--start-paused"]).is_err()
        );
    }
}
