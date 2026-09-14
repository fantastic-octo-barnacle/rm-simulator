<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# rm-simulator

A first-person RoboMaster field simulator. It renders the RMUC 2026 field
from the extracted competition CAD, with both faces of the rune (buff), both
rotating outposts, the bases and the tech cores placed where the CAD puts
them. The rune and outpost rules step on an explicit 1 ms clock and drive the
CAD rune faces and outpost rotors. You drive a four-wheel omni chassis over
the CAD terrain (plateaus, undulating roads, ramps, highlands, tunnels) with a mouse-aimed
gun, or fly a free camera with `--fly`. A referee runs an RMUC 2026 match on
the same clock, and the world can be hosted by a headless server that the
window connects to. Physics and motion, gameplay rules, Bevy rendering, the
server and the interactive application are separate Rust crates. The
[documentation index](docs/README.md) separates current guides from historical plans.

Rune rules, outpost geometry, the CAD scene handling, and the armor artwork
masks were reused from the sibling `../Vision/rm-vision-sim` repository. See
`NOTICE.md`.

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

Text and paint can be exported as native glTF texture patches. Their PNGs,
UVs and alpha-mask materials are embedded in the GLBs; no texture sidecar loader
is needed. Textured CAD materials retain their exported appearance. Package composition and
previous native-texture measurements are documented in [semantic assets](docs/semantic-assets.md).
The later standard-detail build is recorded in [field detail](docs/field-detail.md);
use loader counts and manifest hashes when comparing a different installed package.

The simulator supports rm-map-tools semantic packages: exported joint frames,
stable IDs and fixed geometry drive animation and collision selection. See
[Semantic assets](docs/semantic-assets.md) for package composition, running this
version, geometry counts and remaining fitted detector data.

The default package is built by the sibling `rm-map-tools` project
(`python/export_field_package.py`) from DJI's RMUC2026 V1.2.0 STEP, the
release that still carries the arena's face colours. The floor slab and the
385 placed arena solids are tessellated part by part with their STEP colours
(dark grey slab top, dark plate tops, beige plate sides, white lettering, red
and blue markings), one glTF node per solid, and the manifest records the
slab top as `floor_top_source_z_m` so the simulator puts it at height zero.
The rune, outposts, bases and tech cores are carried over from the earlier
V2.0.0 extraction (`~/dev/RM/assets/rm2026-extracted`); both releases share
the arena frame, and the equipment placements land on the V1.2.0 plates. The
older extraction still loads with `--cad-assets`; its manifest has no slab
height, so its crowned slab is referenced to the level pads near
(±12.8, ±3.0) m.

To assemble the complete simulator package from the field and element exports,
run the standard-library deployment utility in `rm-map-tools`:

```sh
python3 ../rm-map-tools/python/deploy_field.py --replace
```

Its defaults read `~/dev/RM/assets/rm2026-field` and
`~/dev/RM/assets/rm2026-field-elements`, verify their checksums and matching
arena, and install at `~/dev/RM/assets/rm2026-field`. It builds in a temporary
directory first and retains the previous installation as a dated backup.
Use `--field`, `--elements` and `--out` for other locations. No CAD import or
re-tessellation is needed. Restart the app and server after deployment.

The package's `static_assets` list adds the centre platform, both dart
stations, both resource-zone structures and both outpost footings, with
source colours and triangle-mesh collisions. The existing rune and outpost
assets retain their animation hierarchy. Older packages without this list
still load.

Static collision loads each asset's declared, checksummed collision geometry,
falling back to visual triangles for packages without a supported collision
contract. Reproducible approximate meshes are permitted to reduce CPU and memory
cost, provided ramps, clearance, traversable openings and scoring behavior are
verified. Rune and outpost moving targets remain rule-driven bodies. CAD source
materials and semantic joints remain intact. A splash screen shows stage progress and the current
loading activity. Verification, terrain loading, physics construction and
connection setup run in the background. Visual CAD instances load across
frames before local gameplay starts. The bar measures stages and completed
instances, not elapsed time. Press `Esc` to cancel a join and return to the
title screen. GPU uploads
and individual scene instantiations can still briefly stall the window. The
startup log reports triangle counts; the F3 debug panel shows the geometry held by the physics.
Press C to cycle Off / Overlay / Only. The green wireframe is built once from
client-loaded, verified collision assets and reused. It makes no debug requests
to the server. Unknown or mismatched packages remain unavailable.
Amber chassis, armor, projectile and articulated scenery outlines use the client's
predicted and interpolated content. Moving CAD wire meshes are built once in the background; all collider transforms
then follow the same scene poses as the visuals every frame. No physics world is
created and gameplay poses are never replaced. This is a client inspection view,
not a capture of current server physics.
Scenery stays visible until the fixed wireframe is ready.
The panel offers Off / Overlay / Only radio buttons, a visual mesh wireframe
checkbox and a persistent rendering statistics overlay with FPS, frame time,
visible mesh instance counts and render pass CPU times. Visual wireframe is
disabled when the GPU lacks support.
Screenshot mode waits for a requested wireframe and reports an error if it is
unavailable or the build fails. Capture renders into a dedicated image at the
window’s physical resolution. It waits for GPU pipeline compilation and
three settled frames, retries blank readbacks up to three captures, and fails
if capture has not finished within two minutes of gameplay readiness.
GPU mesh uploads still happen through Bevy.

Ground-height queries for chassis spawning use an index of triangle footprints
built during terrain loading. The index retains the original triangles and
height tolerances, including stacked surfaces. Wheel suspension continues to
use Rapier's collision queries.

The V1.2.0 floor slab is flat at height zero and the
terrain plates (the centre-line plateaus at x ≈ ±6.3..8.5 m, the 起伏路段
undulating roads near the side walls, ramps, the central highland, covered
passages) stand on it. The V2.0.0 slab is crowned
instead: about 0.11 m below the reference height along the centre line,
falling to about 0.32 m below at the side walls, with height zero at its
small level pads. Invisible walls, 2 m high, surround the loaded floor bounds
to keep robots inside the arena. The CAD floor remains the driving surface;
there is no invisible catch plane beneath a loaded field.

The default field is enclosed by a dark stadium with neutral lighting. Equipment paint follows team ownership:
red occupies +x and blue occupies -x. Destroying an outpost opens its team's base
shields. Friendly-fire rules are unchanged.

The arena's colours are the CAD's. The V1.2.0 STEP styles every arena face
(white for most surfaces, dark grey slab and plate tops, beige plate sides,
red and blue 1 mm marking sheets), and the package keeps them as glTF
materials. The V2.0.0 STEP styles every arena solid in the exporter's
unpainted cream and its markings are flush cream solids; the simulator
recolours only that yellow-tinted cream (floor dark, arena light grey), so
painted materials, white line markings included, are left alone.

Placements come from the manifests' `placements_in_source_arena_frame`. The CAD
arena frame is forward/left/up in metres; the simulator centres the field on
the origin and puts the top of the playing floor at height zero. Rune hubs are
derived from the `face_0`/`face_1` pivots inside `rune.glb` (targets on the
blade front plane, 698.5 mm orbit) and outpost bases from the outpost
placements, including the CAD's slight lean, so the emissive rune targets and
outpost armor overlays coincide with the imported geometry.

## Modules

| Crate | Responsibility |
|---|---|
| `rm-simulator-gameplay` | Standalone match lifecycle and gameplay state, deterministic commands, rule coverage and unsupported-element observations. No physics or renderer; its live resource component is driven by the world referee. |
| `rm-simulator-physics` | Reusable Rapier dynamics, chassis, projectiles, raw armor contacts, shared geometry and prescribed armor motion. No gameplay, Bevy, server or CAD-loader dependency. |
| `rm-simulator-world` | Complete `Field` facade: explicit ticks, activation, detection, damage, referee integration and restore. Coordinates the physics library and preserves existing public world imports. |
| `rm-simulator-render` | Bevy CAD scenery, lighting, rune and outpost light overlays, projectile spheres, chassis visuals, and pose/visibility/strike synchronization from caller-owned scene state. No world dependency. |
| `rm-simulator-server` | Bevy-free glue: CAD loading and checksums, collision triangles, field layout, the `Simulation` wrapper, a `Host` worker that owns simulation and command ordering, GNS UDP, TCP and in-process channel transports, the HTTP referee panel, and the headless binary. |
| `rm-simulator-bench` | Fixed-camera CAD renderer benchmark, independent of physics, world, server and gameplay. |
| `rm-simulator-app` | The `rm-simulator` binary: window, chassis driving and gimbal camera (or fly camera), gun, HUD, world-to-scene adaptation, and the local or remote session that turns inputs into protocol commands. |

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

## Build and run

Download a platform ZIP from GitHub Releases and extract it completely. Each ZIP
includes the app, headless server, native runtime libraries, and the field package.
Run `bin/rm-simulator` or `bin/rm-simulator.exe`. See [CI and releases](docs/releases.md)
for lightweight PR checks, manual full validation, branch protection setup,
and the stable/alpha/beta/RC release workflow with dry runs.

Install the Rust toolchain from `rust-toolchain.toml`. GameNetworkingSockets
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

A bare launch opens Single Player and Multiplayer choices, with a player name,
team and spectator options. Single Player starts a local practice field.
Multiplayer lists LAN lobbies on the left and creates a named, optionally
password-protected lobby on the right. Select a listing, enter its password if needed, and press Join lobby / address.
You can also enter a direct address. The page and lobby list scroll with the
mouse wheel, trackpad or scrollbar; narrow windows stack the two columns. Public is greyed out pending public connectivity support.
The firewall tip recommends allowing the app on private networks. Lobby names
and addresses are remembered, but passwords are never saved. The fields live in
`$XDG_CONFIG_HOME/rm-simulator/title.json` (`~/.config` by default,
`%APPDATA%` on Windows). Losing the connection, a host failure or a bad
address ends the match and returns to the title screen with the reason; the
window stays open. The CAD package is verified once per process, so a second
join is quick. The title background is a darkened in-game screenshot without
the gameplay HUD. The field sits in a dark stadium, with visible wire fencing
aligned to its two-metre collision boundary at the edge of the grounded CAD
scenery. The slab apron lies outside the fence, with distant stadium seating
and walls beyond it. `--play`, `--connect`, `--listen`, `--screenshot` and
`--console` skip the title screen and enter a match at once; the remaining
options are the defaults every join from the title screen starts from.

For automation, launch with `just run --console --window-mode headless`, then send
runtime spawn, camera, input and screenshot commands through the
[app console](docs/console.md). `normal` and `unfocused` modes also support it.
The existing runtime flags remain launch shortcuts.

| Option | Purpose |
|---|---|
| `--play` | Skip the title screen and start a local practice match at once |
| `--console [ADDR]` | App automation console on localhost, default `127.0.0.1:7790`; see [console commands](docs/console.md) |
| `--window-mode normal\|unfocused\|headless` | Normal visible window, visible without requesting focus, or GPU rendering without an OS window |
| `--robot infantry\|hero` | Local/host chassis preset: omni Infantry by default, or mecanum Hero. The server binary accepts the same option; remote clients receive the host preset. |
| `--cad-assets DIR` | Extracted RMUC CAD directory; relative paths start at the working directory for both manifests and meshes |
| `--big-rune` / `--no-rune` | Big Rune motion and two-target groups for training (a match always starts with the Small Rune and converts at 3:00), or no rune rules |
| `--outpost-speed-rad-s R` | Armor ring rotation rate |
| `--spawn X,Y,Z` | Start position in FLU metres; driving, the chassis is set down on the highest ground below Z (default: your team's half, before the centre-line plateau: red at x = +9, blue at x = −9) |
| `--spawn-yaw-deg D` / `--spawn-pitch-deg D` | Start heading and pitch (default heading: facing the opponent's half) |
| `--fly` | Free-flying camera without a chassis; on a remote host, join as a spectator (no chassis, no firing) |
| `--referee` | Join as the referee: the free camera, no chassis, and the match keys (`F5`, `F6`, `F7`, `F`), which a remote host grants to nobody else |
| `--third-person` | Start driving in the third-person view (`V` toggles) |
| `--collision-view hidden\|overlay\|alone` | Start with the physics geometry drawn as a green wireframe over the scenery or on its own (F3 panel; built in the background on first use; default hidden) |
| `--start-paused` | Open with the world clock paused |
| `--screenshot PATH` | Capture the window or headless camera to a PNG once loaded and settled, then exit |
| `--projectile-mm 17\|42` | Host projectile caliber (default 17; 42 with `--robot hero`; unavailable with `--connect`) |
| `--muzzle-speed-m-s V` | Starting muzzle speed, default 25 m/s for either caliber; unavailable with `--connect` |
| `--fire-rate-hz R` | Starting firing rate in simulation time, default 20 Hz; unavailable with `--connect` |
| `--max-fire-rate-hz R` | Host firing-rate cap, default 30 Hz; unavailable with `--connect` |
| `--max-muzzle-speed-m-s V` | Host actual launch-speed cap, default 30 m/s; unavailable with `--connect` |
| `--muzzle-speed-variation-m-s V` | Host default Gaussian speed variation, 0..1 m/s maximum offset, default 0.3; unavailable with `--connect` |
| `--spread-deg D` | Host default spread half-angle, 0..90 degrees; default 0.3; zero means perfect accuracy; unavailable with `--connect` |
| `--spread-distribution uniform\|gaussian` | Host default distribution: Gaussian angular offsets truncated at three sigma, or a uniform solid-angle cone; unavailable with `--connect` |
| `--spread-seed N` | Repeatable spread seed, default 0; unavailable with `--connect` |
| `--debug-panel` | Open the F3 debug panel at startup |
| `--network-stats MODE` | Show local network diagnostics; also selectable in P/F3 |
| `--render-stats` | Show frame and renderer CPU statistics at startup |
| `--wireframe` | Draw visual mesh wireframe lines if supported by the GPU |
| `--no-field-collision` | Skip the CAD collision meshes; only a flat floor at height zero and armor collide |
| `--no-referee` | Run the field without a referee: the rune is active from the start and there is no match |
| `--no-projectile-retirement` | Keep spent projectiles until the four-second flight limit; by default a ball resting on scenery at or below 2 m/s for 50 ms is removed |
| `--no-prediction` | Use host-driven movement, aim and firing for comparison |
| `--transport gns\|tcp` | Gameplay transport, default GNS over UDP; use the same choice on host and guests |
| `--connect HOST:PORT` | Join a running server instead of simulating locally (field options come from the server) |
| `--lobby-name NAME` | Advertise a LAN lobby while using `--listen` |
| `--password TEXT` | Password for hosting or joining; not saved by the menu |
| `--lobby-host HOST:PORT` | Future public directory, default `127.0.0.1:7791`; LAN never contacts it |
| `--public-lobby` | Reserved public listing option; currently rejected |
| `--advertise-address IP:PORT` | Reserved public game address override |
| `--listen [ADDR]` | Host the local world for other clients (default `0.0.0.0:7700`) |
| `--http [ADDR]` | Serve the HTTP referee panel (default `127.0.0.1:7780`) |
| `--team red\|blue` / `--name NAME` | Team and name announced to the server (default red, `pilot`); a client that names no team is put on the team with fewer pilots. The referee joins on neither team but `F` still spends this team's opportunity |
| `--rune-flash-hz F` / `--rune-flashes N` | Blink rate and count of an activated rune's arms (default 2 Hz, 3 blinks, then lit; rate 0 never blinks, count 0 blinks until the state changes) |
| `--hit-flash-ms T` | How long a struck armor module shows grey (default 50) |

Weapon settings can be changed during a match under **Settings > Weapon**.
Starting values are 20 Hz, 25 m/s, +/-0.3 m/s Gaussian speed variation and
0.3 degrees of Gaussian angular spread. Host caps default to 30 Hz and 30 m/s.
The bullet-speed HUD shows the actual last confirmed launch speed to two decimal
places, or `-- m/s` before the first shot. Rejected shots and settings changes
do not alter the reading.
Each player chooses a fire rate and muzzle speed within the host's limits,
and speed variation, a spread angle, uniform or Gaussian distribution, and seed.
The in-game
spread slider covers 0..2 degrees in 0.01-degree steps; 2 degrees gives a group
about 35 cm across at 5 m. Zero means perfect accuracy. Host CLI defaults can
still specify larger cones up to 90 degrees. The angle is the maximum deviation from the muzzle axis, or three standard deviations for
Gaussian spread. Gaussian and normal mean the same distribution; Gaussian is
the angular default. Muzzle-speed variation is always Gaussian: 0..1 m/s sets
the maximum +/- offset, with standard deviation one third of that value.
Samples stay positive and within the host's speed cap. Thus 30 m/s with +/-1
at a 30 m/s cap yields 29..30 m/s, while 29 m/s with +/-1 yields 28..30 m/s.
The host retains each pilot's settings and applies them at
launch, including to queued fire requests. Already flying bullets are unchanged.
The client sends one reliable update when settings change and waits for its
confirmation before sending new shots. A seed, chassis id and intended launch
time determine the spread sample, so prediction can reproduce it.

Expand **Host weapon settings** on the title screen to set the rate and speed
limits, caliber, and default spread before practice or creating a lobby. These
fields are remembered. Remote joins use the host's defaults; in-game adjustments
last for the current session. The server binary accepts the same weapon flags.

The HUD follows the July 2026 RMUC competitor client manual: red/blue team
status and clock across the top, robot HP below left, ammunition beside the
reticle, and a top-down teammate map below right. Green marks your robot;
defeated robots fade. Referees see both teams on the map. The map uses optional CAD-derived artwork and live positions, without radar
detection. The local status readout includes base HP and shield; ammunition and team gold
come from the live resource tracker. Heat, hardware-module status and a full
competition purchase interface are not modeled by the HUD. The overlay uses native Bevy UI with a responsive
Flair stylesheet; Feathers supplies mouse-driven settings and toolbar buttons.
Controls and graphics preferences are saved in `settings.json` beside the
remembered `title.json`. Open Settings from the title screen or toolbar to rebind
keyboard/mouse actions, invert mouse Y, adjust sensitivity, and select Low,
Medium, High or Ultra graphics with individual overrides. Help labels follow
saved bindings. Graphics changes apply live and do not change simulation physics.
Hover a graphics option or focus it with the keyboard to read its visual effect
and performance tradeoff.

| Preset | Shadow map | Cascades | Shadow distance | MSAA | Bloom |
|---|---:|---:|---:|---:|---|
| Low | Off | — | — | 1× | On |
| Medium | 1024 px | 1 | 40 m | 2× | On |
| High | 2048 px | 1 | 40 m | 4× | On |
| Ultra | 4096 px | 2 | 60 m | 4× | On |

**Warning:** Changing Light emission can alter armor plate and target colors or
hide gameplay indicators, without a meaningful performance saving. Every preset,
including Low, retains the normal emission strength of 12000 and enables bloom.
Disabling bloom can also change the apparent colors of bright armor and target
lights; the settings panel warns when it is disabled.

VSync is on by default; depth prepass and occlusion culling are opt-in on every
preset. All options remain customizable. The [NVIDIA effect sweep](docs/render-effects-2026-09-12.md)
explains the cascade choices; Mac performance still needs separate measurements.

Press Escape to open the Pause menu with Resume, Settings, and Exit Match.
Singleplayer pauses until you resume; multiplayer keeps running, including when
you host. Settings opened from Pause return to that menu. A match that was
already paused stays paused when you resume. On the title screen, Escape opens
a quit confirmation with Cancel and Quit.
Clicking the minimap also opens its expanded view. Settings, the expanded map, and toolbar-opened panels stop driving,
aiming and firing and release the cursor. Hold Tab for both teams' players and HP;
hold F12 for controls. Scroll the wheel to see longer lists while holding the key.
These hold-to-peek panels do not pause gameplay. Close a panel, then click the field
to resume control. The collision wireframe remains an optional developer view.

The HUD stylesheet is embedded from `crates/rm-simulator-app/src/hud.css`, so
UI assets work with any `--cad-assets` location. It follows the July 2026 student
manual's main interface and panels 1, 3, 5 and 6. Mirrored robot slots across the
top show each team's individual robot HP. Empty slots are dimmed; defeated
robots show DOWN. Duplicate types get extra slots so every robot remains visible.
Slot numbers follow the manual; # labels identify simulator chassis. Base HP
is not represented by an aggregate robot health bar. Feathers retains its own theme in a separate subtree.

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

Controls:

| Input | Action |
|---|---|
| Left click | Operate visible controls; click the field to capture the mouse |
| Right mouse (hold) | Auto Aim + Auto Fire; separately rebindable actions in Controls |
| Mouse | Look |
| Left button (held, once captured) | Fire from the barrel (driving) or just ahead of and below the eye (flying) |
| W A S D | Drive forward/back and strafe left/right relative to the aim; the chassis heading follows the aim (driving) or move on the horizontal plane (flying) |
| Left Ctrl | Fast: 5 m/s drive command, 8 m/s flight |
| R | Toggle chassis spin (6 rad/s) while driving |
| V | Toggle first- and third-person view while driving |
| C | Cycle physics colliders: Off / Overlay / Only |
| F3 | Open / close debug panel: collider Off / Overlay / Only, visual wireframe, rendering statistics, remote motion buffering, reset own robot to spawn |
| Space / Left Shift | Move up / down (flying) |
| F6 | Pause or resume the world clock (local world or referee) |
| F7 | Step the world one frame (16 ms) while paused (local world or referee) |
| O / I | Buy one 17 mm / 42 mm round using team gold, at default prices of 1 / 10 gold during a running match |
| F5 | Start the match (or reset a finished one) (local world or referee) |
| F | Activate the rune for your team when it has an opportunity (local world or referee) |
| Tab (hold) | Show team robot status |
| P | Toggle settings; 1 toggles reticle, 2 toggles minimap, - / = adjusts mouse sensitivity |
| M | Toggle the large team map |
| F12 (hold) | Show controls |
| Toolbar | Mouse-driven Settings, Map, Team, Help, Close and Leave match; appears with the cursor released |
| Escape | Close the current panel or open Pause; in Multiplayer, return to the main menu; otherwise open or cancel quit confirmation |

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

## Referee and match

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

By default the field carries a referee that runs a match under the RMUC 2026
rules on the same 1 ms clock: a 5 s countdown, a 7 min round, and two teams,
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
scored from the average ring of the five hits and how many arms were lit and
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

`docs/referee-rules.md` digests the rulebook clauses the referee follows,
with their section and table numbers.

## LAN lobbies

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

## Server and clients

The world can run headless:

```sh
just server --big-rune            # UDP on 0.0.0.0:7700, panel on http://127.0.0.1:7780
just run --connect 127.0.0.1:7700 --team blue --name alice
```

`rm-simulator-server` loads the CAD, builds the same field as the app, steps
it in real time and serves gameplay over Valve GameNetworkingSockets UDP and an
HTTP referee panel. Open UDP port 7700 for guests. Both executables retain
`--transport tcp` for the previous JSON-lines transport. Every client sends `Hello` with its protocol version,
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
the channel connection; remote players retain the selected network transport. Local snapshots target a
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

Small toasts distinguish measured high latency, interrupted updates and a confirmed
disconnect. Silence alone is labeled "Connection interrupted". After one second
without a checkpoint, predicted movement and new shots stop; camera look remains
available. A closed transport keeps the window open with "Disconnected". Joining
again creates a new session and robot; it does not replay old inputs.

`--no-prediction` selects host-driven movement, aim and projectile presentation for
comparison. Firing always uses independent server physics. Local offline play
uses the same ownership and command path. Severe packet loss still limits useful
combat even when local controls remain responsive.

Defeated pilots choose **Respawn here - restore HP** in the defeat menu.
The button restores full HP in place. Position, identity,
ammunition and team gold are retained. This is a training shortcut,
without a respawn timer, cost or invulnerability. The F3 **Reset robot to spawn**
button also restores the original upright spawn pose and stops the robot, while
keeping its identity, ammunition and team gold. O buys one 17 mm round and I buys
one 42 mm round during Running, using the existing resource policy and team gold.
Prices default to 1 and 10 gold and remain editable in the referee panel.
The [networking roadmap](docs/multiplayer-networking.md) records the historical baseline
and follow-up experiments for bandwidth, packet loss, input/shot timing and desyncs.
The [implementation plan](docs/networking-implementation-plan.md) defines the
implemented delivery contracts, validation limits and historical design slices.
The [network harness guide and stats design](docs/network-harness-and-stats.md)
documents the standalone trial runner and implemented player-facing stats overlay.
Run `just network-test` to verify the harness without building Rust. With matching
binaries built, `just network-trial scripts/network-scenarios/smoke-single.json
--output /tmp/rm-network-smoke` launches an isolated bandwidth/loss/latency trial
and writes reports. The output directory must be new. A scenario's `expectations`
block asserts named thresholds on the summary, `--baseline PREVIOUS/summary.json`
fails regressions beyond a relative tolerance, and the run reports `passed`,
`failed` or `completed` with a matching exit code. Use `--network-stats off|compact|detailed` or the Network stats button in P/F3
to show local diagnostics. Native rates and ping are GNS estimates; loss stays
unavailable when the wrapper cannot identify packet loss. The display adds no
server requests. The UDP transport sends independent owner corrections and uses acknowledged
baselines for world deltas. Set `RM_NET_FULL_CHECKPOINTS=1` on the host to compare
full checkpoints under the same pacing. Owner and input numeric state retains f64
precision, while shot intents use bounded compression. RMO4 owner anchors reference
a configuration delivered reliably and explicitly acknowledged by the client.
RMI3 input batches share identity fields and encode exact value changes without
reducing input redundancy. World and owner publication cadence is unchanged.
See [bandwidth experiments](docs/bandwidth-experiments.md) for isolated savings
and [known issues](KNOWN_ISSUES.md) for the remaining bandwidth target.
Application pacing defaults
to 64 KiB/s upstream and 512 KiB/s downstream per peer with the default LAN
profile. Set `RM_NET_PROFILE=limited` on the host to retain the original 40 KiB/s
downstream budget and 10 KiB/s upstream on clients using that profile. Native
congestion control remains active. For comparisons,
set `RM_NET_UP_KIB_S` and `RM_NET_DOWN_KIB_S` to integer values from 4 to 2048.
Input lead adapts to host-observed arrival margins within 32–150 ms. UDP repeats
the newest four samples plus useful older movement transitions, up to twelve
samples in one packet. For development comparisons, `RM_NET_FIXED_INPUT_LEAD=1`
on the client keeps the previous RTT-based lead, and `RM_NET_INPUT_HISTORY=4`
keeps four-sample redundancy. The harness records these overrides. F3 provides automatic or manual remote
interpolation buffering; it does not alter local input lead. Automatic buffering
releases excess delay at up to 50 ms per second after the two-second jitter
window improves. The console reports host downstream queue bytes, age and service
for control, owner and world traffic.
These application budgets exclude native retransmissions and framing; proxy rates
remain the wire-capacity reference. Invalid overrides use the defaults.

Protocol 27 sends authoritative armor contacts reliably, independently of world
snapshots. Recent contacts recovered from snapshots also display once; repeated
snapshots cannot restart their flash. Feedback more than 250 ms old is discarded
instead of replayed after a long interruption. Damage and HP remain host-owned.
Both host and clients must use matching protocol builds.

Auto-aim runs locally. Acquisition follows displayed robot poses, while the impact
solution uses the latest timestamped motion. Auto-fire requires an observation
no more than 150 ms old at intended execution; tracking stops at 300 ms. Its HUD
reports stale observations, motor alignment, blocked paths, weapon cadence and
rune confirmation. Every shot samples current controls even between normal
16 ms input refreshes.

Host handles own their listeners and workers. Shutdown closes active connections,
including incomplete handshakes and HTTP requests, and waits for workers to finish.
Collider drawing expands client-loaded CAD shapes once on a worker and follows
the rendered scene transforms each frame. It makes no host debug requests.

UDP player checkpoints use compact projectile position/velocity vectors with
identity, caliber and launch time. Vectors use 32-bit floats; server physics stays
64-bit. The server retains projectile spin, while client ball prediction starts
with zero spin at each checkpoint and may correct bounced trajectories later.
Clients reconstruct rune/outpost armor poses and omit wheel-contact and failed-hit
diagnostics. HP, registered hits, command/life state and chassis reconciliation
values are preserved. HTTP referee diagnostics keep full precision and detail;
TCP sends the same compact checkpoints, each independent of the last.

The current protocol version is 29, defined by `PROTOCOL_VERSION` in
`crates/rm-simulator-server/src/protocol.rs`. It schedules pilot control transitions
and includes shot results and projectile restore state in snapshots. GNS sends redundant controls
and retried shot intents unreliably; scheduling receipts and terminal shot results
remain reliable. There is no shooter-view fire path, ordered TCP snapshot delta
chain or input-acknowledgement stream. UDP deltas use acknowledged baselines;
TCP checkpoints remain independent.
Placement revisions prevent old input from
crossing robot lives. `Ping` remains a command barrier: the server queues a
resulting snapshot before its `Pong`. Screenshot capture waits for confirmation of
queued discrete commands and for the scene to apply that state. Server and client
must use matching protocol versions.

Known mechanism motion advances locally during up to one second without a newly
received checkpoint. Pauses and screenshots use exact authoritative poses. Clock probes
run at most once per second with one outstanding; their replies never confirm
commands. The estimate assumes roughly symmetric latency and is intended for
low-latency links. TCP socket writers compute deltas against their last transmitted
state after queue coalescing. Full recovery checkpoints reset the delta revision
at least every simulation second or 64 transmitted updates. Result snapshots
before Pong are always full. Readers reconstruct deltas before inbox coalescing;
a missing/invalid baseline closes the connection rather than exposing partial state.

Remote pose history holds at most 32 samples. It interpolates translation,
shortest-path rotation and wheel motion, holds the nearest known pose on underrun,
and discards stale history on placement, defeat, despawn and pause changes.
Screenshots bypass interpolation, keeping confirmation-dependent captures exact.
Measure snapshot bytes and codec CPU with
`cargo run -p rm-simulator-server --example network_bandwidth --locked`.
Use `cargo run --release --locked -p rm-simulator-server --example
network_bandwidth -- --deflate-sweep` to compare deflate 1/4 on identical selected
checkpoints at 64 ms intervals. See the
[cadence and compression measurements](docs/bandwidth-results/cadence-deflate-followup.md)
for live trials and a progressive degraded-network play command.

`GET /api/fire-records` on the host HTTP endpoint returns the latest 256 accepted
pilot shots, oldest first. Records include client-relative input time, estimated
simulation time, observed chassis/muzzle poses, authoritative muzzle pose,
acceptance time and projectile ID. These diagnostics never backdate shots or
change hit detection. The journal is in memory; export it before host shutdown
if needed. Rejected attempts do not create shot records.

Client socket reads and writes run on background workers. Outgoing commands
keep their order in a queue of up to 256 pending commands. The client keeps
only the latest unread snapshot and roster, plus ordered notices and
rejections capped at 256 messages or 1 MiB. Exceeding either queue limit
disconnects with an error shown in the HUD. Successfully queueing a command
does not acknowledge its execution by the host.

The snapshot remains authoritative for rules and chassis physics. A
chassis command carries the pilot's gun aim (heading and elevation in the
world) along with the body velocity, and the snapshot reports each turret's
pose, so other players' guns are drawn where their pilots point them; your
own turret predicts the same motor response locally, ahead of the round trip.
The mouse sets an aim target; camera, barrel and muzzle follow actual motor aim.
The assumed gimbal response is 60 ms with 8 rad/s speed and 80 rad/s²
acceleration limits, evaluated on the same 1 ms clock as the field.

Each chassis is a generic 15 kg infantry-sized box (520 × 520 × 100 mm) on
four 153 mm omni wheels in an X layout, with a 120 mm turret cube 150 mm
above the body (its top is 0.40 m above the ground), four small armor
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

Both CAD releases put a hex-marked truncated pyramid on the centre line at
x ≈ ±6.3..8.5 m: a 1.5 × 1.0 m flat top 0.15 m above the floor with 17°
ramps on all four sides, which the chassis crosses (the default spawn faces
the red-side one). The 起伏路段 undulating road, a 2.4 × 2.1 m strip of
waves about 70 mm high on the 0.2 m deck near each side wall (x ≈ 6.3..8.7,
y ≈ 5.4..7.5 m and its point mirror), exists only in the V2.0.0 STEP; the
default package carries the two V2.0.0 solids grafted onto its flat deck in
V1.2.0's plate colours (listed under `grafted` in the manifest), so both
maps have it. The covered passages under the base highland decks (around
x = ±12.5, y = ±5.5 m) have 0.65 m of clearance in both.

## Verify

```sh
cargo install prek --version 0.4.14 --locked
cargo install cargo-deny --version 0.20.2 --locked
prek install
just verify
```

`just verify` runs the hooks, formatting, check, Clippy with denied warnings,
tests, the module dependency check, the MPL-2.0 notice check, and `cargo deny`.

New exports may declare `collision_method: "source-tessellation-v1"` per asset.
The simulator then uses that asset's checksummed collision GLB, tessellated from
the same source faces with separate tolerances. Unmarked legacy proxies remain
ignored and those assets use their visual geometry. This does not change
rule-driven scoring shapes or joint motion.

Scene reference fixes and reproducible rune-state captures are documented in
[the scene audit](docs/scene-reference-audit.md). Rune arrows follow simulation
time, including pause and step. Neutral lamps preserve the imported field paint;
only rune optics receive the state-dependent material treatment.

## Robot equipment prototypes

`just run --robot hero --third-person` drives the mecanum Hero; omit `--robot`
for the omni Infantry. Both carry approximate AM02 armor, LI01 HP lights,
FI02 underbody RFID hardware, VT03 camera and a muzzle speed monitor. Hero uses
an SM11-sized housing; Infantry uses SM01. HP segments and defeat state follow
the referee. FI02 detection and speed-monitor LED sequences are not simulated.

The host offers one chassis and weapon preset to all pilots. A Hero host defaults
to 42 mm; connected clients use the announced weapon. Configure projectile
caliber, muzzle speed and firing rate on the host. The chassis preset does not
change HP, heat or power policies. The visible equipment is decorative;
armor scoring keeps its existing dimensions. [Reference measurements and
limitations](docs/robot-equipment.md) distinguish modeled details from assumptions.

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
variants. Hero uses 1, infantry uses 3, and outpost uses O. Textures preserve the
source aspect ratio and face outward. Patterns are passive white printing; only
the light bars stop emitting with disabled armor and remain white plastic. The external base package uses the base sprite on its moving dart plate, three
upper armor modules and three lower modules exposed by opening the shields.

Engine microbenchmarks and their limits are described in [performance checks](docs/performance.md).

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

## Performance and optional Steam integration

The recorded standard-detail build has 497,886 placed visual triangles and
312,749 collision triangles including mechanisms. The 100k collision target remains
unmet. Rebuilding, validation and rollback are described in
[reproducible field detail](docs/field-detail.md). Runtime level of detail switching
and mesh chunking are not part of this pass; GPU occlusion culling is an optional
Graphics setting and remains off by default pending measurements.

Use [the loaded-match CPU probe](docs/performance.md) to measure median and tail
costs with several matches running at once. Test client frame times separately
on the target GPU; a server probe does not establish rendering frame rate.

An optional `steam` Cargo feature initializes the Steam client and pumps callbacks.
Ordinary builds remain standalone. The [Steam setup guide](docs/steam.md) explains
App IDs, matched SDK/runtime staging and the local SDK's version mismatch.
Steam Input, lobbies and portable distribution packaging remain future work.

### Rendering benchmarks

The separate `rm-simulator-bench` executable loads only visual field assets and
measures rendering with fixed cameras, GPU timestamps and configurable JSON/raw
output. Use `scripts/benchmark-render.py` for repeated settings sweeps. See
[the benchmark guide](docs/render-benchmark.md) for build commands, Vulkan setup,
Metal CPU-only checks, cases and output definitions.


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

This is a simple game-data aid, inspired by Vision2027's selection, prediction,
ballistics, and fire-control stages. It uses exact snapshot velocities instead
of image detection or an EKF. Acquisition prioritizes targets near the crosshair, falling back to the closest
visible enemy robot within 40 m (including outside the view cone);
constant-velocity robot prediction cannot anticipate collisions or a change of
input. Scenery rays and conservative bounds for intervening robots can withhold
shots near cover. No Vision2027 dependency or copied implementation is required.


### Training bots and base armor

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
