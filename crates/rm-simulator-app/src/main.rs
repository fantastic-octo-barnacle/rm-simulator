// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! First-person RoboMaster field simulator: window, driven omni chassis or fly
//! camera, tick pacing, gun and HUD. The world advances only through explicit
//! ticks derived from frame time, either in a simulation hosted in this
//! process (optionally shared over TCP) or on a remote host.
//!
//! `args` parses the command line, `session` owns the local or remote
//! simulation and sends every input as a protocol command, `controls` reads
//! the player's keys and mouse, `scene` turns world snapshots into the
//! renderer's scene state, `hud` writes the overlay, `debug` draws the
//! physics wireframe, `screenshot` serves `--screenshot` and `frames` holds
//! the FLU-to-Bevy conversions.
mod args;
mod auto_aim;
mod bindings;
mod console;
mod controls;
mod debug;
mod debug_panel;
mod frames;
mod graphics;
mod hit_feedback;
mod hud;
mod interpolation;
mod loading;
mod minimap;
#[cfg(test)]
mod net_harness;
mod network_hud;
mod prediction;
mod preferences;
mod presentation;
mod projectile_prediction;
mod scene;
mod screenshot;
mod session;
#[cfg(feature = "steam")]
mod steam_support;
mod title;

use bevy::prelude::*;
use clap::Parser;
use rm_simulator_render::{
    RenderingConfig,
    cad::CadScenePlugin,
    chassis::ChassisVisualsPlugin,
    projectile::ProjectileVisualsPlugin,
    sync::{SceneSyncPlugin, SceneSyncSet},
};

use args::Args;
use controls::{Drive, Gun, drive_camera, drive_chassis, fire_gun, fly_camera, mouse_capture};
use debug::collision_view;
use hud::update_hud;
use scene::{Appearance, publish_scene};
use screenshot::{ScreenshotRequest, take_screenshot};
use session::advance_world;

fn main() -> AppExit {
    #[cfg(feature = "steam")]
    let steam_runtime = steam_support::SteamRuntime::initialize();
    let mut args = Args::parse();
    if let Err(error) = args.resolve_asset_path() {
        eprintln!("resolving --cad-assets: {error}");
        return AppExit::error();
    }
    let mut app = App::new();
    #[cfg(feature = "steam")]
    steam_support::install(&mut app, steam_runtime);
    let mut plugins = DefaultPlugins
        .set(WindowPlugin {
            primary_window: Some(Window {
                title: "rm-simulator".into(),
                focused: args.window_mode == args::WindowMode::Normal,
                ..default()
            }),
            ..default()
        })
        .set(AssetPlugin {
            file_path: args.cad_assets.to_string_lossy().into_owned(),
            ..default()
        });
    if args.window_mode == args::WindowMode::Headless {
        plugins = plugins.disable::<bevy::winit::WinitPlugin>();
    }
    app.insert_resource(args.window_mode);
    app.add_plugins(plugins)
        .add_plugins(presentation::PresentationPlugin)
        .add_plugins(screenshot::ScreenshotReadinessPlugin)
        .add_plugins(CadScenePlugin {
            instances: Vec::new(),
            rendering: RenderingConfig::field(),
        })
        // Venue-like haze instead of a black void beyond the field edge.
        .insert_resource(ClearColor(Color::srgb(0.13, 0.15, 0.19)))
        .add_plugins(SceneSyncPlugin)
        .insert_resource(Appearance(RenderingConfig::field()))
        .add_plugins(ProjectileVisualsPlugin {
            rendering: RenderingConfig::field(),
        })
        .add_plugins(ChassisVisualsPlugin {
            rendering: RenderingConfig::field(),
            armor: scene::armor_optics(),
        })
        .insert_resource(Gun::new(args.shot(), args.fire_interval_ns()))
        .insert_resource(hud::HudState {
            debug: args.debug_panel,
            network_stats: args.network_stats,
            ..default()
        })
        .init_resource::<auto_aim::AutoAim>()
        .add_plugins(preferences::PreferencesPlugin)
        .add_plugins(graphics::GraphicsPlugin)
        .add_plugins(hud::HudPlugin)
        .add_plugins(debug_panel::DebugPanelPlugin)
        .insert_resource(debug_panel::DebugOptions {
            stats: args.render_stats,
        })
        .insert_resource(bevy::pbr::wireframe::WireframeConfig {
            global: args.wireframe,
            ..default()
        })
        .add_systems(
            Update,
            (hud::panel_input, hud::menu_input)
                .chain()
                .before(fire_gun)
                .before(drive_chassis)
                .before(fly_camera)
                .before(mouse_capture)
                .before(update_hud)
                .before(hud::update_map)
                .before(hud::update_panel_rows),
        )
        .add_plugins(loading::LoadingPlugin)
        .add_plugins(title::TitlePlugin)
        .add_systems(
            Update,
            hud::sync_menus.after(mouse_capture).after(hud::menu_input),
        )
        .add_systems(Startup, scene::setup_camera)
        // Poll, sample mouse/assist, submit controls and shots, then exchange
        // prediction and place the physical camera. A capturing click cannot fire.
        .add_systems(
            Update,
            (
                advance_world,
                controls::sample_drive_aim.run_if(resource_exists::<Drive>),
                fly_camera.run_if(not(resource_exists::<Drive>)),
                auto_aim::update,
                drive_chassis.run_if(resource_exists::<Drive>),
                session::advance_shots,
                fire_gun,
                session::predict_frame,
                drive_camera.run_if(resource_exists::<Drive>),
                publish_scene,
            )
                .chain()
                .run_if(resource_exists::<loading::Ready>)
                .before(SceneSyncSet)
                .before(mouse_capture)
                .before(update_hud)
                .before(hud::update_map),
        )
        .add_systems(
            Update,
            (
                mouse_capture,
                collision_view,
                debug::dynamic_colliders
                    .after(collision_view)
                    .after(publish_scene)
                    .before(SceneSyncSet),
                update_hud,
                hud::update_map,
                network_hud::update,
                hud::update_panel_rows,
            )
                .run_if(resource_exists::<loading::Ready>),
        )
        .add_systems(
            Update,
            take_screenshot
                .after(collision_view)
                .after(SceneSyncSet)
                .run_if(resource_exists::<ScreenshotRequest>)
                .run_if(resource_exists::<loading::Ready>),
        );
    if let Some(path) = &args.screenshot {
        app.insert_resource(ScreenshotRequest::new(path.clone()));
    }
    if let Some(address) = args.console {
        match console::Console::bind(address) {
            Ok(console) => {
                app.insert_resource(console)
                    .add_plugins(console::ConsolePlugin);
            }
            Err(error) => {
                eprintln!("cannot start console: {error}");
                return AppExit::error();
            }
        }
    }
    if args.auto_join() {
        app.insert_resource(loading::JoinRequest(args.clone()));
    } else {
        app.insert_resource(title::TitleScreen::default());
    }
    app.insert_resource(title::BaseArgs(args));
    app.run()
}
