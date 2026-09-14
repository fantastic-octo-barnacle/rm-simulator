// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! The title screen: a name, an address and the ways into a match. Every
//! choice is turned into the same arguments the command line would have
//! given, so the lifecycle in `loading.rs` has one entry. The last name and
//! addresses are remembered in the user's configuration directory.
use crate::{args::Args, loading::JoinRequest};
use bevy::{
    feathers::{
        controls::{
            FeathersButton, FeathersCheckbox, FeathersScrollbar, FeathersTextInput,
            FeathersTextInputContainer,
        },
        theme::ThemedText,
    },
    prelude::*,
    text::{EditableText, TextEdit},
    ui::Checked,
    ui_widgets::{Activate, ActivateOnPress, ValueChange, checkbox_self_update},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Present while the title screen is up. `status` is why the last join or
/// match ended, shown under the buttons.
#[derive(Resource, Default)]
pub struct TitleScreen {
    /// Why the last join or match ended, shown under the buttons; `None` while
    /// a join starts or after a deliberate leave.
    pub status: Option<String>,
}
/// The command-line arguments every join from the title screen starts from.
#[derive(Resource)]
pub struct BaseArgs(pub Args);
/// The address a host binds when the field is left empty.
pub const DEFAULT_LISTEN: &str = "0.0.0.0:7700";

/// One of the buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Open the multiplayer page, which starts a LAN search and shows the
    /// firewall tip.
    Multiplayer,
    /// Leave the multiplayer page for the main menu.
    Back,
    /// Search the LAN for lobbies on a worker thread.
    Refresh,
    /// Hide the firewall tip.
    DismissTip,
    /// Pick the discovered lobby at this index in the last listing. An entry
    /// that is not on the LAN or not compatible is refused with a status.
    Join(usize),
    /// Join the address field; a picked lobby whose address matches it also
    /// supplies the transport.
    Connect,
    /// Host a named lobby on the address the fields give.
    Host,
    /// Start a local match with no host and no connection.
    Practice,
    /// Ask the HUD to confirm quitting.
    Quit,
}
/// What the fields say when a button is pressed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TitleFields {
    /// Lobby directory as HOST:PORT. Its input sits in the hidden public-only
    /// container, so only the default or a remembered value reaches it.
    #[serde(default)]
    pub lobby_host: String,
    /// Name a hosted lobby advertises. `Choice::Host` requires 1 to 64
    /// characters and no control characters.
    #[serde(default)]
    pub lobby_name: String,
    /// List the hosted lobby publicly. Public hosting awaits a relay, so
    /// `join_args` always clears `--public-lobby`.
    #[serde(default)]
    pub public: bool,
    /// Public IP:PORT override for the advertised address; empty lets the
    /// directory observe the address.
    #[serde(default)]
    pub advertised: String,
    /// Password a hosted lobby requires. Skipped by serde, so it is never
    /// written to `title.json`.
    #[serde(skip)]
    pub password: String,
    /// Password sent when joining a lobby. Skipped by serde, so it is never
    /// written to `title.json`.
    #[serde(skip)]
    pub join_password: String,
    /// Player name sent to the host; `join_args` trims it and rejects an empty
    /// one.
    pub name: String,
    /// Host address as HOST:PORT, either typed in or prefilled from a picked
    /// lobby.
    pub address: String,
    /// Address the local host listens on as ADDR:PORT; empty means
    /// `DEFAULT_LISTEN`.
    pub host: String,
    /// Play for the blue team; false plays for red, whose half is +x.
    #[serde(default)]
    pub blue: bool,
    /// Join with the free camera and no robot.
    #[serde(default)]
    pub spectate: bool,
    /// Remembered host weapon fire rate text.
    #[serde(default)]
    pub fire_rate: String,
    /// Remembered shared physics rate in Hz, as typed. Blank keeps the rate the
    /// command line gave. It is frozen for the process once a match starts, so
    /// changing it here only takes effect on the next launch.
    #[serde(default)]
    pub physics_rate: String,
    /// Remembered host rate cap in Hz.
    #[serde(default)]
    pub max_fire_rate: String,
    /// Remembered host speed cap in m/s.
    #[serde(default)]
    pub max_muzzle_speed: String,
    /// Remembered host weapon muzzle speed text.
    #[serde(default)]
    pub muzzle_speed: String,
    /// Remembered default muzzle-speed variation in m/s.
    #[serde(default)]
    pub speed_variation: String,
    /// Remembered host weapon caliber text.
    #[serde(default)]
    pub caliber: String,
    /// Remembered host weapon spread text.
    #[serde(default)]
    pub spread: String,
    /// Remembered host weapon distribution text.
    #[serde(default)]
    pub distribution: String,
    /// Remembered host weapon seed text.
    #[serde(default)]
    pub seed: String,
}
impl TitleFields {
    /// What the fields show first: the remembered values, else the command line.
    pub fn initial(base: &Args, remembered: Option<TitleFields>) -> Self {
        let remembered = remembered.unwrap_or_default();
        let pick = |saved: String, arg: Option<String>, fallback: &str| {
            if !saved.trim().is_empty() {
                saved
            } else {
                arg.unwrap_or_else(|| fallback.into())
            }
        };
        Self {
            lobby_host: pick(
                remembered.lobby_host,
                Some(base.lobby_host.clone()),
                rm_simulator_server::lobby::DEFAULT_HOST,
            ),
            lobby_name: pick(remembered.lobby_name, base.lobby_name.clone(), "My lobby"),
            public: false,
            advertised: pick(
                remembered.advertised,
                Some(base.advertise_address.clone()),
                "",
            ),
            password: base.password.clone(),
            join_password: base.password.clone(),
            name: pick(remembered.name, Some(base.name.clone()), "pilot"),
            address: pick(remembered.address, base.connect.clone(), ""),
            host: pick(remembered.host, base.listen.clone(), DEFAULT_LISTEN),
            blue: base.team == crate::args::TeamArg::Blue || remembered.blue,
            spectate: base.fly || remembered.spectate,
            max_fire_rate: pick(
                remembered.max_fire_rate,
                Some(base.max_fire_rate_hz.to_string()),
                "30",
            ),
            max_muzzle_speed: pick(
                remembered.max_muzzle_speed,
                Some(base.max_muzzle_speed_m_s.to_string()),
                "30",
            ),
            fire_rate: pick(
                remembered.fire_rate,
                Some(base.fire_rate_hz.to_string()),
                "20",
            ),
            physics_rate: pick(
                remembered.physics_rate,
                Some(base.physics_rate_hz.to_string()),
                "1000",
            ),
            muzzle_speed: pick(
                remembered.muzzle_speed,
                Some(
                    base.muzzle_speed_m_s
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                ),
                "",
            ),
            speed_variation: pick(
                remembered.speed_variation,
                Some(base.muzzle_speed_variation_m_s.to_string()),
                "0",
            ),
            caliber: pick(
                remembered.caliber,
                Some(base.projectile_mm.to_string()),
                "17",
            ),
            spread: pick(remembered.spread, Some(base.spread_deg.to_string()), "0"),
            distribution: pick(
                remembered.distribution,
                Some(
                    match base.spread_distribution {
                        rm_simulator_server::protocol::SpreadDistribution::Uniform => "uniform",
                        rm_simulator_server::protocol::SpreadDistribution::Gaussian => "gaussian",
                    }
                    .into(),
                ),
                "gaussian",
            ),
            seed: pick(remembered.seed, Some(base.spread_seed.to_string()), "0"),
        }
    }
}

/// The arguments a choice joins with, or what is missing for it.
pub fn join_args(base: &Args, fields: &TitleFields, choice: Choice) -> Result<Args, String> {
    let mut args = base.clone();
    let name = fields.name.trim();
    if name.is_empty() {
        return Err("Enter a name".into());
    }
    args.name = name.into();
    args.password = if choice == Choice::Connect {
        fields.join_password.clone()
    } else {
        fields.password.clone()
    };
    args.lobby_host = fields.lobby_host.trim().into();
    args.advertise_address = fields.advertised.trim().into();
    args.lobby_name = None;
    args.public_lobby = false;
    args.team = if fields.blue {
        crate::args::TeamArg::Blue
    } else {
        crate::args::TeamArg::Red
    };
    args.fly = fields.spectate;
    match choice {
        Choice::Connect => {
            let address = fields.address.trim();
            if address.is_empty() {
                return Err("Enter the host address as HOST:PORT".into());
            }
            args.connect = Some(address.into());
            args.listen = None;
        }
        Choice::Host => {
            if fields.lobby_name.trim().is_empty()
                || fields.lobby_name.chars().count() > 64
                || fields.lobby_name.chars().any(char::is_control)
            {
                return Err("Enter a lobby name of 1 to 64 printable characters".into());
            }
            args.lobby_name = Some(fields.lobby_name.trim().into());
            args.public_lobby = false; // Public menu hosting awaits a relay.
            let host = fields.host.trim();
            args.connect = None;
            args.listen = Some(if host.is_empty() {
                DEFAULT_LISTEN.into()
            } else {
                host.into()
            });
        }
        Choice::Practice => {
            args.connect = None;
            args.listen = None;
            args.password.clear();
        }
        _ => return Err("nothing to join".into()),
    }
    // Every peer in a match shares the physics rate, so it is applied to a
    // remote join as well: the host refuses a client that predicts at another.
    if !fields.physics_rate.trim().is_empty() {
        let hz: u32 = fields
            .physics_rate
            .trim()
            .parse()
            .map_err(|_| "Physics rate must be 1000, 500, 250 or 128 Hz")?;
        if rm_simulator_world::tick_ns_for_hz(hz).is_none() {
            return Err("Physics rate must be 1000, 500, 250 or 128 Hz".into());
        }
        args.physics_rate_hz = hz;
    }
    if choice != Choice::Connect {
        if !fields.fire_rate.trim().is_empty() {
            args.fire_rate_hz = fields
                .fire_rate
                .trim()
                .parse()
                .map_err(|_| "Enter a fire rate in Hz")?;
        }
        if !args.fire_rate_hz.is_finite() || !(0.1..=1000.0).contains(&args.fire_rate_hz) {
            return Err("Fire rate must be in [0.1, 1000] Hz".into());
        }
        if !fields.caliber.trim().is_empty() {
            args.projectile_mm = fields
                .caliber
                .trim()
                .parse()
                .map_err(|_| "Caliber must be 17 or 42 mm")?;
        }
        if ![17, 42].contains(&args.projectile_mm) {
            return Err("Caliber must be 17 or 42 mm".into());
        }
        args.muzzle_speed_m_s = if fields.muzzle_speed.trim().is_empty() {
            None
        } else {
            Some(
                fields
                    .muzzle_speed
                    .trim()
                    .parse()
                    .map_err(|_| "Enter a muzzle speed in m/s")?,
            )
        };
        if !fields.spread.trim().is_empty() {
            args.spread_deg = fields
                .spread
                .trim()
                .parse()
                .map_err(|_| "Enter a spread angle in degrees")?;
        }
        if !fields.distribution.trim().is_empty() {
            args.spread_distribution = match fields.distribution.trim() {
                "uniform" => rm_simulator_server::protocol::SpreadDistribution::Uniform,
                "gaussian" => rm_simulator_server::protocol::SpreadDistribution::Gaussian,
                _ => return Err("Spread distribution must be uniform or gaussian".into()),
            };
        }
        if !fields.seed.trim().is_empty() {
            args.spread_seed = fields
                .seed
                .trim()
                .parse()
                .map_err(|_| "Spread seed must be a nonnegative integer")?;
        }
        if !fields.speed_variation.trim().is_empty() {
            args.muzzle_speed_variation_m_s = fields
                .speed_variation
                .trim()
                .parse()
                .map_err(|_| "Enter muzzle-speed variation from 0 to 1 m/s")?;
        }
        if !fields.max_fire_rate.trim().is_empty() {
            args.max_fire_rate_hz = fields
                .max_fire_rate
                .trim()
                .parse()
                .map_err(|_| "Enter a maximum rate in Hz")?;
        }
        if !args.max_fire_rate_hz.is_finite() || !(0.1..=1000.0).contains(&args.max_fire_rate_hz) {
            return Err("Maximum fire rate must be in [0.1, 1000] Hz".into());
        }
        if !fields.max_muzzle_speed.trim().is_empty() {
            args.max_muzzle_speed_m_s = fields
                .max_muzzle_speed
                .trim()
                .parse()
                .map_err(|_| "Enter a maximum speed in m/s")?;
        }
        args.weapon_limits()
            .admit(args.caliber(), args.weapon())
            .map_err(str::to_string)?;
    }
    Ok(args)
}

/// Where the fields are remembered: `$XDG_CONFIG_HOME` or `~/.config` on
/// Unix, `%APPDATA%` on Windows, under `rm-simulator/title.json`.
pub fn remembered_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("rm-simulator").join("title.json"))
}
/// Read the remembered fields from `path`. A missing, unreadable or malformed
/// file gives `None`, as does one without its name, address or host key, so the
/// caller falls back to the command line.
pub fn load_remembered(path: &Path) -> Option<TitleFields> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}
/// Write the fields to `path` as pretty JSON, creating the parent directory
/// when it is missing. The two password fields are skipped, so they never reach
/// the file. Serializing the plain fields cannot fail; only the filesystem
/// errors are returned.
pub fn save_remembered(path: &Path, fields: &TitleFields) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(fields).expect("fields serialize"),
    )
}

#[derive(Component)]
struct TitleRoot;
#[derive(Component)]
struct TitleCard;
#[derive(Component)]
struct MenuScrollbar(Entity);

#[derive(Resource)]
struct TitleBackground(Handle<Image>);
impl FromWorld for TitleBackground {
    fn from_world(world: &mut World) -> Self {
        use bevy::{
            asset::RenderAssetUsages,
            image::{CompressedImageFormats, ImageSampler, ImageType},
        };
        let image = Image::from_buffer(
            include_bytes!("../../../assets/title/background.png"),
            ImageType::Extension("png"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::linear(),
            RenderAssetUsages::default(),
        )
        .expect("embedded title screenshot decodes");
        Self(world.resource_mut::<Assets<Image>>().add(image))
    }
}

/// Crop the screenshot to cover the window without stretching it.
fn fit_background(mut roots: Query<(&ComputedNode, &mut ImageNode), With<TitleRoot>>) {
    for (node, mut image) in &mut roots {
        let size = node.size();
        if size.x <= 0.0 || size.y <= 0.0 {
            continue;
        }
        let source = Vec2::new(1280.0, 720.0);
        let scale = (size.x / source.x).max(size.y / source.y);
        let crop = size / scale;
        let rect = Some(Rect::from_center_size(source / 2.0, crop));
        if image.rect != rect {
            image.rect = rect;
        }
    }
}
#[derive(Component, Default, Clone)]
struct NameInput;
#[derive(Component, Default, Clone)]
struct AddressInput;
#[derive(Component, Default, Clone)]
struct HostInput;
#[derive(Component, Default, Clone)]
enum LobbyInput {
    #[default]
    Directory,
    Name,
    Password,
    JoinPassword,
    Advertised,
    FireRate,
    PhysicsRate,
    MaxFireRate,
    MaxMuzzleSpeed,
    MuzzleSpeed,
    SpeedVariation,
    Caliber,
    Spread,
    Distribution,
    Seed,
}
#[derive(Component)]
struct WeaponFields;
#[derive(Component)]
struct MenuPage(bool);
#[derive(Component)]
struct LobbyList;
#[derive(Component)]
struct LobbyColumn;
#[derive(Component)]
struct FirewallTip;
/// Network help shown with the multiplayer fields and on the multiplayer
/// loading splash: allow the app through the firewall and use one network.
pub const FIREWALL_TIP: &str = "Can’t find or join a LAN lobby? Allow RM Simulator through your firewall on private networks. Both players must be on the same network; guest Wi-Fi may block connections.";

#[derive(Component)]
struct StatusText;
/// Text to put into an input once its editor exists.
#[derive(Component)]
struct Prefill(String);
#[derive(Component)]
struct TitleButton;
/// Pressed buttons waiting to be acted on, and the checkbox states, kept
/// here because a checkbox reports changes only.
type DiscoveryResult = (Vec<rm_simulator_server::lobby::Listing>, String);

/// Title screen state the text fields do not hold: the open page, the last LAN
/// listing, a search in flight and the buttons pressed this frame.
#[derive(Resource, Default)]
pub(crate) struct TitleState {
    multiplayer: bool,
    tip: bool,
    public: bool,
    selected: Option<usize>,
    entries: Vec<rm_simulator_server::lobby::Listing>,
    discovery: Option<std::sync::Mutex<std::sync::mpsc::Receiver<DiscoveryResult>>>,
    actions: Vec<Choice>,
    blue: bool,
    spectate: bool,
}

impl TitleState {
    /// Leave the multiplayer page for the main menu. Returns whether it was
    /// open, so Escape offers to quit only from the main page.
    pub(crate) fn back_to_main_menu(&mut self) -> bool {
        let was_multiplayer = self.multiplayer;
        self.multiplayer = false;
        was_multiplayer
    }
}

/// Installs the title resources and the chained update systems that build the
/// screen and act on its buttons. Only the input system requires the
/// `TitleScreen` resource.
pub struct TitlePlugin;
impl Plugin for TitlePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TitleState>()
            .init_resource::<TitleBackground>()
            .add_systems(
                Update,
                (
                    sync_title,
                    poll_lobbies,
                    show_page,
                    fit_columns,
                    fit_scrollbars,
                    prefill,
                    fit_background,
                    title_input
                        .after(crate::hud::panel_input)
                        .run_if(resource_exists::<TitleScreen>),
                )
                    .chain(),
            );
    }
}

fn button(commands: &mut Commands, parent: Entity, title: &str, choice: Choice) -> Entity {
    let title = title.to_owned();
    let caption = title.clone();
    commands
        .spawn_scene(bsn! {
            @FeathersButton { @caption: bsn! { Text(caption) ThemedText } }
            ActivateOnPress
            AccessibleLabel(title)
            Node { height: px(38), flex_shrink: 0.0, padding: UiRect::horizontal(px(18)) }
            on(move |_: On<Activate>, mut state: ResMut<TitleState>| { state.actions.push(choice); })
        })
        .insert((ChildOf(parent), TitleButton))
        .id()
}

fn scrollbar(commands: &mut Commands, parent: Entity, target: Entity) {
    commands.spawn_scene(bsn! {
        @FeathersScrollbar { @target: target, @orientation: {bevy::ui_widgets::ControlOrientation::Vertical} }
        Node { position_type: PositionType::Absolute, right: px(2), top: px(4), bottom: px(4), width: px(8) }
    }).insert((ChildOf(parent), MenuScrollbar(target)));
}

fn field<M: Component + Default + Clone>(
    commands: &mut Commands,
    parent: Entity,
    label: &'static str,
    value: String,
    marker: M,
) {
    commands.spawn((
        ChildOf(parent),
        Text::new(label),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.55, 0.65, 0.72)),
    ));
    let container = commands
        .spawn_scene(bsn! {
            @FeathersTextInputContainer
            Node { width: percent(100), height: px(36), flex_grow: 0.0, flex_shrink: 0.0, padding: { px(4).left() } }
        })
        .insert(ChildOf(parent))
        .id();
    commands
        .spawn_scene(bsn! {
            @FeathersTextInput { @max_characters: 96usize }
        })
        .insert((ChildOf(container), marker, Prefill(value)));
}

/// Show the title screen while `TitleScreen` exists and take it down when
/// a join starts.
fn sync_title(
    mut commands: Commands,
    screen: Option<Res<TitleScreen>>,
    base: Option<Res<BaseArgs>>,
    roots: Query<Entity, With<TitleRoot>>,
    mut status: Query<&mut Text, With<StatusText>>,
    mut state: ResMut<TitleState>,
    background: Res<TitleBackground>,
) {
    let Some(screen) = screen else {
        for root in &roots {
            commands.entity(root).despawn();
        }
        return;
    };
    if let Ok(mut text) = status.single_mut() {
        if screen.is_changed() {
            text.0 = screen.status.clone().unwrap_or_default();
        }
        return;
    }
    if !roots.is_empty() {
        return;
    }
    let Some(base) = base else {
        return;
    };
    let fields = TitleFields::initial(
        &base.0,
        remembered_path().and_then(|path| load_remembered(&path)),
    );
    state.multiplayer = false;
    state.public = fields.public;
    state.blue = fields.blue;
    state.spectate = fields.spectate;
    let root = commands
        .spawn((
            TitleRoot,
            ImageNode {
                image: background.0.clone(),
                color: Color::srgb(0.32, 0.32, 0.32),
                image_mode: bevy::ui::widget::NodeImageMode::Stretch,
                ..default()
            },
            GlobalZIndex(100),
            Node {
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: px(12),
                ..default()
            },
            BackgroundColor(Color::srgb(0.035, 0.05, 0.075)),
        ))
        .id();
    commands.spawn((
        ChildOf(root),
        Text::new("RM SIMULATOR"),
        TextFont {
            font_size: FontSize::Px(38.0),
            ..default()
        },
        TextColor(Color::WHITE),
    ));
    let frame = commands
        .spawn((
            ChildOf(root),
            TitleCard,
            Node {
                width: percent(90),
                max_width: px(1100),
                height: percent(80),
                min_height: px(0),
                padding: UiRect::right(px(12)),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(8)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.035, 0.05, 0.065)),
            BorderColor::all(Color::srgb(0.19, 0.52, 0.56)),
        ))
        .id();
    let viewport = commands
        .spawn((
            ChildOf(frame),
            bevy::ui_widgets::ScrollArea,
            Node {
                width: percent(100),
                max_height: percent(100),
                overflow: Overflow::scroll_y(),
                flex_direction: FlexDirection::Column,
                ..default()
            },
        ))
        .id();
    scrollbar(&mut commands, frame, viewport);
    let card = commands
        .spawn((
            ChildOf(viewport),
            Node {
                width: percent(100),
                padding: UiRect::all(px(20)),
                flex_direction: FlexDirection::Column,
                row_gap: px(10),
                flex_shrink: 0.0,
                ..default()
            },
        ))
        .id();
    // Public-only settings remain in the form state for later activation.
    let future = commands
        .spawn((
            ChildOf(card),
            Node {
                display: Display::None,
                ..default()
            },
        ))
        .id();
    field(&mut commands, card, "Name", fields.name.clone(), NameInput);
    let blue = fields.blue;
    let mut checkbox = commands.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text("Play on the blue team") ThemedText } }
        AccessibleLabel("Play on the blue team")
        on(blue_changed)
    });
    checkbox.insert(ChildOf(card)).observe(checkbox_self_update);
    if blue {
        checkbox.insert(Checked);
    }
    let spectate = fields.spectate;
    let mut checkbox = commands.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text("Spectate with the free camera (no robot)") ThemedText } }
        AccessibleLabel("Spectate with the free camera")
        on(spectate_changed)
    });
    checkbox.insert(ChildOf(card)).observe(checkbox_self_update);
    if spectate {
        checkbox.insert(Checked);
    }
    let row = commands
        .spawn((
            ChildOf(card),
            Node {
                column_gap: px(10),
                margin: UiRect::top(px(10)),
                flex_wrap: FlexWrap::Wrap,
                row_gap: px(10),
                ..default()
            },
        ))
        .id();
    button(&mut commands, row, "Single Player", Choice::Practice);
    button(&mut commands, row, "Multiplayer", Choice::Multiplayer);
    button(&mut commands, row, "Quit", Choice::Quit);
    commands.entity(row).insert(MenuPage(false));
    commands.spawn_scene(bsn! {
        @FeathersButton { @caption: bsn! { Text("Settings") ThemedText } }
        ActivateOnPress
        on(|_: On<Activate>, mut ui: ResMut<crate::hud::HudState>, mut focus: ResMut<bevy::input_focus::InputFocus>| {
            ui.close(); ui.settings = true; ui.consumed = true; focus.clear();
        })
    }).insert(ChildOf(row));

    let multiplayer = commands
        .spawn((
            ChildOf(card),
            MenuPage(true),
            Node {
                width: percent(100),
                column_gap: px(24),
                display: Display::None,
                ..default()
            },
        ))
        .id();
    let left = commands
        .spawn((
            ChildOf(multiplayer),
            LobbyColumn,
            Node {
                width: percent(50),
                min_width: px(0),
                flex_direction: FlexDirection::Column,
                row_gap: px(8),
                ..default()
            },
        ))
        .id();
    commands.spawn((
        ChildOf(left),
        Text::new("Lobbies"),
        TextFont {
            font_size: FontSize::Px(24.0),
            ..default()
        },
    ));
    field(
        &mut commands,
        future,
        "Lobby Host (HOST:PORT)",
        fields.lobby_host.clone(),
        LobbyInput::Directory,
    );
    button(&mut commands, left, "Refresh LAN", Choice::Refresh);
    let tip = commands
        .spawn((
            ChildOf(root),
            GlobalZIndex(110),
            FirewallTip,
            Node {
                position_type: PositionType::Absolute,
                right: px(24),
                bottom: px(24),
                width: px(360),
                max_width: percent(90),
                padding: UiRect::all(px(14)),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgb(0.08, 0.16, 0.20)),
        ))
        .id();
    commands.spawn((
        ChildOf(tip),
        Text::new(FIREWALL_TIP),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
    ));
    button(&mut commands, tip, "Got it", Choice::DismissTip);

    let list_frame = commands
        .spawn((
            ChildOf(left),
            Node {
                width: percent(100),
                height: px(190),
                flex_shrink: 0.0,
                padding: UiRect::right(px(12)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.025, 0.035, 0.045)),
        ))
        .id();
    let list = commands
        .spawn((
            ChildOf(list_frame),
            LobbyList,
            bevy::ui_widgets::ScrollArea,
            Node {
                height: percent(100),
                width: percent(100),
                overflow: Overflow::scroll_y(),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                ..default()
            },
        ))
        .id();
    scrollbar(&mut commands, list_frame, list);
    field(
        &mut commands,
        left,
        "Direct address (HOST:PORT)",
        fields.address.clone(),
        AddressInput,
    );
    field(
        &mut commands,
        left,
        "Lobby password (optional, visible)",
        fields.join_password.clone(),
        LobbyInput::JoinPassword,
    );
    button(&mut commands, left, "Join lobby / address", Choice::Connect);
    let right = commands
        .spawn((
            ChildOf(multiplayer),
            LobbyColumn,
            Node {
                width: percent(50),
                min_width: px(0),
                flex_direction: FlexDirection::Column,
                row_gap: px(8),
                ..default()
            },
        ))
        .id();
    commands.spawn((
        ChildOf(right),
        Text::new("Create lobby"),
        TextFont {
            font_size: FontSize::Px(24.0),
            ..default()
        },
    ));
    field(
        &mut commands,
        right,
        "Lobby name",
        fields.lobby_name.clone(),
        LobbyInput::Name,
    );
    field(
        &mut commands,
        right,
        "Password (optional, visible)",
        fields.password.clone(),
        LobbyInput::Password,
    );
    let mut public = commands.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text("Public (unavailable)") ThemedText } }
        on(|change: On<ValueChange<bool>>, mut state: ResMut<TitleState>| { state.public = change.value; })
    });
    public.insert((ChildOf(right), bevy::ui::InteractionDisabled));

    field(
        &mut commands,
        right,
        "Listen on (ADDR:PORT)",
        fields.host.clone(),
        HostInput,
    );
    field(
        &mut commands,
        future,
        "Public IP:PORT override (optional)",
        fields.advertised.clone(),
        LobbyInput::Advertised,
    );
    commands.spawn((
        ChildOf(right),
        Text::new("LAN only. Public lobbies are disabled until relay support is available."),
        TextFont {
            font_size: FontSize::Px(12.0),
            ..default()
        },
    ));
    button(&mut commands, right, "Create lobby", Choice::Host);
    button(&mut commands, right, "Back", Choice::Back);

    commands.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text("Host weapon settings (practice / create lobby)") ThemedText } }
        on(|change: On<ValueChange<bool>>, mut panels: Query<&mut Node, With<WeaponFields>>| {
            for mut panel in &mut panels { panel.display = if change.value { Display::Flex } else { Display::None }; }
        })
    }).insert(ChildOf(card)).observe(checkbox_self_update);
    let weapon_fields = commands
        .spawn((
            ChildOf(card),
            WeaponFields,
            Node {
                display: Display::None,
                flex_direction: FlexDirection::Column,
                row_gap: px(8),
                flex_shrink: 0.0,
                ..default()
            },
        ))
        .id();
    commands.spawn((ChildOf(weapon_fields), Text::new("Players can adjust rate and speed within the host limits and choose their own spread in Settings > Weapon."), TextFont { font_size: FontSize::Px(14.0), ..default() }));
    field(
        &mut commands,
        weapon_fields,
        "Maximum fire rate (Hz)",
        fields.max_fire_rate.clone(),
        LobbyInput::MaxFireRate,
    );
    field(
        &mut commands,
        weapon_fields,
        "Maximum muzzle speed (m/s)",
        fields.max_muzzle_speed.clone(),
        LobbyInput::MaxMuzzleSpeed,
    );
    field(
        &mut commands,
        weapon_fields,
        "Starting fire rate (Hz)",
        fields.fire_rate.clone(),
        LobbyInput::FireRate,
    );
    field(
        &mut commands,
        weapon_fields,
        "Physics rate (Hz: 1000, 500, 250 or 128; set at launch)",
        fields.physics_rate.clone(),
        LobbyInput::PhysicsRate,
    );
    field(
        &mut commands,
        weapon_fields,
        "Starting muzzle speed (m/s, blank = 25)",
        fields.muzzle_speed.clone(),
        LobbyInput::MuzzleSpeed,
    );
    field(
        &mut commands,
        weapon_fields,
        "Default speed variation (+/- m/s, 0 to 1)",
        fields.speed_variation.clone(),
        LobbyInput::SpeedVariation,
    );
    field(
        &mut commands,
        weapon_fields,
        "Caliber (17 or 42 mm)",
        fields.caliber.clone(),
        LobbyInput::Caliber,
    );
    field(
        &mut commands,
        weapon_fields,
        "Default spread half-angle (degrees, 0 = perfect)",
        fields.spread.clone(),
        LobbyInput::Spread,
    );
    field(
        &mut commands,
        weapon_fields,
        "Default distribution (uniform or gaussian)",
        fields.distribution.clone(),
        LobbyInput::Distribution,
    );
    field(
        &mut commands,
        weapon_fields,
        "Default spread seed",
        fields.seed.clone(),
        LobbyInput::Seed,
    );

    commands.spawn((
        ChildOf(card),
        StatusText,
        Text::new(screen.status.clone().unwrap_or_default()),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.95, 0.55, 0.45)),
    ));
    commands.spawn((
        ChildOf(root),
        Text::new("Matches run on the host computer  |  Esc goes back or opens quit confirmation"),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.55, 0.65, 0.72)),
    ));
}

#[allow(clippy::type_complexity)]
fn show_page(
    state: Res<TitleState>,
    mut pages: Query<(&MenuPage, &mut Node), (Without<FirewallTip>, Without<TitleCard>)>,
    mut tips: Query<&mut Node, (With<FirewallTip>, Without<TitleCard>)>,
    mut cards: Query<&mut Node, With<TitleCard>>,
) {
    for mut node in &mut cards {
        let width = px(if state.multiplayer { 1100.0 } else { 480.0 });
        let height = if state.multiplayer {
            percent(80)
        } else {
            Val::Auto
        };
        if node.max_width != width {
            node.max_width = width;
        }
        if node.height != height {
            node.height = height;
        }
        if node.max_height != percent(80) {
            node.max_height = percent(80);
        }
    }
    for mut node in &mut tips {
        let display = if state.tip && state.multiplayer {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }

    for (page, mut node) in &mut pages {
        let display = if page.0 == state.multiplayer {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
}

#[allow(clippy::type_complexity)]
fn fit_columns(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut pages: Query<(&MenuPage, &mut Node), Without<LobbyColumn>>,
    mut columns: Query<&mut Node, With<LobbyColumn>>,
) {
    let narrow = windows
        .iter()
        .next()
        .is_some_and(|window| window.width() < 900.0);
    for (page, mut node) in &mut pages {
        if page.0 {
            let direction = if narrow {
                FlexDirection::Column
            } else {
                FlexDirection::Row
            };
            if node.flex_direction != direction {
                node.flex_direction = direction;
            }
            if node.row_gap != px(24) {
                node.row_gap = px(24);
            }
        }
    }
    for mut node in &mut columns {
        let width = percent(if narrow { 100.0 } else { 50.0 });
        if node.width != width {
            node.width = width;
        }
    }
}

fn fit_scrollbars(
    mut commands: Commands,
    targets: Query<(&ComputedNode, Has<bevy::ui_widgets::ScrollArea>)>,
    mut bars: Query<(&MenuScrollbar, &mut Node)>,
) {
    for (bar, mut node) in &mut bars {
        if let Ok((target, scrolling)) = targets.get(bar.0) {
            let overflow = target.content_size().y > target.size().y + 1.0;
            // Empty lists should let wheel events reach the page underneath.
            if overflow && !scrolling {
                commands.entity(bar.0).insert(bevy::ui_widgets::ScrollArea);
            }
            if !overflow && scrolling {
                commands
                    .entity(bar.0)
                    .remove::<bevy::ui_widgets::ScrollArea>();
            }
            let display = if overflow {
                Display::Flex
            } else {
                Display::None
            };
            if node.display != display {
                node.display = display;
            }
        }
    }
}

fn poll_lobbies(
    mut commands: Commands,
    mut state: ResMut<TitleState>,
    mut screen: Option<ResMut<TitleScreen>>,
    lists: Query<(Entity, Option<&Children>), With<LobbyList>>,
) {
    let result = state
        .discovery
        .as_ref()
        .and_then(|rx| rx.lock().ok()?.try_recv().ok());
    let Some((entries, message)) = result else {
        return;
    };
    state.discovery = None;
    state.entries = entries;
    state.selected = None;
    if let Some(screen) = screen.as_mut() {
        screen.status = Some(message);
    }
    for (parent, children) in &lists {
        if let Some(children) = children {
            for child in children.iter() {
                commands.entity(child).despawn();
            }
        }
        if state.entries.is_empty() {
            commands.spawn((
                ChildOf(parent),
                Text::new("No lobbies found. Create one, then refresh here."),
                TextFont {
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
            ));
        }
        for (index, entry) in state.entries.iter().enumerate() {
            let label = format!(
                "{} | {}{}{}",
                entry.name,
                if entry.lan { "LAN" } else { "Public" },
                if entry.locked { " | Password" } else { "" },
                if entry.compatible() {
                    ""
                } else {
                    " | Version mismatch"
                }
            );
            let row = button(&mut commands, parent, &label, Choice::Join(index));
            if !entry.lan || !entry.compatible() {
                commands.entity(row).insert(bevy::ui::InteractionDisabled);
            }
        }
    }
}

fn blue_changed(change: On<ValueChange<bool>>, mut state: ResMut<TitleState>) {
    state.blue = change.value;
}

fn spectate_changed(change: On<ValueChange<bool>>, mut state: ResMut<TitleState>) {
    state.spectate = change.value;
}

fn prefill(mut commands: Commands, mut inputs: Query<(Entity, &mut EditableText, &Prefill)>) {
    for (entity, mut text, prefill) in &mut inputs {
        text.queue_edit(TextEdit::SelectAll);
        text.queue_edit(TextEdit::Insert(prefill.0.clone().into()));
        commands.entity(entity).remove::<Prefill>();
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn title_input(
    mut commands: Commands,
    mut state: ResMut<TitleState>,
    keys: Res<ButtonInput<KeyCode>>,
    base: Res<BaseArgs>,
    mut screen: ResMut<TitleScreen>,
    mut ui: Option<ResMut<crate::hud::HudState>>,
    inputs: Query<(
        Entity,
        &EditableText,
        Has<NameInput>,
        Has<AddressInput>,
        Has<HostInput>,
        Option<&LobbyInput>,
    )>,
    focus: Res<bevy::input_focus::InputFocus>,
    controls: Query<(), Or<(With<bevy::ui_widgets::Checkbox>, With<TitleButton>)>>,
) {
    if ui.as_ref().is_some_and(|ui| ui.blocks_input()) {
        state.actions.clear();
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        let choice = if state.multiplayer {
            Choice::Back
        } else {
            Choice::Quit
        };
        state.actions.push(choice);
    }
    if (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter))
        && !focus.get().is_some_and(|entity| controls.contains(entity))
    {
        let choice = if state.multiplayer {
            Choice::Connect
        } else {
            Choice::Practice
        };
        state.actions.push(choice);
    }
    let Some(mut choice) = state.actions.drain(..).next() else {
        return;
    };
    if choice == Choice::Quit {
        if let Some(ui) = ui.as_mut() {
            ui.quit_confirm = true;
            ui.consumed = true;
        }
        return;
    }
    if choice == Choice::DismissTip {
        state.tip = false;
        return;
    }
    if choice == Choice::Back {
        state.multiplayer = false;
        return;
    }
    if choice == Choice::Multiplayer {
        state.multiplayer = true;
        state.tip = true;
        choice = Choice::Refresh;
    }
    let mut fields = TitleFields {
        public: state.public,
        blue: state.blue,
        spectate: state.spectate,
        ..default()
    };
    for (_, text, name, address, host, lobby) in &inputs {
        let value = text.value().to_string();
        if let Some(lobby) = lobby {
            match lobby {
                LobbyInput::Directory => fields.lobby_host = value,
                LobbyInput::Name => fields.lobby_name = value,
                LobbyInput::Password => fields.password = value,
                LobbyInput::JoinPassword => fields.join_password = value,
                LobbyInput::Advertised => fields.advertised = value,
                LobbyInput::FireRate => fields.fire_rate = value,
                LobbyInput::PhysicsRate => fields.physics_rate = value,
                LobbyInput::MaxFireRate => fields.max_fire_rate = value,
                LobbyInput::MaxMuzzleSpeed => fields.max_muzzle_speed = value,
                LobbyInput::MuzzleSpeed => fields.muzzle_speed = value,
                LobbyInput::SpeedVariation => fields.speed_variation = value,
                LobbyInput::Caliber => fields.caliber = value,
                LobbyInput::Spread => fields.spread = value,
                LobbyInput::Distribution => fields.distribution = value,
                LobbyInput::Seed => fields.seed = value,
            }
        } else if name {
            fields.name = value;
        } else if address {
            fields.address = value;
        } else if host {
            fields.host = value;
        }
    }
    if choice == Choice::Refresh {
        if state.discovery.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            state.discovery = Some(std::sync::Mutex::new(rx));
            screen.status = Some("Searching LAN lobbies...".into());
            std::thread::spawn(move || {
                let mut entries = Vec::new();
                let mut errors = Vec::new();
                match rm_simulator_server::lobby::lan_lobbies() {
                    Ok(lan) => entries.extend(lan),
                    Err(e) => errors.push(format!("LAN: {e}")),
                }
                entries.sort_by(|a, b| a.name.cmp(&b.name).then(a.address.cmp(&b.address)));
                let message = if errors.is_empty() {
                    format!("{} lobbies found", entries.len())
                } else {
                    errors.join("; ")
                };
                let _ = tx.send((entries, message));
            });
        }
        return;
    }
    let mut base = base.0.clone();
    if let Choice::Join(index) = choice {
        let Some(entry) = state.entries.get(index) else {
            return;
        };
        if !entry.lan || !entry.compatible() {
            screen.status = Some(
                if entry.protocol != rm_simulator_server::protocol::PROTOCOL_VERSION {
                    rm_simulator_server::protocol::version_mismatch(
                        entry.protocol,
                        rm_simulator_server::protocol::PROTOCOL_VERSION,
                    )
                } else {
                    "This lobby is unavailable or uses an unsupported transport.".into()
                },
            );
            return;
        }
        for (entity, _, _, address, _, _) in &inputs {
            if address {
                commands
                    .entity(entity)
                    .insert(Prefill(entry.address.clone()));
            }
        }
        screen.status = Some(format!(
            "Selected {}. {}Press Join lobby / address to connect.",
            entry.name,
            if entry.locked {
                "Enter its password, then "
            } else {
                ""
            }
        ));
        state.selected = Some(index);
        return;
    }
    if choice == Choice::Connect
        && let Some(entry) = state.selected.and_then(|i| state.entries.get(i))
        && entry.address == fields.address.trim()
    {
        base.transport = entry.transport().expect("compatible selection");
    }
    match join_args(&base, &fields, choice) {
        Ok(args) => {
            if let Some(path) = remembered_path()
                && let Err(error) = save_remembered(&path, &fields)
            {
                warn!(
                    "cannot remember the title fields in {}: {error}",
                    path.display()
                );
            }
            screen.status = None;
            commands.insert_resource(JoinRequest(args));
        }
        Err(message) => screen.status = Some(message),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn weapon_fields_apply_to_hosts_and_are_ignored_when_joining() {
        let base = base();
        let mut fields = TitleFields::initial(&base, None);
        fields.fire_rate = "25".into();
        fields.muzzle_speed = "30".into();
        fields.spread = "2".into();
        fields.distribution = "gaussian".into();
        fields.seed = "123".into();
        fields.speed_variation = "0.5".into();
        let args = join_args(&base, &fields, Choice::Practice).unwrap();
        assert_eq!(args.fire_rate_hz, 25.);
        assert_eq!(args.weapon().shot.speed_m_s, 30.);
        assert_eq!(args.weapon().spread.angle_rad, 2_f64.to_radians());
        assert_eq!(args.weapon().spread.seed, 123);
        assert_eq!(args.weapon().speed_variation_m_s, 0.5);
        fields.spread = "NaN".into();
        assert!(join_args(&base, &fields, Choice::Practice).is_err());
        fields.address = "localhost:7700".into();
        assert!(join_args(&base, &fields, Choice::Connect).is_ok());
    }

    use super::*;
    use clap::Parser;

    #[test]
    fn checkboxes_update_their_visual_state_and_join_choices_in_both_directions() {
        let mut app = App::new();
        app.init_resource::<TitleState>();
        let blue = app
            .world_mut()
            .spawn_empty()
            .observe(blue_changed)
            .observe(checkbox_self_update)
            .id();
        let spectate = app
            .world_mut()
            .spawn_empty()
            .observe(spectate_changed)
            .observe(checkbox_self_update)
            .id();
        for source in [blue, spectate] {
            for expected in [true, false, true, false] {
                let value = !app.world().entity(source).contains::<Checked>();
                app.world_mut().trigger(ValueChange {
                    source,
                    value,
                    is_final: true,
                });
                app.world_mut().flush();
                assert_eq!(app.world().entity(source).contains::<Checked>(), expected);
                let state = app.world().resource::<TitleState>();
                assert_eq!(
                    if source == blue {
                        state.blue
                    } else {
                        state.spectate
                    },
                    expected
                );
            }
        }
    }

    #[test]
    fn enter_on_a_checkbox_does_not_also_connect() {
        let mut app = App::new();
        app.init_resource::<TitleState>()
            .init_resource::<TitleScreen>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<bevy::input_focus::InputFocus>()
            .insert_resource(BaseArgs(base()))
            .add_message::<AppExit>()
            .add_systems(Update, title_input);
        let checkbox = app.world_mut().spawn(bevy::ui_widgets::Checkbox).id();
        app.world_mut()
            .resource_mut::<bevy::input_focus::InputFocus>()
            .set(checkbox, bevy::input_focus::FocusCause::Pressed);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Enter);
        app.update();
        assert!(app.world().resource::<TitleScreen>().status.is_none());
        assert!(!app.world().contains_resource::<JoinRequest>());
    }

    fn base() -> Args {
        Args::try_parse_from(["rm-simulator", "--name", "cli", "--team", "blue"]).unwrap()
    }

    #[test]
    fn remembered_fields_win_over_the_command_line_only_when_set() {
        let fields = TitleFields::initial(
            &base(),
            Some(TitleFields {
                name: "  ".into(),
                address: "10.0.0.2:7700".into(),
                ..default()
            }),
        );
        assert_eq!(fields.name, "cli");
        assert_eq!(fields.address, "10.0.0.2:7700");
        assert_eq!(fields.host, DEFAULT_LISTEN);
        assert!(fields.blue);
        assert!(!fields.spectate);
    }

    #[test]
    fn each_choice_sets_exactly_the_arguments_it_needs() {
        let fields = TitleFields {
            name: " alice ".into(),
            address: "host.local:7700 ".into(),
            lobby_name: "Test lobby".into(),
            host: String::new(),
            blue: false,
            spectate: true,
            ..default()
        };
        let connect = join_args(&base(), &fields, Choice::Connect).unwrap();
        assert_eq!(connect.name, "alice");
        assert_eq!(connect.connect.as_deref(), Some("host.local:7700"));
        assert_eq!(connect.listen, None);
        assert_eq!(connect.team, crate::args::TeamArg::Red);
        assert!(connect.fly);
        let host = join_args(&base(), &fields, Choice::Host).unwrap();
        assert_eq!(host.connect, None);
        assert_eq!(host.listen.as_deref(), Some(DEFAULT_LISTEN));
        let practice = join_args(&base(), &fields, Choice::Practice).unwrap();
        assert_eq!((practice.connect, practice.listen), (None, None));
        assert!(join_args(&base(), &fields, Choice::Quit).is_err());
    }

    #[test]
    fn missing_fields_are_reported_instead_of_joined() {
        let mut fields = TitleFields {
            name: "bob".into(),
            ..default()
        };
        assert!(
            join_args(&base(), &fields, Choice::Connect)
                .unwrap_err()
                .contains("address")
        );
        fields.name.clear();
        assert!(
            join_args(&base(), &fields, Choice::Practice)
                .unwrap_err()
                .contains("name")
        );
    }

    #[test]
    fn escape_returns_from_multiplayer_before_offering_to_quit() {
        let mut app = App::new();
        app.init_resource::<TitleScreen>()
            .init_resource::<crate::hud::HudState>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(TitleState {
                multiplayer: true,
                ..default()
            })
            .add_systems(Update, crate::hud::panel_input);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        assert!(!app.world().resource::<TitleState>().multiplayer);
        assert!(!app.world().resource::<crate::hud::HudState>().quit_confirm);
    }

    #[test]
    fn hosting_and_joining_use_separate_passwords_and_public_stays_disabled() {
        let fields = TitleFields {
            name: "pilot".into(),
            lobby_name: "Lobby".into(),
            address: "127.0.0.1:7700".into(),
            password: "host secret".into(),
            join_password: "join secret".into(),
            public: true,
            ..default()
        };
        assert_eq!(
            join_args(&base(), &fields, Choice::Host).unwrap().password,
            "host secret"
        );
        assert_eq!(
            join_args(&base(), &fields, Choice::Connect)
                .unwrap()
                .password,
            "join secret"
        );
        assert!(
            !join_args(&base(), &fields, Choice::Host)
                .unwrap()
                .public_lobby
        );
        let practice = join_args(&base(), &fields, Choice::Practice).unwrap();
        assert!(practice.password.is_empty() && practice.lobby_name.is_none());
    }

    #[test]
    fn remembered_fields_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("rm-title-{}", std::process::id()));
        let path = dir.join("nested").join("title.json");
        let fields = TitleFields {
            password: "never save this".into(),
            name: "carol".into(),
            address: "1.2.3.4:7700".into(),
            host: "0.0.0.0:7701".into(),
            blue: true,
            spectate: false,
            ..default()
        };
        save_remembered(&path, &fields).unwrap();
        let mut expected = fields;
        expected.password.clear();
        assert_eq!(load_remembered(&path), Some(expected));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(load_remembered(&path), None);
    }
}
