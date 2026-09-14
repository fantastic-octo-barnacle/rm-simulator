// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Headless host: loads the CAD field, runs the simulation on the host
//! clock, serves players over GameNetworkingSockets UDP and the referee over HTTP.
use clap::Parser;
use rm_simulator_server::layout::{self, LayoutOptions};
use rm_simulator_server::protocol::{DEFAULT_HTTP_PORT, DEFAULT_PORT, WeaponConfig};
use rm_simulator_server::{
    cad_assets,
    http::HttpServer,
    net::Server,
    simulation::{BuildProgress, Simulation},
};
use rm_simulator_world::{
    Caliber, ChassisConfig, RuneKind, Shot, outpost, projectile::ProjectilePolicy,
};

#[derive(Parser, Debug)]
#[command(
    name = "rm-simulator-server",
    about = "Headless RoboMaster field host with a referee panel"
)]
struct Args {
    /// Chassis prototype offered to pilots.
    #[arg(long, value_parser = ["infantry", "hero"], default_value = "infantry")]
    robot: String,
    /// Projectile caliber offered to every pilot.
    #[arg(long, default_value = "17", default_value_if("robot", "hero", "42"), value_parser = parse_caliber)]
    projectile_mm: u32,
    /// Starting muzzle speed; defaults to 25 m/s.
    #[arg(long)]
    muzzle_speed_m_s: Option<f64>,
    /// Shared physics rate in Hz: 1000 (the default), 500, 250 or 128. It sets
    /// the tick length this host integrates at, 128 Hz meaning exactly
    /// 7,812,500 ns, and any other value is refused. Every client must predict
    /// at the same rate, which the handshake enforces.
    #[arg(long, default_value_t = 1000, value_parser = parse_physics_rate_hz)]
    physics_rate_hz: u32,
    /// Starting shots per second per chassis.
    #[arg(long, default_value_t = 20.0)]
    fire_rate_hz: f64,
    /// Host maximum firing rate in Hz, independent of the starting rate.
    #[arg(long, default_value_t = 30.0)]
    max_fire_rate_hz: f64,
    /// Host maximum actual launch speed in m/s.
    #[arg(long, default_value_t = 30.0)]
    max_muzzle_speed_m_s: f64,
    /// Maximum Gaussian muzzle-speed deviation in m/s, from 0 to 1 (three sigma).
    #[arg(long, default_value_t = 0.3)]
    muzzle_speed_variation_m_s: f64,
    /// Maximum bullet deviation in degrees; Gaussian uses this as three sigma.
    #[arg(long, default_value_t = 0.3)]
    spread_deg: f64,
    /// Bullet distribution inside the spread cone.
    #[arg(long, value_enum, default_value = "gaussian")]
    spread_distribution: rm_simulator_server::protocol::SpreadDistribution,
    /// Repeatable bullet spread seed.
    #[arg(long, default_value_t = 0)]
    spread_seed: u64,
    /// Directory holding the extracted RMUC2026 CAD (`manifest.json`, `*.glb`, `equipment/`).
    #[arg(long, default_value_os_t = default_cad_assets())]
    cad_assets: std::path::PathBuf,
    /// Address for player connections.
    #[arg(long, default_value_t = format!("0.0.0.0:{DEFAULT_PORT}"))]
    listen: String,
    /// Gameplay transport; GNS uses UDP and works without Steam.
    #[arg(long, value_enum, default_value = "gns")]
    transport: rm_simulator_server::net::Transport,
    /// Address for the referee panel and JSON API; `none` disables it.
    #[arg(long, default_value_t = format!("127.0.0.1:{DEFAULT_HTTP_PORT}"))]
    http: String,
    /// Use the Big Rune sinusoidal motion and two-target groups instead of the Small Rune.
    #[arg(long)]
    big_rune: bool,
    /// Omit the rune rules; the CAD rune stays static.
    #[arg(long, conflicts_with = "big_rune")]
    no_rune: bool,
    #[arg(long, default_value_t = outpost::DEFAULT_SPEED_RAD_S)]
    outpost_speed_rad_s: f64,
    /// Skip the field CAD collision proxies; only a flat floor at height zero and armor collide.
    #[arg(long)]
    no_field_collision: bool,
    /// Offer no chassis; pilots get none and cannot fire.
    #[arg(long)]
    no_chassis: bool,
    /// Run without a referee: the runes stay in their training policy.
    #[arg(long)]
    no_referee: bool,
    /// Keep spent projectiles until the four-second flight limit instead of
    /// retiring a ball that has rested slowly on scenery.
    #[arg(long)]
    no_projectile_retirement: bool,
    /// Open with the world clock paused.
    #[arg(long)]
    start_paused: bool,
}

fn default_cad_assets() -> std::path::PathBuf {
    cad_assets::default_cad_assets()
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

fn parse_caliber(text: &str) -> Result<u32, String> {
    match text.trim() {
        "17" => Ok(17),
        "42" => Ok(42),
        other => Err(format!("unsupported caliber `{other}`; use 17 or 42")),
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cad = cad_assets::load(&args.cad_assets)?;
    let options = LayoutOptions {
        rune: (!args.no_rune).then_some(if args.big_rune {
            RuneKind::Big
        } else {
            RuneKind::Small
        }),
        outpost_speed_rad_s: args.outpost_speed_rad_s,
        terrain: !args.no_field_collision,
        referee: !args.no_referee,
        physics_rate_hz: args.physics_rate_hz,
        projectile_policy: if args.no_projectile_retirement {
            ProjectilePolicy::default().without_retirement()
        } else {
            ProjectilePolicy::default()
        },
    };
    let simulation = Simulation::from_cad(
        &cad,
        &options,
        (!args.no_chassis).then(|| {
            if args.robot == "hero" {
                ChassisConfig::hero()
            } else {
                ChassisConfig::default()
            }
        }),
        args.start_paused,
        |stage| {
            if let BuildProgress::TerrainReady(description) = stage {
                println!("{description}");
            }
        },
    )?
    .with_weapon(weapon(&args))
    .map_err(anyhow::Error::msg)?
    .with_weapon_limits(rm_simulator_server::protocol::WeaponLimits {
        max_speed_m_s: args.max_muzzle_speed_m_s,
        min_interval_ns: (1e9 / args.max_fire_rate_hz.clamp(0.1, 1000.0)).ceil() as u64,
    })
    .map_err(anyhow::Error::msg)?;
    let server = match args.transport {
        rm_simulator_server::net::Transport::Gns => Server::bind_udp(&args.listen, simulation),
        rm_simulator_server::net::Transport::Tcp => Server::bind(&args.listen, simulation),
    }?;
    println!("players: {:?} at {}", args.transport, server.local_addr());
    let _http = if args.http != "none" {
        let http = HttpServer::bind(&args.http, server.handle())?;
        println!("referee panel: http://{}/", http.local_addr());
        Some(http)
    } else {
        None
    };
    println!(
        "field: {} rune(s), {} outpost(s), chassis {}, referee {}, floor catch {} m",
        options.rune.map_or(0, |_| 2),
        cad.outpost.placements.len(),
        if args.no_chassis { "no" } else { "per player" },
        if options.referee { "yes" } else { "no" },
        if options.terrain {
            layout::CATCH_FLOOR_M
        } else {
            0.0
        }
    );
    server.run_clock()
}

fn weapon(args: &Args) -> WeaponConfig {
    let caliber = if args.projectile_mm == 42 {
        Caliber::Mm42
    } else {
        Caliber::Mm17
    };
    WeaponConfig {
        shot: Shot {
            caliber,
            speed_m_s: args.muzzle_speed_m_s.unwrap_or(25.0),
        },
        interval_ns: (1e9 / args.fire_rate_hz.clamp(0.1, 1000.0)).ceil() as u64,
        speed_variation_m_s: args.muzzle_speed_variation_m_s,
        spread: rm_simulator_server::protocol::BulletSpread {
            angle_rad: args.spread_deg.to_radians(),
            distribution: args.spread_distribution,
            seed: args.spread_seed,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_have_network_defaults() {
        let args = Args::try_parse_from(["rm-simulator-server"]).unwrap();
        assert_eq!(args.listen, format!("0.0.0.0:{DEFAULT_PORT}"));
        assert_eq!(args.http, format!("127.0.0.1:{DEFAULT_HTTP_PORT}"));
        assert!(!args.no_referee);
        let args = Args::try_parse_from(["rm-simulator-server", "--http", "none", "--no-chassis"])
            .unwrap();
        assert_eq!(args.http, "none");
        assert!(args.no_chassis);
        assert!(Args::try_parse_from(["rm-simulator-server", "--big-rune", "--no-rune"]).is_err());
        assert_eq!(args.projectile_mm, 17);
        assert_eq!(weapon(&args), WeaponConfig::default());
        let hero = Args::try_parse_from(["rm-simulator-server", "--robot", "hero"]).unwrap();
        assert_eq!(hero.projectile_mm, 42);
        let custom = Args::try_parse_from([
            "rm-simulator-server",
            "--projectile-mm",
            "42",
            "--muzzle-speed-m-s",
            "30",
            "--fire-rate-hz",
            "5",
        ])
        .unwrap();
        assert_eq!(weapon(&custom).shot.speed_m_s, 30.0);
        assert_eq!(weapon(&custom).interval_ns, 200_000_000);
        assert!(Args::try_parse_from(["rm-simulator-server", "--projectile-mm", "19"]).is_err());
    }
}
