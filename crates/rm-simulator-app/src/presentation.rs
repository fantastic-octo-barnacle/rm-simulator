// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Window presentation and the headless camera target.
use crate::{args::WindowMode, controls::PlayerCamera};
use bevy::prelude::*;

/// The image the gameplay camera draws into when no window exists. It holds
/// `None` in the windowed modes, where the camera renders to the window.
#[derive(Resource, Default)]
pub struct CaptureTarget(pub Option<Handle<Image>>);

/// Keeps winit in continuous mode, and in headless mode drives the app from a
/// schedule runner and gives the camera an offscreen render target, since no
/// window or event loop exists to pace frames.
pub struct PresentationPlugin;
impl Plugin for PresentationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CaptureTarget>()
            .insert_resource(bevy::winit::WinitSettings::continuous());
        if *app.world().resource::<WindowMode>() == WindowMode::Headless {
            app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
                std::time::Duration::from_secs_f64(1.0 / 60.0),
            ))
            .add_systems(Startup, setup_headless.after(crate::scene::setup_camera));
        }
    }
}

fn setup_headless(
    mut commands: Commands,
    camera: Single<Entity, With<PlayerCamera>>,
    window: Single<&Window, With<bevy::window::PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut target: ResMut<CaptureTarget>,
) {
    let image = images.add(Image::new_target_texture(
        window.physical_width(),
        window.physical_height(),
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    commands
        .entity(*camera)
        .insert(bevy::camera::RenderTarget::Image(image.clone().into()));
    target.0 = Some(image);
}
