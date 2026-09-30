// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! The title screen: a vertical main menu of Single Player, Multiplayer,
//! Settings and Quit, and under Multiplayer the name, an address and the
//! lobbies. Every
//! choice is turned into the same arguments the command line would have
//! given, so the lifecycle in `loading.rs` has one entry. A choice that enters
//! a match connects first, then opens the 3D staging view. Solo and LAN
//! hosting share this form; only LAN hosting opens a network listener.
//! Player preferences are remembered in the user's configuration directory.
mod password;

use crate::{
    args::Args,
    loading::{JoinRequest, Screen},
};
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
    ui_widgets::{Activate, ActivateOnPress, ValueChange, checkbox_self_update},
};
use rm_simulator_server::protocol::{Chassis, Robot};
#[cfg(test)]
use rm_simulator_world::Caliber;
use rm_simulator_world::Team;
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
/// The player name a match starts with when none was given: Single Player
/// never asks for one, and the multiplayer page starts from it.
pub const DEFAULT_NAME: &str = "pilot";

/// Spectator slots the seat picker offers, above the two team columns.
pub const SPECTATOR_SLOTS: u8 = 5;
/// Referee slots the seat picker offers, under the two team columns.
#[cfg(test)]
pub const REFEREE_SLOTS: u8 = 3;

/// The seat a player takes in a match, as the seat picker offers it. The
/// picker is a lobby board: one slot per competition robot on each team, a row
/// of spectator slots above them and a row of referee slots below.
///
/// A spectator slot and a referee slot carry only their position on the board.
/// The protocol puts a spectator on a team, so the remembered team travels with
/// the join; the slot itself never reaches the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seat {
    /// Drive `robot` for `team`; the Hero fires 42 mm, the others 17 mm.
    Pilot {
        /// Team whose half the robot spawns on.
        team: Team,
        /// Robot driven, which fixes the caliber and the painted number.
        robot: Robot,
    },
    /// Watch with the free camera and no robot, from this spectator slot.
    Spectator {
        /// Position in the spectator row, below [`SPECTATOR_SLOTS`].
        slot: u8,
    },
    /// The referee: no robot, the free camera and the match keys.
    Referee {
        /// Position in the referee row, below [`REFEREE_SLOTS`].
        slot: u8,
    },
}
impl Default for Seat {
    fn default() -> Self {
        Seat::Pilot {
            team: Team::Red,
            robot: Robot::default(),
        }
    }
}
/// The pages of the title screen. It is a sub-state of [`Screen::Title`], so
/// leaving the title screen removes it and returning starts at `Main`.
#[derive(SubStates, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[source(Screen = Screen::Title)]
pub enum Page {
    /// Single Player, Multiplayer, Settings and Quit.
    #[default]
    Main,
    /// The LAN lobby list, the direct address and the hosting fields.
    Multiplayer,
}

/// One of the buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Open the multiplayer page and start a LAN search.
    Multiplayer,
    /// Leave the multiplayer page for the main menu.
    Back,
    /// Search the LAN for lobbies on a worker thread.
    Refresh,
    /// Show LAN connection help.
    ShowTip,
    /// Hide the firewall tip.
    DismissTip,
    /// Pick the discovered lobby at this index in the last listing. An entry
    /// that is not on the LAN or not compatible is refused with a status.
    Join(usize),
    /// Join the address field; a picked lobby prefills the address it
    /// advertised.
    Connect,
    /// Host a named lobby on the address the fields give.
    Host,
    /// Start an embedded host with no network listener.
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
    /// Which spectator slot on the seat picker was taken, below
    /// [`SPECTATOR_SLOTS`]. The board position only; the wire carries the team.
    #[serde(default)]
    pub spectator_slot: u8,
    /// Robot the pilot drives, by its `Robot::id`; blank means the Infantry 3.
    #[serde(default)]
    pub robot: String,
    /// Remembered drivetrain; Auto follows the selected robot.
    #[serde(default)]
    pub chassis: Chassis,
    /// Join as the referee: no robot, the free camera and the match keys.
    /// Skipped by serde, so no later match starts refereed by accident.
    #[serde(skip)]
    pub referee: bool,
    /// Which referee slot on the seat picker was taken, below
    /// [`REFEREE_SLOTS`]. Skipped with `referee`, for the same reason.
    #[serde(skip)]
    pub referee_slot: u8,
    /// Remembered host weapon fire rate text.
    #[serde(default)]
    pub fire_rate: String,
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
            name: pick(remembered.name, Some(base.name.clone()), DEFAULT_NAME),
            address: pick(remembered.address, base.connect.clone(), ""),
            host: pick(remembered.host, base.host.listen.clone(), DEFAULT_LISTEN),
            blue: base.team == crate::args::TeamArg::Blue || remembered.blue,
            spectate: base.fly || remembered.spectate,
            // A slot a shrinking board no longer has falls back to the first.
            spectator_slot: if remembered.spectator_slot < SPECTATOR_SLOTS {
                remembered.spectator_slot
            } else {
                0
            },
            robot: if base.robot != Robot::default() {
                base.robot.id().into()
            } else {
                pick(remembered.robot, None, Robot::default().id())
            },
            chassis: if base.chassis != Chassis::Auto {
                base.chassis
            } else {
                remembered.chassis
            },
            referee: base.referee,
            referee_slot: 0,
            max_fire_rate: pick(
                remembered.max_fire_rate,
                Some(base.host.max_fire_rate_hz.to_string()),
                "30",
            ),
            max_muzzle_speed: pick(
                remembered.max_muzzle_speed,
                Some(base.host.max_muzzle_speed_m_s.to_string()),
                "30",
            ),
            fire_rate: pick(
                remembered.fire_rate,
                Some(base.host.fire_rate_hz.to_string()),
                "20",
            ),
            muzzle_speed: pick(
                remembered.muzzle_speed,
                Some(
                    base.host
                        .muzzle_speed_m_s
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                ),
                "",
            ),
            speed_variation: pick(
                remembered.speed_variation,
                Some(base.host.muzzle_speed_variation_m_s.to_string()),
                "0",
            ),
            spread: pick(
                remembered.spread,
                Some(base.host.spread_deg.to_string()),
                "0",
            ),
            distribution: pick(
                remembered.distribution,
                Some(
                    match base.host.spread_distribution {
                        rm_simulator_server::protocol::SpreadDistribution::Uniform => "uniform",
                        rm_simulator_server::protocol::SpreadDistribution::Gaussian => "gaussian",
                    }
                    .into(),
                ),
                "gaussian",
            ),
            seed: pick(
                remembered.seed,
                Some(base.host.spread_seed.to_string()),
                "0",
            ),
        }
    }
    /// The seat the spectate, referee, slot and robot fields describe. An
    /// unknown robot text is the default robot.
    pub fn seat(&self) -> Seat {
        if self.referee {
            Seat::Referee {
                slot: self.referee_slot,
            }
        } else if self.spectate {
            Seat::Spectator {
                slot: self.spectator_slot,
            }
        } else {
            Seat::Pilot {
                team: if self.blue { Team::Blue } else { Team::Red },
                robot: Robot::parse(self.robot.trim()).unwrap_or_default(),
            }
        }
    }
    /// Write a seat into the fields `seat` reads. A spectator and the referee
    /// keep the remembered team and robot, so leaving that seat restores them.
    pub fn set_seat(&mut self, seat: Seat) {
        self.referee = matches!(seat, Seat::Referee { .. });
        self.spectate = matches!(seat, Seat::Spectator { .. });
        match seat {
            Seat::Pilot { team, robot } => {
                self.blue = team == Team::Blue;
                self.robot = robot.id().into();
            }
            Seat::Spectator { slot } => self.spectator_slot = slot,
            Seat::Referee { slot } => self.referee_slot = slot,
        }
    }
}

/// The arguments a choice joins with, or what is missing for it.
pub fn join_args(base: &Args, fields: &TitleFields, choice: Choice) -> Result<Args, String> {
    let mut args = base.clone();
    let name = fields.name.trim();
    // Only multiplayer asks for a name, so a local match falls back instead.
    let name = match (name.is_empty(), choice) {
        (false, _) => name,
        (true, Choice::Practice) => DEFAULT_NAME,
        (true, _) => return Err("Enter a name".into()),
    };
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
    args.referee = fields.referee;
    args.robot = if fields.robot.trim().is_empty() {
        Robot::default()
    } else {
        Robot::parse(fields.robot.trim()).ok_or(
            "Pick a robot: hero, engineer, infantry-3, infantry-4, infantry-5, sentry or drone",
        )?
    };
    args.chassis = fields.chassis;
    if !args.fly && !args.referee {
        args.chassis.config(args.robot)?;
    }

    match choice {
        Choice::Connect => {
            let address = fields.address.trim();
            if address.is_empty() {
                return Err("Enter the host address as HOST:PORT".into());
            }
            args.connect = Some(address.into());
            args.host.listen = None;
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
            args.host.listen = Some(if host.is_empty() {
                DEFAULT_LISTEN.into()
            } else {
                host.into()
            });
        }
        Choice::Practice => {
            args.connect = None;
            args.host.listen = None;
            args.password.clear();
        }
        _ => return Err("nothing to join".into()),
    }
    if choice != Choice::Connect {
        if !fields.fire_rate.trim().is_empty() {
            args.host.fire_rate_hz = fields
                .fire_rate
                .trim()
                .parse()
                .map_err(|_| "Enter a fire rate in Hz")?;
        }
        if !args.host.fire_rate_hz.is_finite() || !(0.1..=1000.0).contains(&args.host.fire_rate_hz)
        {
            return Err("Fire rate must be in [0.1, 1000] Hz".into());
        }
        args.host.muzzle_speed_m_s = if fields.muzzle_speed.trim().is_empty() {
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
            args.host.spread_deg = fields
                .spread
                .trim()
                .parse()
                .map_err(|_| "Enter a spread angle in degrees")?;
        }
        if !fields.distribution.trim().is_empty() {
            args.host.spread_distribution = match fields.distribution.trim() {
                "uniform" => rm_simulator_server::protocol::SpreadDistribution::Uniform,
                "gaussian" => rm_simulator_server::protocol::SpreadDistribution::Gaussian,
                _ => return Err("Spread distribution must be uniform or gaussian".into()),
            };
        }
        if !fields.seed.trim().is_empty() {
            args.host.spread_seed = fields
                .seed
                .trim()
                .parse()
                .map_err(|_| "Spread seed must be a nonnegative integer")?;
        }
        if !fields.speed_variation.trim().is_empty() {
            args.host.muzzle_speed_variation_m_s = fields
                .speed_variation
                .trim()
                .parse()
                .map_err(|_| "Enter muzzle-speed variation from 0 to 1 m/s")?;
        }
        if !fields.max_fire_rate.trim().is_empty() {
            args.host.max_fire_rate_hz = fields
                .max_fire_rate
                .trim()
                .parse()
                .map_err(|_| "Enter a maximum rate in Hz")?;
        }
        if !args.host.max_fire_rate_hz.is_finite()
            || !(0.1..=1000.0).contains(&args.host.max_fire_rate_hz)
        {
            return Err("Maximum fire rate must be in [0.1, 1000] Hz".into());
        }
        if !fields.max_muzzle_speed.trim().is_empty() {
            args.host.max_muzzle_speed_m_s = fields
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

// One writer keeps successive joins in order without writing on Bevy's frame.
fn remember_in_background(path: PathBuf, fields: TitleFields) {
    type SaveRequest = (PathBuf, TitleFields);
    static WRITER: std::sync::OnceLock<Option<std::sync::mpsc::Sender<SaveRequest>>> =
        std::sync::OnceLock::new();
    let sender = WRITER.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::channel::<SaveRequest>();
        match std::thread::Builder::new()
            .name("title-save".into())
            .spawn(move || {
                for (path, fields) in receiver {
                    if let Err(error) = save_remembered(&path, &fields) {
                        warn!(
                            "cannot remember the title fields in {}: {error}",
                            path.display()
                        );
                    }
                }
            }) {
            Ok(_) => Some(sender),
            Err(error) => {
                warn!("cannot start title save: {error}");
                None
            }
        }
    });
    if let Some(sender) = sender
        && sender.send((path, fields)).is_err()
    {
        warn!("title save worker stopped unexpectedly");
    }
}

#[derive(Component)]
struct TitleRoot;
#[derive(Component)]
struct TitleCard;
#[derive(Component)]
struct TitleScroll;
#[derive(Component)]
struct MenuScrollbar(Entity);

#[derive(Component)]
struct PreviewProgress;
#[derive(Component)]
struct PreviewCaption;

/// A quiet pointer-controlled overview; it owns the camera only on the title screen.
fn preview_camera(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut cameras: Query<&mut Transform, With<crate::controls::PlayerCamera>>,
    time: Res<Time>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let pointer = window.cursor_position().unwrap_or(window.size() * 0.5)
        / window.size().max(Vec2::ONE)
        - Vec2::splat(0.5);
    let eye = rm_simulator_render::flu_position([
        4.0 + pointer.x as f64 * 2.0,
        -12.0,
        9.0 + pointer.y as f64 * 1.5,
    ]);
    let target = Transform::from_translation(eye).looking_at(Vec3::ZERO, Vec3::Y);
    for mut camera in &mut cameras {
        let blend = 1.0 - (-3.0 * time.delta_secs()).exp();
        camera.translation = camera.translation.lerp(target.translation, blend);
        camera.rotation = camera.rotation.slerp(target.rotation, blend);
    }
}

fn preview_progress(world: &mut World) {
    let (progress, caption) = crate::loading::preload_status(world);
    for mut text in world
        .query_filtered::<&mut Text, With<PreviewCaption>>()
        .iter_mut(world)
    {
        if text.0 != caption {
            text.0.clone_from(&caption);
        }
    }
    for mut node in world
        .query_filtered::<&mut Node, With<PreviewProgress>>()
        .iter_mut(world)
    {
        node.width = percent(progress * 100.0);
    }
}
#[derive(Component)]
enum TitleInput {
    Directory,
    PlayerName,
    Address,
    Host,
    Name,
    Password,
    JoinPassword,
    Advertised,
    FireRate,
    MaxFireRate,
    MaxMuzzleSpeed,
    MuzzleSpeed,
    SpeedVariation,
    Spread,
    Distribution,
    Seed,
}
impl TitleInput {
    fn value_mut<'a>(&self, fields: &'a mut TitleFields) -> &'a mut String {
        match self {
            Self::Directory => &mut fields.lobby_host,
            Self::PlayerName => &mut fields.name,
            Self::Address => &mut fields.address,
            Self::Host => &mut fields.host,
            Self::Name => &mut fields.lobby_name,
            Self::Password => &mut fields.password,
            Self::JoinPassword => &mut fields.join_password,
            Self::Advertised => &mut fields.advertised,
            Self::FireRate => &mut fields.fire_rate,
            Self::MaxFireRate => &mut fields.max_fire_rate,
            Self::MaxMuzzleSpeed => &mut fields.max_muzzle_speed,
            Self::MuzzleSpeed => &mut fields.muzzle_speed,
            Self::SpeedVariation => &mut fields.speed_variation,
            Self::Spread => &mut fields.spread,
            Self::Distribution => &mut fields.distribution,
            Self::Seed => &mut fields.seed,
        }
    }

    fn submits_host(&self) -> bool {
        matches!(
            self,
            Self::Host
                | Self::Name
                | Self::Password
                | Self::Advertised
                | Self::FireRate
                | Self::MaxFireRate
                | Self::MaxMuzzleSpeed
                | Self::MuzzleSpeed
                | Self::SpeedVariation
                | Self::Spread
                | Self::Distribution
                | Self::Seed
        )
    }
}
#[derive(Component)]
struct WeaponFields;
/// Host settings shared by solo and LAN setup, hidden from remote joins.
#[derive(Component)]
struct HostSettings;
#[derive(Component)]
struct MenuPage(Page);
/// A row of two columns that stacks on a narrow window.
#[derive(Component)]
struct Columns;
#[derive(Component)]
struct LobbyList;
#[derive(Component)]
struct LobbyColumn;
#[derive(Component)]
struct FirewallTip;
/// Network help shown with the multiplayer fields and on the multiplayer
/// loading splash: allow the app through the firewall and use one network.
pub const FIREWALL_TIP: &str = "Can’t find or join a LAN lobby? Allow RM Simulator through your firewall on private networks. Both players must be on the same network; guest Wi-Fi may block connections.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatusLocation {
    General,
    Join,
    Host,
    Search,
}
#[derive(Component)]
struct StatusText(StatusLocation);

fn status_text(commands: &mut Commands, parent: Entity, location: StatusLocation) {
    commands.spawn((
        ChildOf(parent),
        StatusText(location),
        Text::default(),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.7, 0.82, 0.85)),
        Node {
            display: Display::None,
            flex_shrink: 0.0,
            ..default()
        },
    ));
}
/// Text to put into an input once its editor exists.
#[derive(Component)]
struct Prefill(String);
#[derive(Component)]
struct TitleButton;
/// Pressed buttons waiting to be acted on, and the checkbox states, kept
/// here because a checkbox reports changes only.
type DiscoveryResult = (Vec<rm_simulator_server::lobby::Listing>, String, bool);

/// Title screen state the text fields do not hold: the open page, the choice
/// the robot page was opened for, the chosen seat, the last LAN listing, a
/// search in flight and the buttons pressed this frame.
#[derive(Resource, Default)]
pub(crate) struct TitleState {
    seat: Seat,
    chassis: Chassis,
    tip: bool,
    public: bool,
    selected: Option<usize>,
    entries: Vec<rm_simulator_server::lobby::Listing>,
    discovery: Option<std::sync::Mutex<std::sync::mpsc::Receiver<DiscoveryResult>>>,
    actions: Vec<Choice>,
    feedback: Option<(StatusLocation, bool, String)>,
    search_status: Option<String>,
    search_failed: bool,
}

impl TitleState {
    /// Leave `page` for the one under it: the robot page for the page its
    /// choice was pressed on, the multiplayer page for the main menu.
    /// Returns whether a page was open, so Escape offers to quit only from the
    /// main page.
    pub(crate) fn escape_back(&mut self, page: Page, next: &mut NextState<Page>) -> bool {
        if self.tip && page == Page::Multiplayer {
            self.tip = false;
            return true;
        }
        self.feedback = None;
        match page {
            Page::Main => false,
            Page::Multiplayer => {
                next.set(Page::Main);
                true
            }
        }
    }
}

/// Installs the title resources, the `Page` sub-state, the systems that build
/// and tear the screen down with [`Screen::Title`], and the chained update
/// systems that dress it and act on its buttons.
pub struct TitlePlugin;
impl Plugin for TitlePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(password::PasswordPlugin)
            .init_resource::<TitleState>()
            .init_resource::<TitleScreen>()
            .add_sub_state::<Page>()
            .add_systems(OnEnter(Screen::Title), spawn_title)
            .add_systems(OnExit(Screen::Title), despawn_title)
            .add_systems(
                Update,
                (
                    update_status,
                    poll_lobbies,
                    show_page,
                    sync_page_navigation.run_if(state_changed::<Page>),
                    fit_columns,
                    fit_scrollbars,
                    prefill,
                    preview_camera,
                    preview_progress,
                    title_input.after(crate::hud::panel_input),
                )
                    .chain()
                    .run_if(in_state(Screen::Title)),
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

fn field(
    commands: &mut Commands,
    parent: Entity,
    label: &'static str,
    fields: &mut TitleFields,
    marker: TitleInput,
) -> Entity {
    let value = marker.value_mut(fields).clone();
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
        .insert((
            ChildOf(container),
            marker,
            Prefill(value),
            AccessibleLabel(label.into()),
        ))
        .id()
}

/// The title screen belongs to [`Screen::Title`] and never outlives it.
fn despawn_title(mut commands: Commands, roots: Query<Entity, With<TitleRoot>>) {
    for root in &roots {
        commands.entity(root).despawn();
    }
}

/// Write the reason the last match ended, the form errors and the search
/// progress into the status line each belongs to.
fn update_status(
    screen: Res<TitleScreen>,
    state: Res<TitleState>,
    mut status: Query<(&StatusText, &mut Text, &mut TextColor, &mut Node)>,
) {
    {
        for (location, mut text, mut color, mut node) in &mut status {
            let feedback = state
                .feedback
                .as_ref()
                .filter(|(at, _, _)| *at == location.0);
            let (message, error) = if let Some((_, error, message)) = feedback {
                (Some(message.as_str()), *error)
            } else if location.0 == StatusLocation::General {
                (screen.status.as_deref(), true)
            } else if location.0 == StatusLocation::Search {
                (state.search_status.as_deref(), state.search_failed)
            } else {
                (None, false)
            };
            let value = message.unwrap_or_default();
            if text.0 != value {
                text.0 = value.into();
            }
            let next = if error {
                Color::srgb(1.0, 0.62, 0.52)
            } else {
                Color::srgb(0.7, 0.82, 0.85)
            };
            if color.0 != next {
                color.0 = next;
            }
            node.display = if value.is_empty() {
                Display::None
            } else {
                Display::Flex
            };
        }
    }
}

/// Build the whole title screen on entering [`Screen::Title`]: every page is
/// spawned and `show_page` displays the current one, so a choice made on one
/// page can still read the fields of another.
fn spawn_title(mut commands: Commands, base: Res<BaseArgs>, mut state: ResMut<TitleState>) {
    let mut fields = TitleFields::initial(
        &base.0,
        remembered_path().and_then(|path| load_remembered(&path)),
    );
    state.feedback = None;
    state.search_status = None;
    state.public = fields.public;
    state.seat = fields.seat();
    state.chassis = fields.chassis;
    if let Seat::Pilot { robot, .. } = state.seat
        && state.chassis.config(robot).is_err()
    {
        state.chassis = Chassis::Auto;
    }
    let root = commands
        .spawn((
            TitleRoot,
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
            BackgroundColor(Color::srgba(0.035, 0.05, 0.075, 0.25)),
        ))
        .id();
    commands.spawn((
        ChildOf(root),
        PreviewCaption,
        Text::new("Preparing field preview…"),
        TextFont {
            font_size: FontSize::Px(13.0),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            bottom: px(14),
            ..default()
        },
    ));
    commands.spawn((
        ChildOf(root),
        PreviewProgress,
        Node {
            position_type: PositionType::Absolute,
            bottom: px(0),
            left: px(0),
            height: px(3),
            width: percent(0),
            ..default()
        },
        BackgroundColor(Color::srgb(0.1, 0.75, 0.85)),
    ));
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
            BackgroundColor(Color::srgba(0.035, 0.05, 0.065, 0.90)),
            BorderColor::all(Color::srgb(0.19, 0.52, 0.56)),
        ))
        .id();
    let viewport = commands
        .spawn((
            ChildOf(frame),
            TitleScroll,
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
    status_text(&mut commands, card, StatusLocation::General);
    // The main menu is one vertical stack; the name belongs to multiplayer.
    let row = commands
        .spawn((
            ChildOf(card),
            MenuPage(Page::Main),
            Node {
                width: percent(100),
                margin: UiRect::top(px(10)),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
                row_gap: px(10),
                ..default()
            },
        ))
        .id();
    button(&mut commands, row, "Single Player", Choice::Practice);
    button(&mut commands, row, "Multiplayer", Choice::Multiplayer);
    commands.spawn_scene(bsn! {
        @FeathersButton { @caption: bsn! { Text("Settings") ThemedText } }
        Node { height: px(38), flex_shrink: 0.0, padding: UiRect::horizontal(px(18)) }
        AccessibleLabel("Settings")
        ActivateOnPress
        on(|_: On<Activate>, mut ui: ResMut<crate::hud::HudState>, mut focus: ResMut<bevy::input_focus::InputFocus>| {
            ui.close(); ui.settings = true; ui.consumed = true; focus.clear();
        })
    }).insert(ChildOf(row));
    button(&mut commands, row, "Quit", Choice::Quit);

    let multiplayer = commands
        .spawn((
            ChildOf(card),
            MenuPage(Page::Multiplayer),
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Column,
                row_gap: px(12),
                display: Display::None,
                ..default()
            },
        ))
        .id();
    // The name is asked for here, where it is sent: joining or hosting.
    let identity = commands
        .spawn((
            ChildOf(multiplayer),
            Node {
                width: percent(50),
                min_width: px(0),
                flex_direction: FlexDirection::Column,
                ..default()
            },
        ))
        .id();
    field(
        &mut commands,
        identity,
        "Name",
        &mut fields,
        TitleInput::PlayerName,
    );
    let columns = commands
        .spawn((
            ChildOf(multiplayer),
            Columns,
            Node {
                width: percent(100),
                column_gap: px(24),
                ..default()
            },
        ))
        .id();
    let left = commands
        .spawn((
            ChildOf(columns),
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
        &mut fields,
        TitleInput::Directory,
    );
    let discovery_actions = commands
        .spawn((
            ChildOf(left),
            Node {
                column_gap: px(10),
                row_gap: px(6),
                flex_wrap: FlexWrap::Wrap,
                ..default()
            },
        ))
        .id();
    button(
        &mut commands,
        discovery_actions,
        "Refresh LAN",
        Choice::Refresh,
    );
    button(
        &mut commands,
        discovery_actions,
        "LAN help",
        Choice::ShowTip,
    );
    status_text(&mut commands, left, StatusLocation::Search);
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
                height: px(140),
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
        &mut fields,
        TitleInput::Address,
    );
    let join_password = field(
        &mut commands,
        left,
        "Lobby password (optional)",
        &mut fields,
        TitleInput::JoinPassword,
    );
    password::attach(&mut commands, left, join_password);
    button(&mut commands, left, "Join lobby / address", Choice::Connect);
    status_text(&mut commands, left, StatusLocation::Join);
    let right = commands
        .spawn((
            ChildOf(columns),
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
        &mut fields,
        TitleInput::Name,
    );
    let host_password = field(
        &mut commands,
        right,
        "Password (optional)",
        &mut fields,
        TitleInput::Password,
    );
    password::attach(&mut commands, right, host_password);
    let mut public = commands.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text("Public (unavailable)") ThemedText } }
        on(|change: On<ValueChange<bool>>, mut state: ResMut<TitleState>| { state.public = change.value; })
    });
    public.insert((ChildOf(right), bevy::ui::InteractionDisabled));

    field(
        &mut commands,
        right,
        "Listen on (ADDR:PORT)",
        &mut fields,
        TitleInput::Host,
    );
    field(
        &mut commands,
        future,
        "Public IP:PORT override (optional)",
        &mut fields,
        TitleInput::Advertised,
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
    status_text(&mut commands, right, StatusLocation::Host);
    button(&mut commands, right, "Back", Choice::Back);

    let host_settings = commands
        .spawn((
            ChildOf(card),
            HostSettings,
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Column,
                row_gap: px(10),
                ..default()
            },
        ))
        .id();
    commands.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text("Host weapon limits and defaults") ThemedText } }
        on(|change: On<ValueChange<bool>>, mut panels: Query<&mut Node, With<WeaponFields>>| {
            for mut panel in &mut panels { panel.display = if change.value { Display::Flex } else { Display::None }; }
        })
    }).insert(ChildOf(host_settings)).observe(checkbox_self_update);
    let weapon_fields = commands
        .spawn((
            ChildOf(host_settings),
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
        &mut fields,
        TitleInput::MaxFireRate,
    );
    field(
        &mut commands,
        weapon_fields,
        "Maximum muzzle speed (m/s)",
        &mut fields,
        TitleInput::MaxMuzzleSpeed,
    );
    field(
        &mut commands,
        weapon_fields,
        "Starting fire rate (Hz)",
        &mut fields,
        TitleInput::FireRate,
    );
    field(
        &mut commands,
        weapon_fields,
        "Starting muzzle speed (m/s, blank = 25)",
        &mut fields,
        TitleInput::MuzzleSpeed,
    );
    field(
        &mut commands,
        weapon_fields,
        "Default speed variation (+/- m/s, 0 to 1)",
        &mut fields,
        TitleInput::SpeedVariation,
    );
    field(
        &mut commands,
        weapon_fields,
        "Default spread half-angle (degrees, 0 = perfect)",
        &mut fields,
        TitleInput::Spread,
    );
    field(
        &mut commands,
        weapon_fields,
        "Default distribution (uniform or gaussian)",
        &mut fields,
        TitleInput::Distribution,
    );
    field(
        &mut commands,
        weapon_fields,
        "Default spread seed",
        &mut fields,
        TitleInput::Seed,
    );

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
    current: Res<State<Page>>,
    state: Res<TitleState>,
    mut pages: Query<
        (&MenuPage, &mut Node),
        (
            Without<FirewallTip>,
            Without<TitleCard>,
            Without<HostSettings>,
        ),
    >,
    mut tips: Query<&mut Node, (With<FirewallTip>, Without<TitleCard>, Without<HostSettings>)>,
    mut cards: Query<&mut Node, (With<TitleCard>, Without<HostSettings>)>,
    mut settings: Query<&mut Node, With<HostSettings>>,
) {
    let current = *current.get();
    let multiplayer = current == Page::Multiplayer;
    for mut node in &mut cards {
        let width = px(match current {
            Page::Main => 480.0,
            Page::Multiplayer => 1100.0,
        });
        let height = if multiplayer { percent(80) } else { Val::Auto };
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
        let display = if state.tip && multiplayer {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    for mut node in &mut settings {
        let display = if multiplayer {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }

    for (page, mut node) in &mut pages {
        let display = if page.0 == current {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
}

/// New pages start at the top and cannot keep typing into an input they hide.
/// Runs only when the `Page` sub-state changed.
fn sync_page_navigation(
    mut focus: ResMut<bevy::input_focus::InputFocus>,
    mut scrolls: Query<&mut ScrollPosition, With<TitleScroll>>,
) {
    focus.clear();
    for mut scroll in &mut scrolls {
        *scroll = default();
    }
}

#[allow(clippy::type_complexity)]
fn fit_columns(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut rows: Query<&mut Node, (With<Columns>, Without<LobbyColumn>)>,
    mut columns: Query<&mut Node, With<LobbyColumn>>,
) {
    let narrow = windows
        .iter()
        .next()
        .is_some_and(|window| window.width() < 900.0);
    for mut node in &mut rows {
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
    lists: Query<(Entity, Option<&Children>), With<LobbyList>>,
) {
    let result = state
        .discovery
        .as_ref()
        .and_then(|rx| rx.lock().ok()?.try_recv().ok());
    let Some((entries, message, failed)) = result else {
        return;
    };
    state.discovery = None;
    state.entries = entries;
    state.selected = None;
    state.search_status = Some(message);
    state.search_failed = failed;
    for (parent, children) in &lists {
        if let Some(children) = children {
            for child in children.iter() {
                commands.entity(child).despawn();
            }
        }
        if state.entries.is_empty() {
            commands.spawn((
                ChildOf(parent),
                Text::new("No lobbies found. Refresh after a host creates one, or enter its HOST:PORT below."),
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
    page: Res<State<Page>>,
    mut next_page: ResMut<NextState<Page>>,
    mut next_screen: ResMut<NextState<Screen>>,
    mut ui: Option<ResMut<crate::hud::HudState>>,
    inputs: Query<(Entity, &EditableText, &TitleInput)>,
    focus: Res<bevy::input_focus::InputFocus>,
    controls: Query<(), Or<(With<bevy::ui_widgets::Checkbox>, With<FeathersButton>)>>,
) {
    let page = *page.get();
    if ui.as_ref().is_some_and(|ui| ui.blocks_input()) {
        state.actions.clear();
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        let choice = if page == Page::Main {
            Choice::Quit
        } else {
            Choice::Back
        };
        state.actions.push(choice);
    }
    if (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter))
        && !focus.get().is_some_and(|entity| controls.contains(entity))
    {
        let choice = match page {
            Page::Main => Choice::Practice,
            Page::Multiplayer => {
                let hosting = focus
                    .get()
                    .and_then(|entity| inputs.get(entity).ok())
                    .is_some_and(|(_, _, input)| input.submits_host());
                if hosting {
                    Choice::Host
                } else {
                    Choice::Connect
                }
            }
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
    if matches!(choice, Choice::ShowTip | Choice::DismissTip) {
        state.tip = choice == Choice::ShowTip;
        return;
    }
    if choice == Choice::Back {
        state.feedback = None;
        state.escape_back(page, &mut next_page);
        return;
    }
    if choice == Choice::Multiplayer {
        next_page.set(Page::Multiplayer);
        state.tip = false;
        choice = Choice::Refresh;
    }
    let mut fields = TitleFields {
        public: state.public,
        ..default()
    };
    fields.set_seat(state.seat);
    fields.chassis = state.chassis;
    for (_, text, input) in &inputs {
        *input.value_mut(&mut fields) = text.value().to_string();
    }
    if choice == Choice::Refresh {
        if state.discovery.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            state.discovery = Some(std::sync::Mutex::new(rx));
            state.search_status = Some("Searching LAN lobbies…".into());
            state.search_failed = false;
            std::thread::spawn(move || {
                let mut entries = Vec::new();
                let mut errors = Vec::new();
                match rm_simulator_server::lobby::lan_lobbies() {
                    Ok(lan) => entries.extend(lan),
                    Err(e) => errors.push(format!("LAN: {e}")),
                }
                entries.sort_by(|a, b| a.name.cmp(&b.name).then(a.address.cmp(&b.address)));
                let failed = !errors.is_empty();
                let message = if errors.is_empty() {
                    format!(
                        "{} LAN {} found",
                        entries.len(),
                        if entries.len() == 1 {
                            "lobby"
                        } else {
                            "lobbies"
                        }
                    )
                } else {
                    errors.join("; ")
                };
                let _ = tx.send((entries, message, failed));
            });
        }
        return;
    }
    let base = base.0.clone();
    if let Choice::Join(index) = choice {
        let Some(entry) = state.entries.get(index) else {
            return;
        };
        if !entry.lan || !entry.compatible() {
            state.feedback = Some((
                StatusLocation::Join,
                true,
                if entry.protocol != rm_simulator_server::protocol::PROTOCOL_VERSION {
                    rm_simulator_server::protocol::version_mismatch(
                        entry.protocol,
                        rm_simulator_server::protocol::PROTOCOL_VERSION,
                    )
                } else {
                    "This lobby is unavailable or uses an unsupported transport.".into()
                },
            ));
            return;
        }
        for (entity, _, input) in &inputs {
            if matches!(input, TitleInput::Address) {
                commands
                    .entity(entity)
                    .insert(Prefill(entry.address.clone()));
            }
        }
        state.feedback = Some((
            StatusLocation::Join,
            false,
            format!(
                "Selected {}. {}Press Join lobby / address to connect.",
                entry.name,
                if entry.locked {
                    "Enter its password, then "
                } else {
                    ""
                }
            ),
        ));
        state.selected = Some(index);
        return;
    }
    state.feedback = None;
    match join_args(&base, &fields, choice) {
        Ok(mut args) => {
            if !cfg!(test)
                && let Some(path) = remembered_path()
            {
                remember_in_background(path, fields.clone());
            }
            screen.status = None;
            args.staging = true;
            commands.insert_resource(JoinRequest(args));
            next_screen.set(Screen::Loading);
        }
        Err(message) => {
            let location = if choice == Choice::Connect {
                StatusLocation::Join
            } else if choice == Choice::Host {
                StatusLocation::Host
            } else {
                StatusLocation::General
            };
            screen.status = None;
            state.feedback = Some((location, true, message));
        }
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
        assert_eq!(args.host.fire_rate_hz, 25.);
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

    /// The screen states a title-screen system needs, without the rest of the
    /// app. `Screen::Title` brings the `Page` sub-state up at `Main`.
    fn with_screens(app: &mut App) -> &mut App {
        app.add_plugins(bevy::state::app::StatesPlugin)
            .insert_state(Screen::Title)
            .add_sub_state::<Page>()
    }
    /// Apply a queued transition without running `Update` again, so a test
    /// that holds a key down does not act on it twice.
    fn settle(app: &mut App) {
        app.world_mut()
            .run_schedule(bevy::state::state::StateTransition);
    }
    fn page(app: &App) -> Page {
        *app.world().resource::<State<Page>>().get()
    }
    /// Open `to` as if its button had been pressed.
    fn open(app: &mut App, to: Page) {
        app.world_mut().resource_mut::<NextState<Page>>().set(to);
        settle(app);
    }

    #[test]
    fn the_pages_belong_to_the_title_screen_and_reopen_at_the_main_menu() {
        let mut app = App::new();
        with_screens(&mut app);
        app.update();
        assert_eq!(page(&app), Page::Main);
        open(&mut app, Page::Multiplayer);
        assert_eq!(page(&app), Page::Multiplayer);
        // Joining leaves the title screen, which takes its pages with it.
        app.world_mut()
            .resource_mut::<NextState<Screen>>()
            .set(Screen::Loading);
        settle(&mut app);
        assert!(app.world().get_resource::<State<Page>>().is_none());
        // Returning opens the main menu, whatever page was last shown.
        app.world_mut()
            .resource_mut::<NextState<Screen>>()
            .set(Screen::Title);
        settle(&mut app);
        assert_eq!(page(&app), Page::Main);
    }

    #[test]
    fn only_the_current_page_is_displayed() {
        let mut app = App::new();
        with_screens(&mut app)
            .init_resource::<TitleState>()
            .add_systems(Update, show_page);
        let main = app
            .world_mut()
            .spawn((MenuPage(Page::Main), Node::default()))
            .id();
        let multiplayer = app
            .world_mut()
            .spawn((MenuPage(Page::Multiplayer), Node::default()))
            .id();
        let settings = app.world_mut().spawn((HostSettings, Node::default())).id();
        let display = |app: &App, entity| app.world().get::<Node>(entity).unwrap().display;
        app.update();
        assert_eq!(display(&app, settings), Display::None);
        assert_eq!(display(&app, main), Display::Flex);
        assert_eq!(display(&app, multiplayer), Display::None);
        open(&mut app, Page::Multiplayer);
        app.update();
        assert_eq!(display(&app, main), Display::None);
        assert_eq!(display(&app, multiplayer), Display::Flex);
        assert_eq!(display(&app, settings), Display::Flex);
    }

    #[test]
    fn changing_title_pages_resets_scroll_and_hidden_input_focus() {
        let mut app = App::new();
        app.init_resource::<TitleState>()
            .init_resource::<bevy::input_focus::InputFocus>()
            .add_systems(Update, sync_page_navigation.run_if(state_changed::<Page>));
        with_screens(&mut app);
        let scroll = app
            .world_mut()
            .spawn((TitleScroll, ScrollPosition::default()))
            .id();
        let input = app.world_mut().spawn_empty().id();
        app.update();
        app.world_mut().get_mut::<ScrollPosition>(scroll).unwrap().y = 250.;
        app.world_mut()
            .resource_mut::<bevy::input_focus::InputFocus>()
            .set(input, bevy::input_focus::FocusCause::Pressed);
        app.update();
        assert_eq!(app.world().get::<ScrollPosition>(scroll).unwrap().y, 250.);
        open(&mut app, Page::Multiplayer);
        app.update();
        assert_eq!(app.world().get::<ScrollPosition>(scroll).unwrap().y, 0.);
        assert!(
            app.world()
                .resource::<bevy::input_focus::InputFocus>()
                .get()
                .is_none()
        );
    }

    #[test]
    fn search_results_do_not_replace_form_errors() {
        let mut app = App::new();
        app.init_resource::<TitleState>()
            .add_systems(Update, poll_lobbies);
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let mut state = app.world_mut().resource_mut::<TitleState>();
            state.feedback = Some((StatusLocation::Join, true, "Enter an address".into()));
            state.discovery = Some(std::sync::Mutex::new(rx));
        }
        tx.send((vec![], "0 LAN lobbies found".into(), false))
            .unwrap();
        app.update();
        let state = app.world().resource::<TitleState>();
        assert_eq!(state.feedback.as_ref().unwrap().2, "Enter an address");
        assert_eq!(state.search_status.as_deref(), Some("0 LAN lobbies found"));
    }

    #[test]
    fn practice_connects_before_opening_deployment() {
        let mut app = App::new();
        app.init_resource::<TitleState>()
            .init_resource::<TitleScreen>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<bevy::input_focus::InputFocus>()
            .insert_resource(BaseArgs(base()))
            .add_message::<AppExit>()
            .add_systems(Update, title_input);
        with_screens(&mut app);
        app.update();
        app.world_mut()
            .resource_mut::<TitleState>()
            .actions
            .push(Choice::Practice);
        app.update();
        let args = &app.world().resource::<JoinRequest>().0;
        assert!(args.staging);
        assert!(args.host.listen.is_none());
        assert!(args.connect.is_none());
        assert!(matches!(
            app.world().resource::<NextState<Screen>>(),
            NextState::Pending(Screen::Loading)
        ));
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
        with_screens(&mut app);
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
        assert_eq!(fields.robot, "infantry-3");
        assert_eq!(
            fields.seat(),
            Seat::Pilot {
                team: Team::Blue,
                robot: Robot::Infantry3
            }
        );
        // A remembered robot wins unless the command line named one.
        let remembered = Some(TitleFields {
            robot: "infantry-4".into(),
            ..default()
        });
        assert_eq!(
            TitleFields::initial(&base(), remembered.clone()).robot,
            "infantry-4"
        );
        let hero = Args::try_parse_from(["rm-simulator", "--robot", "hero", "--referee"]).unwrap();
        let fields = TitleFields::initial(&hero, remembered);
        assert_eq!(fields.robot, "hero");
        assert_eq!(fields.seat(), Seat::Referee { slot: 0 });
    }

    /// Every slot the seat board lays out, in its reading order: the
    /// spectator row, each team's line-up, then the referee row.
    fn board() -> Vec<Seat> {
        let mut seats: Vec<Seat> = (0..SPECTATOR_SLOTS)
            .map(|slot| Seat::Spectator { slot })
            .collect();
        for team in [Team::Blue, Team::Red] {
            seats.extend(Robot::ALL.map(|robot| Seat::Pilot { team, robot }));
        }
        seats.extend((0..REFEREE_SLOTS).map(|slot| Seat::Referee { slot }));
        seats
    }

    #[test]
    fn the_board_offers_one_slot_per_competition_robot_and_no_two_are_the_same() {
        let seats = board();
        // 5 spectators, a full line-up on each team, 3 referees.
        assert_eq!(seats.len(), 5 + 2 * 7 + 3);
        for team in [Team::Blue, Team::Red] {
            let line_up: Vec<Robot> = seats
                .iter()
                .filter_map(|seat| match seat {
                    Seat::Pilot { team: t, robot } if *t == team => Some(*robot),
                    _ => None,
                })
                .collect();
            let count = |kind| line_up.iter().filter(|robot| robot.kind() == kind).count();
            use rm_simulator_world::RobotKind;
            assert_eq!(count(RobotKind::Hero), 1);
            assert_eq!(count(RobotKind::Engineer), 1);
            assert_eq!(count(RobotKind::Infantry), 3);
            assert_eq!(count(RobotKind::Sentry), 1);
            assert_eq!(count(RobotKind::Drone), 1);
        }
        // A slot is what identifies a seat, so two of them must never match.
        for (index, seat) in seats.iter().enumerate() {
            assert!(!seats[..index].contains(seat), "{seat:?} appears twice");
        }
    }

    #[test]
    fn every_board_slot_survives_the_fields_it_is_remembered_in() {
        let mut fields = TitleFields::default();
        for seat in board() {
            fields.set_seat(seat);
            assert_eq!(fields.seat(), seat);
        }
    }

    #[test]
    fn seats_round_trip_through_the_fields_and_keep_the_pilot_choice_under_the_referee() {
        let mut fields = TitleFields::default();
        for seat in [
            Seat::Pilot {
                team: Team::Blue,
                robot: Robot::Hero,
            },
            Seat::Pilot {
                team: Team::Red,
                robot: Robot::Infantry5,
            },
            Seat::Spectator { slot: 4 },
            Seat::Referee { slot: 2 },
            Seat::Spectator { slot: 0 },
        ] {
            fields.set_seat(seat);
            assert_eq!(fields.seat(), seat);
        }
        // The last pilot slot survives a spectator or referee seat, so going
        // back to a robot restores it rather than the board's default.
        fields.set_seat(Seat::Pilot {
            team: Team::Red,
            robot: Robot::Infantry4,
        });
        fields.set_seat(Seat::Referee { slot: 0 });
        assert_eq!(fields.robot, "infantry-4");
        assert!(!fields.spectate && !fields.blue);
        fields.referee = false;
        assert_eq!(
            fields.seat(),
            Seat::Pilot {
                team: Team::Red,
                robot: Robot::Infantry4
            }
        );
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
            robot: "hero".into(),
            ..default()
        };
        let connect = join_args(&base(), &fields, Choice::Connect).unwrap();
        assert_eq!(connect.name, "alice");
        assert_eq!(connect.connect.as_deref(), Some("host.local:7700"));
        assert_eq!(connect.host.listen, None);
        assert_eq!(connect.team, crate::args::TeamArg::Red);
        assert!(connect.fly);
        assert!(!connect.referee);
        assert_eq!(connect.robot, Robot::Hero);
        assert_eq!(
            connect.role(),
            rm_simulator_server::protocol::Role::Spectator
        );
        let mut referee = fields.clone();
        referee.set_seat(Seat::Referee { slot: 0 });
        let referee = join_args(&base(), &referee, Choice::Connect).unwrap();
        assert!(referee.referee);
        assert_eq!(referee.role(), rm_simulator_server::protocol::Role::Referee);
        let mut driving = fields.clone();
        driving.set_seat(Seat::Pilot {
            team: Team::Blue,
            robot: Robot::Infantry4,
        });
        let driving = join_args(&base(), &driving, Choice::Practice).unwrap();
        assert_eq!(driving.robot, Robot::Infantry4);
        assert_eq!(driving.team, crate::args::TeamArg::Blue);
        assert_eq!(driving.role(), rm_simulator_server::protocol::Role::Pilot);
        assert_eq!(driving.caliber(), Caliber::Mm17);
        let mut blank = fields.clone();
        blank.robot.clear();
        assert_eq!(
            join_args(&base(), &blank, Choice::Practice).unwrap().robot,
            Robot::default()
        );
        blank.robot = "unknown-robot".into();
        assert!(join_args(&base(), &blank, Choice::Practice).is_err());
        let host = join_args(&base(), &fields, Choice::Host).unwrap();
        assert_eq!(host.connect, None);
        assert_eq!(host.host.listen.as_deref(), Some(DEFAULT_LISTEN));
        let practice = join_args(&base(), &fields, Choice::Practice).unwrap();
        assert_eq!((practice.connect, practice.host.listen), (None, None));
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
        // Only multiplayer asks for a name; Single Player falls back to one.
        assert!(
            join_args(&base(), &fields, Choice::Host)
                .unwrap_err()
                .contains("name")
        );
        fields.address = "localhost:7700".into();
        assert!(
            join_args(&base(), &fields, Choice::Connect)
                .unwrap_err()
                .contains("name")
        );
        assert_eq!(
            join_args(&base(), &fields, Choice::Practice).unwrap().name,
            DEFAULT_NAME
        );
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
            robot: "hero".into(),
            referee: true,
            ..default()
        };
        save_remembered(&path, &fields).unwrap();
        let mut expected = fields;
        expected.password.clear();
        expected.referee = false;
        assert_eq!(load_remembered(&path), Some(expected));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(load_remembered(&path), None);
    }
}
