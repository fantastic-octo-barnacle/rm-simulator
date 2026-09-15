<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# App options, controls and interface

The complete command-line reference for `rm-simulator` and
`rm-simulator-server`, the input bindings, weapon settings and the graphics
presets. For what the simulator does, see the [project README](../README.md);
for automation, see [console commands](console.md).

## Command-line options

`rm-simulator` accepts every option below. `rm-simulator-server` accepts the
field and weapon options that a host owns; a client
joined with `--connect` takes those from the server.

| Option | Purpose |
|---|---|
| `--play` | Skip the title screen and start a local practice match at once |
| `--console [ADDR]` | App automation console on localhost, default `127.0.0.1:7790`; see [console commands](console.md) |
| `--window-mode normal\|unfocused\|headless` | Normal visible window, visible without requesting focus, or GPU rendering without an OS window |
| `--robot hero\|infantry-3\|infantry-4` | Robot you drive, on any host: the mecanum Hero fires 42 mm, the omni Infantry 3 (default; `infantry` is accepted) and Infantry 4 fire 17 mm. The two infantries differ only in the painted number |
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
| `--muzzle-speed-m-s V` | Starting muzzle speed, default 25 m/s for either caliber (the caliber follows each pilot's robot); unavailable with `--connect` |
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

`--play`, `--connect`, `--listen`, `--screenshot` and `--console` skip the title
screen and enter a match at once; the remaining options are the defaults every
join from the title screen starts from.

## Robot page

Single Player, Join lobby / address and Create lobby all open the robot page
before the match starts. The blue column is on the left and the red column on
the right; each offers the Hero (42 mm), Infantry 3 (17 mm), Infantry 4
(17 mm) and a spectating free camera. The Referee seat sits below them with
Back and the Start match or Join lobby button. The chosen frame is lit and a
line below names the seat. Enter confirms, Escape goes back to the page the
choice came from. The robot is remembered with the other fields; the referee
seat is not. Two pilots may drive the same robot: a host has no seat list to
show before the connection is made.

## Weapon settings

Weapon settings can be changed during a match under **Settings > Weapon**.
Starting values are 20 Hz, 25 m/s, +/-0.3 m/s Gaussian speed variation and
0.3 degrees of Gaussian angular spread. Host caps default to 30 Hz and 30 m/s.
The bullet-speed HUD shows the actual last confirmed launch speed to two decimal
places, or `-- m/s` before the first shot. Rejected shots and settings changes
do not alter the reading.
Each player chooses a fire rate and muzzle speed within the host's limits,
and speed variation, a spread angle, uniform or Gaussian distribution, and seed.
The in-game spread slider covers 0..2 degrees in 0.01-degree steps; 2 degrees
gives a group about 35 cm across at 5 m. Zero means perfect accuracy. Host CLI
defaults can still specify larger cones up to 90 degrees. The angle is the
maximum deviation from the muzzle axis, or three standard deviations for
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
limits and default spread before practice or creating a lobby. These
fields are remembered. The caliber is not a setting: each pilot's robot fixes
it, 42 mm for the Hero and 17 mm for the infantries, and the host refuses a
weapon update that names another. Remote joins use the host's defaults; in-game adjustments
last for the current session. The server binary accepts the same weapon flags.

## Interface and HUD

The HUD follows the July 2026 RMUC competitor client manual: red/blue team
status and clock across the top, robot HP below left, ammunition beside the
reticle, and a top-down teammate map below right. Green marks your robot;
defeated robots fade. Referees see both teams on the map. The map uses optional
CAD-derived artwork and live positions, without radar
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

The HUD stylesheet is embedded from `crates/rm-simulator-app/src/hud.css`, so
UI assets work with any `--cad-assets` location. It follows the July 2026 student
manual's main interface and panels 1, 3, 5 and 6. Mirrored robot slots across the
top show each team's individual robot HP. Empty slots are dimmed; defeated
robots show DOWN. Duplicate types get extra slots so every robot remains visible.
Slot numbers follow the manual; # labels identify simulator chassis. Base HP
is not represented by an aggregate robot health bar. Feathers retains its own theme in a separate subtree.

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

## Graphics presets

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
preset. All options remain customizable. Shadow cascade choices were measured on
an NVIDIA laptop GPU; Mac performance still needs separate measurements.

## Controls

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
| Escape | Close the current panel or open Pause; on the robot page, return to the page its choice came from; in Multiplayer, return to the main menu; otherwise open or cancel quit confirmation |
