// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Opt-in referee link: an embedded MQTT broker that serves the official
//! RoboMaster custom-client protocol for the robot this app pilots, so a real
//! custom client can be tested against the simulation.
//!
//! Enabled by the `referee-link` Cargo feature and `--referee-link`. A custom
//! client connects to the broker as it would to the referee system and
//! receives the state topics below, built from the session's newest host
//! snapshot at the protocol's rates. Payloads are raw Protobuf, one topic per
//! message name. Fields the simulation does not model are left absent, never
//! sent as zero.
//!
//! | Topic | Rate | Source |
//! |---|---|---|
//! | `GameStatus` | 5 Hz | referee phase, clock, round and round wins |
//! | `GlobalUnitStatus` | 1 Hz | base, shield and outpost HP; robot HP by number |
//! | `RobotStaticStatus` | 1 Hz | own robot id, type, level and limits |
//! | `RobotDynamicStatus` | 10 Hz | own robot HP, heat, allowance, energy, experience |
//! | `RobotPosition` | 1 Hz | own chassis position |
//! | `RadarInfoToClient` | 1 Hz | every numbered robot's position |
//!
//! Radar positions are sent for every robot at 1 Hz, which is more than a
//! real client receives; this is a test harness.
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use rm_simulator_server::layout::FIELD_HALF_LENGTH_M;
use rm_simulator_server::protocol::PlayerInfo;
use rm_simulator_world::gameplay::{Caliber, RobotState, RoundResult, TeamState};
use rm_simulator_world::referee::game_team;
use rm_simulator_world::{FieldSnapshot, MatchPhase, RobotKind, RobotSnapshot, Team};

use crate::loading::Screen;
use crate::session::Session;

/// Half the field's width along y, in metres (Figure 4-5, 15 m overall).
const FIELD_HALF_WIDTH_M: f64 = 7.5;

/// Robot numbers in `GlobalUnitStatus.robot_health`, per side.
const UNIT_NUMBERS: [u8; 5] = [1, 2, 3, 4, 7];
/// Robot numbers in `RadarInfoToClient.radar_info`, per side.
const RADAR_NUMBERS: [u8; 6] = [1, 2, 3, 4, 6, 7];

/// Protobuf messages of the custom-client protocol, with the field numbers of
/// the V1.3.0 schema (`proto/rm26.proto` in trident-rm `custom-client-27`).
/// Only the messages the link sends are defined.
pub mod wire {
    /// Match round, score, stage and clock.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct GameStatus {
        /// One-based current round.
        #[prost(uint32, optional, tag = "1")]
        pub current_round: Option<u32>,
        /// Rounds in the series.
        #[prost(uint32, optional, tag = "2")]
        pub total_rounds: Option<u32>,
        /// Red's series score.
        #[prost(uint32, optional, tag = "3")]
        pub red_score: Option<u32>,
        /// Blue's series score.
        #[prost(uint32, optional, tag = "4")]
        pub blue_score: Option<u32>,
        /// 0 not started, 1 preparation, 2 self-check, 3 countdown, 4 in match, 5 settling.
        #[prost(uint32, optional, tag = "5")]
        pub current_stage: Option<u32>,
        /// Seconds left in the current stage.
        #[prost(int32, optional, tag = "6")]
        pub stage_countdown_sec: Option<i32>,
        /// Seconds elapsed in the current stage.
        #[prost(int32, optional, tag = "7")]
        pub stage_elapsed_sec: Option<i32>,
        /// Whether the match is paused.
        #[prost(bool, optional, tag = "8")]
        pub is_paused: Option<bool>,
    }

    /// Bases, outposts and robot HP of both sides, own side first.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct GlobalUnitStatus {
        /// Own base HP.
        #[prost(uint32, optional, tag = "1")]
        pub base_health: Option<u32>,
        /// Own base shield.
        #[prost(uint32, optional, tag = "3")]
        pub base_shield: Option<u32>,
        /// Own outpost HP.
        #[prost(uint32, optional, tag = "4")]
        pub outpost_health: Option<u32>,
        /// Opponent base HP.
        #[prost(uint32, optional, tag = "6")]
        pub enemy_base_health: Option<u32>,
        /// Opponent base shield.
        #[prost(uint32, optional, tag = "8")]
        pub enemy_base_shield: Option<u32>,
        /// Opponent outpost HP.
        #[prost(uint32, optional, tag = "9")]
        pub enemy_outpost_health: Option<u32>,
        /// HP of own robots 1, 2, 3, 4, 7, then the opponent's.
        #[prost(uint32, repeated, tag = "11")]
        pub robot_health: Vec<u32>,
        /// Own total damage dealt.
        #[prost(uint32, optional, tag = "13")]
        pub total_damage_ally: Option<u32>,
        /// Opponent total damage dealt.
        #[prost(uint32, optional, tag = "14")]
        pub total_damage_enemy: Option<u32>,
    }

    /// The connected robot's identity and limits.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct RobotStaticStatus {
        /// 0 not connected, 1 connected.
        #[prost(uint32, optional, tag = "1")]
        pub connection_state: Option<u32>,
        /// 0 unknown, 1 alive, 2 defeated.
        #[prost(uint32, optional, tag = "3")]
        pub alive_state: Option<u32>,
        /// Referee robot id: red 1-7, blue 101-107.
        #[prost(uint32, optional, tag = "4")]
        pub robot_id: Option<u32>,
        /// 1 Hero, 2 Engineer, 3 Infantry, 4 Drone, 5 Sentry.
        #[prost(uint32, optional, tag = "5")]
        pub robot_type: Option<u32>,
        /// Current level.
        #[prost(uint32, optional, tag = "8")]
        pub level: Option<u32>,
        /// Maximum HP.
        #[prost(uint32, optional, tag = "9")]
        pub max_health: Option<u32>,
        /// Barrel heat limit.
        #[prost(uint32, optional, tag = "10")]
        pub max_heat: Option<u32>,
        /// Barrel heat cooling per second.
        #[prost(float, optional, tag = "11")]
        pub heat_cooldown_rate: Option<f32>,
        /// Chassis power limit, watts.
        #[prost(uint32, optional, tag = "12")]
        pub max_power: Option<u32>,
    }

    /// The connected robot's live state.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct RobotDynamicStatus {
        /// Current HP.
        #[prost(uint32, optional, tag = "1")]
        pub current_health: Option<u32>,
        /// Current barrel heat.
        #[prost(float, optional, tag = "2")]
        pub current_heat: Option<f32>,
        /// Remaining chassis energy.
        #[prost(uint32, optional, tag = "4")]
        pub current_chassis_energy: Option<u32>,
        /// Current experience.
        #[prost(uint32, optional, tag = "6")]
        pub current_experience: Option<u32>,
        /// Projectiles fired.
        #[prost(uint32, optional, tag = "8")]
        pub total_projectiles_fired: Option<u32>,
        /// Remaining projectile allowance.
        #[prost(uint32, optional, tag = "9")]
        pub remaining_ammo: Option<u32>,
        /// Whether the robot is out of combat.
        #[prost(bool, optional, tag = "10")]
        pub is_out_of_combat: Option<bool>,
    }

    /// The connected robot's position.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct RobotPosition {
        /// World x, metres.
        #[prost(float, optional, tag = "1")]
        pub x: Option<f32>,
        /// World y, metres.
        #[prost(float, optional, tag = "2")]
        pub y: Option<f32>,
    }

    /// Robot positions, opponent 1, 2, 3, 4, 6, 7 then own.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct RadarInfoToClient {
        /// One entry per robot number; 0, 0 when unknown.
        #[prost(message, repeated, tag = "1")]
        pub radar_info: Vec<RadarSingleRobotInfo>,
    }

    /// One robot's position, centimetres.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct RadarSingleRobotInfo {
        /// World x, centimetres.
        #[prost(uint32, optional, tag = "1")]
        pub target_pos_x: Option<u32>,
        /// World y, centimetres.
        #[prost(uint32, optional, tag = "2")]
        pub target_pos_y: Option<u32>,
    }
}

/// What the messages are built from: the session's newest host state.
pub struct View<'a> {
    /// Newest authoritative field.
    pub field: &'a FieldSnapshot,
    /// The side this app plays for.
    pub team: Team,
    /// The chassis this app pilots, if any.
    pub chassis: Option<u32>,
    /// Everyone on the field.
    pub roster: &'a [PlayerInfo],
    /// Whether the host is paused.
    pub paused: bool,
}

/// Converts simulation world metres to the protocol's world frame.
///
/// ASSUMPTION shared with the custom client (`src/field_map.h` there), not yet
/// confirmed against the V2.0.0 protocol: the origin is the red-side corner
/// with x towards blue. Red holds +x here, so the conversion is a half turn
/// about the field centre.
pub fn to_protocol_frame(x_m: f64, y_m: f64) -> (f64, f64) {
    (FIELD_HALF_LENGTH_M - x_m, FIELD_HALF_WIDTH_M - y_m)
}

/// Referee id, red 1-7 and blue 101-107: from the roster, which knows the
/// Infantry numbers, else from the kind when only one number fits.
fn referee_number(roster: &[PlayerInfo], robot: &RobotSnapshot) -> Option<u8> {
    roster
        .iter()
        .find(|player| player.chassis == Some(robot.id))
        .and_then(|player| player.robot)
        .map(|robot| robot.number())
        .or(match robot.kind {
            RobotKind::Hero => Some(1),
            RobotKind::Engineer => Some(2),
            RobotKind::Drone => Some(6),
            RobotKind::Sentry => Some(7),
            RobotKind::Infantry => None,
        })
}

fn referee_id(team: Team, number: u8) -> u32 {
    match team {
        Team::Red => u32::from(number),
        Team::Blue => 100 + u32::from(number),
    }
}

fn whole_seconds(ns: u64) -> i32 {
    i32::try_from(ns.div_ceil(1_000_000_000)).unwrap_or(i32::MAX)
}

fn team_state(game: &rm_simulator_world::gameplay::Snapshot, team: Team) -> Option<&TeamState> {
    game.teams
        .iter()
        .find(|state| state.team == game_team(team))
}

/// `GameStatus`, or `None` without a referee. The simulation plays single
/// rounds, so the series length stays absent and the scores count round wins.
pub fn game_status(view: &View) -> Option<wire::GameStatus> {
    let referee = view.field.referee.as_ref()?;
    let (stage, countdown, elapsed) = match referee.phase {
        MatchPhase::Idle => (0, None, None),
        MatchPhase::Countdown => (3, Some(whole_seconds(referee.countdown_remaining_ns)), None),
        MatchPhase::Running => (
            4,
            Some(whole_seconds(referee.remaining_ns)),
            Some(whole_seconds(referee.match_time_ns)),
        ),
        MatchPhase::Finished => (5, None, None),
    };
    let wins = |team: Team| {
        let won = |result: &RoundResult| matches!(result, RoundResult::Decided { winner: Some(winner), .. } if *winner == game_team(team));
        referee
            .game
            .rounds
            .iter()
            .filter(|round| won(&round.result))
            .count() as u32
    };
    Some(wire::GameStatus {
        current_round: (referee.game.round > 0).then_some(referee.game.round),
        total_rounds: None,
        red_score: Some(wins(Team::Red)),
        blue_score: Some(wins(Team::Blue)),
        current_stage: Some(stage),
        stage_countdown_sec: countdown,
        stage_elapsed_sec: elapsed,
        is_paused: Some(view.paused),
    })
}

/// `GlobalUnitStatus` from this app's side, or `None` without a referee.
/// Numbers with no robot on the field report 0 HP.
pub fn global_units(view: &View) -> Option<wire::GlobalUnitStatus> {
    let referee = view.field.referee.as_ref()?;
    let own = team_state(&referee.game, view.team)?;
    let enemy = team_state(&referee.game, view.team.other())?;
    let health = |team: Team, number: u8| {
        referee
            .robots
            .iter()
            .find(|robot| robot.team == team && referee_number(view.roster, robot) == Some(number))
            .map_or(0, |robot| robot.hp)
    };
    let robot_health = [view.team, view.team.other()]
        .into_iter()
        .flat_map(|team| UNIT_NUMBERS.map(|number| health(team, number)))
        .collect();
    let damage = |team: &TeamState| u32::try_from(team.attack_damage).unwrap_or(u32::MAX);
    Some(wire::GlobalUnitStatus {
        base_health: Some(own.base_hp),
        base_shield: Some(own.base_shield_hp),
        outpost_health: Some(own.outpost_hp),
        enemy_base_health: Some(enemy.base_hp),
        enemy_base_shield: Some(enemy.base_shield_hp),
        enemy_outpost_health: Some(enemy.outpost_hp),
        robot_health,
        total_damage_ally: Some(damage(own)),
        total_damage_enemy: Some(damage(enemy)),
    })
}

/// The piloted robot's referee record and rules state.
fn own_robot<'a>(view: &View<'a>) -> Option<(&'a RobotSnapshot, &'a RobotState)> {
    let chassis = view.chassis?;
    let referee = view.field.referee.as_ref()?;
    let snapshot = referee.robots.iter().find(|robot| robot.id == chassis)?;
    let state = referee
        .game
        .robots
        .iter()
        .find(|robot| robot.config.id == chassis)?;
    Some((snapshot, state))
}

/// `RobotStaticStatus` for the piloted robot.
pub fn robot_static(view: &View) -> Option<wire::RobotStaticStatus> {
    let (robot, state) = own_robot(view)?;
    let stats = state.stats();
    Some(wire::RobotStaticStatus {
        connection_state: Some(1),
        alive_state: Some(if robot.alive() { 1 } else { 2 }),
        robot_id: referee_number(view.roster, robot).map(|number| referee_id(robot.team, number)),
        robot_type: Some(match robot.kind {
            RobotKind::Hero => 1,
            RobotKind::Engineer => 2,
            RobotKind::Infantry => 3,
            RobotKind::Drone => 4,
            RobotKind::Sentry => 5,
        }),
        level: Some(u32::from(robot.level)),
        max_health: Some(robot.max_hp),
        max_heat: Some(stats.heat_limit),
        heat_cooldown_rate: Some(stats.cooling_per_s as f32),
        max_power: Some(stats.chassis_power_w),
    })
}

/// `RobotDynamicStatus` for the piloted robot. Buffer energy and the last
/// launch speed are not in the field snapshot and stay absent.
pub fn robot_dynamic(view: &View) -> Option<wire::RobotDynamicStatus> {
    let (robot, state) = own_robot(view)?;
    let caliber = match robot.kind {
        RobotKind::Hero => Caliber::Mm42,
        _ => Caliber::Mm17,
    };
    let fired: u64 = state.shots_launched.iter().sum();
    Some(wire::RobotDynamicStatus {
        current_health: Some(robot.hp),
        current_heat: Some(state.heat_tenths as f32 / 10.0),
        current_chassis_energy: state.chassis_energy_j,
        current_experience: Some(state.experience_tenths / 10),
        total_projectiles_fired: Some(u32::try_from(fired).unwrap_or(u32::MAX)),
        remaining_ammo: Some(state.allowance[caliber.index()]),
        is_out_of_combat: Some(state.out_of_combat),
    })
}

/// `RobotPosition` for the piloted chassis. Yaw stays absent until the
/// protocol's "0 = true north" reference is known.
pub fn robot_position(view: &View) -> Option<wire::RobotPosition> {
    let chassis = view
        .field
        .chassis
        .iter()
        .find(|chassis| Some(chassis.id) == view.chassis)?;
    let [x, y, _] = chassis.pose.translation_m;
    let (x, y) = to_protocol_frame(x, y);
    Some(wire::RobotPosition {
        x: Some(x as f32),
        y: Some(y as f32),
    })
}

/// `RadarInfoToClient`: opponent then own robots by number.
pub fn radar(view: &View) -> Option<wire::RadarInfoToClient> {
    let referee = view.field.referee.as_ref()?;
    let position = |team: Team, number: u8| {
        let robot = referee.robots.iter().find(|robot| {
            robot.team == team && referee_number(view.roster, robot) == Some(number)
        })?;
        let chassis = view
            .field
            .chassis
            .iter()
            .find(|chassis| chassis.id == robot.id)?;
        let [x, y, _] = chassis.pose.translation_m;
        let (x, y) = to_protocol_frame(x, y);
        let centimetres =
            |metres: f64| (metres * 100.0).round().clamp(0.0, f64::from(u32::MAX)) as u32;
        Some((centimetres(x), centimetres(y)))
    };
    let radar_info = [view.team.other(), view.team]
        .into_iter()
        .flat_map(|team| RADAR_NUMBERS.map(|number| position(team, number)))
        .map(|position| {
            let (x, y) = position.unwrap_or((0, 0));
            wire::RadarSingleRobotInfo {
                target_pos_x: Some(x),
                target_pos_y: Some(y),
            }
        })
        .collect();
    Some(wire::RadarInfoToClient { radar_info })
}

/// The running broker's local link and the publication schedule.
#[derive(Resource)]
pub struct RefereeLink {
    tx: rumqttd::local::LinkTx,
    rx: rumqttd::local::LinkRx,
    fast: Schedule,
    status: Schedule,
    slow: Schedule,
}

/// A fixed publication rate that skips missed periods instead of bursting.
struct Schedule {
    period: Duration,
    next: Instant,
}

impl Schedule {
    fn hz(rate: u32) -> Self {
        Self {
            period: Duration::from_secs(1) / rate,
            next: Instant::now(),
        }
    }
    fn due(&mut self, now: Instant) -> bool {
        if now < self.next {
            return false;
        }
        self.next = now + self.period;
        true
    }
}

/// Starts the broker on `address` on its own thread.
pub fn start(address: SocketAddr) -> anyhow::Result<RefereeLink> {
    // The broker binds on its own thread and only logs a failure there, so
    // check the address here to fail at launch instead.
    drop(std::net::TcpListener::bind(address)?);

    let connections = rumqttd::ConnectionSettings {
        connection_timeout_ms: 5_000,
        max_payload_size: 64 * 1024,
        max_inflight_count: 100,
        auth: None,
        external_auth: None,
        dynamic_filters: true,
    };
    let server = rumqttd::ServerSettings {
        name: "referee-link".into(),
        listen: address,
        tls: None,
        next_connection_delay_ms: 1,
        connections,
    };
    let config = rumqttd::Config {
        router: rumqttd::RouterConfig {
            max_connections: 16,
            max_outgoing_packet_count: 200,
            max_segment_size: 1024 * 1024,
            max_segment_count: 4,
            custom_segment: None,
            initialized_filters: None,
            shared_subscriptions_strategy: Default::default(),
        },
        v4: Some([("referee-link".to_owned(), server)].into()),
        ..Default::default()
    };
    let mut broker = rumqttd::Broker::new(config);
    let (tx, rx) = broker.link("rm-simulator-referee")?;
    std::thread::Builder::new()
        .name("referee-link".into())
        .spawn(move || {
            if let Err(error) = broker.start() {
                eprintln!("referee link stopped: {error}");
            }
        })?;
    println!("referee link: MQTT broker on {address}");
    Ok(RefereeLink {
        tx,
        rx,
        fast: Schedule::hz(10),
        status: Schedule::hz(5),
        slow: Schedule::hz(1),
    })
}

/// Publishes the state topics while a match is loaded.
pub struct RefereeLinkPlugin;

impl Plugin for RefereeLinkPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, publish.run_if(in_state(Screen::InMatch)));
    }
}

fn publish(mut link: ResMut<RefereeLink>, session: Res<Session>) {
    // Nothing is subscribed through the local link yet; drain it so the
    // router never waits on it.
    while let Ok(Some(_)) = link.rx.recv_deadline(Instant::now()) {}

    let view = View {
        field: &session.snapshot,
        team: session.team,
        chassis: session.chassis_id,
        roster: &session.roster,
        paused: session.paused,
    };
    let now = Instant::now();
    let RefereeLink {
        tx,
        fast,
        status,
        slow,
        ..
    } = &mut *link;
    let mut send = |topic: &str, payload: Option<Vec<u8>>| {
        if let Some(payload) = payload
            && let Err(error) = tx.publish(topic.to_owned(), payload)
        {
            warn_once!("referee link cannot publish {topic}: {error:?}");
        }
    };
    use prost::Message as _;
    if fast.due(now) {
        send(
            "RobotDynamicStatus",
            robot_dynamic(&view).map(|m| m.encode_to_vec()),
        );
    }
    if status.due(now) {
        send("GameStatus", game_status(&view).map(|m| m.encode_to_vec()));
    }
    if slow.due(now) {
        send(
            "GlobalUnitStatus",
            global_units(&view).map(|m| m.encode_to_vec()),
        );
        send(
            "RobotStaticStatus",
            robot_static(&view).map(|m| m.encode_to_vec()),
        );
        send(
            "RobotPosition",
            robot_position(&view).map(|m| m.encode_to_vec()),
        );
        send("RadarInfoToClient", radar(&view).map(|m| m.encode_to_vec()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_simulator_server::protocol::{Robot, Role};
    use rm_simulator_server::workload;

    fn pilot(chassis: u32, team: Team, robot: Robot) -> PlayerInfo {
        PlayerInfo {
            client_id: chassis,
            name: format!("pilot {chassis}"),
            team: Some(team),
            role: Role::Pilot,
            chassis: Some(chassis),
            robot: Some(robot),
        }
    }

    /// A red and a blue chassis on the no-CAD measurement field, with the
    /// roster naming them red Infantry 3 and blue Infantry 4.
    fn fixture() -> (FieldSnapshot, Vec<PlayerInfo>, [u32; 2]) {
        let (simulation, chassis) = workload::simulation(2);
        let roster = vec![
            pilot(chassis[0], Team::Red, Robot::Infantry3),
            pilot(chassis[1], Team::Blue, Robot::Infantry4),
        ];
        (simulation.state().field, roster, [chassis[0], chassis[1]])
    }

    #[test]
    fn protocol_frame_puts_the_origin_at_the_red_corner() {
        assert_eq!(to_protocol_frame(14.0, 7.5), (0.0, 0.0));
        assert_eq!(to_protocol_frame(-14.0, -7.5), (28.0, 15.0));
        assert_eq!(to_protocol_frame(0.0, 0.0), (14.0, 7.5));
    }

    #[test]
    fn own_robot_messages_use_the_roster_number() {
        let (field, roster, [red, _]) = fixture();
        let view = View {
            field: &field,
            team: Team::Red,
            chassis: Some(red),
            roster: &roster,
            paused: false,
        };
        let robot = robot_static(&view).expect("the red pilot has a robot record");
        assert_eq!(robot.robot_id, Some(3));
        assert_eq!(robot.robot_type, Some(3));
        assert!(robot.max_health.is_some_and(|hp| hp > 0));
        let dynamic = robot_dynamic(&view).expect("the red pilot has rules state");
        assert_eq!(dynamic.current_health, robot.max_health);
        assert!(robot_position(&view).is_some());
    }

    #[test]
    fn unit_and_radar_lists_put_each_side_in_its_slot() {
        let (field, roster, [_, blue]) = fixture();
        let view = View {
            field: &field,
            team: Team::Blue,
            chassis: Some(blue),
            roster: &roster,
            paused: true,
        };
        let units = global_units(&view).expect("the field has a referee");
        assert_eq!(units.robot_health.len(), 10);
        // Own (blue) Infantry 4 is the fourth own slot; red Infantry 3 the
        // third opponent slot.
        assert!(units.robot_health[3] > 0);
        assert!(units.robot_health[7] > 0);
        assert_eq!(units.robot_health.iter().filter(|hp| **hp > 0).count(), 2);

        let radar = radar(&view).expect("the field has a referee");
        assert_eq!(radar.radar_info.len(), 12);
        let known: Vec<usize> = (0..12)
            .filter(|index| radar.radar_info[*index].target_pos_x != Some(0))
            .collect();
        assert_eq!(known, [2, 9]); // Red 3 first, then own blue 4.

        let status = game_status(&view).expect("the field has a referee");
        assert_eq!(status.current_stage, Some(0));
        assert_eq!(status.is_paused, Some(true));
        assert_eq!(status.total_rounds, None);
    }

    #[test]
    fn messages_round_trip_through_protobuf() {
        use prost::Message as _;
        let (field, roster, [red, _]) = fixture();
        let view = View {
            field: &field,
            team: Team::Red,
            chassis: Some(red),
            roster: &roster,
            paused: false,
        };
        let units = global_units(&view).unwrap();
        let decoded = wire::GlobalUnitStatus::decode(units.encode_to_vec().as_slice()).unwrap();
        assert_eq!(decoded, units);
    }
}
