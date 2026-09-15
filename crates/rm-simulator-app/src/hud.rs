// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Simplified competitor display, following the July 2026 student client
//! manual, main interface and panels 1, 3, 5 and 6. Only live simulator data
//! is shown; the map is a schematic, not a radar detection model.
use crate::bindings::InputAction;
use bevy::prelude::*;
use bevy_flair::prelude::{ClassList, Styled};

mod controls_menu;
mod menus;
mod team_status;
/// The HUD plugin and the two input systems, re-exported from `menus` so
/// `main.rs` wires the overlay through one `hud` path.
pub use menus::{HudPlugin, menu_input, sync_menus};
/// Markers for the settings shade and the toolbar, so match teardown can despawn
/// both without naming the menus module.
pub(crate) use menus::{Settings, Toolbar};
use rm_simulator_world::{Caliber, MatchPhase, Team};

use crate::controls::{Drive, Gun, Player};
use crate::session::Session;

const INK: Color = Color::srgba(0.025, 0.045, 0.065, 0.88);
const WHITE: Color = Color::srgb(0.88, 0.94, 0.96);
const GREEN: Color = Color::srgb(0.25, 0.95, 0.65);
fn team_color(team: Team) -> Color {
    match team {
        Team::Red => Color::srgb(0.95, 0.22, 0.22),
        Team::Blue => Color::srgb(0.18, 0.65, 1.0),
    }
}

/// Overlay panel state and display preferences shared by every HUD system.
#[derive(Resource)]
pub struct HudState {
    /// Key and mouse bindings used for every gameplay action.
    pub controls: crate::bindings::ControlsSettings,
    /// A binding capture is running and owns the keyboard.
    pub rebinding: bool,
    /// A visible window lost focus, so gameplay input is suspended.
    pub unfocused: bool,
    /// Which network diagnostic overlay mode is shown.
    pub network_stats: crate::network_hud::NetworkStatsMode,
    /// The settings panel is open.
    pub settings: bool,
    /// The pause menu is open.
    pub pause_menu: bool,
    /// The quit confirmation dialog is open.
    pub quit_confirm: bool,
    /// The respawn prompt is up after the local robot was defeated.
    pub respawn: bool,
    /// The debug options panel is open.
    pub debug: bool,
    /// The controls help panel is pinned open.
    pub help: bool,
    /// The team status panel is pinned open.
    pub roster: bool,
    /// Swallow the event that closes a menu until the next frame.
    pub consumed: bool,
    /// The field map is expanded to its large layout.
    pub large_map: bool,
    /// Draw the field map while it is not expanded.
    pub show_map: bool,
    /// Draw the aiming reticle.
    pub show_reticle: bool,
    /// Horizontal aim sensitivity in radians of yaw per pixel of mouse motion.
    pub sensitivity: f32,
    /// Escape consumed by a panel must not also quit or release capture.
    pub closed_with_escape: bool,
}
impl Default for HudState {
    fn default() -> Self {
        Self {
            controls: default(),
            rebinding: false,
            unfocused: false,
            network_stats: Default::default(),
            settings: false,
            pause_menu: false,
            quit_confirm: false,
            respawn: false,
            debug: false,
            help: false,
            roster: false,
            consumed: false,
            large_map: false,
            show_map: true,
            show_reticle: true,
            sensitivity: 0.0025,
            closed_with_escape: false,
        }
    }
}
impl HudState {
    /// True while an open panel or a running capture must swallow gameplay input.
    pub fn blocks_input(&self) -> bool {
        self.unfocused
            || self.rebinding
            || self.respawn
            || self.debug
            || self.pause_menu
            || self.quit_confirm
            || self.settings
            || self.large_map
            || self.help
            || self.roster
            || self.closed_with_escape
            || self.consumed
    }
}

/// Keyboard shortcuts remain available alongside the mouse controls.
pub fn panel_input(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    title: Option<Res<crate::title::TitleScreen>>,
    mut title_state: Option<ResMut<crate::title::TitleState>>,
    mut ui: ResMut<HudState>,
) {
    if ui.rebinding || ui.consumed {
        return;
    }
    ui.closed_with_escape = false;
    if keys.just_pressed(KeyCode::Escape) {
        if ui.modal_open() {
            ui.dismiss();
        } else if title.is_some() {
            if !title_state
                .as_mut()
                .is_some_and(|state| state.escape_back())
            {
                ui.quit_confirm = true;
            }
        } else {
            ui.pause_menu = true;
        }
        ui.closed_with_escape = true;
        return;
    }
    if title.is_some() || ui.pause_menu {
        return;
    }
    if ui
        .controls
        .just_pressed(InputAction::Debug, &keys, buttons.as_deref())
    {
        let open = !ui.debug;
        ui.close();
        ui.debug = open;
    }
    if ui
        .controls
        .just_pressed(InputAction::Settings, &keys, buttons.as_deref())
    {
        let open = !ui.settings;
        ui.close();
        ui.settings = open;
    }
    if ui
        .controls
        .just_pressed(InputAction::Map, &keys, buttons.as_deref())
    {
        let open = !ui.large_map;
        ui.close();
        ui.large_map = open;
    }
    if ui.settings {
        if keys.just_pressed(KeyCode::Digit1) {
            ui.show_reticle = !ui.show_reticle;
        }
        if keys.just_pressed(KeyCode::Digit2) {
            ui.show_map = !ui.show_map;
        }
        if keys.just_pressed(KeyCode::Minus) {
            ui.sensitivity = (ui.sensitivity - 0.00025).max(0.0005);
        }
        if keys.just_pressed(KeyCode::Equal) {
            ui.sensitivity = (ui.sensitivity + 0.00025).min(0.005);
        }
    }
}
impl HudState {
    /// True while a panel is open that Escape closes before the match is paused.
    pub fn modal_open(&self) -> bool {
        self.pause_menu
            || self.quit_confirm
            || self.respawn
            || self.debug
            || self.settings
            || self.large_map
            || self.help
            || self.roster
    }
    /// Settings opened from Pause return to their parent menu.
    pub fn dismiss(&mut self) {
        if self.pause_menu && self.settings {
            self.settings = false;
            self.consumed = true;
        } else {
            self.close();
        }
    }
    /// Close every dismissible panel and swallow this frame's remaining input.
    /// The respawn prompt follows defeat instead and stays open.
    pub fn close(&mut self) {
        self.pause_menu = false;
        self.quit_confirm = false;
        self.debug = false;
        self.settings = false;
        self.large_map = false;
        self.help = false;
        self.roster = false;
        self.consumed = true;
    }
}

/// Marker for one text node that `update_hud` rewrites from the session each frame.
#[derive(Component, Clone, Copy)]
pub enum Hud {
    /// Transient connection toast, empty while the link is healthy.
    Connection,
    /// Score card for one team: base HP, shield and outpost HP.
    Team(Team),
    /// Match clock card: phase and remaining time as mm:ss.
    Clock,
    /// Auto-aim status line.
    AutoAim,
    /// Local robot card: kind, HP, speed and drive mode.
    Robot,
    /// Shot speed, caliber and remaining ammo.
    Ammo,
    /// Newest match notices, up to three lines.
    Notice,
    /// Contextual hint, such as the click-to-control prompt.
    Hint,
    /// Title of the held panel, either CONTROLS or TEAM STATUS.
    Panel,
    /// Caption above the field map.
    MapLabel,
}
/// Fill node whose width tracks the local robot's HP fraction.
#[derive(Component)]
pub struct HealthBar;
/// Aiming reticle root, hidden while a panel owns the input.
#[derive(Component)]
pub struct Reticle;
/// Field map frame, which also carries the minimap artwork image once loaded.
#[derive(Component)]
pub struct FieldMap;
/// Last three-column rows pushed into the peek panel, so they are rebuilt only on change.
#[derive(Component, Default)]
pub struct PanelRows(Vec<[String; 3]>);
/// Marker for the hold-to-peek panel frame that `update_panel_rows` shows and hides.
#[derive(Component)]
pub(crate) struct PanelFrame;
/// One labelled marker on the field map, keyed by the item it stands for.
#[derive(Component)]
pub struct MapDot(MapItem);
#[derive(Clone, Copy, PartialEq)]
enum MapItem {
    Robot(u32),
    Outpost(usize),
    Rune(usize),
}

/// Root node of the whole HUD overlay; every HUD widget is spawned under it.
#[derive(Component)]
pub(crate) struct HudRoot;

fn register_styles(app: &mut App) {
    bevy::asset::embedded_asset!(app, "hud.css");
}

fn text_style(size: f32) -> TextFont {
    TextFont {
        font_size: bevy::text::FontSize::Px(size),
        ..default()
    }
}
fn label(commands: &mut Commands, parent: Entity, kind: Hud, class: &str) {
    let mut entity = commands.spawn((
        kind,
        Text::new(""),
        TextLayout::justify(if matches!(kind, Hud::Clock | Hud::Hint | Hud::AutoAim) {
            Justify::Center
        } else {
            Justify::Left
        }),
        Node::default(),
        TextColor(WHITE),
        text_style(16.),
        ClassList::new(class),
        Pickable::IGNORE,
        ChildOf(parent),
    ));
    if matches!(kind, Hud::Panel) {
        entity.insert((bevy::ui_widgets::ScrollArea, Pickable::default()));
    }
}
fn container(commands: &mut Commands, parent: Entity, class: &str) -> Entity {
    commands
        .spawn((
            Node::default(),
            ClassList::new(class),
            Pickable::IGNORE,
            ChildOf(parent),
        ))
        .id()
}
fn bar(commands: &mut Commands, parent: Entity, marker: impl Bundle, color: Color) {
    let track = container(commands, parent, "bar-track");
    commands.spawn((
        marker,
        Node {
            width: percent(100),
            height: percent(100),
            ..default()
        },
        BackgroundColor(color),
        Pickable::IGNORE,
        ChildOf(track),
    ));
}

/// Spawn the HUD root, its embedded stylesheet and every node the HUD systems update.
pub fn spawn_hud(commands: &mut Commands) {
    let root = commands
        .spawn((
            HudRoot,
            Node::default(),
            ClassList::new("hud"),
            Pickable::IGNORE,
            GlobalZIndex(1),
        ))
        .id();
    // The stylesheet is embedded so --cad-assets never changes UI asset lookup.
    commands.queue(move |world: &mut World| {
        if let Some(assets) = world.get_resource::<AssetServer>() {
            let style = assets.load("embedded://rm_simulator/hud.css");
            world.entity_mut(root).insert(Styled::new(style));
        }
    });
    let top = container(commands, root, "scoreboard");
    for team in [Team::Red, Team::Blue] {
        if team == Team::Blue {
            let clock = container(commands, top, "clock-card");
            label(commands, clock, Hud::Clock, "clock");
        }
        let card = container(
            commands,
            top,
            if team == Team::Red {
                "team-card red"
            } else {
                "team-card blue"
            },
        );
        label(commands, card, Hud::Team(team), "team-status");
        team_status::spawn(commands, card, team);
    }
    let bottom = container(commands, root, "robot-card");
    label(commands, bottom, Hud::Robot, "robot-status");
    bar(commands, bottom, HealthBar, GREEN);
    label(commands, root, Hud::Ammo, "ammo");
    label(commands, root, Hud::AutoAim, "auto-aim");
    label(commands, root, Hud::Notice, "notices");
    label(commands, root, Hud::Connection, "connection-toast");
    label(commands, root, Hud::Hint, "hint");
    // Hold-to-peek panels retain their keyboard behavior.
    let panel = container(commands, root, "peek-panel");
    commands.entity(panel).insert((
        PanelFrame,
        GlobalZIndex(10),
        bevy::ui_widgets::ScrollArea,
        Pickable::default(),
    ));
    label(commands, panel, Hud::Panel, "panel-title");
    let rows = container(commands, panel, "panel-rows");
    commands.entity(rows).insert(PanelRows::default());
    commands
        .spawn((
            Reticle,
            ChildOf(root),
            Pickable::IGNORE,
            Node {
                position_type: PositionType::Absolute,
                left: percent(50.),
                top: percent(50.),
                width: px(0),
                height: px(0),
                ..default()
            },
        ))
        .with_children(|p| {
            for (x, y, w, h) in [
                (-13., 0., 8., 1.),
                (6., 0., 8., 1.),
                (0., -13., 1., 8.),
                (0., 6., 1., 8.),
            ] {
                p.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(x),
                        top: px(y),
                        width: px(w),
                        height: px(h),
                        ..default()
                    },
                    BackgroundColor(WHITE),
                ));
            }
            p.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: px(-34),
                    top: px(-34),
                    width: px(68),
                    height: px(68),
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(34)),
                    ..default()
                },
                BorderColor::all(Color::srgba(0.8, 0.9, 1., 0.25)),
            ));
        });
    commands
        .spawn((
            FieldMap,
            ChildOf(root),
            GlobalZIndex(3),
            ClassList::new("field-map"),
            Node {
                position_type: PositionType::Absolute,
                right: percent(2.),
                bottom: percent(9.),
                width: percent(23.),
                height: percent(24.),
                border: UiRect::all(px(1)),
                ..default()
            },
            BackgroundColor(INK),
            BorderColor::all(Color::srgba(0.6, 0.75, 0.8, 0.5)),
        ))
        .observe(
            |_: On<Pointer<Press>>, player: Res<Player>, mut ui: ResMut<HudState>| {
                if !player.captured && !ui.modal_open() {
                    ui.close();
                    ui.large_map = true;
                }
            },
        )
        .with_children(|p| {
            p.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: percent(50.),
                    width: px(1),
                    height: percent(100.),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.6, 0.75, 0.8, 0.25)),
            ));
            p.spawn((
                Hud::MapLabel,
                ClassList::new("map-label"),
                Text::new("FIELD / TEAM POSITIONS"),
                text_style(12.),
                TextColor(WHITE),
                Node {
                    position_type: PositionType::Absolute,
                    left: px(6),
                    top: px(-32),
                    ..default()
                },
            ));
        });
}

fn clock_text(session: &Session) -> String {
    let Some(r) = session.referee() else {
        return "PRACTICE".into();
    };
    let phase = if session.paused {
        "PAUSED"
    } else {
        match r.phase {
            MatchPhase::Idle => "READY",
            MatchPhase::Countdown => "COUNTDOWN",
            MatchPhase::Running => "ROUND 1",
            MatchPhase::Finished => "FINISHED",
        }
    };
    let elapsed_ns = if !session.paused && r.phase == MatchPhase::Running {
        session
            .presentation_time_ns()
            .saturating_sub(session.snapshot.time_ns)
    } else {
        0
    };
    let seconds = r
        .remaining_ns
        .saturating_sub(elapsed_ns)
        .div_ceil(1_000_000_000);
    format!("{phase}\n{:02}:{:02}", seconds / 60, seconds % 60)
}

fn team_text(session: &Session, team: Team) -> String {
    let mut text = team.name().to_uppercase();
    if let Some(base) = session
        .snapshot
        .bases
        .iter()
        .find(|b| b.config.team == team)
    {
        text.push_str(&format!("   BASE {} +{}", base.hp, base.shield_hp));
    }
    if let Some(r) = session.referee() {
        let outpost: u32 = session
            .snapshot
            .outposts
            .iter()
            .zip(&r.outpost_teams)
            .filter(|(_, owner)| **owner == team)
            .map(|(o, _)| o.hp)
            .sum();
        text.push_str(&format!("   OUTPOST {outpost}"));
    }
    text
}

fn panel_text(
    _session: &Session,
    ui: &HudState,
    keys: &ButtonInput<KeyCode>,
    buttons: Option<&ButtonInput<MouseButton>>,
) -> String {
    if ui.pause_menu || ui.quit_confirm || ui.debug || ui.settings || ui.large_map {
        String::new()
    } else if ui.help || ui.controls.pressed(InputAction::Help, keys, buttons) {
        "CONTROLS".into()
    } else if ui.roster || ui.controls.pressed(InputAction::Roster, keys, buttons) {
        "TEAM STATUS".into()
    } else {
        String::new()
    }
}

/// Build real rows instead of using spaces to align a single text block.
#[allow(clippy::too_many_arguments)]
pub fn update_panel_rows(
    mut commands: Commands,
    session: Res<Session>,
    ui: Res<HudState>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    mut frames: Query<(&mut Node, &mut ScrollPosition), With<PanelFrame>>,
    mut previous_title: Local<String>,
    mut panels: Query<(Entity, &mut PanelRows)>,
) {
    let title = panel_text(&session, &ui, &keys, buttons.as_deref());
    for (mut node, mut scroll) in &mut frames {
        if *previous_title != title {
            scroll.0 = Vec2::ZERO;
        }
        node.display = if title.is_empty() {
            Display::None
        } else {
            Display::Flex
        };
    }
    previous_title.clone_from(&title);
    let mut rows: Vec<[String; 3]> = Vec::new();
    if title == "CONTROLS" {
        rows.push([
            "Mouse".into(),
            "Aim".into(),
            "Click field to capture".into(),
        ]);
        rows.push([
            "Esc".into(),
            "Pause menu / back".into(),
            "Singleplayer pauses".into(),
        ]);
        for action in InputAction::ALL {
            if !session.referees()
                && matches!(
                    action,
                    InputAction::Pause | InputAction::Step | InputAction::Start | InputAction::Rune
                )
            {
                continue;
            }
            rows.push([
                ui.controls.label(action),
                action.label().into(),
                String::new(),
            ]);
        }
    } else if title == "TEAM STATUS" {
        rows.push(["PLAYER".into(), "ROLE / ROBOT".into(), "HEALTH".into()]);
        for team in [Some(Team::Red), Some(Team::Blue), None] {
            if team.is_none() && !session.roster.iter().any(|p| p.team.is_none()) {
                continue;
            }
            rows.push([
                team.map_or("SPECTATORS / REFEREE".into(), |t| t.name().to_uppercase()),
                String::new(),
                String::new(),
            ]);
            for p in session.roster.iter().filter(|p| p.team == team) {
                let robot = session
                    .referee()
                    .and_then(|r| r.robots.iter().find(|r| Some(r.id) == p.chassis));
                rows.push([
                    format!(
                        "{}{}",
                        p.name,
                        if p.client_id == session.client_id {
                            " (you)"
                        } else {
                            ""
                        }
                    ),
                    robot.map_or_else(
                        || p.role.name().to_owned(),
                        |r| match p.robot {
                            Some(robot) => format!("#{} {}", r.id, robot.name()),
                            None => format!("#{} {:?}", r.id, r.kind),
                        },
                    ),
                    robot.map_or_else(|| "--".into(), |r| format!("{} / {} HP", r.hp, r.max_hp)),
                ]);
            }
            // Roster and world updates travel separately. Keep every live robot
            // visible even before its player's name arrives.
            for chassis in session.snapshot.chassis.iter().filter(|c| {
                Some(c.team) == team && !session.roster.iter().any(|p| p.chassis == Some(c.id))
            }) {
                let robot = session
                    .referee()
                    .and_then(|r| r.robots.iter().find(|r| r.id == chassis.id));
                rows.push([
                    format!("Robot #{}", chassis.id),
                    robot.map_or_else(|| "Pilot".into(), |r| format!("#{} {:?}", r.id, r.kind)),
                    robot.map_or_else(|| "--".into(), |r| format!("{} / {} HP", r.hp, r.max_hp)),
                ]);
            }
        }
    }
    for (entity, mut previous) in &mut panels {
        if previous.0 == rows {
            continue;
        }
        previous.0.clone_from(&rows);
        commands.entity(entity).despawn_children();
        for row in &rows {
            let parent = container(&mut commands, entity, "panel-row");
            for (i, value) in row.iter().enumerate() {
                commands.spawn((
                    ChildOf(parent),
                    Text::new(value),
                    Node::default(),
                    text_style(15.),
                    TextColor(WHITE),
                    Pickable::IGNORE,
                    ClassList::new(if i == 0 { "row-key" } else { "row-value" }),
                ));
            }
        }
    }
}

/// Rewrite the overlay text, the health bar and the reticle from the latest snapshot.
/// Runs after panel input so a panel opened this frame is already reflected.
// Bevy system parameters keep disjoint UI queries explicit.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn update_hud(
    session: Res<Session>,
    player: Res<Player>,
    gun: Res<Gun>,
    drive: Option<Res<Drive>>,
    ui: Res<HudState>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    mut texts: Query<(&Hud, &mut Text, &mut Node)>,
    mut health: Query<(&mut Node, &mut BackgroundColor), (With<HealthBar>, Without<Hud>)>,
    mut reticle: Query<&mut Visibility, With<Reticle>>,
    assist: Option<Res<crate::auto_aim::AutoAim>>,
) {
    let robot = session
        .referee()
        .and_then(|r| r.robots.iter().find(|r| Some(r.id) == session.chassis_id));
    let ammo = session.referee().and_then(|r| {
        r.gameplay
            .robots
            .iter()
            .find(|r| Some(r.id) == session.chassis_id)
    });
    for (kind, mut text, mut node) in &mut texts {
        let value = match kind {
            Hud::AutoAim => assist
                .as_ref()
                .map_or_else(String::new, |a| a.status.clone()),
            Hud::Team(team) => team_text(&session, *team),
            Hud::Clock => clock_text(&session),
            Hud::Robot => {
                let speed = session
                    .own_chassis()
                    .map_or(0., |c| c.velocity_m_s[0].hypot(c.velocity_m_s[1]));
                let mut value = robot.map_or_else(
                    || session.role.name().to_uppercase(),
                    |r| format!("#{}  {:?}\n{} / {} HP", r.id, r.kind, r.hp, r.max_hp),
                );
                if let Some(drive) = &drive {
                    value.push_str(&format!(
                        "\n{speed:.1} m/s    {}",
                        if drive.spinning { "SPIN" } else { "FOLLOW" }
                    ));
                }
                if let Some(r) = session.referee() {
                    if session.role != rm_simulator_server::protocol::Role::Referee {
                        value
                            .push_str(&format!("\n{} GOLD", r.gameplay.gold[session.team.index()]));
                    }
                    if let Some(buff) = &r.teams[session.team.index()].buff {
                        let elapsed_ns = if !session.paused && r.phase == MatchPhase::Running {
                            session
                                .presentation_time_ns()
                                .saturating_sub(session.snapshot.time_ns)
                        } else {
                            0
                        };
                        let remaining_ns = buff
                            .expires_ns
                            .saturating_sub(r.match_time_ns.saturating_add(elapsed_ns));
                        value.push_str(&format!(
                            "   DEF +{}% {}s",
                            buff.defense_pct,
                            remaining_ns.div_ceil(1_000_000_000)
                        ));
                    }
                }
                // Which physics arm this session runs, so a play-test of
                // `--physics-rate-hz` always says which rate is on screen.
                value.push_str(&match rm_simulator_world::hz_for_tick_ns(
                    rm_simulator_world::tick_ns(),
                ) {
                    Some(hz) => format!("\n{hz} Hz PHYSICS"),
                    None => format!("\n{} ns PER TICK", rm_simulator_world::tick_ns()),
                });
                value
            }
            Hud::Ammo => {
                let index = if gun.shot.caliber == Caliber::Mm17 {
                    0
                } else {
                    1
                };
                let allowance = ammo.map_or("--".into(), |a| {
                    a.allowance[index]
                        .saturating_sub(session.reserved_ammo(gun.shot.caliber))
                        .to_string()
                });
                let mode = if session
                    .referee()
                    .is_some_and(|r| r.gameplay.settings.enforce_allowance)
                {
                    "LEFT"
                } else {
                    "TRACKED"
                };
                format!(
                    "{}\n{} mm   {allowance} {mode}",
                    session
                        .last_bullet_speed_m_s()
                        .map(|speed| format!("{speed:.2} m/s"))
                        .unwrap_or_else(|| "-- m/s".into()),
                    if index == 0 { 17 } else { 42 }
                )
            }
            Hud::Connection => session.connection_toast.unwrap_or_default().into(),
            Hud::Notice => session
                .notices
                .iter()
                .rev()
                .take(3)
                .map(|(_, n)| n.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            Hud::Hint => {
                if robot.is_some_and(|r| !r.alive()) {
                    "ROBOT DEFEATED\nWaiting for revival".into()
                } else if !player.captured {
                    "CLICK FIELD TO CONTROL".into()
                } else {
                    "ESC  Pause menu".into()
                }
            }
            Hud::Panel => {
                if ui.debug || ui.settings || ui.large_map {
                    String::new()
                } else {
                    panel_text(&session, &ui, &keys, buttons.as_deref())
                }
            }
            Hud::MapLabel => {
                if ui.large_map {
                    "TEAM POSITIONS   |   Green: you   O: outpost   R: rune".into()
                } else {
                    "FIELD / TEAM POSITIONS".into()
                }
            }
        };
        let display = if value.is_empty() {
            Display::None
        } else {
            Display::Flex
        };
        if node.display != display {
            node.display = display;
        }
        if text.0 != value {
            text.0 = value;
        }
    }
    for (mut node, mut color) in &mut health {
        let fraction = robot.map_or(0., |r| r.hp as f32 / r.max_hp.max(1) as f32);
        node.width = percent(100. * fraction);
        color.0 = if fraction < 0.2 {
            team_color(Team::Red)
        } else {
            GREEN
        };
    }
    for mut visibility in &mut reticle {
        *visibility = if ui.show_reticle && !ui.blocks_input() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

/// Size and place the field map, then keep one labelled dot per robot, outpost and rune.
/// Robots of the other team are drawn only for the referee.
#[allow(clippy::too_many_arguments)]
pub fn update_map(
    mut commands: Commands,
    session: Res<Session>,
    ui: Res<HudState>,
    mut map: Single<(Entity, &mut Node), With<FieldMap>>,
    mut dots: Query<(Entity, &MapDot, &mut Node, &mut BackgroundColor), Without<FieldMap>>,
    artwork: Option<Res<crate::minimap::Minimap>>,
    window: Option<Single<&Window, With<bevy::window::PrimaryWindow>>>,
    images: Query<&ImageNode, With<FieldMap>>,
) {
    let (map_entity, node) = &mut *map;
    node.display = if ui.large_map || ui.show_map {
        Display::Flex
    } else {
        Display::None
    };
    node.right = percent(if ui.large_map { 18. } else { 2. });
    node.bottom = percent(if ui.large_map { 19. } else { 9. });
    let bounds = artwork
        .as_ref()
        .map_or([-15., -9., 15., 9.], |map| map.bounds_m);
    if let Some(artwork) = &artwork
        && images.get(*map_entity).is_err()
    {
        commands
            .entity(*map_entity)
            .insert(ImageNode::new(artwork.image.clone()));
    }
    let aspect = ((bounds[2] - bounds[0]) / (bounds[3] - bounds[1])) as f32;
    let (width, height) = window
        .as_ref()
        .map_or((1280., 720.), |w| (w.width(), w.height()));
    let map_width = (width * if ui.large_map { 0.64 } else { 0.23 })
        .min(height * if ui.large_map { 0.62 } else { 0.24 } * aspect);
    node.width = percent(map_width / width * 100.);
    node.height = percent(map_width / aspect / height * 100.);
    if ui.large_map {
        node.right = percent((1. - map_width / width) * 50.);
        node.bottom = percent((1. - map_width / aspect / height) * 50.);
    }
    let mut items = Vec::new();
    for c in session
        .snapshot
        .chassis
        .iter()
        .filter(|c| session.role.referees() || c.team == session.team)
    {
        let color = if Some(c.id) == session.chassis_id {
            GREEN
        } else {
            team_color(c.team)
        };
        items.push((
            MapItem::Robot(c.id),
            c.pose.translation_m,
            if c.defeated {
                color.with_alpha(0.3)
            } else {
                color
            },
            c.id.to_string(),
        ));
    }
    for (i, o) in session.snapshot.outposts.iter().enumerate() {
        let color = session
            .referee()
            .and_then(|r| r.outpost_teams.get(i))
            .map_or(WHITE, |t| team_color(*t));
        items.push((
            MapItem::Outpost(i),
            o.origin.translation_m,
            if o.destroyed {
                color.with_alpha(0.3)
            } else {
                color
            },
            "O".into(),
        ));
    }
    for (i, r) in session.snapshot.runes.iter().enumerate() {
        items.push((
            MapItem::Rune(i),
            r.hub_pose.translation_m,
            WHITE,
            "R".into(),
        ));
    }
    for (entity, dot, _, _) in &mut dots {
        if !items.iter().any(|(id, _, _, _)| *id == dot.0) {
            commands.entity(entity).despawn();
        }
    }
    for (id, position, color, title) in items {
        let (x, y) = crate::minimap::position(position, bounds);
        if let Some((_, _, mut node, mut background)) =
            dots.iter_mut().find(|(_, dot, _, _)| dot.0 == id)
        {
            node.left = percent(x);
            node.top = percent(y);
            background.0 = color;
        } else {
            commands.entity(*map_entity).with_children(|p| {
                p.spawn((
                    MapDot(id),
                    Node {
                        position_type: PositionType::Absolute,
                        left: percent(x),
                        top: percent(y),
                        min_width: px(16),
                        height: px(18),
                        padding: UiRect::horizontal(px(3)),
                        border_radius: BorderRadius::all(px(3)),
                        ..default()
                    },
                    BackgroundColor(color),
                ))
                .with_children(|p| {
                    p.spawn((
                        Text::new(title),
                        text_style(13.),
                        TextColor(INK),
                        ClassList::new("map-dot-label"),
                        Pickable::IGNORE,
                    ));
                });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hud_systems_render_practice_and_switch_panels_without_query_conflicts() {
        let mut app = App::new();
        app.insert_resource(crate::session::test_session(true))
            .insert_resource(Player::at(Vec3::ZERO, 0., 0.))
            .insert_resource(Gun::new(
                rm_simulator_world::Shot::at_limit(Caliber::Mm17),
                100_000_000,
            ))
            .init_resource::<HudState>()
            .add_systems(PreUpdate, |mut ui: ResMut<HudState>| ui.consumed = false)
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Startup, |mut commands: Commands| spawn_hud(&mut commands))
            .add_systems(
                Update,
                (panel_input, update_hud, update_map, update_panel_rows).chain(),
            );
        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&MapDot>()
                .iter(app.world())
                .filter(|dot| matches!(dot.0, MapItem::Robot(_)))
                .count(),
            1
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyP);
        app.update();
        let panel = app
            .world_mut()
            .query::<(&Hud, &Text)>()
            .iter(app.world())
            .find(|(kind, _)| matches!(kind, Hud::Panel))
            .unwrap()
            .1;
        assert!(panel.0.is_empty());
        assert!(app.world().resource::<HudState>().settings);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyM);
        app.update();
        assert!(app.world().resource::<HudState>().large_map);
        assert!(!app.world().resource::<HudState>().settings);
        app.world_mut().resource_mut::<HudState>().large_map = false;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F12);
        app.update();
        let rows = app
            .world_mut()
            .query::<&PanelRows>()
            .single(app.world())
            .unwrap();
        assert!(rows.0.iter().any(|r| r[0] == "W" && r[1] == "Move forward"));
        assert!(rows.0.iter().any(|r| r[0] == "P" && r[1] == "Settings"));
    }
    #[test]
    fn debug_panel_is_exclusive_and_escape_blocks_gameplay() {
        let mut app = App::new();
        app.init_resource::<HudState>()
            .add_systems(PreUpdate, |mut ui: ResMut<HudState>| ui.consumed = false)
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Update, panel_input);
        app.world_mut().resource_mut::<HudState>().settings = true;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F3);
        app.update();
        let ui = app.world().resource::<HudState>();
        assert!(ui.debug && ui.blocks_input());
        assert!(!ui.settings);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        let ui = app.world().resource::<HudState>();
        assert!(!ui.modal_open());
        assert!(ui.blocks_input());
    }

    #[test]
    fn panel_keys_toggle_settings_and_escape_is_consumed() {
        let mut app = App::new();
        app.init_resource::<HudState>()
            .add_systems(PreUpdate, |mut ui: ResMut<HudState>| ui.consumed = false)
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Update, panel_input);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyP);
        app.update();
        assert!(app.world().resource::<HudState>().blocks_input());
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        assert!(!app.world().resource::<HudState>().settings);
        assert!(app.world().resource::<HudState>().closed_with_escape);
    }
}

#[cfg(test)]
mod title_settings_tests {
    use super::*;
    #[test]
    fn typing_a_settings_binding_on_title_does_not_open_a_panel() {
        let mut app = App::new();
        app.init_resource::<HudState>()
            .init_resource::<crate::title::TitleScreen>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Update, panel_input);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyP);
        app.update();
        assert!(!app.world().resource::<HudState>().settings);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut().resource_mut::<HudState>().settings = true;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        assert!(!app.world().resource::<HudState>().settings);
        assert!(app.world().resource::<HudState>().closed_with_escape);
    }
}

/// Hold-to-peek panels remain scrollable while the game owns the mouse.
/// Released cursors use the ordinary UI ScrollArea observer instead.
pub(super) fn scroll_peek_panel(
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    player: Option<Res<Player>>,
    mut panels: Query<(&Node, &ComputedNode, &mut ScrollPosition), With<PanelFrame>>,
) {
    use bevy::input::mouse::MouseScrollUnit;
    let delta: f32 = wheel
        .read()
        .map(|event| {
            event.y
                * if event.unit == MouseScrollUnit::Line {
                    32.
                } else {
                    1.
                }
        })
        .sum();
    if !player.is_some_and(|p| p.captured) {
        return;
    }
    for (node, computed, mut scroll) in &mut panels {
        if node.display == Display::None {
            continue;
        }
        let max = ((computed.content_size().y - computed.size().y) * computed.inverse_scale_factor)
            .max(0.);
        scroll.y = (scroll.y - delta).clamp(0., max);
    }
}

#[cfg(test)]
mod peek_scroll_tests {
    use super::*;
    #[test]
    fn captured_wheel_scrolls_visible_panel_and_clamps_both_ends() {
        use bevy::input::{
            mouse::{MouseScrollUnit, MouseWheel},
            touch::TouchPhase,
        };
        let mut app = App::new();
        let mut player = Player::at(Vec3::ZERO, 0., 0.);
        player.captured = true;
        app.insert_resource(player)
            .add_message::<MouseWheel>()
            .add_systems(Update, scroll_peek_panel);
        let panel = app
            .world_mut()
            .spawn((
                PanelFrame,
                Node::default(),
                ScrollPosition::default(),
                ComputedNode {
                    size: Vec2::new(800., 400.),
                    content_size: Vec2::new(800., 1200.),
                    inverse_scale_factor: 1.,
                    ..default()
                },
            ))
            .id();
        for (delta, expected) in [(-100., 100.), (-2000., 800.), (2000., 0.)] {
            app.world_mut().write_message(MouseWheel {
                unit: MouseScrollUnit::Pixel,
                x: 0.,
                y: delta,
                window: Entity::PLACEHOLDER,
                phase: TouchPhase::Moved,
            });
            app.update();
            assert_eq!(
                app.world().get::<ScrollPosition>(panel).unwrap().y,
                expected
            );
        }
        app.world_mut().resource_mut::<Player>().captured = false;
        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.,
            y: -5.,
            window: Entity::PLACEHOLDER,
            phase: TouchPhase::Moved,
        });
        app.update();
        assert_eq!(
            app.world().get::<ScrollPosition>(panel).unwrap().y,
            0.,
            "released cursor uses UI scrolling, not both paths"
        );
    }
}
