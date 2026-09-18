// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Headless host: loads the CAD field, runs the simulation on the host
//! clock, serves players over GameNetworkingSockets UDP and the referee over HTTP.
use clap::Parser;
use rm_simulator_server::host_args::HostArgs;
use rm_simulator_server::layout;
use rm_simulator_server::{
    cad_assets,
    http::HttpServer,
    net::Server,
    simulation::{BuildProgress, Simulation},
};
use rm_simulator_world::ChassisConfig;

/// The headless host's command line: the shared host options plus the one
/// switch only a dedicated host has. The app cannot offer `--no-chassis` because
/// it decides a chassis from each joining player's role instead.
#[derive(Parser, Debug)]
#[command(
    name = "rm-simulator-server",
    about = "Headless RoboMaster field host with a referee panel"
)]
struct Args {
    #[command(flatten)]
    host: HostArgs,
    /// Offer no chassis; pilots get none and cannot fire.
    #[arg(long)]
    no_chassis: bool,
    /// Name this host advertises to LAN lobby discovery.
    #[arg(long, default_value = "RM Simulator server")]
    lobby_name: String,
    /// Do not answer LAN lobby discovery.
    #[arg(long)]
    no_lobby: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cad = cad_assets::load(&args.host.cad_assets)?;
    let options = args.host.layout_options();
    let simulation = Simulation::from_cad(
        &cad,
        &options,
        (!args.no_chassis).then(ChassisConfig::default),
        args.host.start_paused,
        |stage| {
            if let BuildProgress::TerrainReady(description) = stage {
                println!("{description}");
            }
        },
    )?
    .with_weapon(args.host.weapon())
    .map_err(anyhow::Error::msg)?
    .with_weapon_limits(args.host.weapon_limits())
    .map_err(anyhow::Error::msg)?
    .with_hero_weapon(args.host.hero_weapon())
    .map_err(anyhow::Error::msg)?
    .with_hero_weapon_limits(args.host.hero_weapon_limits())
    .map_err(anyhow::Error::msg)?;
    let listen = args.host.listen_address();
    let server = Server::bind(&listen, simulation)?;
    println!("players: GNS UDP at {}", server.local_addr());
    // A LAN advertisement that cannot bind its port (another advertised host
    // on this computer) leaves the host reachable by address only.
    let _lobby = if args.no_lobby {
        None
    } else {
        match rm_simulator_server::lobby::Advertisement::start(
            rm_simulator_server::lobby::DEFAULT_HOST,
            &args.lobby_name,
            server.local_addr(),
            false,
            false,
            "",
        ) {
            Ok(advertisement) => {
                println!("lobby: \"{}\" on LAN discovery", args.lobby_name.trim());
                Some(advertisement)
            }
            Err(e) => {
                eprintln!("lobby: not advertised: {e}");
                None
            }
        }
    };
    let http_address = args.host.http_address();
    let _http = if http_address != "none" {
        let http = HttpServer::bind(&http_address, server.handle())?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use rm_simulator_server::protocol::{DEFAULT_HTTP_PORT, DEFAULT_PORT, WeaponConfig};

    #[test]
    fn arguments_have_network_defaults() {
        let args = Args::try_parse_from(["rm-simulator-server"]).unwrap();
        assert_eq!(
            args.host.listen_address(),
            format!("0.0.0.0:{DEFAULT_PORT}")
        );
        assert_eq!(
            args.host.http_address(),
            format!("127.0.0.1:{DEFAULT_HTTP_PORT}")
        );
        assert!(!args.host.no_referee);
        assert!(!args.no_lobby);
        assert_eq!(args.lobby_name, "RM Simulator server");
        let args = Args::try_parse_from(["rm-simulator-server", "--http", "none", "--no-chassis"])
            .unwrap();
        assert_eq!(args.host.http_address(), "none");
        assert!(args.no_chassis);
        assert!(Args::try_parse_from(["rm-simulator-server", "--big-rune", "--no-rune"]).is_err());
        assert_eq!(args.host.weapon(), WeaponConfig::default());
        assert!(Args::try_parse_from(["rm-simulator-server", "--robot", "hero"]).is_err());
        let custom = Args::try_parse_from([
            "rm-simulator-server",
            "--muzzle-speed-m-s",
            "30",
            "--fire-rate-hz",
            "5",
        ])
        .unwrap();
        assert_eq!(custom.host.weapon().shot.speed_m_s, 30.0);
        assert_eq!(custom.host.weapon().interval_ns, 200_000_000);
        assert!(Args::try_parse_from(["rm-simulator-server", "--projectile-mm", "42"]).is_err());
    }
}
