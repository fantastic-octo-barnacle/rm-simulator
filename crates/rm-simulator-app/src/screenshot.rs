// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! `--screenshot`: capture the visible game window, save it, then exit.
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use rm_simulator_render::cad::CadSceneStatus;

use bevy::render::{Render, RenderApp, RenderSystems, render_resource::PipelineCache};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

/// Settled frames after GPU pipeline compilation and scene preparation finish.
const SCREENSHOT_WARMUP_FRAMES: u32 = 3;
const MAX_CAPTURE_ATTEMPTS: u32 = 3;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(120);

/// Shared between the render and main worlds; querying it never waits on the GPU.
#[derive(Resource, Clone)]
pub struct CaptureReadiness(Arc<AtomicUsize>);
impl CaptureReadiness {
    /// True when the render world's pipeline cache reports nothing still
    /// compiling.
    pub fn ready(&self) -> bool {
        self.0.load(Ordering::Acquire) == 0
    }
}
impl Default for CaptureReadiness {
    fn default() -> Self {
        Self(Arc::new(AtomicUsize::new(usize::MAX)))
    }
}
/// Adds [`CaptureReadiness`] to both worlds and refreshes it from the render
/// world's count of pipelines still compiling.
pub struct ScreenshotReadinessPlugin;
impl Plugin for ScreenshotReadinessPlugin {
    fn build(&self, app: &mut App) {
        let readiness = CaptureReadiness::default();
        app.insert_resource(readiness.clone());
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.insert_resource(readiness).add_systems(
                Render,
                report_capture_readiness.in_set(RenderSystems::Cleanup),
            );
        }
    }
}
fn report_capture_readiness(cache: Res<PipelineCache>, readiness: Res<CaptureReadiness>) {
    readiness
        .0
        .store(cache.waiting_pipelines().count(), Ordering::Release);
}

/// One pending PNG capture, requested by `--screenshot` or by the console.
/// [`take_screenshot`] records the outcome in `result` once the file is saved
/// or the attempt is abandoned.
#[derive(Resource)]
pub struct ScreenshotRequest {
    /// Destination PNG path. Its parent directory must already exist.
    pub path: std::path::PathBuf,
    /// Consecutive ready frames counted so far; reset to zero when readiness
    /// lapses.
    pub frames: u32,
    /// A capture entity has been spawned and its readback has not arrived yet.
    pub requested: bool,
    attempts: u32,
    capture: Option<Entity>,
    /// Saved or failed once the capture finished; `None` while one is pending.
    pub result: Option<Result<(), String>>,
    /// Exit the app with the capture's status once it finishes. Set by
    /// `--screenshot` and cleared by console captures, which stay open.
    pub exit_after: bool,
    started: Option<Instant>,
}
impl ScreenshotRequest {
    /// A pending request that saves to `path` and exits the app with the
    /// capture's status.
    pub fn new(path: std::path::PathBuf) -> Self {
        Self {
            path,
            frames: 0,
            requested: false,
            attempts: 0,
            capture: None,
            result: None,
            exit_after: true,
            started: None,
        }
    }
}

/// Requests one capture once everything it draws is ready: scenery loaded, no
/// collision capture pending, every command confirmed and no pipeline
/// compiling. The first frames after readiness are warmup, and a blank
/// readback is retried; any failure or timeout lands in `result`, and
/// `exit_after` decides whether the app then stops.
#[allow(clippy::too_many_arguments)]
pub fn take_screenshot(
    mut request: ResMut<ScreenshotRequest>,
    cad: Res<CadSceneStatus>,
    debug: Res<crate::debug::CollisionDebug>,
    session: Res<crate::session::Session>,
    readiness: Res<CaptureReadiness>,
    target: Option<Res<crate::presentation::CaptureTarget>>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    if request.result.is_some() {
        return;
    }
    if request.started.get_or_insert_with(Instant::now).elapsed() >= CAPTURE_TIMEOUT {
        error!("screenshot timed out waiting for a rendered frame");
        finish_capture(
            &mut request,
            Err("screenshot timed out waiting for a rendered frame".into()),
            &mut exit,
        );
        return;
    }
    if let Some(failure) = &cad.failed {
        error!("cannot take screenshot: {failure}");
        finish_capture(&mut request, Err(failure.clone()), &mut exit);
        return;
    }
    if let Some(failure) = debug.failure() {
        error!("cannot take screenshot: {failure}");
        finish_capture(&mut request, Err(failure.into()), &mut exit);
        return;
    }
    if !cad.ready()
        || debug.pending()
        || !session.commands_confirmed()
        || readiness.0.load(Ordering::Acquire) != 0
    {
        request.frames = 0;
        return;
    }
    request.frames += 1;
    if request.requested || request.frames < SCREENSHOT_WARMUP_FRAMES {
        return;
    }
    request.requested = true;
    request.attempts += 1;
    let path = request.path.clone();
    let capture = target
        .and_then(|t| t.0.clone())
        .map_or_else(Screenshot::primary_window, Screenshot::image);
    let entity = commands
        .spawn(capture)
        .observe(
            move |event: On<ScreenshotCaptured>,
                  request: Option<ResMut<ScreenshotRequest>>,
                  mut exit: MessageWriter<AppExit>| {
                let Some(mut request) = request else {
                    return;
                };
                if request.capture != Some(event.entity) || request.result.is_some() {
                    return;
                }
                // The simulator always draws a background and HUD. An all-black
                // readback is an unrendered capture, not a valid scene screenshot.
                if capture_is_blank(&event.image) {
                    if request.attempts < MAX_CAPTURE_ATTEMPTS {
                        warn!("blank screenshot readback; waiting for another rendered frame");
                        request.requested = false;
                        request.frames = 0;
                    } else {
                        error!("screenshot remained blank after {MAX_CAPTURE_ATTEMPTS} attempts");
                        finish_capture(
                            &mut request,
                            Err("screenshot remained blank after retries".into()),
                            &mut exit,
                        );
                    }
                    return;
                }
                let result = event
                    .image
                    .clone()
                    .try_into_dynamic()
                    .map_err(|e| format!("converting screenshot: {e}"))
                    .and_then(|image| {
                        image
                            .save(&path)
                            .map_err(|e| format!("saving {}: {e}", path.display()))
                    });
                finish_capture(&mut request, result, &mut exit);
            },
        )
        .id();
    request.capture = Some(entity);
}

fn finish_capture(
    request: &mut ScreenshotRequest,
    result: Result<(), String>,
    exit: &mut MessageWriter<AppExit>,
) {
    if let Err(error) = &result {
        error!("{error}");
    } else {
        info!("saved screenshot to {}", request.path.display());
    }
    if request.exit_after {
        exit.write(if result.is_ok() {
            AppExit::Success
        } else {
            AppExit::error()
        });
    }
    request.result = Some(result);
}

fn capture_is_blank(image: &Image) -> bool {
    image.clone().try_into_dynamic().map_or(true, |image| {
        image.to_rgb8().pixels().all(|pixel| pixel.0 == [0, 0, 0])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Session, test_session, wait_for_session};

    fn blank_image() -> Image {
        use bevy::asset::RenderAssetUsages;
        use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
        Image::new_fill(
            Extent3d {
                width: 2,
                height: 2,
                ..default()
            },
            TextureDimension::D2,
            &[0, 0, 0, 255],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD,
        )
    }

    #[test]
    fn blank_readbacks_are_detected_even_with_opaque_alpha() {
        let mut image = blank_image();
        assert!(capture_is_blank(&image));
        image.data.as_mut().unwrap()[0] = 20;
        assert!(!capture_is_blank(&image));
    }

    #[test]
    fn capture_waits_for_the_confirmed_step_snapshot() {
        let mut session = test_session(true);
        session.ready();
        session.apply(rm_simulator_server::protocol::Command::Step { ticks: 16 });
        let mut app = App::new();
        app.add_message::<AppExit>();
        app.insert_resource(session);
        app.insert_resource(CaptureReadiness(Arc::new(AtomicUsize::new(0))));
        app.insert_resource(CadSceneStatus {
            expected: 1,
            loaded: 1,
            failed: None,
        });
        app.insert_resource(crate::debug::CollisionDebug::new(
            crate::debug::CollisionView::Hidden,
        ));
        let mut request = ScreenshotRequest::new("unused.png".into());
        request.frames = SCREENSHOT_WARMUP_FRAMES;
        app.insert_resource(request);
        app.add_systems(Update, take_screenshot);
        app.update();
        assert!(!app.world().resource::<ScreenshotRequest>().requested);
        wait_for_session(
            &mut app.world_mut().resource_mut::<Session>(),
            Session::commands_confirmed,
        );
        assert_eq!(app.world().resource::<Session>().snapshot.tick, 16);
        let readiness = app.world().resource::<CaptureReadiness>().clone();
        readiness.0.store(2, Ordering::Release);
        for _ in 0..SCREENSHOT_WARMUP_FRAMES + 2 {
            app.update();
        }
        assert!(!app.world().resource::<ScreenshotRequest>().requested);
        assert_eq!(app.world().resource::<ScreenshotRequest>().frames, 0);
        readiness.0.store(0, Ordering::Release);
        app.update();
        readiness.0.store(1, Ordering::Release);
        app.update();
        assert_eq!(app.world().resource::<ScreenshotRequest>().frames, 0);
        readiness.0.store(0, Ordering::Release);
        for _ in 0..SCREENSHOT_WARMUP_FRAMES {
            app.update();
        }
        assert!(app.world().resource::<ScreenshotRequest>().requested);
        for attempt in 1..=MAX_CAPTURE_ATTEMPTS {
            let entity = app
                .world_mut()
                .query_filtered::<Entity, With<Screenshot>>()
                .single(app.world())
                .unwrap();
            assert!(matches!(
                &app.world().get::<Screenshot>(entity).unwrap().0,
                bevy::camera::RenderTarget::Window(bevy::window::WindowRef::Primary)
            ));
            app.world_mut().trigger(ScreenshotCaptured {
                entity,
                image: blank_image(),
            });
            app.world_mut().despawn(entity);
            assert_eq!(
                app.world().resource::<ScreenshotRequest>().attempts,
                attempt
            );
            if attempt < MAX_CAPTURE_ATTEMPTS {
                assert!(!app.world().resource::<ScreenshotRequest>().requested);
                assert!(app.world().resource::<Messages<AppExit>>().is_empty());
                for _ in 0..SCREENSHOT_WARMUP_FRAMES {
                    app.update();
                }
            }
        }
        assert!(matches!(
            app.world_mut()
                .resource_mut::<Messages<AppExit>>()
                .drain()
                .next(),
            Some(AppExit::Error(_))
        ));
        app.world_mut().resource_mut::<ScreenshotRequest>().result = None;
        app.world_mut().resource_mut::<ScreenshotRequest>().started =
            Some(Instant::now() - CAPTURE_TIMEOUT);
        app.update();
        assert!(matches!(
            app.world_mut()
                .resource_mut::<Messages<AppExit>>()
                .drain()
                .next(),
            Some(AppExit::Error(_))
        ));
    }
}
