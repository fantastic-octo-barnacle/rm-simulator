// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! The app's client view of an embedded or remote authoritative server.
//! All gameplay input and snapshots use the same connection path.
use bevy::prelude::*;
use rm_simulator_server::{
    cad_assets::CadAssets,
    clock::TimeSource,
    net::{Client, Server, describe_seat},
    protocol::{Command, PlayerInfo, Role, WeaponConfig},
    simulation::{BuildProgress, Simulation},
};
use rm_simulator_world::{
    ChassisCommand, ChassisConfig, ChassisSnapshot, FieldSnapshot, RefereeSnapshot, Team,
};
use std::collections::VecDeque;

use crate::args::Args;

/// HUD notices (rejected commands, players joining) stay this long.
const NOTICE_NS: u64 = 4_000_000_000;

/// Keep the embedded host alive for this session. Simulation access stays in
/// server workers. Collider drawing reads client-owned assets and presentation.
struct EmbeddedHost {
    _lobby: Option<rm_simulator_server::lobby::Advertisement>,
    server: Server,
    _http: Option<rm_simulator_server::http::HttpServer>,
}
/// Visual settings for the world-to-scene adaptation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlashSettings {
    /// How long a detected armor hit stays lit, in ns. An app setting: the
    /// rulebook gives no flash duration.
    pub hit_flash_ns: u64,
    /// Blink rate of a newly activated rune, in Hz. The rulebook fixes only
    /// that the arms light up, so the rate is an app setting.
    pub rune_flash_hz: f64,
    /// Number of blinks a newly activated rune shows. An app setting.
    pub rune_flashes: u32,
}
/// Replay state owned and reset by the session, independent of presentation and transport.
struct PredictionState {
    worker: Option<crate::prediction::PredictionWorker>,
    geometry: Option<rm_simulator_world::StaticGeometry>,
    context: Option<(u64, std::sync::Arc<FieldSnapshot>)>,
    inputs: rm_simulator_server::prediction::InputHistory,
    chassis: Option<ChassisSnapshot>,
    time_ns: u64,
    last_frame_time_ns: u64,
    moving_frames: u64,
    held_moving_frames: u64,
    results_accepted: u64,
    results_discarded: u64,
    context_input_epoch: u64,
    proof: Option<rm_simulator_server::prediction::PredictionProof>,
    epoch: u64,
}

/// The app's view of the simulation, refreshed once per frame.
#[derive(Resource)]
pub struct Session {
    prediction: PredictionState,
    shots: shots::LocalShots,
    /// Every wall-clock reading in the session and its client comes from here.
    /// Simulation time still arrives only in snapshots and anchors.
    pub(crate) time: TimeSource,
    last_checkpoint: std::time::Instant,
    last_context: std::time::Instant,
    owner_anchor: Option<rm_simulator_server::owner_stream::OwnerAnchor>,
    /// Transport trouble to show in the HUD, or `None` while the link looks
    /// healthy. Silence alone is not a disconnect.
    pub connection_toast: Option<&'static str>,
    client: Client,
    /// The newest pilot input frame sent for the own chassis. A later `Fire`
    /// carries it so the host aims the shot where the client did.
    pub(crate) last_input_frame: Option<rm_simulator_server::input_stream::InputFrame>,
    last_input_sent: std::time::Instant,
    sampled_drive: Option<(u32, ChassisCommand)>,
    pub(crate) hit_feedback: crate::hit_feedback::HitFeedback,
    next_shot_id: u64,
    /// Whether own-chassis prediction is enabled. False with `--no-prediction`
    /// and for a screenshot run, which present host state directly.
    pub local_aim: bool,
    camera_offset_m: [f64; 3],
    camera_offset_at: std::time::Instant,
    snapshot_id: u64,
    input_epoch: u64,
    frame_time_ns: u64,
    reset_clock: std::sync::atomic::AtomicBool,
    host: Option<EmbeddedHost>,
    singleplayer: bool,
    /// Newest authoritative field state from the host. The app only reads it;
    /// it never steps this field.
    pub snapshot: FieldSnapshot,
    /// Interpolation delay for peer motion, automatic or set by hand in the
    /// F3 menu.
    pub remote_buffer: crate::interpolation::Buffer,
    /// Simulation time, in ns, at which peer chassis are sampled for drawing.
    /// The own chassis is never delayed to it.
    pub remote_view_time_ns: u64,
    /// Recent peer chassis poses, sampled at `remote_view_time_ns`.
    pub remote_history: crate::interpolation::RemoteHistory,
    /// Whether the host's simulation is paused. Set from snapshots and owner
    /// anchors, and changed only as a match control.
    pub paused: bool,
    /// The side this player is on: `F` spends its rune opportunity. The
    /// referee is on no team but still needs one for `F`.
    pub team: Team,
    /// This player's server-assigned id in the roster.
    pub client_id: u32,
    /// This player's assigned role: pilot, spectator or referee. The referee
    /// is a spectator on no team and drives no robot.
    pub role: Role,
    /// The chassis this player drives; only a pilot has one.
    pub chassis_id: Option<u32>,
    /// This pilot's current weapon settings, within host limits. Its shot and
    /// cadence gate the client's provisional fire.
    pub weapon: WeaponConfig,
    /// Host defaults and upper bounds for client rate and muzzle speed.
    pub weapon_limits: rm_simulator_server::protocol::WeaponLimits,
    /// Host starting values restored by Reset settings.
    pub weapon_defaults: WeaponConfig,
    weapon_update_pending: bool,
    /// Everyone on the field, this player included.
    pub roster: Vec<PlayerInfo>,
    /// Flash timings the scene adapter uses for hits and activated runes.
    pub flash: FlashSettings,
    /// The CAD collision proxies are in the world.
    /// Recent notices with the world time they arrived.
    pub notices: VecDeque<(u64, String)>,
    shot_rejection_count: u64,
    last_shot_rejection: Option<String>,
    /// Running count of rejected commands, for the console and network traces.
    pub rejection_count: u64,
    /// The most recent rejection text, or `None` until one arrives.
    pub last_rejection: Option<String>,
}
/// What opening a session decides for the rest of the app.
pub struct Opened {
    /// The opened session, polled once per frame from here on.
    pub session: Session,
    /// Where the eye starts, in world FLU metres.
    pub eye_flu: [f64; 3],
}
#[path = "session_shots.rs"]
mod shots;
impl Session {
    /// Debug drawing reads only the client's verified asset geometry.
    pub fn collision_geometry(
        &self,
    ) -> Option<Box<dyn FnOnce() -> Result<rm_simulator_world::StaticGeometry, String> + Send>>
    {
        let geometry = self.prediction.geometry.as_ref()?.fixed_only();
        Some(Box::new(move || Ok(geometry)))
    }

    /// Verified client-side collision geometry, or `None` before the host's
    /// prediction scene arrives. Debug drawing reads it without a host request.
    pub fn client_collision_geometry(&self) -> Option<rm_simulator_world::StaticGeometry> {
        self.prediction.geometry.clone()
    }

    /// Called only once scenery is ready. The held clock never accumulates
    /// loading time, and the simulation keeps the requested initial pause state.
    pub fn ready(&self) {
        self.reset_clock
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(host) = &self.host {
            host.server.ready();
        }
    }

    /// True once the host confirmed every command queued so far. Poll until
    /// true before capturing a frame that must show a command's result.
    pub fn commands_confirmed(&self) -> bool {
        self.client.commands_confirmed()
    }

    /// Join the host named by `--connect`, or build the field from the CAD
    /// and host it as the arguments ask.
    pub fn open(
        args: &Args,
        cad: &CadAssets,
        spawn: [f64; 3],
        spawn_yaw_deg: f64,
        progress: impl Fn(f32, &str),
    ) -> anyhow::Result<Opened> {
        // The tick length is process-wide and frozen on first use, so the first
        // match of a run fixes it. A later match asking for another rate is
        // refused here rather than silently running at the frozen one.
        let wanted =
            rm_simulator_world::tick_ns_for_hz(args.host.physics_rate_hz).ok_or_else(|| {
                anyhow::anyhow!("unsupported physics rate {} Hz", args.host.physics_rate_hz)
            })?;
        let frozen = rm_simulator_world::set_tick_ns(wanted);
        anyhow::ensure!(
            frozen == wanted,
            "this run already froze the physics rate at {} Hz; relaunch with --physics-rate-hz {}",
            rm_simulator_world::hz_for_tick_ns(frozen).unwrap_or(0),
            args.host.physics_rate_hz,
        );
        let role = args.role();
        let (client, host) = if let Some(address) = &args.connect {
            progress(0.3, "Connecting to host");
            (
                Client::connect_udp_with_password(
                    address,
                    &args.name,
                    Some(args.team.into()),
                    role,
                    args.robot,
                    &args.password,
                )
                .map_err(|e| anyhow::anyhow!("connecting to {address}: {e}"))?,
                None,
            )
        } else {
            let team: Team = args.team.into();
            let options = args.host.layout_options();
            let simulation = Simulation::from_cad(
                cad,
                &options,
                Some(ChassisConfig::default()),
                args.host.start_paused,
                |stage| match stage {
                    BuildProgress::ReadingTerrain => progress(0.3, "Reading terrain triangles"),
                    BuildProgress::BuildingPhysics => progress(0.5, "Building field physics"),
                    BuildProgress::TerrainReady(description) => println!("{description}"),
                    BuildProgress::PreparingChassis => {
                        progress(0.65, "Preparing chassis and session")
                    }
                },
            )?
            .with_password(args.password.clone())
            .with_weapon(args.weapon())
            .map_err(anyhow::Error::msg)?
            .with_weapon_limits(args.host.weapon_limits())
            .map_err(anyhow::Error::msg)?;
            let address = args.host.listen.as_deref().unwrap_or("127.0.0.1:0");
            let server = if args.host.listen.is_none() && args.lobby_name.is_none() {
                Server::in_process(simulation, false)
            } else {
                Server::bind_suspended(address, simulation)
            }?;
            server.spawn_clock()?;
            let client =
                server.connect_owner(&args.name, team, role, args.robot, spawn, spawn_yaw_deg)?;
            if args.host.listen.is_some() {
                println!("hosting players at {}", server.local_addr());
            }
            let http = if let Some(address) = &args.host.http {
                let http = rm_simulator_server::http::HttpServer::bind(address, server.handle())
                    .map_err(|e| anyhow::anyhow!("serving the referee panel on {address}: {e}"))?;
                println!("referee panel: http://{}/", http.local_addr());
                Some(http)
            } else {
                None
            };
            let lobby = args
                .lobby_name
                .as_ref()
                .map(|name| {
                    rm_simulator_server::lobby::Advertisement::start(
                        &args.lobby_host,
                        name,
                        server.local_addr(),
                        !args.password.is_empty(),
                        args.public_lobby,
                        &args.advertise_address,
                    )
                })
                .transpose()?;
            (
                client,
                Some(EmbeddedHost {
                    _lobby: lobby,
                    server,
                    _http: http,
                }),
            )
        };
        let mut opened =
            Self::from_client(client, TimeSource::system(), args, cad, spawn, progress)?;
        opened.session.host = host;
        Ok(opened)
    }

    /// Build the session around a connection the caller already has. `open`
    /// calls this after connecting or hosting; a test hands in a client over a
    /// scripted transport and a clock it advances by hand. Every wall-clock
    /// reading the session and its client make comes from `time`.
    pub fn from_client(
        mut client: Client,
        time: TimeSource,
        args: &Args,
        cad: &CadAssets,
        spawn: [f64; 3],
        progress: impl Fn(f32, &str),
    ) -> anyhow::Result<Opened> {
        client.set_time_source(time.clone());
        let flash = FlashSettings {
            hit_flash_ns: args.hit_flash_ms * 1_000_000,
            rune_flash_hz: args.rune_flash_hz.max(0.0),
            rune_flashes: args.rune_flashes,
        };
        let role = args.role();
        let welcome = client.welcome().clone();
        let team = welcome.team.unwrap_or(args.team.into());
        let client_id = welcome.client_id;
        let chassis_id = welcome.chassis.as_ref().map(|c| c.id);
        let weapon = welcome.weapon;
        println!(
            "joined as client {}, {}",
            client_id,
            describe_seat(
                welcome.team,
                welcome.role,
                chassis_id,
                welcome.chassis.as_ref().map(|c| c.robot)
            )
        );
        let state = client
            .wait_snapshot(std::time::Duration::from_secs(5))
            .ok_or_else(|| anyhow::anyhow!("the host sent no initial state"))?
            .clone();
        let snapshot = state.field;
        let paused = state.paused;
        // The eye starts where the chassis is (a remote host places it),
        // else at the requested point.
        let eye_flu = snapshot
            .chassis
            .iter()
            .find(|chassis| Some(chassis.id) == chassis_id)
            .map_or(spawn, |chassis| chassis.pose.translation_m);
        let roster = client.roster().to_vec();
        let mut remote_history = crate::interpolation::RemoteHistory::default();
        remote_history.push_identified(&snapshot, paused, state.snapshot_id);
        let local_aim = !args.no_prediction && args.screenshot.is_none();
        let mut shots = shots::LocalShots::default();
        let prediction_geometry = welcome.prediction_scene.as_ref().and_then(|scene| {
            progress(0.8, "Preparing collision geometry");
            match scene.geometry(cad) {
                Ok(geometry) => Some(geometry),
                Err(error) => {
                    warn!("{error}");
                    None
                }
            }
        });
        let predictor = if chassis_id.is_some() && local_aim {
            prediction_geometry.as_ref().and_then(|(geometry, floor)| {
                progress(0.8, "Preparing chassis prediction");
                match (|| {
                    shots.worker = Some(
                        crate::projectile_prediction::Worker::new(geometry.clone(), *floor)
                            .map_err(|e| e.to_string())?,
                    );
                    crate::prediction::PredictionWorker::new(geometry.clone(), *floor)
                        .map_err(|e| e.to_string())
                })() {
                    Ok(worker) => Some(worker),
                    Err(error) => {
                        warn!("{error}");
                        None
                    }
                }
            })
        } else {
            None
        };
        let now = time.now();
        let session = Session {
            prediction: PredictionState {
                worker: predictor,
                geometry: prediction_geometry.map(|(geometry, _)| geometry),
                context: None,
                inputs: Default::default(),
                chassis: None,
                time_ns: 0,
                last_frame_time_ns: 0,
                moving_frames: 0,
                held_moving_frames: 0,
                results_accepted: 0,
                results_discarded: 0,
                context_input_epoch: state.input_epoch,
                proof: None,
                epoch: 0,
            },
            shots,
            time,
            sampled_drive: None,
            hit_feedback: Default::default(),
            last_checkpoint: now,
            last_context: now,
            owner_anchor: None,
            connection_toast: None,
            last_input_frame: None,
            last_input_sent: now,
            next_shot_id: 1,

            local_aim,
            camera_offset_m: [0.; 3],
            camera_offset_at: now,
            snapshot_id: state.snapshot_id,
            input_epoch: state.input_epoch,
            frame_time_ns: snapshot.time_ns,
            reset_clock: std::sync::atomic::AtomicBool::new(true),
            remote_buffer: Default::default(),
            remote_view_time_ns: 0,
            remote_history,
            client,
            host: None,
            singleplayer: args.connect.is_none() && args.host.listen.is_none(),
            snapshot,
            paused,
            team,
            client_id,
            role,
            chassis_id,
            weapon,
            weapon_limits: welcome.weapon_limits,
            weapon_defaults: weapon,
            weapon_update_pending: false,
            roster,
            flash,
            notices: VecDeque::new(),
            shot_rejection_count: 0,
            last_shot_rejection: None,
            rejection_count: 0,
            last_rejection: None,
        };
        Ok(Opened { session, eye_flu })
    }
    /// The chassis this player drives, as of the current snapshot.
    pub fn own_chassis(&self) -> Option<&ChassisSnapshot> {
        if let Some(anchor) = &self.owner_anchor
            && anchor.snapshot_id > self.snapshot_id
        {
            return Some(&anchor.owner);
        }
        self.snapshot
            .chassis
            .iter()
            .find(|chassis| Some(chassis.id) == self.chassis_id)
    }
    /// Presentation only. Authoritative state remains in `snapshot`.
    pub fn presented_chassis(&self) -> Option<&ChassisSnapshot> {
        self.prediction
            .chassis
            .as_ref()
            .or_else(|| self.own_chassis())
    }
    /// True when this process hosts the only world: neither `--connect` nor
    /// `--listen` was given.
    pub fn is_singleplayer(&self) -> bool {
        self.singleplayer && !self.is_remote()
    }
    /// True when this session has no embedded host and talks to a remote one.
    pub fn is_remote(&self) -> bool {
        self.host.is_none()
    }
    /// May run the match: the referee, or anyone whose world is local.
    pub fn referees(&self) -> bool {
        self.role.referees() || !self.is_remote()
    }
    /// Send a validated weapon update on the reliable, confirmed path. New
    /// local shots wait for its barrier; projectiles already flying are untouched.
    pub fn set_weapon(&mut self, weapon: WeaponConfig) -> Result<(), String> {
        self.weapon_limits
            .admit(self.weapon_defaults.shot.caliber, weapon)
            .map_err(str::to_string)?;
        if weapon == self.weapon {
            return Ok(());
        }
        if let Some(chassis) = self.chassis_id {
            self.apply_confirmed(Command::ConfigureWeapon { chassis, weapon })?;
            self.weapon_update_pending = true;
        }
        self.weapon = weapon;
        Ok(())
    }
    /// Apply a command; rejections become HUD notices.
    pub fn apply(&mut self, command: Command) {
        if self.client.disconnected().is_some() {
            return;
        }
        let command = self.number_input(command);
        if let Command::Fire { shooter, .. } = command
            && self.last_input_frame.is_some()
            && !self.paused
        {
            if let Err(reason) = self.begin_local_shot(shooter) {
                self.notice(format!("shot delayed: {reason}"));
            }
            return;
        }
        let command = if let Command::Fire { shooter, timing } = command {
            if let Some(input) = self.last_input_frame {
                let shot_id = self.next_shot_id;
                self.next_shot_id = self.next_shot_id.saturating_add(1);
                Command::FireAimed {
                    shooter,
                    shot_id,
                    input,
                    timing,
                }
            } else {
                command
            }
        } else {
            command
        };
        let result = if matches!(
            command,
            Command::Chassis { .. } | Command::PilotInput { .. } | Command::FireAimed { .. }
        ) {
            self.client.send(command)
        } else {
            self.client.send_confirmed(command)
        };
        if let Err(reason) = result {
            self.notice(format!("rejected: {reason}"));
        }
    }
    /// Re-read every wall clock from `time`, as if the session had opened on
    /// it. `open` hosts over a loopback connection, which a held clock cannot
    /// drive; a test pairs that live host with a session clock it controls.
    #[cfg(test)]
    fn rebase_clock(&mut self, time: TimeSource) {
        let now = time.now();
        self.client.set_time_source(time.clone());
        self.time = time;
        self.last_checkpoint = now;
        self.last_context = now;
        self.last_input_sent = now;
        self.camera_offset_at = now;
    }
    /// One pilot drive frame. A control transition leaves at once; aim, and
    /// the values derived from it every frame (the body-frame wish and the
    /// follow yaw rate), wait for the 16 ms refresh grid, and the frame sent at
    /// the boundary carries the newest aim. Frame rate is not input rate; see
    /// the input-cadence notes in the README's server and clients section.
    pub fn drive(&mut self, chassis: u32, command: ChassisCommand, transition: bool) {
        self.sampled_drive = Some((chassis, command));
        if transition || self.input_refresh_due() {
            self.apply(Command::Chassis { chassis, command });
        }
    }
    /// The 16 ms input grid, measured from the last frame actually sent.
    fn input_refresh_due(&self) -> bool {
        self.time.since(self.last_input_sent) >= std::time::Duration::from_millis(16)
    }
    fn number_input(&mut self, command: Command) -> Command {
        if let Command::Chassis {
            chassis,
            command: input,
        } = command
            && Some(chassis) == self.chassis_id
        {
            self.client.reset_input_timing(
                self.input_epoch,
                self.own_chassis().map_or(0, |c| c.placement_revision),
            );
            let time = self.input_time_ns();
            if let Some(pending) = self.prediction.inputs.push(time, input) {
                let frame = rm_simulator_server::input_stream::InputFrame {
                    input_epoch: self.input_epoch,
                    sequence: pending.sequence,
                    sampled_time_ns: pending.time_ns,
                    // A 16 ms sample interval, however many ticks that is.
                    duration_ticks: rm_simulator_server::simulation::step_ticks().max(1) as u32,
                    placement_revision: self.own_chassis().map_or(0, |c| c.placement_revision),
                    command: input,
                };
                self.last_input_frame = Some(frame);
                self.last_input_sent = self.time.now();
                return Command::PilotInput { chassis, frame };
            }
        }
        command
    }
    /// Queue a snapshot barrier after every command sent so far. The error is
    /// a transport failure; poll `commands_confirmed` for the outcome.
    pub fn confirm_inputs(&mut self) -> Result<(), String> {
        self.client.confirm().map_err(|e| e.to_string())
    }
    /// Number the command as pilot input when it belongs to the own chassis,
    /// then send it on the confirmed path. The error is a transport failure,
    /// not a host rejection.
    pub fn apply_confirmed(&mut self, command: Command) -> Result<(), String> {
        let command = self.number_input(command);
        self.client
            .send_confirmed(command)
            .map_err(|e| e.to_string())
    }
    /// Show a HUD notice for `NOTICE_NS`. Text opening with "rejected:" also
    /// bumps the rejection counters the console reports.
    pub fn notice(&mut self, text: String) {
        if text.starts_with("rejected:") {
            self.rejection_count += 1;
            self.last_rejection = Some(text.clone());
        }
        warn!("{text}");
        self.notices.push_back((self.snapshot.time_ns, text));
    }
    /// Take the newest host state without waiting for sockets or physics.
    pub fn presentation_time_ns(&self) -> u64 {
        self.frame_time_ns
    }
    /// The simulation time at which a shot fired this frame executes on the
    /// host: the presentation time plus the input lead. Targets whose motion
    /// is a function of time are drawn here so aiming needs no latency lead.
    pub fn fire_time_ns(&self) -> u64 {
        self.input_time_ns()
    }
    /// Timing attached to a shot: the host's probes plus the snapshot time and
    /// chassis pose the client aimed from.
    pub fn fire_timing(&self) -> rm_simulator_server::protocol::FireTiming {
        let mut timing = self.client.fire_timing();
        timing.observed_snapshot_time_ns = Some(if self.prediction.chassis.is_some() {
            self.prediction.time_ns
        } else {
            self.snapshot.time_ns
        });
        timing.observed_chassis_pose = self.presented_chassis().map(|chassis| chassis.pose);
        timing
    }
    /// Drain the connection, apply new host state and advance prediction and
    /// provisional shots. An `Err` means the embedded host failed; a remote
    /// disconnect is reported through `connection_toast` instead.
    pub fn poll(&mut self) -> Result<(), String> {
        self.poll_inputs()?;
        self.predict_frame();
        Ok(())
    }
    fn poll_inputs(&mut self) -> Result<(), String> {
        if let Some(error) = self.host.as_ref().and_then(|host| host.server.failure()) {
            return Err(format!("embedded server failed: {error}"));
        }
        self.client.poll();
        if self.client.disconnected().is_none() {
            let _ = self.client.synchronize_clock();
        }
        if let Some(state) = self
            .client
            .take_state()
            .filter(|state| state.snapshot_id > self.snapshot_id)
        {
            self.last_checkpoint = self.time.now();
            self.last_context = self.time.now();
            let previous = self.own_chassis();
            let next = state
                .field
                .chassis
                .iter()
                .find(|c| Some(c.id) == self.chassis_id);
            let reset = state.input_epoch != self.input_epoch
                || state.paused != self.paused
                || state.field.time_ns < self.snapshot.time_ns
                || previous.zip(next).is_none_or(|(old, new)| {
                    old.placement_revision != new.placement_revision
                        || old.defeated != new.defeated
                        || old.config != new.config
                });
            if reset
                && self
                    .owner_anchor
                    .as_ref()
                    .is_none_or(|a| state.snapshot_id >= a.snapshot_id)
            {
                self.cancel_local_shots();
                self.remote_buffer.reset_timeline();
                self.prediction.inputs.reset();
                self.last_input_frame = None;
                self.prediction.chassis = None;
                self.frame_time_ns = state.field.time_ns;
                self.camera_offset_m = [0.; 3];
                self.prediction.proof = None;
                self.prediction.epoch = self.prediction.epoch.wrapping_add(1);
            }
            self.prediction.inputs.finalize(state.field.time_ns);
            self.remote_history
                .push_identified(&state.field, state.paused, state.snapshot_id);
            self.reconcile_shots(&state);
            self.snapshot_id = state.snapshot_id;
            if self
                .owner_anchor
                .as_ref()
                .is_none_or(|a| state.snapshot_id >= a.snapshot_id)
            {
                self.input_epoch = state.input_epoch;
                self.paused = state.paused;
            }
            self.prediction.context_input_epoch = state.input_epoch;
            self.snapshot = state.field;
        }
        self.poll_owner_anchor();
        if self.client.roster() != self.roster.as_slice() {
            self.roster = self.client.roster().to_vec();
        }
        self.poll_shot_results();
        for text in self.client.take_notices() {
            self.notice(text);
        }
        self.connection_toast = connection_warning(
            self.client.disconnected().is_some(),
            self.time.since(self.last_checkpoint),
            self.client.response_delay_ns(),
        );
        if self.client.disconnected().is_some() {
            self.cancel_local_shots();
        }
        if self
            .reset_clock
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            self.client
                .reset_presentation_clock(self.snapshot.time_ns, self.paused);
        }
        let frame_time_ns = self
            .client
            .presentation_time_ns()
            .max(self.owner_time_ns())
            .min(
                self.owner_time_ns()
                    .saturating_add(rm_simulator_server::prediction::MAX_CONTINUOUS_REPLAY_NS),
            )
            / rm_simulator_world::tick_ns()
            * rm_simulator_world::tick_ns();
        if !self.prediction_limited() {
            self.frame_time_ns = frame_time_ns.max(self.frame_time_ns);
        }
        self.remote_view_time_ns = self.remote_buffer.sample_time(
            self.frame_time_ns,
            self.remote_history.bounds_ns(),
            self.paused,
            self.snapshot
                .chassis
                .iter()
                .any(|c| Some(c.id) != self.chassis_id),
        );
        let result = self
            .prediction
            .worker
            .as_ref()
            .and_then(|worker| worker.take_result());
        self.accept_prediction(result);
        let now = self.time.now();
        self.hit_feedback.observe_epoch(self.input_epoch);
        for (epoch, id, hit) in self.client.take_hits() {
            self.hit_feedback
                .receive(epoch, id, hit, now, self.frame_time_ns);
        }
        if self.prediction.context_input_epoch == self.input_epoch {
            self.hit_feedback.recover(
                self.prediction.context_input_epoch,
                &self.snapshot,
                now,
                self.frame_time_ns,
            );
        }

        let now = self.snapshot.time_ns;
        self.notices
            .retain(|(at, _)| now.saturating_sub(*at) < NOTICE_NS);
        Ok(())
    }
    fn owner_time_ns(&self) -> u64 {
        self.owner_anchor
            .as_ref()
            .filter(|a| a.snapshot_id > self.snapshot_id)
            .map_or(self.snapshot.time_ns, |a| a.time_ns)
    }
    fn poll_owner_anchor(&mut self) {
        if let Some(anchor) = self.client.take_owner_anchor() {
            self.apply_owner_anchor(anchor);
        }
    }
    fn apply_owner_anchor(&mut self, anchor: rm_simulator_server::owner_stream::OwnerAnchor) {
        if Some(anchor.owner.id) != self.chassis_id
            || anchor.snapshot_id <= self.snapshot_id
            || self
                .owner_anchor
                .as_ref()
                .is_some_and(|old| anchor.snapshot_id <= old.snapshot_id)
        {
            return;
        }
        let reset = anchor.input_epoch != self.input_epoch
            || anchor.paused != self.paused
            || self.own_chassis().is_none_or(|old| {
                old.placement_revision != anchor.owner.placement_revision
                    || old.defeated != anchor.owner.defeated
                    || old.config != anchor.owner.config
            });
        if reset {
            self.cancel_local_shots();
            self.remote_buffer.reset_timeline();
            self.prediction.inputs.reset();
            self.last_input_frame = None;
            self.prediction.chassis = None;
            self.prediction.proof = None;
            self.prediction.epoch = self.prediction.epoch.wrapping_add(1);
            self.camera_offset_m = [0.; 3];
            self.frame_time_ns = anchor.time_ns;
        }
        self.prediction.inputs.finalize(anchor.time_ns);
        self.input_epoch = anchor.input_epoch;
        self.paused = anchor.paused;
        self.last_checkpoint = self.time.now();
        self.owner_anchor = Some(anchor);
    }
    /// Complete each prediction request before a scripted trace advances time.
    #[cfg(test)]
    pub fn synchronize_prediction_for_test(&mut self) {
        if let Some(worker) = &mut self.prediction.worker {
            worker.synchronize_for_test();
        }
    }

    fn update_prediction(&mut self) {
        use rm_simulator_server::prediction::Replay;
        let time = self.input_time_ns();
        let decay = (1. - self.time.since(self.camera_offset_at).as_secs_f64() / 0.1).clamp(0., 1.);
        self.camera_offset_m = self.camera_offset_m.map(|v| v * decay);
        if self.camera_offset_m.iter().map(|v| v * v).sum::<f64>() < 1e-8 {
            self.camera_offset_m = [0.; 3];
        }
        self.camera_offset_at = self.time.now();
        let enabled = self.prediction.context_input_epoch == self.input_epoch
            && !self.paused
            && self.own_chassis().is_some_and(|c| !c.defeated)
            && self.prediction.inputs.inputs().is_some();
        if !enabled {
            return;
        }
        if self.prediction_limited() {
            return;
        }
        let captures = self
            .prediction
            .worker
            .as_ref()
            .map(|worker| worker.shot_samples(self.pending_shot_samples()))
            .unwrap_or_default();
        self.resolve_shot_muzzles(captures);
        let Some(worker) = &self.prediction.worker else {
            return;
        };
        let anchor = self
            .owner_anchor
            .as_ref()
            .filter(|a| a.snapshot_id > self.snapshot_id);
        let mut states = self.snapshot.chassis.clone();
        if let Some(anchor) = anchor {
            states.retain(|s| s.id != anchor.owner.id);
            states.push(anchor.owner.clone());
        }
        let context = match &self.prediction.context {
            Some((id, snapshot)) if *id == self.snapshot_id => snapshot.clone(),
            _ => {
                let snapshot = std::sync::Arc::new(self.snapshot.clone());
                self.prediction.context = Some((self.snapshot_id, snapshot.clone()));
                snapshot
            }
        };
        let replay = Replay {
            snapshot: context,
            snapshot_id: anchor.map_or(self.snapshot_id, |a| a.snapshot_id),
            context_id: self.snapshot_id,
            context_time_ns: self.snapshot.time_ns,
            chassis: self.chassis_id.expect("enabled prediction owns chassis"),
            states,
            snapshot_time_ns: self.owner_time_ns(),
            target_time_ns: time,
            inputs: self.prediction.inputs.inputs().unwrap_or_default(),
        };
        let result = worker.exchange(self.prediction.epoch, replay);
        self.accept_prediction(result);
    }
    fn accept_prediction(
        &mut self,
        result: Option<(
            u64,
            rm_simulator_server::prediction::PredictionProof,
            ChassisSnapshot,
        )>,
    ) {
        if result.is_some() {
            self.prediction.results_discarded += 1;
        }
        if let Some((epoch, proof, state)) = result
            && self.prediction_result_is_usable(epoch, proof)
        {
            if self
                .prediction
                .proof
                .is_some_and(|old| old.snapshot_id != proof.snapshot_id)
                && let Some(old) = &self.prediction.chassis
            {
                let dt = proof.target_time_ns.saturating_sub(self.prediction.time_ns) as f64 * 1e-9;
                let offset = std::array::from_fn(|i| {
                    self.camera_offset_m[i] + old.pose.translation_m[i] + old.velocity_m_s[i] * dt
                        - state.pose.translation_m[i]
                });
                self.camera_offset_m = if offset.iter().map(|v| v * v).sum::<f64>() <= 0.01 {
                    offset
                } else {
                    [0.; 3]
                };
            }
            self.prediction.results_discarded -= 1;
            self.prediction.results_accepted += 1;
            self.prediction.time_ns = proof.target_time_ns;
            self.prediction.proof = Some(proof);
            self.prediction.chassis = Some(state);
        }
    }
    fn prediction_result_is_usable(
        &self,
        epoch: u64,
        proof: rm_simulator_server::prediction::PredictionProof,
    ) -> bool {
        // A replay uses the collision context captured in its request. Ordinary
        // newer snapshots must not starve its presentation. Life/config/pause
        // changes invalidate the generation before we consume worker results.
        epoch == self.prediction.epoch
            && proof.snapshot_id
                <= self
                    .owner_anchor
                    .as_ref()
                    .map_or(self.snapshot_id, |a| a.snapshot_id.max(self.snapshot_id))
            && proof.target_time_ns >= self.owner_time_ns()
            && self.input_time_ns().saturating_sub(proof.target_time_ns)
                <= rm_simulator_server::prediction::MAX_CONTINUOUS_REPLAY_NS
            && self.prediction.proof.is_none_or(|old| {
                proof.snapshot_id >= old.snapshot_id && proof.target_time_ns >= old.target_time_ns
            })
    }
    /// True while a full target checkpoint arrived in the last 300 ms. Owner
    /// anchors cannot extend enemy observation freshness. Aim assist waits
    /// for fresh observation instead of firing on stale state.
    pub fn aim_observation_fresh(&self) -> bool {
        self.client.disconnected().is_none()
            && self.time.since(self.last_context) < std::time::Duration::from_millis(300)
    }
    /// Read-only scenery for aim-assist visibility; no reduced physics world.
    pub fn aim_geometry(&self) -> Option<rm_simulator_world::StaticGeometry> {
        self.prediction.geometry.as_ref().map(|geometry| {
            geometry.at_mechanisms(&rm_simulator_world::referee::mechanism_view(
                self.snapshot.referee.as_ref(),
                self.fire_time_ns(),
            ))
        })
    }
    /// Camera-only correction. Collision and the sampled shot muzzle stay unmodified.
    pub fn corrected_eye(&self, eye: Vec3) -> Vec3 {
        if self.camera_offset_m == [0.; 3] {
            return eye;
        }
        let original = rm_simulator_render::flu_vector(eye);
        let center = std::array::from_fn(|i| original[i] + self.camera_offset_m[i] * 0.5);
        let remote_near = self.snapshot.chassis.iter().any(|c| {
            Some(c.id) != self.chassis_id
                && c.pose
                    .translation_m
                    .into_iter()
                    .zip(center)
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum::<f64>()
                    < 4.
        });
        if !remote_near
            && self.prediction.geometry.as_ref().is_some_and(|geometry| {
                geometry
                    .at_mechanisms(&rm_simulator_world::referee::mechanism_view(
                        self.snapshot.referee.as_ref(),
                        self.frame_time_ns,
                    ))
                    .sphere_clear(center, 0.15)
            })
        {
            eye + rm_simulator_render::flu_position(self.camera_offset_m)
        } else {
            eye
        }
    }
    /// Transport statistics for the network HUD. An embedded host reports
    /// transport `local` with no remote RTT or native path.
    pub fn network_stats(&self) -> rm_simulator_server::network_stats::NetworkStats {
        let mut stats = self.client.network_stats();
        if self.host.is_some() {
            stats.transport = "local".into();
            stats.native = None;
            stats.app_rtt_ms = None;
            stats.pending_response_age_ms = None;
        }
        stats
    }
    /// Console JSON of correction, buffering, input timing and shot
    /// diagnostics. The values are presentation estimates, not rule state.
    pub fn network_diagnostics(&self) -> serde_json::Value {
        let correction = self
            .prediction
            .worker
            .as_ref()
            .and_then(|p| p.correction_stats());
        let telemetry = self
            .client
            .host_telemetry()
            .filter(|t| t.0 == self.input_epoch);
        serde_json::json!({
            "remote_view_age_ms": self.remote_buffer.view_age_ms,
            "remote_buffer_delay_ms": self.remote_buffer.delay_ms(),
            "remote_underruns": self.remote_buffer.underruns,
            "correction_orientation_rad": correction.as_ref().and_then(|s| s.orientation_rad),
            "correction_velocity_m_s": correction.as_ref().and_then(|s| s.velocity_m_s),
            "correction_aim_rad": correction.as_ref().and_then(|s| s.aim_rad),
            "remote_buffer": { "automatic": self.remote_buffer.automatic,
                "delay_ms": self.remote_buffer.delay_ms(), "view_age_ms": self.remote_buffer.view_age_ms,
                "underruns": self.remote_buffer.underruns },
            "event_samples": {
                "shot_confirmation_ms": self.shots.confirmation_samples,
                "shot_execution_offset_ms": self.shots.execution_samples,
                "rtt_ms": self.client.round_trip_samples(),
                "checkpoint_arrival_interval_ms": self.client.checkpoint_interval_samples(),
            },
            "shot_confirmation_ms": self.shots.last_confirmation_ms,
            "shot_execution_offset_ms": self.shots.last_execution_offset_ms,
            "collision_context_gap_ms": self.time.since(self.last_context).as_secs_f64() * 1000.,
            "collision_context_age_ms": self.owner_time_ns().saturating_sub(self.snapshot.time_ns) as f64 / 1e6,
            "collision_context_coherent": self.owner_time_ns() == self.snapshot.time_ns,
            "host_input": telemetry,
            "input_timing_hint": telemetry.map(|t| if t.3.executed_time_ns > t.3.intended_time_ns {
                "late_input"
            } else if t.3.lease_expiries > 0 { "lease_expiry_observed" } else { "unknown" }),
            "input_execution_lateness_ms": telemetry.map(|t| t.3.executed_time_ns.saturating_sub(t.3.intended_time_ns) as f64 / 1e6),
            "correction_position_m": correction.as_ref().and_then(|s| s.position_m),
            "correction": correction,
            "stats": self.network_stats(),
            "downstream_queues": self.client.delivery_stats(),
            "trace": self.client.trace_report(),
            "checkpoint_gap_ms": self.time.since(self.last_checkpoint).as_secs_f64() * 1000.,
            "rtt_ms": self.client.round_trip_ns().map(|ns| ns as f64 / 1e6),
            "input_lead_ms": self.client.input_lead_ns() as f64 / 1e6,
            "snapshot_clock_offset_ms": self.frame_time_ns.saturating_sub(self.snapshot.time_ns) as f64 / 1e6,
            "prediction_worker": self.prediction.worker.as_ref().and_then(|worker| worker.diagnostics()),
            "prediction_moving_frames": self.prediction.moving_frames,
            "prediction_held_moving_frames": self.prediction.held_moving_frames,
            "prediction_results_accepted": self.prediction.results_accepted,
            "prediction_results_discarded": self.prediction.results_discarded,
            "predicted_time_ns": self.prediction.time_ns,
            "prediction_backlog_ms": self.input_time_ns().saturating_sub(self.prediction.time_ns) as f64 / 1e6,
            "prediction_limited": self.prediction_limited(),
            "unresolved_shot_outcomes": self.shots.unresolved_outcomes,
            "confirmed_launches": self.shots.confirmed_launches,
            "hit_feedback": self.hit_feedback.diagnostics(self.time.now()),
            "rejections": self.shot_rejection_count,
            "last_rejection": self.last_shot_rejection,
        })
    }
    fn input_time_ns(&self) -> u64 {
        self.frame_time_ns
            .saturating_add(if self.paused {
                0
            } else {
                self.client.input_lead_ns()
            })
            .min(
                self.owner_time_ns()
                    .saturating_add(rm_simulator_server::prediction::MAX_CONTINUOUS_REPLAY_NS),
            )
            / rm_simulator_world::tick_ns()
            * rm_simulator_world::tick_ns()
    }
    fn prediction_limited(&self) -> bool {
        self.client.disconnected().is_some()
            || self.time.since(self.last_checkpoint) >= std::time::Duration::from_secs(1)
    }
    /// The match referee's snapshot, or `None` when this field has no referee.
    pub fn referee(&self) -> Option<&RefereeSnapshot> {
        self.snapshot.referee.as_ref()
    }
}

/// Narrow views the deterministic network trace asserts on. They read fields
/// `network_diagnostics` already publishes as JSON, without the round trip.
#[cfg(test)]
impl Session {
    /// Local shots begun so far, whatever became of them.
    pub(crate) fn shot_intents(&self) -> u64 {
        self.next_shot_id - 1
    }
    /// Shots that expired with no authoritative answer.
    pub(crate) fn unresolved_shot_outcomes(&self) -> u64 {
        self.shots.unresolved_outcomes
    }
    pub(crate) fn shot_rejections(&self) -> u64 {
        self.shot_rejection_count
    }
    /// Inputs the last checkpoint has not finalized; a replay may reapply these
    /// and nothing older.
    pub(crate) fn pending_inputs(
        &self,
    ) -> Option<Vec<rm_simulator_server::prediction::PendingInput>> {
        self.prediction.inputs.inputs()
    }
    pub(crate) fn correction_stats(
        &self,
    ) -> Option<rm_simulator_server::network_stats::CorrectionStats> {
        self.prediction
            .worker
            .as_ref()
            .and_then(|p| p.correction_stats())
    }
    pub(crate) fn predicts(&self) -> bool {
        self.prediction.worker.is_some()
    }
}

/// Poll the host and handle match controls. For the referee (or any local world) `F6` pauses, `F7`
/// steps while paused, `F5` starts or resets a match and `F` spends a rune
/// opportunity.
pub fn advance_world(
    ui: Res<crate::hud::HudState>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    mut session: ResMut<Session>,
    mut commands: Commands,
) {
    use crate::bindings::InputAction;
    use rm_simulator_server::simulation::step_ticks;
    use rm_simulator_world::{MatchPhase, RefereeCommand};
    if ui.blocks_input() {
        poll_session(&mut session, &mut commands);
        return;
    }
    if let Some(chassis) = session.chassis_id {
        for (key, caliber) in [
            (InputAction::Buy17, rm_simulator_world::Caliber::Mm17),
            (InputAction::Buy42, rm_simulator_world::Caliber::Mm42),
        ] {
            if ui.controls.just_pressed(key, &keys, buttons.as_deref()) {
                session.apply(Command::BuyAmmo { chassis, caliber });
            }
        }
    }
    let match_key = [
        InputAction::Pause,
        InputAction::Step,
        InputAction::Start,
        InputAction::Rune,
    ]
    .into_iter()
    .any(|key| ui.controls.just_pressed(key, &keys, buttons.as_deref()));
    if match_key && !session.referees() {
        session.notice("only the referee runs the match".into());
    }
    if !session.referees() {
        poll_session(&mut session, &mut commands);
        return;
    }
    if ui
        .controls
        .just_pressed(InputAction::Pause, &keys, buttons.as_deref())
    {
        let paused = !session.paused;
        session.apply(Command::Pause { paused });
    }
    if session.paused
        && ui
            .controls
            .just_pressed(InputAction::Step, &keys, buttons.as_deref())
    {
        session.apply(Command::Step {
            ticks: step_ticks(),
        });
    }
    if ui
        .controls
        .just_pressed(InputAction::Start, &keys, buttons.as_deref())
    {
        match session.referee().map(|referee| referee.phase) {
            Some(MatchPhase::Idle) => session.apply(Command::Referee(RefereeCommand::StartMatch)),
            Some(MatchPhase::Finished) => {
                session.apply(Command::Referee(RefereeCommand::ResetMatch))
            }
            Some(_) => session.notice("match in progress (reset from the referee panel)".into()),
            None => session.notice("no referee on this field".into()),
        }
    }
    if ui
        .controls
        .just_pressed(InputAction::Rune, &keys, buttons.as_deref())
    {
        let team = session.team;
        match session.referee() {
            Some(_) => session.apply(Command::Referee(RefereeCommand::ActivateRune { team })),
            None => session.notice("no referee on this field".into()),
        }
    }
    poll_session(&mut session, &mut commands);
}
/// A failed poll ends the match, not the app: the title screen shows why.
fn poll_session(session: &mut Session, commands: &mut Commands) {
    if let Err(error) = session.poll_inputs() {
        commands.insert_resource(crate::loading::LeaveRequest(Some(error)));
    }
}

/// Silence is not proof of disconnect. Only transport closure uses that label.
fn connection_warning(
    disconnected: bool,
    gap: std::time::Duration,
    rtt_ns: Option<u64>,
) -> Option<&'static str> {
    if disconnected {
        Some("Disconnected")
    } else if gap >= std::time::Duration::from_millis(250) {
        Some("Connection interrupted")
    } else if rtt_ns.is_some_and(|rtt| rtt >= 200_000_000) {
        Some("High latency")
    } else {
        None
    }
}

/// Advance provisional flight after this frame's aim and controls have been sampled.
pub fn advance_shots(mut session: ResMut<Session>) {
    session.advance_local_shots();
}
/// Exchange a whole-field replay after sampling this frame's controls and shots.
pub fn predict_frame(mut session: ResMut<Session>) {
    session.predict_frame();
}
impl Session {
    fn predict_frame(&mut self) {
        let previous_prediction_time_ns = self.prediction.last_frame_time_ns;
        self.update_prediction();
        self.prediction.last_frame_time_ns = self.prediction.time_ns;
        if !self.paused
            && self
                .prediction
                .chassis
                .as_ref()
                .is_some_and(|state| state.velocity_m_s.iter().map(|v| v * v).sum::<f64>() > 0.01)
        {
            self.prediction.moving_frames += 1;
            if previous_prediction_time_ns == self.prediction.time_ns {
                self.prediction.held_moving_frames += 1;
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn test_session(paused: bool) -> Session {
    test_session_mode(paused, false, TimeSource::system())
}

/// The embedded host still runs on the real clock and a loopback connection;
/// `time` paces only the session and its client leg.
#[cfg(test)]
fn test_session_mode(paused: bool, predict: bool, time: TimeSource) -> Session {
    use clap::Parser;
    let mut args = Args::try_parse_from([
        "rm-simulator",
        "--no-field-collision",
        "--no-rune",
        "--no-referee",
    ])
    .unwrap();
    args.host.start_paused = paused;
    args.no_prediction = !predict;
    let cad = test_cad_assets();
    let mut opened = Session::open(&args, &cad, [2.0, 3.0, 1.0], 90.0, |_, _| {}).unwrap();
    opened.session.rebase_clock(time);
    opened.session
}

/// A package stand-in for tests that never touch terrain. `--no-field-collision`
/// keeps every path that would read these files out of the build.
#[cfg(test)]
pub(crate) fn test_cad_assets() -> CadAssets {
    use rm_simulator_server::cad_assets::CadAsset;
    let asset = CadAsset {
        file: "unused.glb".into(),
        semantics: None,
        placements: vec![rm_simulator_world::Pose::at([10.0, 10.0, 10.0])],
        collision: None,
        source_tessellated_collision: false,
    };
    CadAssets {
        root: "unused".into(),
        floor_top_cad_m: 0.0,
        collision_solids: false,
        floor: asset.clone(),
        arena_static: asset.clone(),
        rune: asset.clone(),
        outpost: asset.clone(),
        base: asset.clone(),
        tech_core: asset,
        static_assets: Vec::new(),
    }
}

#[cfg(test)]
pub(crate) fn wait_for_session(session: &mut Session, done: impl Fn(&Session) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        session.poll().unwrap();
        if done(session) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "session update timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_prediction_survives_new_anchor_without_rewinding_or_crossing_reset() {
        use rm_simulator_server::prediction::PredictionProof;
        let mut session = test_session(true);
        session.owner_anchor = None;
        session.snapshot_id = 12;
        let time = session.snapshot.time_ns;
        let proof = PredictionProof {
            snapshot_id: 11,
            target_time_ns: time + 20_000_000,
            last_input_sequence: 5,
        };
        session.prediction.proof = Some(PredictionProof {
            snapshot_id: 10,
            target_time_ns: time + 10_000_000,
            last_input_sequence: 4,
        });
        let epoch = session.prediction.epoch;
        assert!(session.prediction_result_is_usable(epoch, proof));
        assert!(!session.prediction_result_is_usable(epoch.wrapping_add(1), proof));
        assert!(!session.prediction_result_is_usable(
            epoch,
            PredictionProof {
                target_time_ns: time + 9_000_000,
                ..proof
            }
        ));
        assert!(!session.prediction_result_is_usable(
            epoch,
            PredictionProof {
                snapshot_id: 9,
                ..proof
            }
        ));
        assert!(!session.prediction_result_is_usable(
            epoch,
            PredictionProof {
                target_time_ns: time.saturating_sub(1),
                ..proof
            }
        ));
        assert!(!session.prediction_result_is_usable(
            epoch,
            PredictionProof {
                snapshot_id: 13,
                ..proof
            }
        ));
    }

    #[test]
    fn collision_drawing_needs_no_host_or_prediction_worker() {
        let mut session = super::test_session(true);
        session.host = None;
        session.prediction.worker = None;
        session.prediction.geometry = Some(rm_simulator_world::StaticGeometry::default());
        assert!(session.collision_geometry().unwrap()().is_ok());
        assert!(session.client_collision_geometry().is_some());
    }

    #[test]
    fn owner_anchor_survives_missing_world_context_and_rejects_old_lives() {
        let mut session = test_session(true);
        let original = session.snapshot.clone();
        let mut anchor = rm_simulator_server::owner_stream::OwnerAnchor {
            snapshot_id: session.snapshot_id + 2,
            input_epoch: session.input_epoch,
            time_ns: session.snapshot.time_ns + 50_000_000,
            paused: true,
            owner: session.own_chassis().unwrap().clone(),
        };
        anchor.owner.pose.translation_m[0] += 1.;
        session.apply_owner_anchor(anchor.clone());
        assert_eq!(session.snapshot, original);
        assert_eq!(session.own_chassis().unwrap().pose, anchor.owner.pose);
        assert_eq!(
            session.network_diagnostics()["collision_context_coherent"],
            false
        );
        let mut stale = anchor.clone();
        stale.snapshot_id -= 1;
        stale.owner.pose.translation_m[0] -= 10.;
        session.apply_owner_anchor(stale);
        assert_eq!(session.own_chassis().unwrap().pose, anchor.owner.pose);
        anchor.snapshot_id += 1;
        anchor.owner.placement_revision += 1;
        let epoch = session.prediction.epoch;
        session.apply_owner_anchor(anchor.clone());
        assert_eq!(session.prediction.epoch, epoch + 1);
        assert_eq!(
            session.own_chassis().unwrap().placement_revision,
            anchor.owner.placement_revision
        );
    }
    #[test]
    fn embedded_session_holds_loading_then_runs_and_confirms_pause_and_step() {
        let mut session = test_session(false);
        assert!(!session.is_remote());
        assert!(!session.local_aim);
        assert!(session.prediction.worker.is_none());
        assert!(session.referees());
        assert_eq!(session.role, Role::Pilot);
        assert_eq!(
            &session.own_chassis().unwrap().pose.translation_m[..2],
            &[2.0, 3.0]
        );
        assert_eq!(session.roster.len(), 1);
        std::thread::sleep(std::time::Duration::from_millis(30));
        session.poll().unwrap();
        assert_eq!(session.snapshot.tick, 0);
        session.ready();
        wait_for_session(&mut session, |session| session.snapshot.tick > 0);
        let start = std::time::Instant::now();
        for _ in 0..20 {
            session
                .client
                .wait_snapshot(std::time::Duration::from_secs(5))
                .unwrap();
        }
        eprintln!(
            "embedded snapshot interval over 20 updates: {:?}",
            start.elapsed() / 20
        );
        session.apply(Command::Pause { paused: true });
        assert!(!session.commands_confirmed());
        wait_for_session(&mut session, Session::commands_confirmed);
        assert!(session.paused);
        let before = session.snapshot.tick;
        session.apply(Command::Step { ticks: 16 });
        wait_for_session(&mut session, Session::commands_confirmed);
        assert_eq!(session.snapshot.tick, before + 16);
        assert!(
            session
                .host
                .as_ref()
                .unwrap()
                .server
                .listening_addr()
                .is_none()
        );
        assert_eq!(session.client.network_stats().transport, "local");
        drop(session);
    }

    #[test]
    fn prediction_is_presentation_only_and_resets_on_pause_and_placement() {
        let mut session = test_session_mode(false, true, TimeSource::system());
        session.ready();
        wait_for_session(&mut session, |s| s.prediction.chassis.is_some());
        let id = session.chassis_id.unwrap();
        session.apply(Command::Chassis {
            chassis: id,
            command: rm_simulator_world::ChassisCommand {
                forward_m_s: 1.0,
                ..Default::default()
            },
        });
        assert_eq!(session.prediction.inputs.inputs().unwrap().len(), 1);
        session.apply(Command::Pause { paused: true });
        wait_for_session(&mut session, Session::commands_confirmed);
        assert!(session.paused);
        assert!(session.prediction.chassis.is_none());
        assert_eq!(session.presented_chassis(), session.own_chassis());
        session.apply(Command::PlaceChassis {
            chassis: id,
            position_m: [10.0, 5.0, 1.0],
            yaw_deg: 0.0,
        });
        wait_for_session(&mut session, Session::commands_confirmed);
        assert_eq!(
            &session.presented_chassis().unwrap().pose.translation_m[..2],
            &[10.0, 5.0]
        );
        assert!(session.prediction.inputs.inputs().unwrap().is_empty());
        session.apply(Command::Pause { paused: false });
        wait_for_session(&mut session, |s| s.prediction.chassis.is_some());
        assert_eq!(
            session.presented_chassis().unwrap().placement_revision,
            session.own_chassis().unwrap().placement_revision
        );
    }

    #[test]
    fn aim_only_frames_are_coalesced_to_the_sixteen_millisecond_grid() {
        use rm_simulator_server::clock::ManualTime;
        let time = ManualTime::new();
        let mut session = test_session_mode(true, false, time.source());
        let chassis = session.chassis_id.unwrap();
        // One simulated second of 240 fps aim motion, no drive transitions.
        let frame = std::time::Duration::from_nanos(1_000_000_000 / 240);
        let mut aim_yaw_rad = 0.;
        for _ in 0..240 {
            time.advance(frame);
            aim_yaw_rad += 0.001;
            session.drive(
                chassis,
                ChassisCommand {
                    aim_yaw_rad,
                    ..Default::default()
                },
                false,
            );
        }
        let sent = session.last_input_frame.expect("a drive frame was sent");
        assert_eq!(
            sent.sequence, 60,
            "one input frame per 16 ms, not per frame"
        );
        // The frame at the boundary carries the aim sampled for it.
        assert_eq!(sent.command.aim_yaw_rad, aim_yaw_rad);
    }

    #[test]
    fn firing_flushes_aim_between_refreshes() {
        let time = rm_simulator_server::clock::ManualTime::new();
        let mut session = test_session_mode(true, false, time.source());
        let chassis = session.chassis_id.unwrap();
        session.drive(chassis, ChassisCommand::default(), true);
        time.advance(std::time::Duration::from_millis(1));
        session.drive(
            chassis,
            ChassisCommand {
                aim_yaw_rad: 0.5,
                ..Default::default()
            },
            false,
        );
        assert_eq!(session.last_input_frame.unwrap().command.aim_yaw_rad, 0.);
        session.begin_local_shot(chassis).unwrap();
        assert_eq!(session.last_input_frame.unwrap().command.aim_yaw_rad, 0.5);
        assert_eq!(session.pending_shot_samples()[0].aim.aim_yaw_rad, 0.5);
    }

    #[test]
    fn owner_updates_do_not_refresh_enemy_observations() {
        let time = rm_simulator_server::clock::ManualTime::new();
        let mut session = test_session_mode(true, false, time.source());
        time.advance(std::time::Duration::from_millis(301));
        session.last_checkpoint = time.source().now();
        assert!(!session.aim_observation_fresh());
    }

    #[test]
    fn a_drive_transition_does_not_wait_for_the_grid() {
        use rm_simulator_server::clock::ManualTime;
        let time = ManualTime::new();
        let mut session = test_session_mode(true, false, time.source());
        let chassis = session.chassis_id.unwrap();
        let driving = ChassisCommand {
            forward_m_s: 1.,
            ..Default::default()
        };
        session.drive(chassis, driving, true);
        assert_eq!(session.last_input_frame.unwrap().sequence, 1);
        time.advance(std::time::Duration::from_millis(1));
        session.drive(
            chassis,
            ChassisCommand {
                aim_yaw_rad: 0.5,
                ..driving
            },
            false,
        );
        assert_eq!(
            session.last_input_frame.unwrap().sequence,
            1,
            "aim alone waits for the grid"
        );
        session.drive(chassis, ChassisCommand::default(), true);
        assert_eq!(session.last_input_frame.unwrap().sequence, 2);
        assert_eq!(session.last_input_frame.unwrap().command.forward_m_s, 0.);
    }

    #[test]
    fn connection_toast_distinguishes_latency_silence_and_closed_transport() {
        use std::time::Duration;
        assert_eq!(
            connection_warning(false, Duration::ZERO, Some(50_000_000)),
            None
        );
        assert_eq!(
            connection_warning(false, Duration::ZERO, Some(220_000_000)),
            Some("High latency")
        );
        assert_eq!(
            connection_warning(false, Duration::from_millis(600), Some(50_000_000)),
            Some("Connection interrupted")
        );
        assert_eq!(
            connection_warning(true, Duration::ZERO, None),
            Some("Disconnected")
        );
    }

    #[test]
    fn start_paused_stays_paused_and_disconnect_keeps_the_window_alive() {
        let mut session = test_session(true);
        session.ready();
        std::thread::sleep(std::time::Duration::from_millis(20));
        session.poll().unwrap();
        assert!(session.paused);
        assert_eq!(session.snapshot.tick, 0);
        session.host.as_ref().unwrap().server.stop();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while session.connection_toast != Some("Disconnected") {
            session.poll().unwrap();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}
