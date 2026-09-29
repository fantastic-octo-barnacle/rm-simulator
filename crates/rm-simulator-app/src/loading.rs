// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! The match lifecycle. A [`JoinRequest`] runs field preparation on a worker
//! while the window polls a splash; success installs the session and
//! [`Ready`], failure returns to the title screen with the reason. A
//! [`LeaveRequest`] tears a running match down the same way, whatever caused
//! it: a link failure, the escape key or the toolbar. The CAD package and the
//! scenery it spawns are kept across matches; only the field, the overlays and
//! the HUD are rebuilt.
use crate::{
    args::Args,
    controls::{Drive, Gun, Player},
    debug::CollisionDebug,
    scene,
    session::{Opened, Session},
    title::TitleScreen,
};
use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use rm_simulator_render::{
    RenderingConfig,
    cad::{CadSceneConfig, CadSceneStatus},
    flu_position,
    sync::{SceneInput, SceneState},
};
use rm_simulator_server::{cad_assets, layout::default_spawn};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

/// Which screen the window is showing. Exactly one is current, so the title
/// screen, the loading splash and the gameplay systems cannot run at once.
#[derive(States, Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
pub enum Screen {
    /// The title screen and its pages; no match is running.
    #[default]
    Title,
    /// A join is being prepared on a worker behind the splash.
    Loading,
    /// A match is running and the gameplay systems are unlocked.
    InMatch,
    /// Connected, with an overhead camera and deployment form; no gameplay input.
    Staging,
}
/// Whether a match is running, for the exclusive systems that cannot carry a
/// run condition. Equivalent to the `in_state(Screen::InMatch)` condition.
pub fn in_match(world: &World) -> bool {
    world
        .get_resource::<State<Screen>>()
        .is_some_and(|screen| *screen.get() == Screen::InMatch)
}
/// Insert this and enter [`Screen::Loading`] to start a join; the worker is
/// spawned when the transition runs.
#[derive(Resource)]
pub struct JoinRequest(pub Args);
/// Inserting this ends the running match at the end of the frame. The reason
/// is shown on the title screen; a deliberate leave has none.
#[derive(Resource)]
pub struct LeaveRequest(pub Option<String>);
/// The verified CAD package, loaded once per process and reused by every join.
#[derive(Resource, Clone)]
struct CadCache(Arc<cad_assets::CadAssets>);
/// Shared verification job: joins wait on their worker, never on the UI thread.
#[derive(Resource, Clone)]
struct Preload(Arc<OnceLock<Result<Arc<cad_assets::CadAssets>, String>>>);

fn preload(world: &mut World) {
    if world.contains_resource::<Preload>() || world.contains_resource::<CadCache>() {
        return;
    }
    let path = world
        .resource::<crate::title::BaseArgs>()
        .0
        .host
        .cad_assets
        .clone();
    let shared = Arc::new(OnceLock::new());
    let worker = shared.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("field-preload".into())
        .spawn(move || {
            worker.get_or_init(|| {
                cad_assets::load(&path)
                    .map(Arc::new)
                    .map_err(|e| format!("{e:#}"))
            });
        })
    {
        warn!("Could not preload field: {error}");
        return;
    }
    world.insert_resource(Preload(shared));
}

fn poll_preload(world: &mut World) {
    if world.contains_resource::<CadCache>() {
        return;
    }
    let Some(cad) = world
        .get_resource::<Preload>()
        .and_then(|job| job.0.get())
        .cloned()
    else {
        return;
    };
    if let Ok(cad) = cad {
        let args = &world.resource::<crate::title::BaseArgs>().0;
        let instances = scene::cad_instances(&cad, !args.host.no_rune, true);
        world.resource_mut::<CadSceneStatus>().expected = instances.len();
        world.insert_resource(CadSceneConfig {
            instances,
            rendering: RenderingConfig::field(),
        });
        world.insert_resource(CadCache(cad));
    }
}

/// Stage-based progress for the menu; verification and scene import are asynchronous.
pub fn preload_status(world: &World) -> (f32, String) {
    if let Some(Err(error)) = world.get_resource::<Preload>().and_then(|job| job.0.get()) {
        return (0.0, format!("Field preview unavailable: {error}"));
    }
    let cad = world.resource::<CadSceneStatus>();
    if let Some(error) = &cad.failed {
        return (0.0, format!("Field preview unavailable: {error}"));
    }
    if !world.contains_resource::<CadCache>() {
        return (0.05, "Verifying field files…".into());
    }
    if cad.ready() {
        return (1.0, "Field ready · Move the pointer to look around".into());
    }
    (
        0.2 + 0.8 * cad.loaded as f32 / cad.expected.max(1) as f32,
        format!("Loading field scenery: {} / {}", cad.loaded, cad.expected),
    )
}

#[derive(Component)]
struct Splash;
#[derive(Component)]
struct ProgressText;
#[derive(Component)]
struct ProgressBar;
type Prepared = anyhow::Result<(
    Arc<cad_assets::CadAssets>,
    Opened,
    Args,
    f64,
    Option<(Image, [f64; 4])>,
    [[f64; 3]; 2],
)>;

enum Update {
    Progress(f32, String),
    Complete(Box<Prepared>),
}
#[derive(Resource)]
struct Loading {
    receiver: Mutex<mpsc::Receiver<Update>>,
    progress: f32,
    text: String,
    prepared: bool,
    ready_frames: u32,
}
/// Starts the join when [`Screen::Loading`] is entered, polls the splash in
/// `Update` while it is current and clears it on the way out, and adds the
/// leave system to `Last`. Entering the state runs before `Update`, so a join
/// requested this frame is still polled in the same frame.
pub struct LoadingPlugin;
impl Plugin for LoadingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(Screen::Title), preload)
            .add_systems(bevy::prelude::Update, poll_preload.before(poll))
            .add_systems(OnEnter(Screen::Loading), start)
            .add_systems(OnExit(Screen::Loading), despawn_splash)
            .add_systems(
                bevy::prelude::Update,
                // A failed start leaves the state until the next transition.
                poll.run_if(in_state(Screen::Loading).and_then(resource_exists::<Loading>)),
            )
            .add_systems(
                Last,
                leave_requested.run_if(resource_exists::<LeaveRequest>),
            );
    }
}
/// The splash belongs to the loading screen and never outlives it.
fn despawn_splash(mut commands: Commands, splash: Query<Entity, With<Splash>>) {
    for entity in &splash {
        commands.entity(entity).despawn();
    }
}
fn start(world: &mut World) {
    let args = world
        .remove_resource::<JoinRequest>()
        .expect("join request")
        .0;
    let multiplayer = args.connect.is_some() || args.host.listen.is_some();
    world.resource_mut::<TitleScreen>().status = None;
    let cached = world.get_resource::<CadCache>().cloned();
    let preload = world.get_resource::<Preload>().cloned();
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("field-loading".into())
        .spawn(move || {
            let progress = |value, text: &str| {
                let _ = sender.send(Update::Progress(value, text.into()));
            };
            let result = (|| {
                let cad = match cached {
                    Some(cache) => cache.0,
                    None => {
                        progress(0.05, "Verifying field files");
                        if let Some(job) = preload {
                            job.0
                                .get_or_init(|| {
                                    cad_assets::load(&args.host.cad_assets)
                                        .map(Arc::new)
                                        .map_err(|e| format!("{e:#}"))
                                })
                                .clone()
                                .map_err(anyhow::Error::msg)?
                        } else {
                            Arc::new(cad_assets::load(&args.host.cad_assets)?)
                        }
                    }
                };
                let (side_spawn, side_yaw) = default_spawn(args.team.into());
                let spawn = args.spawn.unwrap_or(side_spawn);
                let yaw = args.spawn_yaw_deg.map_or(side_yaw, f64::from);
                let minimap = crate::minimap::load(&args.host.cad_assets);
                let opened = Session::open(&args, &cad, spawn, yaw, progress)?;
                let floor = rm_simulator_server::collision_mesh::load_glb(
                    &cad.floor.physics_file(&cad.root),
                )?
                .into_flu(cad.floor_top_cad_m);
                let scenery = rm_simulator_server::collision_mesh::load_glb(
                    &cad.arena_static.physics_file(&cad.root),
                )?
                .into_flu(cad.floor_top_cad_m);
                let bounds = rm_simulator_server::layout::arena_boundary_bounds(&floor, &scenery)?;
                Ok((cad, opened, args, yaw, minimap, bounds))
            })();
            let _ = sender.send(Update::Complete(Box::new(result)));
        });
    let failed = worker.err().map(|e| format!("Startup failed: {e}"));
    world.insert_resource(Loading {
        receiver: Mutex::new(receiver),
        progress: 0.0,
        text: failed.clone().unwrap_or_else(|| "Preparing field".into()),
        prepared: false,
        ready_frames: 0,
    });
    if let Some(text) = failed {
        to_title(world, Some(text));
        return;
    }
    world
        .spawn((
            Splash,
            GlobalZIndex(100),
            Node {
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: px(20),
                ..default()
            },
            BackgroundColor(Color::srgb(0.035, 0.05, 0.075)),
        ))
        .with_children(|parent| {
            if multiplayer {
                parent.spawn((
                    Text::new(crate::title::FIREWALL_TIP),
                    Node {
                        max_width: px(600),
                        ..default()
                    },
                    TextFont {
                        font_size: FontSize::Px(16.0),
                        ..default()
                    },
                    TextLayout::justify(Justify::Center),
                ));
            }
            parent.spawn((
                Text::new("RM SIMULATOR"),
                TextFont {
                    font_size: FontSize::Px(38.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            ));
            parent.spawn((
                Text::new("Preparing field"),
                ProgressText,
                Node {
                    max_width: percent(85),
                    ..default()
                },
                TextLayout::justify(Justify::Center),
                TextFont {
                    font_size: FontSize::Px(18.0),
                    ..default()
                },
            ));
            parent
                .spawn((
                    Node {
                        width: percent(60),
                        max_width: px(560),
                        height: px(6),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.12, 0.17, 0.22)),
                ))
                .with_children(|bar| {
                    bar.spawn((
                        ProgressBar,
                        Node {
                            width: percent(0),
                            height: percent(100),
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.1, 0.75, 0.85)),
                    ));
                });
            parent.spawn((
                Text::new("Loading progress by stage  |  Esc to cancel"),
                TextFont {
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
                TextColor(Color::srgb(0.55, 0.65, 0.72)),
            ));
        });
}
fn poll(world: &mut World) {
    let mut loading = world.remove_resource::<Loading>().expect("loading state");
    if world
        .resource::<ButtonInput<KeyCode>>()
        .just_pressed(KeyCode::Escape)
    {
        // Dropping the receiver makes a late worker result drop on the worker.
        // The title must not wait for verification, a connection, or physics.
        to_title(world, None);
        return;
    }
    let mut outcome: Option<Option<String>> = None;
    loop {
        let message = loading
            .receiver
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_recv();
        match message {
            Ok(Update::Progress(value, text)) => {
                loading.progress = value;
                loading.text = text;
            }
            Ok(Update::Complete(result)) => match *result {
                Ok((cad, opened, args, yaw, minimap, bounds)) => {
                    world.insert_resource(crate::staging::StagingConfig(args.clone()));
                    world.insert_resource(scene::ArenaBounds(bounds));
                    world.insert_resource(Gun::new(
                        opened.session.weapon.shot,
                        opened.session.weapon.interval_ns,
                    ));
                    // Direct CLI joins may skip title preloading; install scenery
                    // here only when the preload has not already supplied it.
                    if !world.contains_resource::<CadCache>() {
                        let instances = scene::cad_instances(
                            &cad,
                            !args.host.no_rune,
                            !opened.session.snapshot.bases.is_empty(),
                        );
                        world.resource_mut::<CadSceneStatus>().expected = instances.len();
                        world.insert_resource(CadSceneConfig {
                            instances,
                            rendering: RenderingConfig::field(),
                        });
                    }
                    world.insert_resource(CadCache(cad));
                    world.insert_resource(CollisionDebug::new(args.collision_view));
                    world.insert_resource(Player::at(
                        flu_position(opened.eye_flu),
                        yaw.to_radians() as f32,
                        args.spawn_pitch_deg.to_radians(),
                    ));
                    if let Some(id) = opened.session.chassis_id {
                        world.insert_resource(Drive::new(id, args.third_person));
                    }
                    if let Some((image, bounds_m)) = minimap
                        && !world.contains_resource::<crate::minimap::Minimap>()
                    {
                        let image = world.resource_mut::<Assets<Image>>().add(image);
                        world.insert_resource(crate::minimap::Minimap { image, bounds_m });
                    }
                    world.insert_resource(opened.session);
                    world
                        .run_system_cached(scene::setup_stadium)
                        .expect("stadium setup");
                    world.run_system_cached(scene::setup).expect("scene setup");
                    loading.prepared = true;
                    break;
                }
                Err(error) => {
                    let text = format!("Startup failed: {error:#}");
                    error!("{text}");
                    outcome = Some(Some(text));
                    break;
                }
            },
            Err(mpsc::TryRecvError::Disconnected) if !loading.prepared => {
                outcome = Some(Some("Startup worker stopped unexpectedly".into()));
                break;
            }
            Err(_) => break,
        }
    }
    if let Some(status) = outcome {
        to_title(world, status);
        return;
    }
    if loading.prepared
        && let Some(mut session) = world.get_resource_mut::<Session>()
        && let Err(error) = session.poll()
    {
        to_title(world, Some(format!("Startup failed: {error}")));
        return;
    }
    if loading.prepared {
        let cad = world.resource::<CadSceneStatus>();
        if let Some(error) = &cad.failed {
            let text = format!("Startup failed: {error}");
            to_title(world, Some(text));
            return;
        }
        loading.progress =
            0.7 + 0.3 * cad.loaded.min(cad.expected) as f32 / cad.expected.max(1) as f32;
        loading.text = format!(
            "Loading field scenery: {} / {} instances",
            cad.loaded, cad.expected
        );
        if cad.ready() {
            loading.ready_frames += 1;
            loading.text = "Preparing first frame".into();
            if loading.ready_frames >= 3 {
                if let Some(session) = world.get_resource::<Session>() {
                    session.ready();
                }
                let staging = world
                    .get_resource::<crate::staging::StagingConfig>()
                    .is_some_and(|config| config.0.staging);
                enter(
                    world,
                    if staging {
                        Screen::Staging
                    } else {
                        Screen::InMatch
                    },
                );
                return;
            }
        }
    }
    for mut text in world
        .query_filtered::<&mut Text, With<ProgressText>>()
        .iter_mut(world)
    {
        text.0.clone_from(&loading.text);
    }
    for mut node in world
        .query_filtered::<&mut Node, With<ProgressBar>>()
        .iter_mut(world)
    {
        node.width = percent(loading.progress * 100.0);
    }
    world.insert_resource(loading);
}

fn leave_requested(world: &mut World) {
    let reason = world
        .remove_resource::<LeaveRequest>()
        .expect("leave request")
        .0;
    match &reason {
        Some(reason) => error!("leaving the match: {reason}"),
        None => info!("leaving the match"),
    }
    to_title(world, reason);
}

/// Leave whatever is running, whether a match or a join in progress, and
/// show the title screen with `status`. Dropping the session closes the
/// connection and, when this window hosted, shuts the host down. In
/// screenshot mode there is nobody to read the title screen, so a failure
/// exits instead.
pub fn to_title(world: &mut World, status: Option<String>) {
    world.remove_resource::<Loading>();
    world.remove_resource::<LeaveRequest>();
    world.remove_resource::<Session>();
    world.remove_resource::<crate::staging::StagingConfig>();
    if let Some(mut assist) = world.get_resource_mut::<crate::auto_aim::AutoAim>() {
        *assist = default();
    }
    world.remove_resource::<Drive>();
    if let Some(mut ui) = world.get_resource_mut::<crate::hud::HudState>() {
        ui.close();
        ui.respawn = false;
    }
    if let Some(mut player) = world.get_resource_mut::<Player>() {
        player.captured = false;
    }
    for mut cursor in world
        .query_filtered::<&mut CursorOptions, With<PrimaryWindow>>()
        .iter_mut(world)
    {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
    if world.contains_resource::<SceneInput>() {
        // An empty scene despawns the chassis and hides the projectile pool.
        world.insert_resource(SceneInput(Some(SceneState::default())));
    }
    teardown(world);
    if status.is_some() && world.contains_resource::<crate::screenshot::ScreenshotRequest>() {
        world.write_message(AppExit::error());
    }
    world.insert_resource(TitleScreen { status });
    enter(world, Screen::Title);
}

/// Queue a screen transition from an exclusive system. It is applied before
/// the next `Update`, so the systems of the screen being left do not run again.
fn enter(world: &mut World, screen: Screen) {
    world.init_resource::<NextState<Screen>>();
    world.resource_mut::<NextState<Screen>>().set(screen);
}

/// Despawn everything a match or a join put on screen. The CAD scenery, the
/// camera and the lighting stay.
fn teardown(world: &mut World) {
    despawn_all::<Splash>(world);
    despawn_all::<rm_simulator_render::rune::RuneVisual>(world);
    despawn_all::<rm_simulator_render::outpost::OutpostArmor>(world);
    despawn_all::<rm_simulator_render::outpost::BaseArmor>(world);
    despawn_all::<crate::hud::HudRoot>(world);
    despawn_all::<crate::hud::Toolbar>(world);
    despawn_all::<crate::hud::Settings>(world);
    despawn_all::<crate::debug_panel::DebugPanel>(world);
    despawn_all::<crate::network_hud::NetworkOverlay>(world);
    despawn_all::<crate::debug::CollisionWireframe>(world);
    despawn_all::<crate::debug::DynamicCollisionWireframe>(world);
}

fn despawn_all<M: Component>(world: &mut World) {
    let entities: Vec<Entity> = world
        .query_filtered::<Entity, With<M>>()
        .iter(world)
        .collect();
    for entity in entities {
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }
}

/// The reason the last join or match ended, visible to automation while the
/// title screen shows it.
pub fn failure(world: &World) -> Option<String> {
    world
        .get_resource::<TitleScreen>()
        .and_then(|title| title.status.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The screen `poll` or `to_title` queued. A bare test world runs no
    /// state transitions, so nothing applies it.
    fn queued(world: &World) -> Option<Screen> {
        match world.get_resource::<NextState<Screen>>() {
            Some(NextState::Pending(screen)) => Some(*screen),
            _ => None,
        }
    }
    /// Apply what a transition would have done to the splash.
    fn leave_loading(world: &mut World) {
        world
            .run_system_cached(despawn_splash)
            .expect("despawn the splash");
    }

    fn loading_world(prepared: bool) -> (World, mpsc::Sender<Update>) {
        let (sender, receiver) = mpsc::channel();
        let mut world = World::new();
        world.init_resource::<ButtonInput<KeyCode>>();
        world.insert_resource(TitleScreen::default());
        world.insert_resource(CadSceneStatus {
            expected: 2,
            loaded: 0,
            failed: None,
        });
        world.insert_resource(Loading {
            receiver: Mutex::new(receiver),
            progress: 0.0,
            text: String::new(),
            prepared,
            ready_frames: 0,
        });
        (world, sender)
    }

    #[test]
    fn unfinished_preload_keeps_the_ui_available_and_failure_is_reported() {
        let (mut world, _sender) = loading_world(false);
        let shared = Arc::new(OnceLock::new());
        world.insert_resource(Preload(shared.clone()));
        poll_preload(&mut world);
        assert!(!world.contains_resource::<CadCache>());
        assert!(preload_status(&world).1.contains("Verifying"));
        assert!(shared.set(Err("missing field manifest".into())).is_ok());
        poll_preload(&mut world);
        assert!(preload_status(&world).1.contains("missing field manifest"));
        assert!(!world.contains_resource::<Session>());
        assert_eq!(queued(&world), None);
    }

    #[test]
    fn pending_worker_does_not_unlock_gameplay_and_disconnect_returns_to_the_title() {
        let (mut world, sender) = loading_world(false);
        sender
            .send(Update::Progress(0.3, "Reading terrain triangles".into()))
            .unwrap();
        poll(&mut world);
        assert_eq!(world.resource::<Loading>().progress, 0.3);
        assert_ne!(queued(&world), Some(Screen::InMatch));
        drop(sender);
        poll(&mut world);
        assert!(!world.contains_resource::<Loading>());
        assert_ne!(queued(&world), Some(Screen::InMatch));
        assert!(failure(&world).unwrap().contains("stopped unexpectedly"));
    }

    #[test]
    fn scenery_must_finish_before_splash_is_removed() {
        let (mut world, _sender) = loading_world(true);
        let splash = world.spawn(Splash).id();
        for _ in 0..4 {
            poll(&mut world);
        }
        assert_ne!(queued(&world), Some(Screen::InMatch));
        world.resource_mut::<CadSceneStatus>().loaded = 2;
        for _ in 0..3 {
            poll(&mut world);
        }
        assert_eq!(queued(&world), Some(Screen::InMatch));
        assert!(!world.contains_resource::<Loading>());
        leave_loading(&mut world);
        assert!(world.get_entity(splash).is_err());
    }

    #[test]
    fn scenery_readiness_releases_the_embedded_clock() {
        let (mut world, _sender) = loading_world(true);
        world.insert_resource(crate::session::test_session(false));
        poll(&mut world);
        assert_eq!(world.resource::<Session>().snapshot.tick, 0);
        world.resource_mut::<CadSceneStatus>().loaded = 2;
        for _ in 0..3 {
            poll(&mut world);
        }
        assert_eq!(queued(&world), Some(Screen::InMatch));
        crate::session::wait_for_session(&mut world.resource_mut::<Session>(), |session| {
            session.snapshot.tick > 0
        });
    }

    #[test]
    fn asset_failure_returns_to_the_title_with_the_error() {
        let (mut world, _sender) = loading_world(true);
        world.insert_resource(crate::session::test_session(false));
        world.spawn(Splash);
        world.resource_mut::<CadSceneStatus>().failed = Some("missing.glb".into());
        poll(&mut world);
        assert!(failure(&world).unwrap().contains("missing.glb"));
        assert_ne!(queued(&world), Some(Screen::InMatch));
        assert!(!world.contains_resource::<Session>());
        assert!(
            world
                .query_filtered::<(), With<Splash>>()
                .iter(&world)
                .next()
                .is_none()
        );
    }

    #[test]
    fn escape_cancels_scenery_and_first_frame_preparation() {
        for loaded in [0, 2] {
            let (mut world, _sender) = loading_world(true);
            world.insert_resource(crate::session::test_session(false));
            let splash = world.spawn(Splash).id();
            world.resource_mut::<CadSceneStatus>().loaded = loaded;
            world
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::Escape);
            poll(&mut world);
            assert!(world.contains_resource::<TitleScreen>());
            assert!(!world.contains_resource::<Loading>());
            assert_ne!(queued(&world), Some(Screen::InMatch));
            assert!(!world.contains_resource::<Session>());
            assert!(world.get_entity(splash).is_err());
            assert_eq!(failure(&world), None);
        }
    }

    #[test]
    fn a_cancelled_join_discards_the_worker_result() {
        let (mut world, sender) = loading_world(false);
        world
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        poll(&mut world);
        assert!(!world.contains_resource::<Loading>());
        assert!(
            sender
                .send(Update::Complete(Box::new(Err(anyhow::anyhow!("late")))))
                .is_err()
        );
        assert_eq!(failure(&world), None);
        assert!(world.contains_resource::<TitleScreen>());
    }

    #[test]
    fn a_join_that_cannot_read_the_field_returns_to_the_title() {
        let mut world = World::new();
        world.init_resource::<ButtonInput<KeyCode>>();
        world.insert_resource(CadSceneStatus {
            expected: 0,
            loaded: 0,
            failed: None,
        });
        world.insert_resource(TitleScreen::default());
        let args =
            Args::try_parse_from(["rm-simulator", "--cad-assets", "/nonexistent/field"]).unwrap();
        world.insert_resource(JoinRequest(args));
        start(&mut world);
        assert_eq!(failure(&world), None);
        assert!(world.contains_resource::<Loading>());
        let started = std::time::Instant::now();
        while world.contains_resource::<Loading>() {
            assert!(started.elapsed() < std::time::Duration::from_secs(30));
            std::thread::sleep(std::time::Duration::from_millis(5));
            poll(&mut world);
        }
        assert!(failure(&world).unwrap().contains("Startup failed"));
        assert_ne!(queued(&world), Some(Screen::InMatch));
    }

    #[test]
    fn leaving_a_match_tears_everything_down() {
        let mut world = World::new();
        world.insert_resource(TitleScreen::default());
        world.insert_resource(crate::session::test_session(false));
        world.insert_resource(Drive::new(1, false));
        world.insert_resource(Player::at(Vec3::ZERO, 0.0, 0.0));
        world.resource_mut::<Player>().captured = true;
        world.init_resource::<crate::hud::HudState>();
        world.resource_mut::<crate::hud::HudState>().settings = true;
        world.insert_resource(SceneInput(None));
        world.spawn((
            PrimaryWindow,
            CursorOptions {
                grab_mode: CursorGrabMode::Locked,
                visible: false,
                ..default()
            },
        ));
        let hud = world.spawn(crate::hud::HudRoot).id();
        let rune = world
            .spawn(rm_simulator_render::rune::RuneVisual { rune: 0 })
            .id();
        let light = world.spawn(ChildOf(rune)).id();
        world.insert_resource(LeaveRequest(Some("link lost".into())));
        leave_requested(&mut world);
        assert_ne!(queued(&world), Some(Screen::InMatch));
        assert!(!world.contains_resource::<Session>());
        assert!(!world.contains_resource::<Drive>());
        assert!(!world.contains_resource::<LeaveRequest>());
        assert_eq!(failure(&world).as_deref(), Some("link lost"));
        for entity in [hud, rune, light] {
            assert!(world.get_entity(entity).is_err());
        }
        assert!(!world.resource::<Player>().captured);
        assert!(!world.resource::<crate::hud::HudState>().settings);
        let cursor = world
            .query_filtered::<&CursorOptions, With<PrimaryWindow>>()
            .single(&world)
            .unwrap();
        assert!(cursor.visible);
        assert_eq!(cursor.grab_mode, CursorGrabMode::None);
        assert_eq!(
            world.resource::<SceneInput>().0,
            Some(SceneState::default())
        );
    }
}
