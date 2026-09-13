// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! App console commands shared by automation and a future in-game console.
//! The TCP adapter accepts one local controller and executes requests in order.
//! Socket work is nonblocking and bounded per frame; no worker touches Bevy state.
use crate::{
    controls::{Drive, Player},
    screenshot::{CaptureReadiness, ScreenshotRequest},
    session::Session,
};
use bevy::{input::mouse::AccumulatedMouseMotion, prelude::*};
use rm_simulator_server::protocol::Command;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    time::{Duration, Instant},
};

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(120);

/// One JSON line from the console client: a caller-chosen id and a typed command.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Caller-chosen identifier echoed in the reply; any 64-bit unsigned value is accepted.
    pub id: u64,
    /// Command to run; serde rejects unknown fields and unknown command names.
    pub command: ConsoleCommand,
}

/// Typed commands are independent of the TCP transport. Distances and headings
/// use world FLU metres and degrees; mouse motion uses pixels.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
// Commands stay inline in the bounded local console queue.
#[allow(clippy::large_enum_variant)]
pub enum ConsoleCommand {
    /// Reports readiness and the supported command names; accepted during loading.
    Help {},
    /// Waits until scenery, pipelines and confirmed commands are all ready.
    Ready {},
    /// Reports tick, clock, role, chassis, camera, UI and network state; accepted during loading.
    State {},
    /// Sets the free-camera pose, or the local pilot's aim yaw and pitch.
    Camera {
        /// New eye position in world FLU metres; refused while a local pilot is driven.
        position_m: Option<[f32; 3]>,
        /// Heading counter-clockwise from world +x, wrapped into [0, 360) degrees.
        yaw_deg: Option<f32>,
        /// Positive looks up, in -89..=89 degrees; the value must be finite.
        pitch_deg: Option<f32>,
        /// Selects the third-person pilot camera; requires a local pilot.
        third_person: Option<bool>,
    },
    /// Places the local pilot on the ground below a position, or moves the free camera.
    Spawn {
        /// Target position in world FLU metres, finite and within 100000 m.
        position_m: [f64; 3],
        /// Post-spawn heading counter-clockwise from world +x, wrapped into [0, 360).
        #[serde(default)]
        yaw_deg: f32,
    },
    /// Sets the world clock's paused flag.
    Pause {
        /// True pauses the clock, false resumes it.
        paused: bool,
    },
    /// Advances a paused world by a whole number of 1 ms ticks.
    Step {
        /// Number of milliseconds to advance, from 1 to 60000.
        ticks: u64,
    },
    /// Saves the current rendered view and HUD to a PNG without exiting the app.
    Screenshot {
        /// Destination file, which must end in `.png`; its parent directory must exist.
        path: std::path::PathBuf,
    },
    /// Holds or releases one game key as if the keyboard sent it.
    Key {
        /// Case-insensitive name such as `w`, `space` or `f3`; unknown names are errors.
        key: String,
        /// True presses the key, false releases it.
        pressed: bool,
    },
    /// Presses or releases a mouse button in gameplay and UI picking.
    MouseButton {
        /// One of `left`, `right` or `middle`.
        button: String,
        /// True presses the button, false releases it.
        pressed: bool,
    },
    /// Adds relative mouse motion to the frame's accumulated delta.
    MouseMotion {
        /// Horizontal motion in pixels.
        dx: f32,
        /// Vertical motion in pixels.
        dy: f32,
    },
    /// Moves the primary window's pointer for UI picking.
    Cursor {
        /// Horizontal position in logical pixels from the window's left edge.
        x: f32,
        /// Vertical position in logical pixels from the window's top edge.
        y: f32,
    },
    /// Sets gameplay mouse capture without grabbing or hiding the OS cursor.
    Capture {
        /// True captures the mouse for aiming, false releases it.
        captured: bool,
    },
    /// Releases every console-held key and button and ends gameplay mouse capture.
    ReleaseInputs {},
    /// Sets the runtime inspection options that the debug overlays read.
    Inspect {
        /// Opens or closes the debug options panel.
        debug_panel: Option<bool>,
        /// Turns the global wireframe overlay on or off.
        wireframe: Option<bool>,
        /// One of `hidden`, `overlay` or `alone`; other values are errors.
        collision_view: Option<String>,
        /// Shows or hides the render statistics in the debug panel.
        render_stats: Option<bool>,
    },
    /// Submits a multiplayer protocol command through the ordinary session.
    World {
        /// Command to submit; match controls need a session that may referee.
        command: Command,
    },
    /// Acknowledges the request, then exits the app once the reply is flushed.
    Quit {},
}

struct Connection {
    stream: TcpStream,
    input: Vec<u8>,
    output: Vec<u8>,
    written: usize,
}
impl Connection {
    fn reply(&mut self, id: Value, result: Result<Value, String>) {
        let value = match result {
            Ok(result) => json!({"id":id, "ok":true, "result":result}),
            Err(error) => json!({"id":id, "ok":false, "error":error}),
        };
        self.output = serde_json::to_vec(&value).expect("console response serializes");
        self.output.push(b'\n');
        self.written = 0;
    }
    fn pump(&mut self) -> io::Result<()> {
        // At most one bounded write/read per frame, including slow clients.
        if self.written < self.output.len() {
            let end = (self.written + MAX_REQUEST_BYTES).min(self.output.len());
            match self.stream.write(&self.output[self.written..end]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => self.written += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
        }
        if self.written == self.output.len() {
            self.output.clear();
            self.written = 0;
        }
        let mut bytes = [0; 8192];
        match self.stream.read(&mut bytes) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                if self.input.len() + n > MAX_REQUEST_BYTES {
                    return Err(io::Error::other("console request buffer exceeds 64 KiB"));
                }
                self.input.extend_from_slice(&bytes[..n]);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        Ok(())
    }
}

/// How a pending request finishes; the completion stage polls it on every later frame.
pub(crate) enum Wait {
    /// Completes when scenery, pipelines and confirmed commands are ready.
    Ready,
    /// Completion passes left before the reply; the last pass confirms the session inputs.
    Applied(u32),
    /// Completes when the pending `ScreenshotRequest` reports a saved or failed capture.
    Screenshot,
    /// Queues the quit acknowledgement; the app exits after the client reads it.
    Quit,
    /// The acknowledgement is queued; the app exits once it has been written out.
    Quitting,
}
struct Pending {
    id: u64,
    started: Instant,
    rejection_count: u64,
    wait: Wait,
}

/// Console endpoint resource: the loopback listener, the one accepted controller
/// and the one request awaiting completion.
#[derive(Resource)]
pub struct Console {
    listener: TcpListener,
    connection: Option<Connection>,
    pending: Option<Pending>,
}
#[derive(Resource, Default)]
struct ConsoleInputs {
    keys: HashSet<KeyCode>,
    buttons: HashSet<MouseButton>,
}
impl Console {
    /// Binds a nonblocking console listener and prints the resolved `tcp://` address.
    /// A non-loopback address is refused so the endpoint stays on this host.
    /// Port 0 allocates a free port, which the printed line reports.
    pub fn bind(address: SocketAddr) -> io::Result<Self> {
        if !address.ip().is_loopback() {
            return Err(io::Error::other("console address must be loopback"));
        }
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        println!("app console: tcp://{}", listener.local_addr()?);
        Ok(Self {
            listener,
            connection: None,
            pending: None,
        })
    }
    fn reply(&mut self, id: u64, result: Result<Value, String>) {
        if let Some(connection) = &mut self.connection {
            connection.reply(json!(id), result);
        }
    }
}

fn release_inputs(world: &mut World) {
    world.resource_scope(|world, mut inputs: Mut<ConsoleInputs>| {
        for key in inputs.keys.drain() {
            world.resource_mut::<ButtonInput<KeyCode>>().release(key);
        }
        for button in inputs.buttons.drain() {
            let _ = pointer_button(world, button, false);
            world
                .resource_mut::<ButtonInput<MouseButton>>()
                .release(button);
        }
        set_capture(world, false);
    });
}

/// Adds the console systems around a `Console` resource inserted before this plugin.
/// Requests are read before picking and completed after the frame's own updates.
pub struct ConsolePlugin;
impl Plugin for ConsolePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConsoleInputs>()
            .add_systems(
                PreUpdate,
                receive
                    .after(bevy::input::InputSystems)
                    .before(bevy::picking::PickingSystems::ProcessInput),
            )
            .add_systems(PostUpdate, complete);
    }
}

fn receive(world: &mut World) {
    world.resource_scope(|world, mut console: Mut<Console>| {
        // Refuse additional controllers without allowing their inputs to interleave.
        if let Ok((stream, _)) = console.listener.accept()
            && console.connection.is_none()
            && stream.set_nonblocking(true).is_ok()
        {
            let _ = stream.set_nodelay(true);
            console.connection = Some(Connection {
                stream,
                input: Vec::new(),
                output: Vec::new(),
                written: 0,
            });
        }
        let disconnected = console
            .connection
            .as_mut()
            .is_some_and(|c| c.pump().is_err());
        if console
            .pending
            .as_ref()
            .is_some_and(|p| matches!(p.wait, Wait::Quitting))
            && console
                .connection
                .as_ref()
                .is_some_and(|c| c.output.is_empty())
        {
            world.write_message(AppExit::Success);
        }
        if disconnected {
            release_inputs(world);
            if console
                .pending
                .as_ref()
                .is_some_and(|p| matches!(p.wait, Wait::Screenshot))
            {
                world.remove_resource::<ScreenshotRequest>();
            }
            console.pending = None;
            console.connection = None;
            return;
        }
        // Focus events may clear Bevy input state. Reapply only console-held inputs.
        let keys: Vec<_> = world
            .resource::<ConsoleInputs>()
            .keys
            .iter()
            .copied()
            .collect();
        let buttons: Vec<_> = world
            .resource::<ConsoleInputs>()
            .buttons
            .iter()
            .copied()
            .collect();
        for key in keys {
            let mut input = world.resource_mut::<ButtonInput<KeyCode>>();
            input.press(key);
            input.clear_just_pressed(key);
        }
        for button in buttons {
            let mut input = world.resource_mut::<ButtonInput<MouseButton>>();
            input.press(button);
            input.clear_just_pressed(button);
        }
        if console.pending.is_some() {
            return;
        }
        let Some(connection) = &mut console.connection else {
            return;
        };
        if !connection.output.is_empty() {
            return;
        }
        let Some(end) = connection.input.iter().position(|b| *b == b'\n') else {
            return;
        };
        let line: Vec<_> = connection.input.drain(..=end).collect();
        let request = match serde_json::from_slice::<Request>(&line) {
            Ok(request) => request,
            Err(error) => {
                let id = serde_json::from_slice::<Value>(&line)
                    .ok()
                    .and_then(|v| v.get("id").cloned())
                    .unwrap_or(Value::Null);
                connection.reply(id, Err(format!("invalid request: {error}")));
                return;
            }
        };
        let rejection_count = world
            .get_resource::<Session>()
            .map_or(0, |s| s.rejection_count);
        match execute(world, request.command) {
            Ok(wait) => {
                console.pending = Some(Pending {
                    id: request.id,
                    started: Instant::now(),
                    rejection_count,
                    wait,
                })
            }
            Err(error) => console.reply(request.id, Err(error)),
        }
    });
}

/// Execute a typed command on the app thread. The completion barrier is handled
/// separately so a caller never blocks rendering or session polling.
pub(crate) fn execute(world: &mut World, command: ConsoleCommand) -> Result<Wait, String> {
    if matches!(command, ConsoleCommand::Ready { .. }) {
        return Ok(Wait::Ready);
    }
    if matches!(command, ConsoleCommand::Quit { .. }) {
        return Ok(Wait::Quit);
    }
    if matches!(
        command,
        ConsoleCommand::Help { .. } | ConsoleCommand::State { .. }
    ) {
        // Handled as immediate queries by the completion stage, including during loading.
        return Ok(Wait::Applied(0));
    }
    if !world.contains_resource::<crate::loading::Ready>() {
        return Err(crate::loading::failure(world)
            .unwrap_or_else(|| "scene is still loading; use ready".into()));
    }
    match command {
        ConsoleCommand::Camera {
            position_m,
            yaw_deg,
            pitch_deg,
            third_person,
        } => {
            if position_m.is_some() && world.contains_resource::<Drive>() {
                return Err(
                    "camera position requires a free camera; use spawn to place a pilot".into(),
                );
            }
            if third_person.is_some() && !world.contains_resource::<Drive>() {
                return Err("third_person requires a pilot".into());
            }
            if position_m.is_some_and(|p| !p.iter().all(|v| v.is_finite()))
                || yaw_deg.is_some_and(|v| !v.is_finite())
                || pitch_deg.is_some_and(|v| !v.is_finite() || !(-89.0..=89.0).contains(&v))
            {
                return Err(
                    "camera pose must be finite; pitch must be between -89 and 89 degrees".into(),
                );
            }
            let mut player = world.resource_mut::<Player>();
            if let Some(position) = position_m {
                player.position = rm_simulator_render::flu_position(position.map(f64::from));
            }
            if let Some(yaw) = yaw_deg {
                player.yaw_rad = yaw.rem_euclid(360.0).to_radians();
            }
            if let Some(pitch) = pitch_deg {
                player.pitch_rad = pitch.to_radians();
            }
            if let Some(third_person) = third_person {
                world.resource_mut::<Drive>().third_person = third_person;
            }
        }
        ConsoleCommand::Spawn {
            position_m,
            yaw_deg,
        } => {
            if !position_m
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 100_000.0)
                || !yaw_deg.is_finite()
            {
                return Err("spawn must be finite and within 100000 metres".into());
            }
            if let Some(drive) = world.get_resource::<Drive>() {
                let chassis = drive.chassis_id;
                let mut session = world.resource_mut::<Session>();
                if session.is_remote() {
                    return Err("pilot placement requires the embedded host".into());
                }
                session.apply_confirmed(Command::PlaceChassis {
                    chassis,
                    position_m,
                    yaw_deg: f64::from(yaw_deg.rem_euclid(360.0)),
                })?;
                // Forget the cached drive command so held controls are resubmitted.
                let third_person = world.resource::<Drive>().third_person;
                world.insert_resource(Drive::new(chassis, third_person));
            } else {
                world.resource_mut::<Player>().position =
                    rm_simulator_render::flu_position(position_m);
            }
            let mut player = world.resource_mut::<Player>();
            player.yaw_rad = yaw_deg.rem_euclid(360.0).to_radians();
            player.pitch_rad = 0.0;
        }
        ConsoleCommand::Pause { paused } => submit(world, Command::Pause { paused })?,
        ConsoleCommand::Step { ticks } => {
            if !world.resource::<Session>().paused {
                return Err("pause the simulation before stepping".into());
            }
            if !(1..=60_000).contains(&ticks) {
                return Err("step between 1 and 60000 ticks".into());
            }
            submit(world, Command::Step { ticks })?;
        }
        ConsoleCommand::World { command } => submit(world, command)?,
        ConsoleCommand::Screenshot { path } => {
            if world.contains_resource::<ScreenshotRequest>() {
                return Err("a screenshot is already pending".into());
            }
            if path.extension().and_then(|s| s.to_str()) != Some("png") {
                return Err("screenshot path must end in .png".into());
            }
            let mut request = ScreenshotRequest::new(path);
            request.exit_after = false;
            world.insert_resource(request);
            return Ok(Wait::Screenshot);
        }
        ConsoleCommand::Key { key, pressed } => {
            let key = parse_key(&key)
                .ok_or_else(|| format!("unsupported key: {key}; see docs/console.md"))?;
            if pressed {
                world.resource_mut::<ConsoleInputs>().keys.insert(key);
                world.resource_mut::<ButtonInput<KeyCode>>().press(key);
            } else {
                world.resource_mut::<ConsoleInputs>().keys.remove(&key);
                world.resource_mut::<ButtonInput<KeyCode>>().release(key);
            }
        }
        ConsoleCommand::MouseButton { button, pressed } => {
            let button = match button.as_str() {
                "left" => MouseButton::Left,
                "right" => MouseButton::Right,
                "middle" => MouseButton::Middle,
                _ => return Err("button must be left, right or middle".into()),
            };
            pointer_button(world, button, pressed)?;
            if pressed {
                world.resource_mut::<ConsoleInputs>().buttons.insert(button);
                world
                    .resource_mut::<ButtonInput<MouseButton>>()
                    .press(button);
            } else {
                world
                    .resource_mut::<ConsoleInputs>()
                    .buttons
                    .remove(&button);
                world
                    .resource_mut::<ButtonInput<MouseButton>>()
                    .release(button);
            }
        }
        ConsoleCommand::MouseMotion { dx, dy } => {
            if !dx.is_finite() || !dy.is_finite() {
                return Err("mouse motion must be finite".into());
            }
            world.resource_mut::<AccumulatedMouseMotion>().delta += Vec2::new(dx, dy);
        }
        ConsoleCommand::Cursor { x, y } => {
            if !x.is_finite() || !y.is_finite() {
                return Err("cursor position must be finite".into());
            }
            let mut query =
                world.query_filtered::<&mut Window, With<bevy::window::PrimaryWindow>>();
            let mut window = query.single_mut(world).map_err(|e| e.to_string())?;
            let old = window.cursor_position().unwrap_or(Vec2::ZERO);
            window.set_cursor_position(Some(Vec2::new(x, y)));
            pointer_event(
                world,
                bevy::picking::pointer::PointerAction::Move {
                    delta: Vec2::new(x, y) - old,
                },
            )?;
        }
        ConsoleCommand::Capture { captured } => set_capture(world, captured),
        ConsoleCommand::ReleaseInputs { .. } => release_inputs(world),
        ConsoleCommand::Inspect {
            debug_panel,
            wireframe,
            collision_view,
            render_stats,
        } => {
            let view = collision_view
                .map(|view| match view.as_str() {
                    "hidden" => Ok(crate::debug::CollisionView::Hidden),
                    "overlay" => Ok(crate::debug::CollisionView::Overlay),
                    "alone" => Ok(crate::debug::CollisionView::Alone),
                    _ => Err("collision_view must be hidden, overlay or alone".to_string()),
                })
                .transpose()?;
            if let Some(view) = view {
                world.resource_mut::<crate::debug::CollisionDebug>().view = view;
            }
            if let Some(debug) = debug_panel {
                world.resource_mut::<crate::hud::HudState>().debug = debug;
            }
            if let Some(wireframe) = wireframe {
                world
                    .resource_mut::<bevy::pbr::wireframe::WireframeConfig>()
                    .global = wireframe;
            }
            if let Some(stats) = render_stats {
                world
                    .resource_mut::<crate::debug_panel::DebugOptions>()
                    .stats = stats;
            }
        }
        ConsoleCommand::Help { .. }
        | ConsoleCommand::Ready { .. }
        | ConsoleCommand::State { .. }
        | ConsoleCommand::Quit { .. } => unreachable!(),
    }
    Ok(Wait::Applied(2))
}

fn submit(world: &mut World, command: Command) -> Result<(), String> {
    let mut session = world.resource_mut::<Session>();
    if command.is_match_control() && !session.referees() {
        return Err("only the referee runs a remote match".into());
    }
    session.apply_confirmed(command)
}

fn complete(world: &mut World) {
    world.resource_scope(|world, mut console: Mut<Console>| {
        let Some(mut pending) = console.pending.take() else {
            return;
        };
        let failure = if matches!(pending.wait, Wait::Quit | Wait::Quitting) {
            None
        } else {
            crate::loading::failure(world).or_else(|| {
                world.get_resource::<Session>().and_then(|s| {
                    (s.rejection_count != pending.rejection_count)
                        .then(|| s.last_rejection.clone())
                        .flatten()
                })
            })
        };
        let result = if let Some(error) = failure {
            Some(Err(error))
        } else if pending.started.elapsed() > TIMEOUT {
            Some(Err("console command timed out after 120 seconds".into()))
        } else {
            match &mut pending.wait {
                Wait::Quit => {
                    // Queue the reply first; exit only after the next pump flushed it.
                    if console
                        .connection
                        .as_ref()
                        .is_some_and(|c| c.output.is_empty())
                    {
                        console.reply(pending.id, Ok(json!({})));
                        pending.wait = Wait::Quitting;
                    }
                    None
                }
                Wait::Quitting => {
                    if console
                        .connection
                        .as_ref()
                        .is_some_and(|c| c.output.is_empty())
                    {
                        world.write_message(AppExit::Success);
                    }
                    None
                }
                Wait::Applied(frames) => {
                    if *frames == 1
                        && let Some(mut session) = world.get_resource_mut::<Session>()
                        && let Err(error) = session.confirm_inputs()
                    {
                        console.reply(pending.id, Err(error));
                        return;
                    }
                    *frames = frames.saturating_sub(1);
                    if *frames == 0
                        && world
                            .get_resource::<Session>()
                            .is_none_or(|s| s.commands_confirmed())
                    {
                        Some(Ok(state(world)))
                    } else {
                        None
                    }
                }
                Wait::Ready => {
                    if world.contains_resource::<crate::loading::Ready>()
                        && world.resource::<CaptureReadiness>().ready()
                        && world.resource::<Session>().commands_confirmed()
                    {
                        Some(Ok(state(world)))
                    } else {
                        None
                    }
                }
                Wait::Screenshot => world
                    .get_resource::<ScreenshotRequest>()
                    .and_then(|s| s.result.clone().map(|r| r.map(|_| json!({"path":s.path})))),
            }
        };
        if let Some(result) = result {
            if matches!(pending.wait, Wait::Screenshot) {
                world.remove_resource::<ScreenshotRequest>();
            }
            console.reply(pending.id, result);
        } else {
            console.pending = Some(pending);
        }
    });
}

fn state(world: &mut World) -> Value {
    let Some(session) = world.get_resource::<Session>() else {
        return json!({"ready":false});
    };
    let player = world.get_resource::<Player>();
    json!({
        "ready": world.contains_resource::<crate::loading::Ready>(),
        "tick": session.snapshot.tick, "paused":session.paused,
        "presentation_time_ns": session.presentation_time_ns(),
        "role":session.role, "chassis_id":session.chassis_id,
        "chassis":session.own_chassis(),
        "presented_chassis":session.presented_chassis(),
        "pending_shots":session.pending_shots(),
        "connection_toast":session.connection_toast,
        "network":session.network_diagnostics(),
        "shots_fired":session.snapshot.shots_fired,
        "hits_detected":session.snapshot.hits_detected,
        "notices":session.notices,
        "camera":player.map(|p| json!({
            "position_m": rm_simulator_render::flu_vector(p.position),
            "yaw_deg":p.yaw_rad.to_degrees(), "pitch_deg":p.pitch_rad.to_degrees(),
            "captured":p.captured,
            "third_person":world.get_resource::<Drive>().is_some_and(|d| d.third_person),
        })),
        "ui": world.get_resource::<crate::hud::HudState>().map(|ui| json!({"debug_panel":ui.debug, "settings":ui.settings, "blocks_input":ui.blocks_input()})),
        "commands":["help","ready","state","camera","spawn","pause","step","screenshot","key",
            "mouse_button","mouse_motion","cursor","capture","release_inputs","inspect","world","quit"],
    })
}

fn parse_key(key: &str) -> Option<KeyCode> {
    use KeyCode::*;
    Some(match key.to_ascii_lowercase().as_str() {
        "b" => KeyB,
        "g" => KeyG,
        "h" => KeyH,
        "i" => KeyI,
        "j" => KeyJ,
        "k" => KeyK,
        "l" => KeyL,
        "o" => KeyO,
        "t" => KeyT,
        "u" => KeyU,
        "x" => KeyX,
        "y" => KeyY,
        "z" => KeyZ,
        "0" => Digit0,
        "1" => Digit1,
        "2" => Digit2,
        "3" => Digit3,
        "4" => Digit4,
        "5" => Digit5,
        "6" => Digit6,
        "7" => Digit7,
        "8" => Digit8,
        "9" => Digit9,
        "alt" | "alt_left" => AltLeft,
        "shift_right" => ShiftRight,
        "control_right" => ControlRight,
        "w" => KeyW,
        "a" => KeyA,
        "s" => KeyS,
        "d" => KeyD,
        "q" => KeyQ,
        "e" => KeyE,
        "r" => KeyR,
        "f" => KeyF,
        "p" => KeyP,
        "m" => KeyM,
        "v" => KeyV,
        "n" => KeyN,
        "c" => KeyC,
        "space" => Space,
        "shift" | "shift_left" => ShiftLeft,
        "control" | "control_left" => ControlLeft,
        "escape" => Escape,
        "enter" => Enter,
        "tab" => Tab,
        "backspace" => Backspace,
        "arrow_up" => ArrowUp,
        "arrow_down" => ArrowDown,
        "arrow_left" => ArrowLeft,
        "arrow_right" => ArrowRight,
        "f1" => F1,
        "f2" => F2,
        "f3" => F3,
        "f4" => F4,
        "f5" => F5,
        "f6" => F6,
        "f7" => F7,
        "f8" => F8,
        "f9" => F9,
        "f10" => F10,
        "f11" => F11,
        "f12" => F12,
        _ => return None,
    })
}

fn set_capture(world: &mut World, captured: bool) {
    if let Some(mut player) = world.get_resource_mut::<Player>() {
        player.captured = captured;
    }
    // Automation capture must not lock or hide the user's OS cursor.
    for mut cursor in world
        .query::<&mut bevy::window::CursorOptions>()
        .iter_mut(world)
    {
        cursor.grab_mode = bevy::window::CursorGrabMode::None;
        cursor.visible = true;
    }
}

fn pointer_event(
    world: &mut World,
    action: bevy::picking::pointer::PointerAction,
) -> Result<(), String> {
    use bevy::{
        camera::RenderTarget,
        picking::pointer::{Location, PointerId, PointerInput},
    };
    let mut query = world.query_filtered::<(Entity, &Window), With<bevy::window::PrimaryWindow>>();
    let (entity, window) = query.single(world).map_err(|e| e.to_string())?;
    let position = window.cursor_position().unwrap_or(Vec2::ZERO);
    let target = world
        .resource::<crate::presentation::CaptureTarget>()
        .0
        .clone()
        .map_or_else(
            || RenderTarget::Window(bevy::window::WindowRef::Entity(entity)),
            |image| RenderTarget::Image(image.into()),
        );
    let location = Location {
        target: target.normalize(Some(entity)).ok_or("no camera target")?,
        position,
    };
    world.write_message(PointerInput::new(PointerId::Mouse, location, action));
    Ok(())
}
fn pointer_button(world: &mut World, button: MouseButton, pressed: bool) -> Result<(), String> {
    use bevy::picking::pointer::{PointerAction, PointerButton};
    let button = match button {
        MouseButton::Left => PointerButton::Primary,
        MouseButton::Right => PointerButton::Secondary,
        _ => PointerButton::Middle,
    };
    pointer_event(
        world,
        if pressed {
            PointerAction::Press(button)
        } else {
            PointerAction::Release(button)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn harness() -> (App, TcpStream) {
        let console = Console::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let stream = TcpStream::connect(console.listener.local_addr().unwrap()).unwrap();
        stream.set_nonblocking(true).unwrap();
        let session = crate::session::test_session(true);
        session.ready();
        let drive = Drive::new(session.chassis_id.unwrap(), false);
        let mut app = App::new();
        app.insert_resource(console)
            .insert_resource(session)
            .insert_resource(drive)
            .insert_resource(Player::at(Vec3::ZERO, 0.0, 0.0))
            .insert_resource(crate::loading::Ready)
            .init_resource::<crate::hud::HudState>()
            .init_resource::<crate::presentation::CaptureTarget>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<AccumulatedMouseMotion>()
            .add_message::<AppExit>()
            .add_message::<bevy::picking::pointer::PointerInput>()
            .add_plugins(ConsolePlugin)
            .add_systems(
                Update,
                (
                    crate::controls::drive_chassis,
                    crate::session::advance_world,
                )
                    .chain(),
            );
        (app, stream)
    }

    fn response(app: &mut App, stream: &mut TcpStream) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut bytes = Vec::new();
        loop {
            app.update();
            let mut buffer = [0; 8192];
            match stream.read(&mut buffer) {
                Ok(0) => panic!("console disconnected"),
                Ok(n) => bytes.extend_from_slice(&buffer[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("{e}"),
            }
            if bytes.ends_with(b"\n") {
                return serde_json::from_slice(&bytes).unwrap();
            }
            assert!(Instant::now() < deadline, "console did not reply");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn request(app: &mut App, stream: &mut TcpStream, command: Value) -> Value {
        stream
            .write_all(format!("{}\n", json!({"id":42,"command":command})).as_bytes())
            .unwrap();
        let result = response(app, stream);
        assert_eq!(result["id"], 42);
        result
    }

    #[test]
    fn tcp_commands_confirm_input_and_step_then_release_on_disconnect() {
        let (mut app, mut stream) = harness();
        let result = request(
            &mut app,
            &mut stream,
            json!({"type":"key","key":"w","pressed":true}),
        );
        assert_eq!(result["ok"], true);
        // The fixture chassis faces +y while the camera faces +x, so W
        // commands body-right motion. The forward component is roundoff near
        // zero, whose sign can differ across platforms.
        let command = &result["result"]["chassis"]["command"];
        assert!(command["forward_m_s"].as_f64().unwrap().abs() < 1e-9);
        assert!((command["left_m_s"].as_f64().unwrap() + 2.0).abs() < 1e-9);
        let result = request(&mut app, &mut stream, json!({"type":"step","ticks":16}));
        assert_eq!(result["ok"], true);
        assert_eq!(result["result"]["tick"], 16);
        drop(stream);
        let deadline = Instant::now() + Duration::from_secs(2);
        while app.world().resource::<Console>().connection.is_some() {
            app.update();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            !app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(app.world().resource::<ConsoleInputs>().keys.is_empty());
        // A new controller can attach after disconnect.
        let address = app
            .world()
            .resource::<Console>()
            .listener
            .local_addr()
            .unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        stream.set_nonblocking(true).unwrap();
        let result = request(&mut app, &mut stream, json!({"type":"step","ticks":1}));
        assert_eq!(result["result"]["tick"], 17);
        assert_eq!(result["result"]["chassis"]["command"]["forward_m_s"], 0.0);
        assert_eq!(result["result"]["chassis"]["command"]["left_m_s"], 0.0);
    }

    #[test]
    fn bad_requests_are_errors_and_the_connection_remains_usable() {
        let (mut app, mut stream) = harness();
        // Deliberately fragment a request across frames.
        stream.write_all(br#"{"id":9,"#).unwrap();
        app.update();
        stream
            .write_all(b"\"command\":{\"type\":\"step\",\"ticks\":0}}\n")
            .unwrap();
        let result = response(&mut app, &mut stream);
        assert_eq!(result["id"], 9);
        assert_eq!(result["ok"], false);
        let bad = request(
            &mut app,
            &mut stream,
            json!({"type":"key","key":"unknown","pressed":true}),
        );
        assert_eq!(bad["ok"], false);
        let bad = request(&mut app, &mut stream, json!({"type":"state","typo":true}));
        assert_eq!(bad["ok"], false);
        let good = request(&mut app, &mut stream, json!({"type":"state"}));
        assert_eq!(good["ok"], true);
        assert_eq!(good["result"]["tick"], 0);
    }

    #[test]
    fn console_rejects_public_bind_and_cannot_place_another_chassis() {
        assert!(Console::bind("0.0.0.0:0".parse().unwrap()).is_err());
        let (mut app, mut stream) = harness();
        let result = request(
            &mut app,
            &mut stream,
            json!({
                "type":"world", "command":{"PlaceChassis":{"chassis":999,"position_m":[0,0,1],"yaw_deg":0}}
            }),
        );
        assert_eq!(result["ok"], false);
        assert!(result["error"].as_str().unwrap().contains("not yours"));
    }
}
