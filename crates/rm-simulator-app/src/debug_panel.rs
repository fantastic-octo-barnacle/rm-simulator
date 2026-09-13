// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Developer controls and a persistent renderer statistics overlay.
use crate::{
    debug::{CollisionDebug, CollisionView},
    hud::HudState,
};
use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    feathers::{
        controls::{FeathersButton, FeathersCheckbox, FeathersRadio},
        theme::ThemedText,
    },
    pbr::wireframe::{WireframeConfig, WireframePlugin},
    prelude::*,
    render::{diagnostic::RenderDiagnosticsPlugin, renderer::RenderDevice, settings::WgpuFeatures},
    ui::{Checked, InteractionDisabled},
    ui_widgets::{Activate, ActivateOnPress, RadioGroup, ValueChange},
};

/// Add the F3 developer panel, its defeated-robot respawn prompt and the
/// wireframe and diagnostic plugins the panel needs.
pub struct DebugPanelPlugin;
impl Plugin for DebugPanelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            respawn_menu
                .after(crate::session::advance_world)
                .before(crate::controls::mouse_capture)
                .run_if(resource_exists::<crate::loading::Ready>),
        )
        .init_resource::<DebugOptions>()
        .add_plugins((
            WireframePlugin::default(),
            FrameTimeDiagnosticsPlugin::default(),
            RenderDiagnosticsPlugin,
        ))
        .add_systems(
            Update,
            (spawn_panel, sync_panel, sync_buffer_status)
                .chain()
                .after(crate::hud::menu_input)
                .after(crate::hud::panel_input)
                .before(crate::debug::collision_view)
                .run_if(resource_exists::<crate::loading::Ready>),
        );
    }
}
/// Debug overlay flags that survive a match teardown; the panel checkboxes and
/// the console `inspect` command write them.
#[derive(Resource, Default)]
pub(crate) struct DebugOptions {
    /// Whether the frame and render pass statistics overlay is drawn. Set by
    /// `--render-stats`, the panel checkbox and the console `inspect` command.
    pub stats: bool,
}
/// Marker for the panel root node; `sync_panel` shows it while
/// `HudState::debug` is set, and match teardown despawns it.
#[derive(Component)]
pub(crate) struct DebugPanel;
#[derive(Component)]
struct Statistics;
#[derive(Component)]
struct Status;
#[derive(Component)]
struct BufferStatus;
#[derive(Component)]
struct CollisionChoice(CollisionView);
#[derive(Component)]
struct StatsChoice;
#[derive(Component)]
struct WireframeChoice;

fn spawn_panel(
    mut commands: Commands,
    existing: Query<(), With<DebugPanel>>,
    assets: Res<AssetServer>,
) {
    if !existing.is_empty() {
        return;
    }
    let font: Handle<Font> = assets.load(bevy::feathers::constants::fonts::REGULAR);
    let label_font = TextFont {
        font: font.into(),
        font_size: bevy::text::FontSize::Px(16.),
        ..default()
    };
    let panel = commands
        .spawn((
            DebugPanel,
            GlobalZIndex(30),
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                right: px(20),
                top: px(120),
                width: px(410),
                max_width: percent(95),
                max_height: percent(70),
                overflow: Overflow::scroll_y(),
                padding: UiRect::all(px(22)),
                row_gap: px(18),
                flex_direction: FlexDirection::Column,
                border_radius: BorderRadius::all(px(8)),
                ..default()
            },
            bevy::ui_widgets::ScrollArea,
            BackgroundColor(Color::srgba(0.025, 0.04, 0.055, 0.97)),
        ))
        .id();
    commands.spawn((
        ChildOf(panel),
        Text::new("DEBUG"),
        ThemedText,
        label_font.clone(),
    ));
    commands.spawn_scene(bsn! {
        @FeathersButton { @caption: bsn! { Text("Network stats: Off / Compact / Detailed") ThemedText } }
        ActivateOnPress
        on(|_: On<Activate>, mut ui: ResMut<HudState>| { ui.network_stats = ui.network_stats.next(); })
    }).insert(ChildOf(panel));
    commands.spawn((
        ChildOf(panel),
        BufferStatus,
        Text::new("Remote motion buffering"),
        ThemedText,
        label_font.clone(),
    ));
    commands.spawn_scene(bsn! {
        @FeathersButton { @caption: bsn! { Text("Buffer mode: Automatic / Manual") ThemedText } }
        ActivateOnPress
        on(|_: On<Activate>, mut session: ResMut<crate::session::Session>| {
            session.remote_buffer.automatic = !session.remote_buffer.automatic;
        })
    }).insert(ChildOf(panel));
    for (caption, delta) in [
        ("Less buffering (-10 ms)", -10),
        ("More buffering (+10 ms)", 10),
    ] {
        commands
            .spawn_scene(bsn! {
                @FeathersButton { @caption: bsn! { Text(caption) ThemedText } }
                ActivateOnPress
                on(move |_: On<Activate>, mut session: ResMut<crate::session::Session>| {
                    session.remote_buffer.adjust_manual(delta);
                })
            })
            .insert(ChildOf(panel));
    }
    commands
        .spawn_scene(bsn! {
            @FeathersButton { @caption: bsn! { Text("Reset buffering to defaults") ThemedText } }
            ActivateOnPress
            on(|_: On<Activate>, mut session: ResMut<crate::session::Session>| {
                session.remote_buffer = Default::default();
            })
        })
        .insert(ChildOf(panel));
    commands.spawn((
        ChildOf(panel),
        Text::new("Physics colliders"),
        ThemedText,
        label_font.clone(),
    ));
    let group = commands
        .spawn((
            ChildOf(panel),
            RadioGroup,
            bevy::input_focus::tab_navigation::TabIndex(0),
            AccessibleLabel("Physics collider view".into()),
            Node {
                column_gap: px(18),
                ..default()
            },
        ))
        .id();
    for (title, view) in [
        ("Off", CollisionView::Hidden),
        ("Overlay", CollisionView::Overlay),
        ("Only", CollisionView::Alone),
    ] {
        commands
            .spawn_scene(bsn! {
                @FeathersRadio { @caption: bsn! { Text(title) ThemedText } }
                AccessibleLabel(title)
                on(move |change: On<ValueChange<bool>>, mut debug: ResMut<CollisionDebug>| {
                    if change.value { debug.view = view; }
                })
            })
            .insert((ChildOf(group), CollisionChoice(view)));
    }
    commands.spawn((
        ChildOf(panel),
        Text::new("Green: fixed geometry\nAmber: chassis, armor and projectiles\nClient poses; no server debug requests"),
        ThemedText,
        label_font.clone(),
    ));
    commands.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text("Visual mesh wireframe") ThemedText } }
        AccessibleLabel("Visual mesh wireframe")
        on(|change: On<ValueChange<bool>>, mut config: ResMut<WireframeConfig>| { config.global = change.value; })
    }).insert((ChildOf(panel), WireframeChoice));
    commands.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text("Show rendering statistics") ThemedText } }
        AccessibleLabel("Show rendering statistics")
        on(|change: On<ValueChange<bool>>, mut options: ResMut<DebugOptions>| { options.stats = change.value; })
    }).insert((ChildOf(panel), StatsChoice));
    commands.spawn((
        ChildOf(panel),
        Status,
        Text::new(""),
        ThemedText,
        label_font.clone(),
    ));
    commands.spawn((
        ChildOf(panel),
        Text::new("Esc closes this panel.\nClick the field to resume control."),
        ThemedText,
        label_font.clone(),
    ));
    commands.spawn_scene(bsn! {
        @FeathersButton { @caption: bsn! { Text("Reset robot to spawn") ThemedText } }
        ActivateOnPress
        AccessibleLabel("Reset robot to spawn")
        on(|_: On<Activate>, mut session: ResMut<crate::session::Session>, mut ui: ResMut<HudState>| {
            ui.consumed = true;
            if let Some(chassis) = session.chassis_id {
                session.apply(rm_simulator_server::protocol::Command::ResetRobot { chassis });
            }
        })
    }).insert(ChildOf(panel));
    commands.spawn_scene(bsn! {
        @FeathersButton { @caption: bsn! { Text("Close debug panel") ThemedText } }
        ActivateOnPress
        AccessibleLabel("Close debug panel")
        on(|_: On<Activate>, mut ui: ResMut<HudState>, mut focus: ResMut<bevy::input_focus::InputFocus>| {
            ui.close();
            focus.clear();
        })
    }).insert(ChildOf(panel));
    commands.spawn((
        Statistics,
        GlobalZIndex(25),
        Node {
            position_type: PositionType::Absolute,
            left: px(18),
            top: px(100),
            max_width: percent(48),
            padding: UiRect::all(px(12)),
            display: Display::None,
            ..default()
        },
        Text::new(""),
        TextFont {
            font_size: bevy::text::FontSize::Px(14.),
            ..default()
        },
        TextColor(Color::WHITE),
        BackgroundColor(Color::srgba(0., 0., 0., 0.82)),
        Pickable::IGNORE,
    ));
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn sync_panel(
    mut commands: Commands,
    ui: Res<HudState>,
    debug: Res<CollisionDebug>,
    options: Res<DebugOptions>,
    mut config: ResMut<WireframeConfig>,
    device: Res<RenderDevice>,
    diagnostics: Res<DiagnosticsStore>,
    time: Res<Time>,
    mut elapsed: Local<f32>,
    mut panel: Query<&mut Node, (With<DebugPanel>, Without<Statistics>)>,
    mut stats: Query<
        (&mut Node, &mut Text),
        (With<Statistics>, Without<DebugPanel>, Without<Status>),
    >,
    mut status: Query<&mut Text, (With<Status>, Without<Statistics>)>,
    choices: Query<
        (
            Entity,
            Has<Checked>,
            Option<&CollisionChoice>,
            Has<StatsChoice>,
            Has<WireframeChoice>,
        ),
        Or<(
            With<CollisionChoice>,
            With<StatsChoice>,
            With<WireframeChoice>,
        )>,
    >,
    meshes: Query<&ViewVisibility, With<Mesh3d>>,
) {
    let supported = device
        .features()
        .contains(WgpuFeatures::POLYGON_MODE_LINE | WgpuFeatures::IMMEDIATES);
    if !supported && config.global {
        config.global = false;
    }
    for mut node in &mut panel {
        node.display = if ui.debug {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (entity, checked, collision, stats, wireframe) in &choices {
        let value = collision.map_or(if stats { options.stats } else { config.global }, |c| {
            c.0 == debug.view
        });
        if checked != value {
            if value {
                commands.entity(entity).insert(Checked);
            } else {
                commands.entity(entity).remove::<Checked>();
            }
        }
        if wireframe && !supported {
            commands.entity(entity).insert(InteractionDisabled);
        }
    }
    for mut text in &mut status {
        let mut value = debug
            .failure()
            .unwrap_or(if debug.pending() {
                "Building fixed collider mesh…"
            } else {
                ""
            })
            .to_string();
        if !supported {
            value.push_str("\nVisual wireframe is unsupported on this GPU.");
        }
        text.set_if_neq(Text::new(value));
    }
    *elapsed += time.delta_secs();
    for (mut node, mut text) in &mut stats {
        node.display = if options.stats {
            Display::Flex
        } else {
            Display::None
        };
        if !options.stats || (*elapsed < 0.25 && !options.is_changed()) {
            continue;
        }
        *elapsed = 0.;
        let value = |path: &bevy::diagnostic::DiagnosticPath| {
            diagnostics
                .get(path)
                .and_then(|d| d.smoothed())
                .map_or("--".into(), |v| format!("{v:.1}"))
        };
        let mut lines = vec![
            format!(
                "{} FPS   {} ms/frame",
                value(&FrameTimeDiagnosticsPlugin::FPS),
                value(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
            ),
            format!(
                "Mesh instances visible: {} / {}",
                meshes.iter().filter(|v| v.get()).count(),
                meshes.iter().count()
            ),
        ];
        let mut render: Vec<_> = diagnostics
            .iter()
            .filter(|d| {
                d.path().as_str().starts_with("render/")
                    && d.path().as_str().ends_with("elapsed_cpu")
            })
            .collect();
        render.sort_by(|a, b| a.path().as_str().cmp(b.path().as_str()));
        for diagnostic in render.into_iter().take(8) {
            if let Some(ms) = diagnostic.smoothed() {
                lines.push(format!(
                    "{}: {ms:.2} ms",
                    diagnostic
                        .path()
                        .as_str()
                        .trim_start_matches("render/")
                        .trim_end_matches("/elapsed_cpu")
                ));
            }
        }
        lines.push("Render pass CPU times; GPU timings are device dependent.".into());
        text.set_if_neq(Text::new(lines.join("\n")));
    }
}

#[derive(Component)]
struct RespawnMenu;

/// Death remains visible until the host confirms the player's explicit respawn.
fn respawn_menu(
    mut commands: Commands,
    session: Res<crate::session::Session>,
    mut ui: ResMut<HudState>,
    mut panels: Query<&mut Node, With<RespawnMenu>>,
) {
    let defeated = session
        .own_chassis()
        .is_some_and(|chassis| chassis.defeated);
    if ui.respawn && !defeated {
        ui.consumed = true;
    }
    ui.respawn = defeated;
    if panels.is_empty() {
        let panel = commands
            .spawn((
                RespawnMenu,
                GlobalZIndex(40),
                Node {
                    display: if defeated {
                        Display::Flex
                    } else {
                        Display::None
                    },
                    position_type: PositionType::Absolute,
                    width: percent(100),
                    height: percent(100),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    flex_direction: FlexDirection::Column,
                    row_gap: px(18),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.01, 0.02, 0.03, 0.75)),
            ))
            .id();
        commands.spawn((ChildOf(panel), Text::new("Robot defeated"), ThemedText));
        commands.spawn_scene(bsn! {
            @FeathersButton { @caption: bsn! { Text("Respawn here - restore HP") ThemedText } }
            ActivateOnPress
            AccessibleLabel("Respawn here - restore HP")
            on(|_: On<Activate>, mut session: ResMut<crate::session::Session>, mut ui: ResMut<HudState>| {
                ui.consumed = true;
                if session.commands_confirmed() && let Some(chassis) = session.chassis_id {
                    session.apply(rm_simulator_server::protocol::Command::Respawn { chassis });
                }
            })
        }).insert(ChildOf(panel));
    }
    for mut panel in &mut panels {
        panel.display = if defeated {
            Display::Flex
        } else {
            Display::None
        };
    }
}

fn sync_buffer_status(
    session: Res<crate::session::Session>,
    mut labels: Query<&mut Text, With<BufferStatus>>,
) {
    let buffer = &session.remote_buffer;
    let mode = if buffer.automatic {
        "Automatic"
    } else {
        "Manual"
    };
    let value = format!(
        "Remote motion: {mode}\nManual setting: {} ms (0–250)\nEffective delay: {} ms   View age: {:.0} ms\nUnderruns: {}\nMore buffering makes remote motion smoother but older.",
        buffer.manual_ms,
        buffer.delay_ms(),
        buffer.view_age_ms,
        buffer.underruns
    );
    for mut text in &mut labels {
        if text.0 != value {
            text.0.clone_from(&value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defeat_menu_blocks_controls_until_confirmed_health_returns() {
        let mut session = crate::session::test_session(true);
        session.snapshot.chassis[0].defeated = true;
        let mut app = App::new();
        app.insert_resource(session)
            .init_resource::<HudState>()
            .add_systems(Update, respawn_menu);
        let panel = app.world_mut().spawn((RespawnMenu, Node::default())).id();
        app.update();
        assert!(app.world().resource::<HudState>().blocks_input());
        assert_eq!(
            app.world().get::<Node>(panel).unwrap().display,
            Display::Flex
        );
        app.world_mut()
            .resource_mut::<crate::session::Session>()
            .snapshot
            .chassis[0]
            .defeated = false;
        app.update();
        let ui = app.world().resource::<HudState>();
        assert!(!ui.respawn);
        assert!(ui.consumed);
        assert_eq!(
            app.world().get::<Node>(panel).unwrap().display,
            Display::None
        );
    }
}
