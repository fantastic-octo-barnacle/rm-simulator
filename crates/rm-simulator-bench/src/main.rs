// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Standalone, fixed-camera field rendering benchmark.
mod assets;
mod config;
mod gpu;
mod report;
mod scene;
use bevy::{
    prelude::*,
    render::{
        RenderPlugin,
        settings::{Backends, RenderCreation, WgpuFeatures, WgpuSettings},
    },
    window::{PresentMode, WindowResolution},
};
use clap::Parser;
use config::{Args, Backend, Case};

/// Load the case, create the output directory, and run one Bevy app until the report finishes.
fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut case: Case = if let Some(path) = &args.case {
        serde_json::from_slice(&std::fs::read(path)?)?
    } else {
        Case::default()
    };
    // Benchmarks are uncapped by default, including when selecting a game preset.
    case.graphics.overrides.vsync.get_or_insert(false);
    case.validate()?;
    let loaded = assets::load(&args.cad_assets)?;
    let settings = backend_settings(&args)?;
    std::fs::create_dir(&args.output)?;
    let mut app = App::new();
    let plugins = DefaultPlugins
        .set(AssetPlugin {
            file_path: args
                .cad_assets
                .canonicalize()?
                .to_string_lossy()
                .into_owned(),
            ..default()
        })
        .set(RenderPlugin {
            render_creation: RenderCreation::Automatic(Box::new(settings)),
            ..default()
        })
        .set(WindowPlugin {
            primary_window: (!args.headless).then(|| Window {
                title: format!("Render benchmark: {}", case.name),
                resolution: WindowResolution::new(case.resolution[0], case.resolution[1])
                    .with_scale_factor_override(1.),
                present_mode: if case.graphics.resolved().vsync {
                    PresentMode::AutoVsync
                } else {
                    PresentMode::AutoNoVsync
                },
                resizable: false,
                ..default()
            }),
            exit_condition: bevy::window::ExitCondition::DontExit,
            ..default()
        });
    if args.headless {
        app.add_plugins(plugins.disable::<bevy::winit::WinitPlugin>())
            .add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
                std::time::Duration::ZERO,
            ));
    } else {
        app.add_plugins(plugins)
            .insert_resource(bevy::winit::WinitSettings::continuous());
    }
    app.add_plugins(gpu::GpuTimerPlugin);
    app.init_resource::<gpu::Capture>();
    let graphics = case.graphics.resolved();
    app.add_plugins(rm_simulator_render::cad::CadScenePlugin {
        instances: loaded.instances.clone(),
        rendering: rm_simulator_render::RenderingConfig {
            profile: rm_simulator_render::RenderingProfile::Field,
            exposure_ev100: graphics.exposure_ev100,
            emissive_strength: graphics.emissive_strength,
            bloom_intensity: graphics.bloom_intensity,
        },
    })
    .init_resource::<rm_simulator_render::sync::SceneInput>()
    .init_resource::<scene::Target>()
    .init_resource::<scene::GeometryState>()
    .insert_resource(loaded)
    .insert_resource(case)
    .insert_resource(args)
    .add_plugins(report::ReportPlugin)
    .add_systems(Startup, scene::setup)
    .add_systems(Update, (scene::configure_lights, scene::prepare_geometry));
    let result = app.run();
    if !result.is_success() {
        anyhow::bail!("benchmark failed; see output directory and log");
    }
    Ok(())
}
/// Build the wgpu settings for one run.
///
/// GPU timestamp measurement requires a native Vulkan or DX12 machine, so macOS is rejected
/// unless `--cpu-only` is set; that flag also disables the timestamp and pipeline-statistics features.
fn backend_settings(args: &Args) -> anyhow::Result<WgpuSettings> {
    anyhow::ensure!(
        !cfg!(target_os = "macos") || args.cpu_only,
        "GPU timestamp benchmarking is unavailable on macOS; use --cpu-only or run on a native Vulkan/DX12 machine"
    );
    let backend = args.backend;
    let mut settings = WgpuSettings::default();
    if backend != Backend::Auto {
        settings.backends = Some(match backend {
            Backend::Vulkan => Backends::VULKAN,
            Backend::Metal => Backends::METAL,
            Backend::Dx12 => Backends::DX12,
            Backend::Gl => Backends::GL,
            Backend::Auto => unreachable!(),
        });
    }
    if args.cpu_only {
        settings.disabled_features = Some(
            WgpuFeatures::TIMESTAMP_QUERY
                | WgpuFeatures::TIMESTAMP_QUERY_INSIDE_ENCODERS
                | WgpuFeatures::TIMESTAMP_QUERY_INSIDE_PASSES
                | WgpuFeatures::PIPELINE_STATISTICS_QUERY,
        );
    }
    Ok(settings)
}
