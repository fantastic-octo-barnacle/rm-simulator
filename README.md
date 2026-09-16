<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# rm-simulator

A first-person RoboMaster field simulator written in Rust with Bevy. It loads
the RMUC 2026 field from the extracted competition CAD, with both faces of the
rune, both rotating outposts, the bases and the tech cores placed where the CAD
puts them, and steps the rune and outpost rules on an explicit clock — 1 ms per
tick by default — and drives the CAD rune faces and outpost rotors. You drive a
four-wheel omni
chassis over the CAD terrain (plateaus, undulating roads, ramps, highlands,
tunnels) with a mouse-aimed gun, or fly a free camera with `--fly`. A referee
runs an RMUC 2026 match on the same clock, and a headless server can host the
world for connected windows.

Physics and motion, gameplay rules, Bevy rendering, the server and the
interactive application are separate Rust crates. Rune rules, outpost geometry,
the CAD scene handling, and the armor artwork masks were reused from the sibling
`../Vision/rm-vision-sim` repository; see `NOTICE.md`. The
[documentation index](docs/README.md) separates current guides from historical plans.

## Contents

- [Quick start](#quick-start)
- [Controls and options](#controls-and-options)
- [Title screen and LAN lobbies](#title-screen-and-lan-lobbies)
- [Crate layout and dependencies](#crate-layout-and-dependencies)
- [Field assets](#field-assets)
- [Running a server and joining](#running-a-server-and-joining)
- [Match rules and scoring](#match-rules-and-scoring)
- [Robot equipment](#robot-equipment)
- [Performance](#performance)
- [Distributing builds](#distributing-builds)
- [Development](#development)
- [License](#license)

## Quick start

Download a platform ZIP from GitHub Releases and extract it completely. Each ZIP
includes the app, headless server, native runtime libraries, and the field package.
Run `bin/rm-simulator` or `bin/rm-simulator.exe`.

With Nix on Apple Silicon macOS or Linux, use the pinned project environment:

```sh
direnv allow  # once, after reviewing .envrc and flake.nix
# Or enter explicitly without direnv:
nix develop
just server
```

The shell supplies CMake, Clang/libclang, pkg-config, Protobuf 21 and OpenSSL.
Rust remains managed by rustup and `rust-toolchain.toml`. Nix builds use
`target/nix/`, keeping objects linked against Homebrew out of this build.
Commit `flake.nix`, `flake.lock` and `.envrc`; `.direnv/` stays local. Update
native dependencies with `nix flake update`, then rebuild and test before
committing the new lockfile. No global Homebrew dependencies are needed in this shell.

Without Nix, install the Rust toolchain from `rust-toolchain.toml`. GameNetworkingSockets
also needs CMake, Clang/libclang, pkg-config, Protobuf and OpenSSL to build.
On Ubuntu, install `cmake libclang-dev pkg-config protobuf-compiler libprotobuf-dev libssl-dev`.
On macOS:

```sh
brew install cmake pkg-config protobuf@21 openssl@3
export CMAKE_PREFIX_PATH="$(brew --prefix protobuf@21):$(brew --prefix openssl@3)"
export PKG_CONFIG_PATH="$(brew --prefix protobuf@21)/lib/pkgconfig:$(brew --prefix openssl@3)/lib/pkgconfig"
export PATH="$(brew --prefix protobuf@21)/bin:$PATH"
```

Windows builds need the MSVC C++ tools, CMake, Git and LLVM/libclang. The native
crate builds Protobuf through vcpkg. Use a short checkout/build path to avoid
Windows path-length limits.

Then:

```sh
just run
just run --play --big-rune --outpost-speed-rad-s 3.0
```

## Controls and options

The essentials, for driving and for match control:

| Input | Action |
|---|---|
| Mouse | Look |
| Left button (held, once captured) | Fire from the barrel (driving) or just ahead of and below the eye (flying) |
| Right mouse (hold) | Auto Aim + Auto Fire; separately rebindable actions in Controls |
| W A S D | Drive forward/back and strafe left/right relative to the aim; the chassis heading follows the aim (driving) or move on the horizontal plane (flying) |
| Left Ctrl | Fast: 5 m/s drive command, 8 m/s flight |
| R | Toggle chassis spin (6 rad/s) while driving |
| V | Toggle first- and third-person view while driving |
| C | Cycle physics colliders: Off / Overlay / Only |
| F3 | Open / close debug panel: collider Off / Overlay / Only, visual wireframe, rendering statistics, remote motion buffering, reset own robot to spawn |
| Space / Left Shift | Move up / down (flying) |
| F5 | Start the match (or reset a finished one) (local world or referee) |
| F6 | Pause or resume the world clock (local world or referee) |
| F7 | Step the world one frame (16 ms, or the next tick boundary after it at a reduced rate) while paused (local world or referee) |
| F | Activate the rune for your team when it has an opportunity (local world or referee) |
| Tab (hold) | Show team robot status |
| P | Toggle settings; 1 toggles reticle, 2 toggles minimap, - / = adjusts mouse sensitivity |
| M | Toggle the large team map |
| F12 (hold) | Show controls |
| Escape | Close the current panel or open Pause; on the robot page, return to the page its choice came from; in Multiplayer, return to the main menu; otherwise open or cancel quit confirmation |

The complete command-line reference for both binaries, the weapon settings, the
HUD and interface behavior, the graphics presets and the full controls table are
in the [app options reference](docs/app-options.md). Settings, weapon preferences
and graphics choices are edited from **Settings** or the toolbar and saved in
`settings.json`.

## Title screen and LAN lobbies

A bare launch opens Single Player and Multiplayer choices with a player name.
Single Player starts a local practice field.
Multiplayer lists LAN lobbies on the left and creates a named, optionally
password-protected lobby on the right. Select a listing, enter its password if needed, and press Join lobby / address.
Every way in then opens the robot page: blue seats on the left, red on the
right, each offering the Hero (42 mm), Infantry 3 or Infantry 4 (17 mm) or a
spectating camera, with the referee seat below. Confirm with Start match or
Join lobby, or press Enter.
You can also enter a direct address. The page and lobby list scroll with the
mouse wheel, trackpad or scrollbar; narrow windows stack the two columns. Public is greyed out pending public connectivity support.
The firewall tip recommends allowing the app on private networks. Lobby names
and addresses are remembered, but passwords are never saved. The fields live in
`$XDG_CONFIG_HOME/rm-simulator/title.json` (`~/.config` by default,
`%APPDATA%` on Windows). Losing the
connection, a host failure or a bad address ends the match and returns to the
title screen with the reason; the
window stays open. The CAD package is verified once per process, so a second
join is quick. The title background is a darkened in-game screenshot without
the gameplay HUD. The field sits in a dark stadium, with visible wire fencing
aligned to its two-metre collision boundary at the edge of the grounded CAD
scenery. The slab apron lies outside the fence, with distant stadium seating
and walls beyond it.

### LAN lobbies

Create a lobby in Multiplayer, then click Refresh LAN on another computer on
that network. Select the lobby and press Join lobby / address. The creator runs the match and must keep it open. Discovery uses
UDP port 7792; gameplay uses UDP 7700 by default with GNS, or TCP with
`--transport tcp`. Allow the app through the firewall on private networks.
Guest Wi-Fi isolation and separate subnets can prevent discovery. Direct
connections remain available when broadcast discovery is blocked.

LAN discovery needs no directory server, internet access, router registration
or port forwarding. One advertised lobby can run per computer, since the LAN
responder owns a fixed port. Leaving the match closes its listing. Refresh
removes departed hosts from the displayed list. Passwords apply to pilots and
spectators, including direct connections. The current text widget displays
passwords visibly; they are never saved to `title.json`. TCP carries the game
handshake without encryption, so use a lobby-specific password.

Public hosting and joining from the menu are disabled. The directory code and
Docker setup are in [`services/lobby`](services/lobby/README.md), ready for a
later deployment. No directory is required or running for this feature.

## Crate layout and dependencies

| Crate | Responsibility |
|---|---|
| [`rm-simulator-gameplay`](crates/rm-simulator-gameplay/README.md) | Standalone match lifecycle and gameplay state, deterministic commands, rule coverage and unsupported-element observations. No physics or renderer; its live resource component is driven by the world referee. |
| [`rm-simulator-physics`](crates/rm-simulator-physics/README.md) | Reusable Rapier dynamics, chassis, projectiles, raw armor contacts, shared geometry and prescribed armor motion. No gameplay, Bevy, server or CAD-loader dependency. |
| [`rm-simulator-world`](crates/rm-simulator-world/README.md) | Complete `Field` facade: explicit ticks, activation, detection, damage, referee integration and restore. Coordinates the physics library and preserves existing public world imports. |
| [`rm-simulator-render`](crates/rm-simulator-render/README.md) | Bevy CAD scenery, lighting, rune and outpost light overlays, projectile spheres, chassis visuals, and pose/visibility/strike synchronization from caller-owned scene state. No world dependency. |
| [`rm-simulator-server`](crates/rm-simulator-server/README.md) | Bevy-free glue: CAD loading and checksums, collision triangles, field layout, the `Simulation` wrapper, a `Host` worker that owns simulation and command ordering, GNS UDP, TCP and in-process channel transports, the HTTP referee panel, and the headless binary. |
| [`rm-simulator-bench`](crates/rm-simulator-bench/README.md) | Fixed-camera CAD renderer benchmark, independent of physics, world, server and gameplay. |
| [`rm-simulator-app`](crates/rm-simulator-app/README.md) | The `rm-simulator` binary: window, chassis driving and gimbal camera (or fly camera), gun, HUD, world-to-scene adaptation, and the local or remote session that turns inputs into protocol commands. |

Use the physical library without a match or server:

```sh
cargo run -p rm-simulator-physics --example moving_armor --locked
```

An optional Bevy composition example draws moving armor and projectiles without
using `Field`, the referee or server APIs:

```sh
cargo run -p rm-simulator-app --example physical_scene --locked
```

The example advances 16 simulation milliseconds per rendered frame. Production
applications supply their own pacing. See [physics reuse](docs/physics-reuse.md)
and the [refactor contract](docs/architecture-refactor.md).

Coordinates: the world uses metres in forward/left/up (FLU) with wxyz quaternions.
The renderer converts to Bevy's right/up/back frame at the scene boundary.
Crate ownership, tick order, restore and ECS boundaries are documented in
[simulation architecture](docs/architecture-refactor.md).

## Field assets

The exported RMUC map is versioned in `field/` through Git LFS. After cloning, run
`git lfs install` and `git lfs pull --include='field/**'` to download it.
It is read at start-up from a field package directory. `--cad-assets` overrides discovery; otherwise the loader
tries `field/` beside an installed `bin/` directory, the build checkout's
`local-assets/field`, the checkout's LFS `field/`, then
`~/dev/RM/assets/rm2026-field`. The selected directory
must contain `manifest.json`, the `*.glb`
visuals and `*-collision.glb` proxies it lists, and `equipment/manifest.json`
with `base.glb` and `tech-core.glb`. Every file is checked against the
SHA-256 recorded in its manifest during loading.

The default package is built by the sibling `rm-map-tools` project from DJI's
RMUC 2026 V1.2.0 STEP and keeps that release's arena face colours; its manifest
records the slab top as `floor_top_source_z_m` so the simulator puts it at
height zero. The rune and the outposts come from the earlier V2.0.0 extraction,
while the bases and tech cores are exported from the V1.2.0 equipment package.
The older extraction at `~/dev/RM/assets/rm2026-extracted`
still loads with `--cad-assets`.

The V1.2.0 floor slab is flat at height zero and the terrain plates (the
centre-line plateaus at x ≈ ±6.3..8.5 m, the 起伏路段 undulating roads near the
side walls, ramps, the central highland, covered passages) stand on it. The
V2.0.0 slab is crowned instead: about 0.11 m below the reference height along the
centre line, falling to about 0.32 m below at the side walls, with height zero at
its small level pads. Invisible walls, 2 m high, surround the loaded floor bounds
to keep robots inside the arena. The CAD floor remains the driving surface;
there is no invisible catch plane beneath a loaded field.

The default field is enclosed by a dark stadium with neutral lighting. Equipment
paint follows team ownership:
red occupies +x and blue occupies -x. Destroying an outpost opens its team's base
shields. Friendly-fire rules are unchanged.

The arena's colours are the CAD's. The V1.2.0 STEP styles every arena face
(white for most surfaces, dark grey slab and plate tops, beige plate sides,
red and blue 1 mm marking sheets), and the package keeps them as glTF
materials. The V2.0.0 STEP styles every arena solid in the exporter's
unpainted cream and its markings are flush cream solids; the simulator
recolours only that yellow-tinted cream (floor dark, arena light grey), so
painted materials, white line markings included, are left alone.

Generate the simplified map from your field package with the sibling
`rm-map-tools` checkout. This is an offline tool, not a runtime dependency:

```sh
uv run --with numpy --with pillow python scripts/generate-minimap.py
```

Use `--cad-assets PATH` and `--map-tools PATH` for other locations. The generator
writes `minimap.png` and `minimap.json` beside the external field manifest.
The app checks the image and package hashes, preserves the map aspect ratio,
and uses the recorded FLU bounds for marker placement. Regenerate after changing
the package. Missing or stale artwork falls back to a position-only schematic.
The map is a simplified top-down projection; it does not show underpasses.

Package composition, deployment, native-texture patches, `static_assets`,
collision contracts, terrain, placement frames, arena colours and loading detail
are in [the field package guide](docs/field-package.md).

## Running a server and joining

The world can run headless:

```sh
just server --big-rune            # UDP on 0.0.0.0:7700, panel on http://127.0.0.1:7780
just run --connect 127.0.0.1:7700 --team blue --name alice
```

`rm-simulator-server` loads the CAD, builds the same field as the app, steps
it in real time and serves gameplay over Valve GameNetworkingSockets UDP and an
HTTP referee panel. Open UDP port 7700 for guests. Both executables retain
`--transport tcp` for the previous JSON-lines transport.

### Transports and roles

Every client sends `Hello` with its protocol version,
name, preferred team and role (`Pilot`, `Spectator` or `Referee`), then
commands (`Chassis`, `Fire`, `Referee`, `Pause`, `Step`), and receives `Welcome`, `Snapshot` (the full field snapshot, coalesced
to about 31 Hz), `Rejected`, `Pong`, `Notice` and `Roster` messages. Every
pilot gets its own chassis, spawned in the next free slot beside its team's
spawn point and removed when the client leaves; a chassis answers only to
the client it was assigned to, and a `Fire` must name it as the shooter.
The host derives its muzzle from the held turret aim and enforces its configured
caliber, speed and firing interval using simulation time. `Welcome` announces
that weapon configuration to every client. Spectators get no chassis and cannot
fire. The embedded free camera uses a separate `SpawnProjectile` command;
remote peers cannot submit it.

GNS sends commands and confirmation replies reliably in order. Periodic snapshots
are independently compressed and fragmented into small unreliable messages. Losing
a fragment drops that snapshot; the next complete snapshot needs no baseline.
Sequence numbers prevent late state from moving the client backwards. GNS handles
congestion control and encrypted transport. This standalone library needs no Steam
account or internet service; Steam lobbies and Steam Datagram Relay are not integrated.

The referee is a spectator on no team who alone may send the `Referee`,
`Pause` and `Step` commands; from anyone else they are rejected (the host's
own HTTP panel is not restricted). The roster (name, team, role and chassis
of every client) is sent whenever it changes; hold Tab to see team players.
`--no-chassis` on the server gives pilots no chassis. The window's
`--listen` hosts the same server from the app's own world so a second window
can `--connect` to it, and `--http` opens the panel next to it. The panel is
a single page that polls `/api/state`, posts `/api/command` and
`/api/referee`, and shows the clock, teams, robots, outposts and event log.

Standalone play starts an embedded server and a private in-process channel
connection, without a gameplay socket or JSON serialization. It works offline without a separate server process or Steam. Its clock runs on a worker, held until scenery finishes loading,
and preserves `--start-paused`. The local owner retains match controls and
free-camera firing; these privileges cannot be requested in a network hello.
`--listen` exposes the same world to other players. Its local player also uses
the channel connection; remote players retain the selected network transport.

### Host authority

The hosted simulation and roster belong to one worker. Gameplay transports and HTTP
handlers submit bounded mailbox requests; joins, departures, commands and clock
advances execute in the worker's order. A confirmation queues its resulting
snapshot before its Pong, and socket writers encode snapshots after capture.
Each peer's bounded outbox replaces unsent periodic snapshots before enqueueing.
Confirmation snapshots and Pongs remain ordered and cannot be coalesced away;
a peer that exhausts the reliable backlog is disconnected.
Command arrival order still depends on the transports; the host establishes a
single application order, not a reproducible ordering of simultaneous arrivals.

For Rust callers, `Server::bind` and `bind_suspended` take ownership of a
`Simulation` and retain TCP. `bind_udp` and `bind_udp_suspended` select GNS.
Use `server.handle()` for operator commands and state queries, and
pass that handle to `HttpServer::bind`. `Host::new` also supports a host without
TCP. Request handling starts immediately; `spawn_clock` or `run_clock` enables
real-time pacing. Operator command receipts contain their application sequence
and pre-command tick. State queries wait for earlier mailbox requests; after
shutdown they return the final captured state. Debug capture requests keep the
app's frame thread independent of the simulation worker.

Host handles own their listeners and workers. Shutdown closes active connections,
including incomplete handshakes and HTTP requests, and waits for workers to finish.
Collider drawing expands client-loaded CAD shapes once on a worker and follows
the rendered scene transforms each frame. It makes no host debug requests.

### Snapshots and prediction

Local snapshots target a
4 ms interval, while remote snapshots use a 32 ms interval; physics load
can reduce either rate. Clients reconstruct outpost and rune motion between
snapshots using an estimated host simulation clock. Remote chassis use automatic interpolation buffering, starting at 64 ms and
adapting within 32–250 ms, with at most 100 ms of extrapolation on underrun.
F3 offers a manual 0–250 ms delay, reset to automatic defaults, and effective
delay, view-age and underrun readouts. These settings last for the session and
affect remote chassis only. Local
movement, gimbal and projectile prediction are enabled by default for matching
field packages. Physics persists between frames and continues for up to one second
without a checkpoint. A new checkpoint restores authoritative state and replays
only control transitions whose scheduled ticks remain unprocessed. Missing past
inputs are discarded. Small camera corrections blend over about 100 ms where
geometry allows; large corrections snap. Completed predictions remain usable when
a newer ordinary snapshot arrives, provided their robot generation is current
and neither predicted time nor the correction baseline moves backward.

Small toasts distinguish measured high latency, interrupted updates and a confirmed
disconnect. Silence alone is labeled "Connection interrupted". After one second
without a checkpoint, predicted movement and new shots stop; camera look remains
available. A closed transport keeps the window open with "Disconnected". Joining
again creates a new session and robot; it does not replay old inputs.

`--no-prediction` selects host-driven movement, aim and projectile presentation for
comparison. Firing always uses independent server physics. Local offline play
uses the same ownership and command path. Severe packet loss still limits useful
combat even when local controls remain responsive.

UDP player checkpoints use compact projectile position/velocity vectors with
identity, caliber and launch time. Vectors use 32-bit floats; server physics stays
64-bit. The server retains projectile spin, while client ball prediction starts
with zero spin at each checkpoint and may correct bounced trajectories later.
Clients reconstruct rune/outpost armor poses and omit wheel-contact and failed-hit
diagnostics. HP, registered hits, command/life state and chassis reconciliation
values are preserved. HTTP referee diagnostics keep full precision and detail;
TCP sends the same compact checkpoints, each independent of the last.

Remote pose history holds at most 32 samples. It interpolates translation,
shortest-path rotation and wheel motion, holds the nearest known pose on underrun,
and discards stale history on placement, defeat, despawn and pause changes.
Screenshots bypass interpolation, keeping confirmation-dependent captures exact.

Known mechanism motion advances locally during up to one second without a newly
received checkpoint. Pauses and screenshots use exact authoritative poses. Clock probes
run at most once per second with one outstanding; their replies never confirm
commands. The estimate assumes roughly symmetric latency and is intended for
low-latency links. The acknowledged-baseline delta encoder is driven only by the
UDP/GNS per-peer codec; TCP carries independent compact checkpoints and never
deltas. The UDP encoder rotates its acknowledged baselines every 32 encoded
frames. Result snapshots
before Pong are always full. Readers reconstruct deltas before inbox coalescing;
a missing/invalid baseline closes the connection rather than exposing partial state.

The snapshot remains authoritative for rules and chassis physics. A
chassis command carries the pilot's gun aim (heading and elevation in the
world) along with the body velocity, and the snapshot reports each turret's
pose, so other players' guns are drawn where their pilots point them; your
own turret predicts the same motor response locally, ahead of the round trip.
The mouse sets an aim target; camera, barrel and muzzle follow actual motor aim.
The assumed gimbal response is 25 ms with 12 rad/s speed and 240 rad/s²
acceleration limits, evaluated on the same clock as the field.

### Input cadence

The server schedules future shots at their intended movement tick, applying controls
before deriving the muzzle with the shot's exact aim, then advancing physics.
Eligible late shots launch from the current authoritative position. Each pilot
has at most 32 queued intents. A scheduling receipt stops retries but does not
confirm a bullet; terminal results carry the actual execution tick. Cooldown
rejections are final for that shot ID, and pause, defeat or a new placement cancels
queued shots. The client predicts legal firing cadence and the same launch tick. Local projectiles and
contacts are provisional. A visible local hit may become a server miss, or the
server may register a contact that the client did not predict. HP, confirmed hit
flashes, ammunition and match results remain authoritative. Shot IDs prevent
retries from duplicating projectiles or damage. Expired shots are rejected instead
of creating a burst when a connection recovers. No per-frame shooter-view evidence
is sent by normal clients.

Input lead adapts to host-observed arrival margins within 32–150 ms. UDP repeats
the newest four samples plus useful older movement transitions, up to twelve
samples in one packet. The lead always adapts to host feedback; there is no
fixed or shortened-history override. F3 provides automatic or manual remote
interpolation buffering; it does not alter local input lead. Automatic buffering
releases excess delay at up to 50 ms per second after the two-second jitter
window improves.

RMI3 input batches share identity fields and encode exact value changes without
reducing input redundancy. Placement revisions prevent old input from
crossing robot lives. `Ping` remains a command barrier: the server queues a
resulting snapshot before its `Pong`. Screenshot capture waits for confirmation of
queued discrete commands and for the scene to apply that state.

Client socket reads and writes run on background workers. Outgoing commands
keep their order in a queue of up to 256 pending commands. The client keeps
only the latest unread snapshot and roster, plus ordered notices and
rejections capped at 256 messages or 1 MiB. Exceeding either queue limit
disconnects with an error shown in the HUD. Successfully queueing a command
does not acknowledge its execution by the host.

### Protocol versions

| Protocol | Behavior |
|---|---|
| 27 | Sends authoritative armor contacts reliably, independently of world snapshots. Recent contacts recovered from snapshots also display once; repeated snapshots cannot restart their flash. Feedback more than 250 ms old is discarded instead of replayed after a long interruption. Damage and HP remain host-owned. Both host and clients must use matching protocol builds. |
| 29 | Combines referenced owner configurations with lossless RMI3 input batches; experimental version 28 builds carried only one of the two changes. |
| 32 | Defaults periodic UDP world checkpoints to bitpacked fine fixed point with a separately trained, embedded ZSTD dictionary. Both peers must run this build. Owner anchors and full confirmations retain their existing precision. |
| 33 | Removes the `ShotFinished` message, which no host ever produced: a shot's end was only ever reported as a `ShotResult`. |
| 34 | Makes packed checkpoints the only periodic snapshot encoding and ZSTD the only wire codec, removing the JSON checkpoint path and the DEFLATE codec. |

The current protocol version is 34, defined by `PROTOCOL_VERSION` in
`crates/rm-simulator-server/src/protocol.rs`. GNS sends redundant controls
and retried shot intents unreliably; scheduling receipts and terminal shot results
remain reliable. There is no shooter-view fire path, ordered TCP snapshot delta
chain or input-acknowledgement stream. UDP deltas use acknowledged baselines;
TCP checkpoints remain independent. Server and client
must use matching protocol versions.

### Compression and environment variables

The UDP transport sends independent owner corrections and uses acknowledged
baselines for world deltas. Owner and input numeric state retains f64
precision, while shot intents use bounded compression. RMO4 owner anchors reference
a configuration delivered reliably and explicitly acknowledged by the client.
World and owner publication cadence is unchanged.

Application pacing defaults
to 64 KiB/s upstream and 512 KiB/s downstream per peer with the default LAN
profile. Native congestion control remains active. These application budgets
exclude native retransmissions and framing; proxy rates remain the wire-capacity
reference.

| Variable | Effect |
|---|---|
| `RM_NET_UP_KIB_S` / `RM_NET_DOWN_KIB_S` | Integer values from 4 to 2048 that override the application pacing for a trial. Invalid overrides use the defaults. |

Measure snapshot bytes and codec CPU with
`cargo run -p rm-simulator-server --example network_bandwidth --locked`.
Periodic checkpoints are packed fine fixed point compressed with the embedded
checkpoint dictionary at ZSTD level 3; every other wire message is plain ZSTD at
level 3. There is no codec selector and no dictionaryless or DEFLATE path.

### Diagnostics

Set `--network-stats off|compact|detailed` or use the Network stats button in P/F3
to show local diagnostics. Native rates and ping are GNS estimates; loss stays
unavailable when the wrapper cannot identify packet loss. The display adds no
server requests. The console reports host downstream queue bytes, age and service
for control, owner and world traffic.

`GET /api/fire-records` on the host HTTP endpoint returns the latest 256 accepted
pilot shots, oldest first. Records include client-relative input time, estimated
simulation time, observed chassis/muzzle poses, authoritative muzzle pose,
acceptance time and projectile ID. These diagnostics never backdate shots or
change hit detection. The journal is in memory; export it before host shutdown
if needed. Rejected attempts do not create shot records.

Run `just network-test` to verify the harness without building Rust. With matching
binaries built, `just network-trial scripts/network-scenarios/smoke-single.json
--output /tmp/rm-network-smoke` launches an isolated bandwidth/loss/latency trial
and writes reports. The output directory must be new. A scenario's `expectations`
block asserts named thresholds on the summary, `--baseline PREVIOUS/summary.json`
fails regressions beyond a relative tolerance, and the run reports `passed`,
`failed` or `completed` with a matching exit code.

The [bandwidth experiment record](docs/bandwidth-experiments.md) retains the
isolated trials behind the current delivery contracts, including the measured
savings and the outstanding bandwidth target. Open networking issues and
measurement gaps are tracked in [known issues](KNOWN_ISSUES.md).

## Match rules and scoring

Projectiles follow RMUC 2026 rules: 17 mm balls (16.8 mm, 3.2 g) and 42 mm balls
(42.5 mm, 44.5 g) fly under gravity and quadratic drag, bounce off the floor,
the field CAD and the armor housings, and expire after four seconds. Outpost
armor registers strikes on its 101 × 94 mm effective detection area (Figure
5-16, most of the plate face but not the bezel) above 12 m/s (17 mm) or 10 m/s
(42 mm) normal speed, at most once per 50 ms (17 mm) or 200 ms (42 mm) per
module, and loses 20 or 200 HP from 1500 (×1.5 in the 10 mm centre square); the
rotor stops when destroyed. Chassis armor detects the same way and costs its
robot 10 or 100 HP (Table 5-2).
Rune targets register only 17 mm strikes above 12 m/s inside the 300 mm
effective disk. A struck module's lights turn grey for 50 ms after confirmed feedback arrives
(`--hit-flash-ms`). Bullet trails and contact-point markers are not displayed.

### Referee and match clock

By default the field carries a referee that runs a match under the RMUC 2026
rules on the same default 1 ms clock: a 5 s countdown, a 7 min round, and two teams,
red and blue. The CAD paints its red markings on the +x half of the field
and its blue markings on the −x half, so red owns the outpost standing at
x > 0 and the rune face that looks towards +x, and blue the other pair
(section 4.3.2.2: one side of the rune is red's, the other blue's). Rune
targets and outpost light bars glow in their owner's colour. Each team gets a small rune
opportunity at 0:00 and 1:30 and a big rune opportunity at 3:00, 4:15 and
5:30 (sections 5.5.2 and 6.6); `F` (or a panel command) spends one and puts
the rune into Activating for that team, which expires after 20 s. Until
3:00 the rune is the Small Rune, afterwards the Big Rune. A small
activation grants 25 % defense for 45 s (section 5.5.2); a big activation is
scored from the average ring of the recorded hits (5 to 10 arms lit), using
the lit-arm count to pick the buff (Tables 5-16 and 5-17).
grants the defense, attack and cooling buff of Tables 5-16 and 5-17 (only the defense
buff has an effect here, on the team's bases, outposts and robots; attack and cooling are
reported). After one big activation the rune only detects rings 4 to 10,
after two rings 7 to 10 (Figure 5-18; rings are 15 mm wide, ring 10 the
centre). The referee keeps a robot per chassis (HP, defeat, revive; a
defeated robot stands still and cannot fire until revived) and can
be driven from the HTTP panel: start, finish or reset the match, skip to a
time (buffs and activation windows age with the round clock), grant or
activate rune opportunities, damage or revive robots. The
rulebook does not say how an activated rune looks beyond its arms being lit;
the three 2 Hz blinks are this simulator's choice (`--rune-flash-hz 0`
disables them, `--rune-flashes` changes the count).
Without a referee (`--no-referee`) the rune is active from the start and
restarts itself, as before.
The separate `rm-simulator-gameplay` crate provides a deterministic, headless
match engine and an inventory of supported and unsupported gameplay rules.
Run `just gameplay-test` for rule tests or `just gameplay-demo` for a complete
BO3 scenario. See [the gameplay guide](docs/gameplay.md) for its command API,
rule coverage and manual ambiguities. The live resource component now tracks
projectiles and economy through the existing app/server referee clock. Its full
standalone match engine remains separate.

`docs/referee-rules.md` digests the rulebook clauses the referee follows,
with their section and table numbers.

### Referee panel controls

The referee panel has per-team Open/Close controls for the dart door and base
shields. The same overrides accept JSON at `POST /api/referee`:

```json
{"SetDartDoorOpen":{"team":"Red","open":false}}
{"SetBaseOpen":{"team":"Blue","open":true}}
```

Send each object as a separate request. Both `Red` and `Blue` accept either
boolean. These controls move the visual and collision triangles immediately
between the package's joint endpoints. Travel is illustrative in rm-map-tools,
not a calibrated actuator stroke. They require a package with the corresponding
semantic joints; older packages retain their fixed geometry. Match start/reset
closes the base shields and opens the dart doors, matching the exported rest pose.
Destroying an outpost opens its base shields. Base damage and training controls
are described in the training section below.
The base's upper dart target sweeps along its rail every four seconds, using
simulation time and moving its collision mesh with it. The period is an
illustrative app setting. It requires the `base.dart_target.slide` asset binding;
see [the reproducible asset build](docs/semantic-assets.md).

Destroyed outposts stop rotating and use disabled armor: light bars are
fully off while the printed pattern stays white, and further contacts cannot score or trigger flashes. The housing
still collides. Restoring HP enables the armor again.

The HTTP panel also edits live ammo allowances and shot counts by caliber,
team gold, automatic income and referee resupply prices. Ammo enforcement is
off by default; enable it in the panel to reject firing with no allowance.
Starting allowances default to zero and apply when a match starts or resets,
and to newly joined pilots. Shot tracking and income operate only while
Running. Free-camera shots have separate counts and do not consume robot ammo.
The competitor HUD shows ammunition allowance and team gold.

Equipment overrides set outpost HP, rune opportunities and timed rune buffs.
Zero HP destroys an outpost; restoring HP resumes its configured rotation.
Start/reset restores all outposts to 1500 HP. Only rune defense affects damage;
attack and cooling remain reported values. Base armor hits now update shared
base HP and shield; the panel can edit both.
All edits use referee commands, so network pilots and spectators cannot apply
them. The HTTP panel retains the host's existing referee authority.

### Base armor and training bots

In singleplayer, Escape opens buttons to **Spawn spinning enemy** and **Remove
bots**. Bots occupy their team's free spawn slots and use ordinary chassis
physics, armor and HP. They spin at 3 rad/s and do not fire. The referee web
panel supports either team, a signed spin speed from -20 to 20 rad/s, individual
removal and clearing all bots. Up to 32 bots may exist; remote pilots cannot
manage them. Pause freezes their motion.

Each base has three lower scoring plates, three upper plates, and a moving dart
plate. Their team-colored lights flash on registered hits and dim on destruction.
Base HP and shield appear beside each team's outpost HP and in the referee panel,
which also supports setting HP/shield. Bases start with 5,000 HP and a separate
150-point shield, spent first. Starting or resetting a match restores both.
During a match, a surviving defending outpost prevents base damage. Idle practice
allows damage; use the referee's **Open base** control to expose the lower plates.
Destroying a base during Running ends the match.

The six ordinary plates follow Table 5-2: 17 mm causes 5 damage to the upper
front plate and 20 to the other five; 42 mm causes 200. The central 10 mm square
has a 150% multiplier. These values were checked against the local V2.2.0 manual;
this does not migrate unrelated rules. As a training override, the dart plate
also accepts ordinary projectiles at 20/200 damage, without a centre bonus.
This does not implement dart launch detection or the rulebook's dart target modes.

Scoring poses are fitted at startup from the verified rm-map-tools package,
including its reconstructed lower armor. No field meshes are modified. Older
packages without the reconstruction retain scenery-only bases. Reproduce the fit
and exercise all fourteen scoring faces against the CAD collision geometry with:

```sh
cargo run -p rm-simulator-server --example base_targets -- local-assets/field --check
```

The check fires close to each exposed face; normal gameplay still respects rail,
shield and other scenery occlusion. The dart fit mirrors the CAD's single red
light bar about the carriage centre to recover the paired optical plane.

### Training shortcuts and ammunition

Defeated pilots choose **Respawn here - restore HP** in the defeat menu.
The button restores full HP in place. Position, identity,
ammunition and team gold are retained. This is a training shortcut,
without a respawn timer, cost or invulnerability. The F3 **Reset robot to spawn**
button also restores the original upright spawn pose and stops the robot, while
keeping its identity, ammunition and team gold. O buys one 17 mm round and I buys
one 42 mm round during Running, using the existing resource policy and team gold.
Prices default to 1 and 10 gold and remain editable in the referee panel.

### Ground-truth aim assist

Hold right mouse while driving to acquire an enemy robot, enemy outpost, enemy base, or your
team's active rune near the crosshair. The assist predicts linear movement and
rotation from snapshots, leads a scoring face, and compensates for gravity and
projectile drag. Both small and big rune motion use their transmitted phase and
motion model. Rune targeting requires 17 mm ammunition.

Controls contains separate **Hold auto aim** and **Hold auto fire** actions.
They may share a binding and both default to right mouse. Clear Auto Fire for
assisted aiming with manual left-click shooting, or assign different buttons.
Auto Fire by itself checks your current barrel alignment without steering it.
The HUD reports searching, aim only, waiting, or firing.
Rune auto-fire is limited to two shots per second and waits for flight time plus
confirmation before another shot. An unchanged blade gets a slower retry after
at least 1.2 seconds; releasing and repressing the button does not bypass this.

Automatic shots wait for the actual motor pose to align with a predicted scoring
area, sufficient normal impact speed, and a clear shot path. The host still
checks fire cadence, ammo, heat, and match rules through the normal fire command.
Releasing the actions, opening a menu, defeat, or stale observations stop the
assist. Manual fire remains independent.

Auto-aim runs locally. Acquisition follows displayed robot poses, while the impact
solution uses the latest timestamped motion. Auto-fire requires an observation
no more than 150 ms old at intended execution; tracking stops at 300 ms. Its HUD
reports stale observations, motor alignment, blocked paths, weapon cadence and
rune confirmation. Every shot samples current controls even between normal
16 ms input refreshes.

This is a simple game-data aid, inspired by Vision2027's selection, prediction,
ballistics, and fire-control stages. It uses exact snapshot velocities instead
of image detection or an EKF. Acquisition prioritizes targets near the crosshair, falling back to the closest
visible enemy robot within 40 m (including outside the view cone);
constant-velocity robot prediction cannot anticipate collisions or a change of
input. Scenery rays and conservative bounds for intervening robots can withhold
shots near cover. No Vision2027 dependency or copied implementation is required.

## Robot equipment

`just run --robot hero --third-person` drives the mecanum Hero; omit `--robot`
for the omni Infantry 3, or pick it on the robot page of the title screen. Both carry approximate AM02 armor, LI01 HP lights,
FI02 underbody RFID hardware, VT03 camera and a muzzle speed monitor. Hero uses
an SM11-sized housing; Infantry uses SM01. HP segments and defeat state follow
the referee. FI02 detection and speed-monitor LED sequences are not simulated.

Every pilot names its robot when it joins, and the host builds that chassis
and fixes its caliber: the Hero fires 42 mm, the Infantry 3 and Infantry 4
fire 17 mm. Muzzle speed and firing rate defaults come from the host. The
robot does not change HP, heat or power policies, and two pilots may pick the
same robot. The visible equipment is decorative;
armor scoring keeps its existing dimensions. [Reference measurements and
limitations](docs/robot-equipment.md) distinguish modeled details from assumptions.

Each chassis is a generic 15 kg infantry-sized box (520 × 520 × 100 mm) on
four 153 mm omni wheels in an X layout, with a 120 mm turret cube 150 mm
above the body (its top is about 0.42 m above the ground), four small armor
modules on its sides (the outpost's 128 × 113 mm module, leaning back 15°,
scoring on the 101 × 94 mm area of Figure 5-16), 30 mm of unloaded
spring/damper extension with progressive bump stops beyond 40 mm compression, motor-limited
drive (40 N stall, 3.8 m/s no-load wheel speed, so about 5.4 m/s
straight-line top speed) and Coulomb-limited ideal omni tyres. These figures
are assumptions, not rulebook values. A shared 120 W infantry or 160 W hero
drivetrain budget includes mechanical work and torque-dependent motor loss.
Climbing and rotating compete for that budget and available tyre grip.
Robot contacts use low restitution; projectiles touching or leaving the arena
perimeter are absorbed while the boundary still contains robots.
Wheels are ray casts against the ground
mesh, so the chassis climbs the plateau ramps, feels the undulating road
through its wheels, takes the ramps and drops into the
lower floor sections; the body and the turret collide with walls and roofs,
so a covered passage lower than the turret top stops the chassis. The HUD
shows its speed and follow/spin mode. The gun pivot is stabilised above the chassis, so aiming does not
follow the body's pitch and roll. Projectiles that strike an armor module
above the detection speed damage the robot (10 HP per 17 mm, 100 HP per
42 mm) and light it grey for the flash; the module's light bars go dark when
the robot is defeated. Strikes elsewhere on the chassis bounce off without
scoring. The visuals show the omni wheels with their rollers, a two-axis
gimbal bolted to the body (a turntable and fork that turn about the body's
up axis, a cradle between the arms that pitches with the barrel, feeder and
camera; the two angles are solved so the barrel points along the stabilised
aim even on a tilted chassis) and the armor light bars in the team colour.

Review both models without loading field CAD:

```sh
cargo run -p rm-simulator-render --example robots
cargo run -p rm-simulator-render --example robots -- /tmp/robots.png healthy
cargo run -p rm-simulator-render --example robots -- /tmp/robots-hit.png hit
cargo run -p rm-simulator-render --example robots -- /tmp/robots-rear.png healthy rear
cargo run -p rm-simulator-render --example robots -- /tmp/robots-defeated.png defeated
```

The comparison stages Hero HP at 60%. `hit` freezes a front armor flash;
`defeated` switches module LEDs off. These are static visual fixtures.

### Armor artwork

Chassis and outpost armor use the shared [SVG-derived sprite atlas](assets/armor-atlas/README.md).
It includes numbers 1–5, guard, outpost and base identifiers with small/large
variants. Hero uses 1, Infantry 3 uses 3, Infantry 4 uses 4, and outpost uses O.
Textures preserve the source aspect ratio and face outward. Patterns are passive
white printing; only the light bars stop emitting with disabled armor and remain
white plastic. The external base package uses the base sprite on its moving dart
plate, three upper armor modules and three lower modules exposed by opening the
shields.

## Performance

The recorded standard-detail build has 497,886 placed visual triangles and
312,749 collision triangles including mechanisms. The 100k collision target remains
unmet. Rebuilding, validation and rollback are described in
[reproducible field detail](docs/field-detail.md). Runtime level of detail switching
and mesh chunking are not part of this pass; GPU occlusion culling is an optional
Graphics setting and remains off by default pending measurements.

Use [the loaded-match CPU probe](docs/performance.md) to measure median and tail
costs with several matches running at once. Test client frame times separately
on the target GPU; a server probe does not establish rendering frame rate.
Engine microbenchmarks and their limits are described in
[performance checks](docs/performance.md).

The separate `rm-simulator-bench` executable loads only visual field assets and
measures rendering with fixed cameras, GPU timestamps and configurable JSON/raw
output. Use `scripts/benchmark-render.py` for repeated settings sweeps. See
[the benchmark guide](docs/render-benchmark.md) for build commands, Vulkan setup,
Metal CPU-only checks, cases and output definitions.

## Distributing builds

Windows release ZIPs include `Join Red.cmd` and `Join Blue.cmd`. Extract the
whole ZIP, double-click the desired team script, and enter the server IP or
hostname. Port 7700 is the default. Both scripts remember the last address in
`server.txt` beside them; press Enter to reuse it. Player names combine the Windows computer
name with a new random suffix for each launch. The scripts resolve assets from
the extracted folder, so no working-directory setup is needed.

Local play in a distributed Windows package uses `Play Local.cmd`. It launches
`bin/rm-simulator.exe --cad-assets field` without `--connect`. Keep the complete
`field/` directory beside `bin/`; the executable also discovers that layout when
launched directly. The embedded host starts automatically.

GameNetworkingSockets itself is linked into the executable. Release packagers must
also include required platform runtime libraries. Windows uses the MSVC runtime;
macOS builds link Protobuf and OpenSSL dynamically, so bundle their dylibs and fix
their install names before distributing. Check the final package on a machine
without Rust, Homebrew or build tools. A Windows package has not been validated
by the macOS test run.

## Development

Install the pinned tools once, then run the full check:

```sh
cargo install prek --version 0.4.14 --locked
cargo install cargo-deny --version 0.20.2 --locked
prek install
just verify
```

`just verify` runs the hooks, formatting, check, Clippy with denied warnings,
tests, the module dependency check, the MPL-2.0 notice check, and `cargo deny`.
The [development guide](docs/development.md) documents the full target list and
where the tests live.

For automation, launch with `just run --console --window-mode headless`, then send
runtime spawn, camera, input and screenshot commands through the
[app console](docs/console.md). `normal` and `unfocused` modes also support it.

Build, CI and release workflow is in [CI and releases](docs/releases.md):
lightweight PR checks, manual full validation, branch protection setup, and the
stable/alpha/beta/RC release workflow with dry runs. Download and platform notes
for a published build are in [release notes](docs/release-notes.md). An optional
`steam` Cargo feature initializes the Steam client and pumps callbacks.
Ordinary builds remain standalone. The [Steam setup guide](docs/steam.md) explains
App IDs, matched SDK/runtime staging and the local SDK's version mismatch.
Steam Input, lobbies and portable distribution packaging remain future work.

## License

`rm-simulator` is dual-licensed under your choice of either the
[MIT License](LICENSE-MIT) or the [Apache License, Version 2.0](LICENSE-APACHE),
at your option. Every source file carries its SPDX identifier and the copyright
line `Copyright (c) 2026 hxyulin <hxyulin@proton.me>`.

Third-party material keeps its own terms. In particular the competitor UI pulls
in five unmodified MPL-2.0 components through Bevy Flair; their versions,
sources and the MPL-2.0 source offer are recorded in [NOTICE.md](NOTICE.md) with
the full license text at [LICENSES/MPL-2.0.txt](LICENSES/MPL-2.0.txt). Run
`just mpl` to check that notice against the resolved dependency graph.

Networking traces can be recorded with `RM_NET_TRACE_DIR=/tmp/rm-network-traces`.
The detailed network overlay reports recording status; console `state` includes
message counters, encoding costs and trace status. See
[network tracing](docs/network-tracing.md) for byte definitions, bounded recording,
and the summary command. Keep traces outside Git.
