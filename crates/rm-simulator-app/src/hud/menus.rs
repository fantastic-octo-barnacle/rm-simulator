// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Mouse controls use Feathers; the competitor overlay uses a separate Flair subtree.
use super::HudState;
use bevy::{
    feathers::{
        FeathersPlugins,
        controls::{FeathersButton, FeathersCheckbox, FeathersSlider},
        dark_theme::create_dark_theme,
        theme::{ThemedText, UiTheme},
    },
    input_focus::InputFocus,
    picking::hover::Hovered,
    prelude::*,
    ui::Checked,
    ui_widgets::{
        Activate, ActivateOnPress, SliderPrecision, SliderStep, SliderValue, ValueChange,
    },
};

/// Adds the Feathers and Flair plugins, the dark UI theme and every system that
/// spawns and drives the toolbar, settings card and pause and quit dialogs.
pub struct HudPlugin;
impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((bevy_flair::FlairPlugin, FeathersPlugins))
            .insert_resource(UiTheme(create_dark_theme()))
            .init_resource::<SettingsTab>()
            .init_resource::<MenuActions>()
            .init_resource::<super::controls_menu::RebindState>()
            .add_observer(forward_settings_scroll)
            .add_systems(
                PreUpdate,
                (clear_consumed, window_focus)
                    .chain()
                    .before(bevy::picking::PickingSystems::ProcessInput),
            )
            .add_systems(
                Update,
                (
                    spawn_menus,
                    super::scroll_peek_panel.after(super::update_panel_rows),
                    super::team_status::update.after(crate::session::advance_world),
                    sync_dialogs.after(menu_input).after(super::panel_input),
                    pause_singleplayer
                        .after(menu_input)
                        .after(super::panel_input)
                        .before(crate::session::advance_world),
                    sync_sections,
                    sync_weapon_controls,
                    super::controls_menu::sync_overlay,
                    super::controls_menu::capture_input.before(super::panel_input),
                    super::controls_menu::sync.after(menu_input),
                ),
            );
        super::register_styles(app);
    }
}
/// Marks the bottom toolbar of menu buttons; `sync_menus` hides it while the
/// pause menu or quit dialog is open, before the match is ready, and while the
/// mouse is captured with no modal open.
#[derive(Component)]
pub(crate) struct Toolbar;
/// Marks the settings panel's full-screen shade; `sync_menus` shows it while
/// the HUD state has settings open.
#[derive(Component)]
pub(crate) struct Settings;
/// Marks a menu button, whose hover makes `menu_input` treat a left click as
/// consumed instead of letting it reach the field.
#[derive(Component)]
pub(crate) struct MenuButton;
/// Marks the aiming-reticle checkbox that `sync_menus` syncs with the HUD
/// state's reticle flag.
#[derive(Component)]
pub(crate) struct ReticleSetting;
/// Marks the minimap checkbox that `sync_menus` syncs with the HUD state's map
/// flag.
#[derive(Component)]
pub(crate) struct MapSetting;
/// Marks the mouse-sensitivity slider, whose 20 to 200 percent range maps onto
/// the HUD state's sensitivity in radians per mouse pixel.
#[derive(Component)]
pub(crate) struct Sensitivity;
#[derive(Clone, Copy)]
enum Action {
    Settings,
    Debug,
    Map,
    Help,
    Roster,
    Close,
    Reset,
    Leave,
    Resume,
    SpawnBot,
    ClearBots,
    PauseSettings,
    CancelQuit,
    ConfirmQuit,
}
/// Queue of menu actions pushed by button observers and drained each frame by
/// `menu_input`.
#[derive(Resource, Default)]
pub(crate) struct MenuActions(Vec<Action>);

fn clear_consumed(mut ui: ResMut<HudState>) {
    ui.consumed = false;
}

fn button(commands: &mut Commands, parent: Entity, title: &'static str, action: Action) -> Entity {
    commands
        .spawn_scene(bsn! {
            @FeathersButton { @caption: bsn! { Text(title) ThemedText } }
        ActivateOnPress
        AccessibleLabel(title)
            Node { height: px(34), padding: UiRect::horizontal(px(14)) }
            on(move |_: On<Activate>, mut actions: ResMut<MenuActions>| { actions.0.push(action); })
        })
        .insert((ChildOf(parent), MenuButton))
        .id()
}
fn spawn_menus(
    mut commands: Commands,
    existing: Query<(), With<Toolbar>>,
    assets: Res<AssetServer>,
    preferences: Res<crate::preferences::Preferences>,
) {
    if !existing.is_empty() {
        return;
    }
    let font: Handle<Font> = assets.load(bevy::feathers::constants::fonts::REGULAR);
    let text_font = |size| TextFont {
        font: font.clone().into(),
        ..super::text_style(size)
    };
    let toolbar = commands
        .spawn((
            Toolbar,
            GlobalZIndex(20),
            Node {
                position_type: PositionType::Absolute,
                bottom: px(14),
                left: percent(50),
                column_gap: px(6),
                ..default()
            },
            UiTransform::from_translation(Val2::new(percent(-50), px(0))),
        ))
        .id();
    for (title, action) in [
        ("Settings", Action::Settings),
        ("Debug", Action::Debug),
        ("Map", Action::Map),
        ("Team", Action::Roster),
        ("Help", Action::Help),
        ("Close panel", Action::Close),
        ("Leave match", Action::Leave),
    ] {
        button(&mut commands, toolbar, title, action);
    }
    spawn_dialog(&mut commands, false);
    spawn_dialog(&mut commands, true);
    let shade = commands
        .spawn((
            Settings,
            GlobalZIndex(200),
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.005, 0.012, 0.018, 0.58)),
        ))
        .id();
    commands.spawn((
        ChildOf(shade),
        Text::new("Scroll to see all settings"),
        Node {
            position_type: PositionType::Absolute,
            bottom: percent(8.),
            ..default()
        },
        text_font(13.),
        TextColor(Color::srgb(0.7, 0.75, 0.8)),
    ));
    let card = commands
        .spawn((
            ChildOf(shade),
            bevy::ui_widgets::ScrollArea,
            Node {
                width: percent(80),
                max_width: px(920),
                max_height: percent(76),
                overflow: Overflow::scroll_y(),
                padding: UiRect::all(px(24)),
                flex_direction: FlexDirection::Column,
                row_gap: px(22),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(8)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.035, 0.05, 0.065)),
            BorderColor::all(Color::srgb(0.19, 0.52, 0.56)),
        ))
        .id();
    commands.spawn((
        ChildOf(card),
        Text::new("SETTINGS"),
        text_font(24.),
        TextColor(super::WHITE),
    ));
    let tabs = commands
        .spawn((
            ChildOf(card),
            Node {
                column_gap: px(10.),
                ..default()
            },
        ))
        .id();
    for (title, tab) in [
        ("Controls", SettingsTab::Controls),
        ("Graphics", SettingsTab::Graphics),
        ("Display", SettingsTab::Display),
        ("Weapon", SettingsTab::Weapon),
    ] {
        commands.spawn_scene(bsn! {
            @FeathersButton { @caption: bsn! { Text(title) ThemedText } }
            ActivateOnPress
            on(move |_: On<Activate>, mut selected: ResMut<SettingsTab>, mut ui: ResMut<HudState>| { *selected = tab; ui.consumed = true; })
        }).insert(ChildOf(tabs));
    }
    let display = section(&mut commands, card, SettingsTab::Display);
    let controls = section(&mut commands, card, SettingsTab::Controls);
    let graphics = section(&mut commands, card, SettingsTab::Graphics);
    let weapon = section(&mut commands, card, SettingsTab::Weapon);
    spawn_weapon_controls(&mut commands, weapon);
    commands.spawn((
        ChildOf(display),
        Text::new("Display"),
        text_font(14.),
        TextColor(super::GREEN),
    ));
    commands.spawn_scene(bsn! {
        @FeathersButton { @caption: bsn! { Text("Network stats: Off / Compact / Detailed") ThemedText } }
        ActivateOnPress
        on(|_: On<Activate>, mut ui: ResMut<HudState>| { ui.network_stats = ui.network_stats.next(); })
    }).insert(ChildOf(display));
    commands.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text("Show aiming reticle") ThemedText } }
        Checked
        AccessibleLabel("Show aiming reticle")
        on(|change: On<ValueChange<bool>>, mut ui: ResMut<HudState>| { ui.show_reticle = change.value; })
    }).insert((ChildOf(display), ReticleSetting));
    commands.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text("Show minimap") ThemedText } }
        Checked
        AccessibleLabel("Show minimap")
        on(|change: On<ValueChange<bool>>, mut ui: ResMut<HudState>| { ui.show_map = change.value; })
    }).insert((ChildOf(display), MapSetting));
    commands.spawn((
        ChildOf(controls),
        Text::new("Mouse sensitivity (%)"),
        text_font(16.),
        TextColor(super::WHITE),
    ));
    commands
        .spawn_scene(bsn! {
            @FeathersSlider { @value: 100.0, @min: 20.0, @max: 200.0 }
            SliderPrecision(0)
            SliderStep(5.0)
            AccessibleLabel("Mouse sensitivity percent")
            Node { width: percent(100), height: px(36), min_height: px(36), flex_shrink: 0.0 }
            on(|change: On<ValueChange<f32>>, mut ui: ResMut<HudState>| {
                ui.sensitivity = change.value.clamp(20., 200.) * 0.000025;
            })
        })
        .insert((ChildOf(controls), Sensitivity));
    super::controls_menu::spawn(&mut commands, controls, shade);
    crate::graphics::spawn_graphics_controls(&mut commands, graphics, &preferences.graphics);
    commands.spawn((ChildOf(card), Text::new("Drive, aim and fire are suspended while a panel is open.\nEsc closes settings. Click the field to resume control."),
        text_font(13.), TextColor(Color::srgb(0.6, 0.7, 0.74))));
    let footer = commands
        .spawn((
            ChildOf(card),
            Node {
                column_gap: px(10),
                min_height: px(34),
                flex_shrink: 0.0,
                ..default()
            },
        ))
        .id();
    button(&mut commands, footer, "Reset all settings", Action::Reset);
    button(&mut commands, footer, "Close", Action::Close);
}

/// Drains the queued menu button actions and applies them to the HUD state.
/// A left click that lands on a menu button is swallowed, so the press cannot
/// fire or capture the mouse in the same frame. Runs before drive, aiming and
/// firing, and sets `consumed` until the next frame.
/// Spawn and clear bot commands reach the session only when it is singleplayer.
#[allow(clippy::too_many_arguments)]
pub fn menu_input(
    mut commands: Commands,
    mut ui: ResMut<HudState>,
    mut actions: ResMut<MenuActions>,
    mut preferences: Option<ResMut<crate::preferences::Preferences>>,
    player: Option<Res<crate::controls::Player>>,
    buttons: Res<ButtonInput<MouseButton>>,
    hovered: Query<&Hovered, With<MenuButton>>,
    mut focus: ResMut<InputFocus>,
) {
    for action in actions.0.drain(..) {
        match action {
            Action::Reset => {
                commands.queue(|world: &mut World| {
                    let Some(mut session) = world.get_resource_mut::<crate::session::Session>()
                    else {
                        return;
                    };
                    let weapon = session.weapon_defaults;
                    if let Err(error) = session.set_weapon(weapon) {
                        session.notice(error);
                        return;
                    }
                    if let Some(mut gun) = world.get_resource_mut::<crate::controls::Gun>() {
                        gun.interval_ns = weapon.interval_ns;
                        gun.shot = weapon.shot;
                    }
                });
                ui.controls = default();
                ui.network_stats = default();
                if let Some(preferences) = preferences.as_mut() {
                    preferences.graphics = default();
                }
                ui.show_map = true;
                ui.show_reticle = true;
                ui.sensitivity = 0.0025;
            }
            Action::SpawnBot | Action::ClearBots => {
                let spawn = matches!(action, Action::SpawnBot);
                commands.queue(move |world: &mut World| {
                    if let Some(mut session) = world.get_resource_mut::<crate::session::Session>()
                        && session.is_singleplayer()
                    {
                        let command = if spawn {
                            rm_simulator_server::protocol::Command::SpawnBot {
                                team: if session.team == rm_simulator_world::Team::Red {
                                    rm_simulator_world::Team::Blue
                                } else {
                                    rm_simulator_world::Team::Red
                                },
                                spin_rad_s: 3.,
                            }
                        } else {
                            rm_simulator_server::protocol::Command::ClearBots
                        };
                        session.apply(command);
                    }
                });
            }
            Action::Resume => ui.close(),
            Action::PauseSettings => {
                ui.settings = true;
            }
            Action::CancelQuit => ui.close(),
            Action::ConfirmQuit => {
                if ui.quit_confirm {
                    commands.queue(|world: &mut World| {
                        world.write_message(AppExit::Success);
                    });
                }
            }
            Action::Close => ui.dismiss(),
            Action::Leave => {
                commands.insert_resource(crate::loading::LeaveRequest(None));
            }
            action => {
                let was_open = match action {
                    Action::Settings => ui.settings,
                    Action::Debug => ui.debug,
                    Action::Map => ui.large_map,
                    Action::Help => ui.help,
                    Action::Roster => ui.roster,
                    _ => true,
                };
                ui.close();
                if !was_open {
                    match action {
                        Action::Settings => ui.settings = true,
                        Action::Debug => ui.debug = true,
                        Action::Map => ui.large_map = true,
                        Action::Help => ui.help = true,
                        Action::Roster => ui.roster = true,
                        _ => {}
                    }
                }
                focus.clear();
            }
        }
        ui.consumed = true;
    }
    if !player.as_ref().is_some_and(|p| p.captured)
        && buttons.pressed(MouseButton::Left)
        && hovered.iter().any(|h| h.0)
    {
        ui.consumed = true;
    }
    if ui.consumed && !ui.modal_open() {
        focus.clear();
    }
}

/// Mirrors the HUD state into the menu nodes: settings shade and toolbar
/// visibility, checkbox ticks and the sensitivity slider value.
/// Writes a checkbox or slider only when its widget value differs, and runs
/// after mouse capture so a capturing click can close a panel in the same frame.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn sync_menus(
    mut commands: Commands,
    ui: Res<HudState>,
    player: Option<Res<crate::controls::Player>>,
    mut panels: Query<&mut Node, (With<Settings>, Without<Toolbar>)>,
    mut toolbar: Query<&mut Node, (With<Toolbar>, Without<Settings>)>,
    ready: Option<Res<crate::loading::Ready>>,
    checks: Query<
        (Entity, Has<Checked>, Has<ReticleSetting>),
        Or<(With<ReticleSetting>, With<MapSetting>)>,
    >,
    sliders: Query<(Entity, &SliderValue), With<Sensitivity>>,
) {
    for mut node in &mut panels {
        node.display = if ui.settings {
            Display::Flex
        } else {
            Display::None
        };
    }
    for mut node in &mut toolbar {
        node.display = if ui.pause_menu
            || ui.quit_confirm
            || ready.is_none()
            || (player.as_ref().is_some_and(|p| p.captured) && !ui.modal_open())
        {
            Display::None
        } else {
            Display::Flex
        };
    }
    for (entity, checked, reticle) in &checks {
        let value = if reticle {
            ui.show_reticle
        } else {
            ui.show_map
        };
        if value != checked {
            if value {
                commands.entity(entity).insert(Checked);
            } else {
                commands.entity(entity).remove::<Checked>();
            }
        }
    }
    for (entity, value) in &sliders {
        let next = ui.sensitivity / 0.000025;
        if (value.0 - next).abs() > 0.01 {
            commands.entity(entity).insert(SliderValue(next));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_actions_switch_panels_and_reset_both_display_and_sensitivity() {
        let mut app = App::new();
        app.init_resource::<HudState>()
            .init_resource::<MenuActions>()
            .init_resource::<InputFocus>()
            .init_resource::<ButtonInput<MouseButton>>()
            .insert_resource(crate::controls::Player::at(Vec3::ZERO, 0., 0.))
            .add_systems(Update, menu_input);
        for action in [Action::Settings, Action::Map, Action::Help, Action::Roster] {
            app.world_mut().resource_mut::<MenuActions>().0.push(action);
            app.update();
            let ui = app.world().resource::<HudState>();
            assert_eq!(
                [ui.settings, ui.large_map, ui.help, ui.roster]
                    .into_iter()
                    .filter(|v| *v)
                    .count(),
                1
            );
            assert!(ui.blocks_input());
            assert!(ui.consumed);
        }
        {
            let mut ui = app.world_mut().resource_mut::<HudState>();
            ui.show_map = false;
            ui.show_reticle = false;
            ui.sensitivity = 0.005;
        }
        app.world_mut()
            .resource_mut::<MenuActions>()
            .0
            .push(Action::Reset);
        app.update();
        let ui = app.world().resource::<HudState>();
        assert!(ui.show_map && ui.show_reticle);
        assert_eq!(ui.sensitivity, 0.0025);
        app.world_mut()
            .resource_mut::<MenuActions>()
            .0
            .push(Action::Close);
        app.update();
        let ui = app.world().resource::<HudState>();
        assert!(!ui.modal_open());
        assert!(ui.blocks_input(), "the close event cannot reach the field");
    }
}

fn window_focus(
    automation: Option<Res<crate::console::ConsoleInputs>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut ui: ResMut<HudState>,
) {
    let captured = automation.is_some_and(|input| input.captured);
    let unfocused = !captured
        && windows
            .iter()
            .any(|window| window.visible && !window.focused);
    if ui.unfocused != unfocused {
        ui.unfocused = unfocused;
        if !captured {
            ui.consumed = true;
        }
    }
}

#[derive(Resource, Clone, Copy, Default, PartialEq, Eq)]
enum SettingsTab {
    #[default]
    Controls,
    Graphics,
    Display,
    Weapon,
}
#[derive(Component)]
struct SettingsSection(SettingsTab);
fn section(commands: &mut Commands, parent: Entity, tab: SettingsTab) -> Entity {
    commands
        .spawn((
            ChildOf(parent),
            SettingsSection(tab),
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(14.),
                ..default()
            },
        ))
        .id()
}
fn sync_sections(selected: Res<SettingsTab>, mut sections: Query<(&SettingsSection, &mut Node)>) {
    for (tab, mut node) in &mut sections {
        let display = if tab.0 == *selected {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;
    #[test]
    fn losing_focus_blocks_controls_until_the_window_returns() {
        let mut app = App::new();
        app.init_resource::<HudState>()
            .add_systems(Update, window_focus);
        let window = app
            .world_mut()
            .spawn((
                Window {
                    focused: false,
                    ..default()
                },
                bevy::window::PrimaryWindow,
            ))
            .id();
        app.update();
        assert!(app.world().resource::<HudState>().blocks_input());
        app.world_mut()
            .entity_mut(window)
            .get_mut::<Window>()
            .unwrap()
            .focused = true;
        app.update();
        assert!(!app.world().resource::<HudState>().unfocused);
        assert!(app.world().resource::<HudState>().consumed);
    }
}

#[cfg(test)]
mod section_tests {
    use super::*;
    #[test]
    fn settings_sections_are_entities_and_only_selected_tab_is_visible() {
        let mut app = App::new();
        app.init_resource::<SettingsTab>()
            .add_systems(Update, sync_sections);
        for tab in [
            SettingsTab::Controls,
            SettingsTab::Display,
            SettingsTab::Graphics,
            SettingsTab::Weapon,
        ] {
            app.world_mut()
                .spawn((SettingsSection(tab), Node::default()));
        }
        app.update();
        for selected in [
            SettingsTab::Graphics,
            SettingsTab::Display,
            SettingsTab::Controls,
            SettingsTab::Weapon,
        ] {
            *app.world_mut().resource_mut::<SettingsTab>() = selected;
            app.update();
            let nodes: Vec<_> = app
                .world_mut()
                .query::<(&SettingsSection, &Node)>()
                .iter(app.world())
                .map(|(tab, node)| (tab.0, node.display))
                .collect();
            assert_eq!(nodes.len(), 4);
            assert_eq!(
                nodes
                    .iter()
                    .filter(|(_, display)| *display == Display::Flex)
                    .count(),
                1
            );
            assert!(
                nodes
                    .iter()
                    .any(|(tab, display)| *tab == selected && *display == Display::Flex)
            );
        }
    }
}

/// Bevy 0.19's ScrollArea observer checks the original target. A wheel event
/// over a button must be retargeted to the containing settings card once.
#[derive(Component)]
struct SettingsScroll;
#[allow(clippy::type_complexity)]
fn forward_settings_scroll(
    mut event: On<Pointer<bevy::picking::events::Scroll>>,
    cards: Query<(), Or<(With<SettingsScroll>, With<super::PanelFrame>)>>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    if cards.contains(event.entity) {
        return;
    }
    if let Some(card) = parents
        .iter_ancestors(event.entity)
        .find(|entity| cards.contains(*entity))
    {
        event.propagate(false);
        commands.trigger(Pointer::new(
            event.pointer_id,
            event.pointer_location.clone(),
            event.event.clone(),
            card,
        ));
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;
    #[test]
    fn wheel_events_bubble_from_rows_and_reach_both_ends_at_720p() {
        use bevy::{
            camera::NormalizedRenderTarget,
            input::{mouse::MouseScrollUnit, touch::TouchPhase},
            picking::{
                backend::HitData,
                events::Scroll,
                pointer::{Location, PointerId},
            },
            ui_widgets::{ScrollArea, ScrollAreaPlugin},
        };
        let mut app = App::new();
        // DefaultPlugins installs this same driver through UiWidgetsPlugins.
        app.add_plugins(ScrollAreaPlugin)
            .add_observer(forward_settings_scroll);
        let visible_height = 720. * 0.76;
        let panel = app
            .world_mut()
            .spawn((
                ScrollArea,
                SettingsScroll,
                Node {
                    overflow: Overflow::scroll_y(),
                    ..default()
                },
                ComputedNode {
                    size: Vec2::new(920., visible_height),
                    content_size: Vec2::new(920., 1800.),
                    inverse_scale_factor: 1.,
                    ..default()
                },
            ))
            .id();
        let row = app.world_mut().spawn(ChildOf(panel)).id();
        app.update();
        for (target, delta, expected) in [
            (row, -100., 100.),
            (panel, -50., 150.),
            (row, -10000., 1800. - visible_height),
            (row, 10000., 0.),
        ] {
            app.world_mut().trigger(Pointer::new(
                PointerId::Mouse,
                Location {
                    target: NormalizedRenderTarget::None {
                        width: 1280,
                        height: 720,
                    },
                    position: Vec2::new(500., 500.),
                },
                Scroll {
                    unit: MouseScrollUnit::Pixel,
                    x: 0.,
                    y: delta,
                    hit: HitData::new(Entity::PLACEHOLDER, 0., None, None),
                    phase: TouchPhase::Moved,
                },
                target,
            ));
            app.world_mut().flush();
            assert!(
                (app.world().get::<ScrollPosition>(panel).unwrap().y - expected).abs() < 0.01,
                "got {} expected {expected}",
                app.world().get::<ScrollPosition>(panel).unwrap().y
            );
        }
    }
}

#[derive(Component)]
struct EscapeDialog {
    quit: bool,
}

fn spawn_dialog(commands: &mut Commands, quit: bool) {
    let shade = commands
        .spawn((
            EscapeDialog { quit },
            GlobalZIndex(210),
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.005, 0.012, 0.018, 0.75)),
        ))
        .id();
    let card = commands
        .spawn((
            ChildOf(shade),
            Node {
                width: px(380),
                max_width: percent(90),
                padding: UiRect::all(px(28)),
                flex_direction: FlexDirection::Column,
                row_gap: px(16),
                border_radius: BorderRadius::all(px(8)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.035, 0.05, 0.065)),
        ))
        .id();
    commands.spawn((
        ChildOf(card),
        Text::new(if quit { "Quit the simulator?" } else { "Pause" }),
        TextFont {
            font_size: 26.0.into(),
            ..default()
        },
    ));
    commands.spawn((
        ChildOf(card),
        Text::new(if quit {
            "Are you sure you want to quit?"
        } else {
            "Singleplayer pauses here. Multiplayer keeps running."
        }),
        TextFont {
            font_size: 16.0.into(),
            ..default()
        },
    ));
    if quit {
        button(commands, card, "Cancel", Action::CancelQuit);
        button(commands, card, "Quit", Action::ConfirmQuit);
    } else {
        button(commands, card, "Resume", Action::Resume);
        button(commands, card, "Settings", Action::PauseSettings);
        let spawn = button(commands, card, "Spawn spinning enemy", Action::SpawnBot);
        let clear = button(commands, card, "Remove bots", Action::ClearBots);
        commands.entity(spawn).insert(SingleplayerBotButton);
        commands.entity(clear).insert(SingleplayerBotButton);
        button(commands, card, "Exit Match", Action::Leave);
    }
}

#[derive(Component)]
struct SingleplayerBotButton;
fn sync_dialogs(
    ui: Res<HudState>,
    session: Option<Res<crate::session::Session>>,
    mut dialogs: Query<(&EscapeDialog, &mut Node), Without<SingleplayerBotButton>>,
    mut bot_buttons: Query<&mut Node, With<SingleplayerBotButton>>,
) {
    for mut node in &mut bot_buttons {
        node.display = if session.as_ref().is_some_and(|s| s.is_singleplayer()) {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (dialog, mut node) in &mut dialogs {
        let visible = if dialog.quit {
            ui.quit_confirm
        } else {
            ui.pause_menu && !ui.settings
        };
        node.display = if visible {
            Display::Flex
        } else {
            Display::None
        };
    }
}

/// Restore only the pause introduced by this menu; preserve an existing referee pause.
fn pause_singleplayer(
    ui: Res<HudState>,
    session: Option<ResMut<crate::session::Session>>,
    mut resume_on_close: Local<bool>,
) {
    let Some(mut session) = session else {
        *resume_on_close = false;
        return;
    };
    if !session.is_singleplayer() {
        *resume_on_close = false;
        return;
    }
    if ui.pause_menu && !*resume_on_close && !session.paused {
        session.apply(rm_simulator_server::protocol::Command::Pause { paused: true });
        *resume_on_close = true;
    } else if !ui.pause_menu && *resume_on_close {
        session.apply(rm_simulator_server::protocol::Command::Pause { paused: false });
        *resume_on_close = false;
    }
}

#[cfg(test)]
mod escape_tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<HudState>()
            .init_resource::<MenuActions>()
            .init_resource::<InputFocus>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_message::<AppExit>()
            .add_systems(PreUpdate, clear_consumed)
            .add_systems(
                Update,
                (super::super::panel_input, menu_input, pause_singleplayer).chain(),
            );
        app
    }
    fn escape(app: &mut App) {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.reset_all();
        keys.press(KeyCode::Escape);
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
    }
    fn confirmed(app: &mut App) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let mut session = app.world_mut().resource_mut::<crate::session::Session>();
            session.poll().unwrap();
            if session.commands_confirmed() {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    #[test]
    fn pause_settings_back_and_resume_preserve_prior_pause() {
        for initially_paused in [false, true] {
            let mut app = app();
            let session = crate::session::test_session(initially_paused);
            session.ready();
            app.insert_resource(session);
            escape(&mut app);
            confirmed(&mut app);
            assert!(app.world().resource::<HudState>().pause_menu);
            assert!(app.world().resource::<crate::session::Session>().paused);
            app.world_mut()
                .resource_mut::<MenuActions>()
                .0
                .push(Action::PauseSettings);
            app.update();
            assert!(app.world().resource::<HudState>().settings);
            escape(&mut app);
            assert!(app.world().resource::<HudState>().pause_menu);
            assert!(!app.world().resource::<HudState>().settings);
            escape(&mut app);
            confirmed(&mut app);
            assert!(!app.world().resource::<HudState>().pause_menu);
            assert_eq!(
                app.world().resource::<crate::session::Session>().paused,
                initially_paused
            );
            assert!(
                !app.world()
                    .contains_resource::<crate::loading::LeaveRequest>()
            );
        }
    }
    #[test]
    fn hosting_multiplayer_does_not_pause() {
        use clap::Parser;
        let args = crate::args::Args::try_parse_from([
            "rm-simulator",
            "--listen",
            "127.0.0.1:0",
            "--no-field-collision",
            "--no-rune",
            "--no-referee",
        ])
        .unwrap();
        let session = crate::session::Session::open(
            &args,
            &crate::session::test_cad_assets(),
            [2., 3., 1.],
            90.,
            |_, _| {},
        )
        .unwrap()
        .session;
        session.ready();
        assert!(!session.is_remote());
        assert!(!session.is_singleplayer());
        let mut app = app();
        app.insert_resource(session);
        escape(&mut app);
        confirmed(&mut app);
        assert!(app.world().resource::<HudState>().pause_menu);
        assert!(!app.world().resource::<crate::session::Session>().paused);
    }
    #[test]
    fn homepage_escape_requires_explicit_quit_and_can_cancel() {
        let mut app = app();
        app.init_resource::<crate::title::TitleScreen>();
        escape(&mut app);
        assert!(app.world().resource::<HudState>().quit_confirm);
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
        escape(&mut app);
        assert!(!app.world().resource::<HudState>().quit_confirm);
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
        escape(&mut app);
        app.world_mut()
            .resource_mut::<MenuActions>()
            .0
            .push(Action::ConfirmQuit);
        app.update();
        assert_eq!(app.world().resource::<Messages<AppExit>>().len(), 1);
    }
}

/// Numeric weapon controls use host limits from the active session.
#[derive(Component, Clone, Copy)]
enum WeaponSlider {
    Rate,
    Speed,
    SpeedVariation,
    Spread,
}
#[derive(Component)]
struct WeaponSummary;
#[derive(Component)]
struct GaussianSpread;
fn spawn_weapon_controls(commands: &mut Commands, parent: Entity) {
    commands.spawn((
        ChildOf(parent),
        WeaponSummary,
        Text::new("Join a match to adjust your weapon. Host limits are on the title screen."),
        super::text_style(14.),
    ));
    for (label, kind, min, max, value) in [
        (
            "Fire rate (Hz)",
            WeaponSlider::Rate,
            0.1_f32,
            30_f32,
            30_f32,
        ),
        (
            "Muzzle speed (m/s)",
            WeaponSlider::Speed,
            0.1_f32,
            30_f32,
            30_f32,
        ),
        (
            "Muzzle speed variation (+/- m/s)",
            WeaponSlider::SpeedVariation,
            0_f32,
            1_f32,
            0_f32,
        ),
        (
            "Spread half-angle (degrees)",
            WeaponSlider::Spread,
            0_f32,
            2_f32,
            0_f32,
        ),
    ] {
        let step = if matches!(kind, WeaponSlider::Spread | WeaponSlider::SpeedVariation) {
            0.01_f32
        } else {
            0.1_f32
        };
        commands.spawn((ChildOf(parent), Text::new(label), super::text_style(16.)));
        commands.spawn_scene(bsn! {
            @FeathersSlider { @value: value, @min: min, @max: max }
            SliderPrecision(2)
            SliderStep(step)
            AccessibleLabel(label)
            Node { width: percent(100), height: px(36), min_height: px(36), flex_shrink: 0.0 }
            on(move |change: On<ValueChange<f32>>, session: Option<ResMut<crate::session::Session>>, gun: Option<ResMut<crate::controls::Gun>>| {
                let Some(mut session) = session else { return; };
                let mut weapon = session.weapon;
                let value = f64::from(change.value);
                match kind {
                    WeaponSlider::Rate => weapon.interval_ns = ((1e9 / value.max(0.1)).round() as u64).max(session.weapon_limits.min_interval_ns),
                    WeaponSlider::Speed => weapon.shot.speed_m_s = value.max(0.001).min(session.weapon_limits.max_speed_m_s),
                    WeaponSlider::SpeedVariation => weapon.speed_variation_m_s = (value.clamp(0., 1.) * 100.).round() / 100.,
                    WeaponSlider::Spread => weapon.spread.angle_rad = ((value.clamp(0., 2.) * 100.).round() / 100.).to_radians(),
                }
                if session.weapon_limits.admit(session.weapon_defaults.shot.caliber, weapon).is_ok() {
                    if let Err(error) = session.set_weapon(weapon) { session.notice(error); return; }
                    if let Some(mut gun) = gun { gun.interval_ns = weapon.interval_ns; gun.shot = weapon.shot; }
                }
            })
        }).insert((ChildOf(parent), kind));
    }
    commands.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text("Gaussian (normal) angular spread; unchecked = uniform cone") ThemedText } }
        AccessibleLabel("Gaussian spread")
        on(|change: On<ValueChange<bool>>, session: Option<ResMut<crate::session::Session>>| {
            if let Some(mut session) = session {
                let mut weapon = session.weapon;
                weapon.spread.distribution = if change.value {
                    rm_simulator_server::protocol::SpreadDistribution::Gaussian
                } else { rm_simulator_server::protocol::SpreadDistribution::Uniform };
                if let Err(error) = session.set_weapon(weapon) { session.notice(error); }
            }
        })
    }).insert((ChildOf(parent), GaussianSpread));
    commands.spawn((ChildOf(parent), Text::new("Zero spread is perfectly accurate. 2 degrees spans about 35 cm at 5 m. The angle is the cone radius; Gaussian spread is truncated at three standard deviations. Speed variation is Gaussian, truncated at the selected +/- range and host speed cap. Changes apply to new shots."), super::text_style(14.)));
    commands.spawn_scene(bsn! {
        @FeathersButton { @caption: bsn! { Text("Next spread seed") ThemedText } }
        ActivateOnPress
        on(|_: On<Activate>, session: Option<ResMut<crate::session::Session>>| {
            if let Some(mut session) = session {
                let mut weapon = session.weapon; weapon.spread.seed = weapon.spread.seed.wrapping_add(1);
                if let Err(error) = session.set_weapon(weapon) { session.notice(error); }
            }
        })
    }).insert(ChildOf(parent));
    commands.spawn_scene(bsn! {
        @FeathersButton { @caption: bsn! { Text("Restore host weapon defaults") ThemedText } }
        ActivateOnPress
        on(|_: On<Activate>, session: Option<ResMut<crate::session::Session>>, gun: Option<ResMut<crate::controls::Gun>>| {
            if let Some(mut session) = session {
                let weapon = session.weapon_defaults;
                if let Err(error) = session.set_weapon(weapon) { session.notice(error); return; }
                if let Some(mut gun) = gun { gun.interval_ns = session.weapon.interval_ns; gun.shot = session.weapon.shot; }
            }
        })
    }).insert(ChildOf(parent));
}
fn sync_weapon_controls(
    mut commands: Commands,
    session: Option<Res<crate::session::Session>>,
    mut summaries: Query<&mut Text, With<WeaponSummary>>,
    sliders: Query<(
        Entity,
        &WeaponSlider,
        &SliderValue,
        &bevy::ui_widgets::SliderRange,
    )>,
    gaussian: Query<(Entity, Has<Checked>), With<GaussianSpread>>,
) {
    let Some(session) = session else {
        return;
    };
    let weapon = session.weapon;
    let limits = session.weapon_limits;
    let summary = format!(
        "Host limits: {:.2} Hz, {:.2} m/s. Caliber: {} mm. Spread seed: {}.",
        1e9 / limits.min_interval_ns as f64,
        limits.max_speed_m_s,
        if weapon.shot.caliber == rm_simulator_world::Caliber::Mm17 {
            17
        } else {
            42
        },
        weapon.spread.seed
    );
    for mut text in &mut summaries {
        if text.0 != summary {
            text.0.clone_from(&summary);
        }
    }
    for (entity, kind, value, range) in &sliders {
        let (next, min, max) = match kind {
            WeaponSlider::Rate => (
                1e9 / weapon.interval_ns as f64,
                0.1,
                1e9 / limits.min_interval_ns as f64,
            ),
            WeaponSlider::Speed => (
                weapon.shot.speed_m_s,
                limits.max_speed_m_s.min(0.1),
                limits.max_speed_m_s,
            ),
            WeaponSlider::SpeedVariation => (weapon.speed_variation_m_s, 0., 1.),
            WeaponSlider::Spread => (weapon.spread.angle_rad.to_degrees(), 0., 2.),
        };
        if value.0 != next as f32 {
            commands.entity(entity).insert(SliderValue(next as f32));
        }
        if range.start() != min as f32 || range.end() != max as f32 {
            commands
                .entity(entity)
                .insert(bevy::ui_widgets::SliderRange::new(min as f32, max as f32));
        }
    }
    let enabled =
        weapon.spread.distribution == rm_simulator_server::protocol::SpreadDistribution::Gaussian;
    for (entity, checked) in &gaussian {
        if enabled && !checked {
            commands.entity(entity).insert(Checked);
        }
        if !enabled && checked {
            commands.entity(entity).remove::<Checked>();
        }
    }
}
