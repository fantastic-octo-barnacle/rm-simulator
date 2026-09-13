<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# App automation console

Start the app with a local console. Window mode is chosen at launch.

~~~sh
just run --console --window-mode headless
~~~

| Mode | Behavior |
|---|---|
| `normal` | Visible window with normal focus behavior; the default |
| `unfocused` | Visible window that requests no focus on creation |
| `headless` | No OS window or window event loop; the gameplay camera renders to an image |

All modes render and process commands while unfocused. Headless rendering still
requires a working GPU backend. The default headless image is 1280 × 720.
Whether a visible window can start unfocused depends on the operating system and
window manager. `--window-mode unfocused` sets the native window's initial focus
request; it does not keep forcing focus away later.

`--console` defaults to `127.0.0.1:7790`. Use `--console 127.0.0.1:0` for an
allocated port; the app prints the actual address. Only loopback addresses are
accepted. This is an app automation endpoint, separate from the multiplayer TCP
server. It has access to the local app's controls and screenshot filesystem paths.
One controller connects at a time; additional connections are closed.

## Send commands

The bundled Python client waits for the scene to be ready, runs commands in
order, prints each reply, and stops with a nonzero exit status on errors.

~~~sh
python3 scripts/console.py \
  '{"type":"pause","paused":true}' \
  '{"type":"spawn","position_m":[9,0,2],"yaw_deg":180}' \
  '{"type":"camera","pitch_deg":10}' \
  '{"type":"key","key":"w","pressed":true}' \
  '{"type":"step","ticks":100}' \
  '{"type":"release_inputs"}' \
  '{"type":"screenshot","path":"/tmp/field.png"}'
~~~

The app stays open. Send `{"type":"quit"}` to close it. The client also reads
command objects, one per line, from stdin. Use `--host` and `--port` to select
another local endpoint and `--no-wait` for queries or shutdown during loading.

Hold a single connection open for multi-command input sequences. Disconnecting
releases console-held keys and mouse buttons and releases mouse capture.
Discrete commands already sent to the simulation are not rolled back.

Existing launch flags such as `--spawn`, `--third-person`, `--start-paused`,
`--collision-view`, and `--screenshot` remain available as shortcuts. The
screenshot flag still captures once and exits; the console command supports
repeated captures without exiting. Both use the window in visible modes and
the camera image in headless mode.

## Wire format and completion

Send one JSON request per line:

~~~json
{"id":1,"command":{"type":"step","ticks":16}}
~~~

Replies echo the unsigned integer ID:

~~~json
{"id":1,"ok":true,"result":{"tick":16,"paused":true}}
{"id":2,"ok":false,"error":"pause the simulation before stepping"}
~~~

State replies include camera, chassis, role and UI fields. `ui.unfocused` and
`ui.consumed` identify focus and one-frame input blocking. `auto_aim` reports the
selected target/status, execution time, observation age and fire permission.
`network.confirmed_launches` counts this client's accepted launches once, unlike
the global `shots_fired` counter. `network.downstream_queues` reports control,
owner and world queue bytes, oldest age in ms and cumulative bytes sent.
`network.hit_feedback` reports event count, impact tick, receipt age and the last
receipt-to-scene interval in ms. Scene submission does not measure GPU scanout.
Requests are executed sequentially, including pipelined requests. There is at
most one command in progress. Malformed requests get an error; unparsable IDs
are returned as `null`. Unknown fields and commands are errors.

The input buffer is limited to 64 KiB, including pipelined requests. Exceeding
that limit closes the connection and releases held inputs. A command waiting on
readiness, a snapshot or a screenshot times out after 120 seconds. Socket reads
and writes are nonblocking and bounded per frame.

Successful input and world-command replies wait for the app to process the
input and receive the server's resulting snapshot. Host rejections are errors,
not successful acknowledgements. This preserves existing pilot/referee
permissions. A screenshot succeeds only after the scene, pipelines, requested
collision geometry and command snapshots are ready, and the PNG has been saved.
Blank frames are retried. Save errors are returned without closing the app.
A timeout does not cancel a simulation command already submitted.

For repeatable world motion, pause, press the controls, step an exact number of
ticks, release the controls, then capture. Each tick is 1 ms. Free-camera keyboard
movement and held firing use app frames and are not deterministic tick scripts;
use explicit camera poses or the `world` command for those cases.

## Commands

Positions are world forward/left/up metres. Yaw is counter-clockwise from +x;
positive pitch looks up. Camera pitch follows the existing gameplay limits.

| Type | Fields | Effect |
|---|---|---|
| `help` | None | State and supported command names; available during loading |
| `ready` | None | Wait for loaded scenery, compiled pipelines and confirmed commands |
| `state` | None | Current readiness, tick, estimated `presentation_time_ns`, pause, role, own/predicted chassis, camera, UI, connection toast and network diagnostics |
| `camera` | Optional `position_m: [x,y,z]`, `yaw_deg`, `pitch_deg`, `third_person` | Set free-camera position or pilot aim; third person requires a pilot |
| `spawn` | `position_m: [x,y,z]`, optional `yaw_deg` default 0 | Place the existing local pilot using the ground below z, or move a free camera. Resets pitch, body velocity and wheel contacts; preserves ID, HP and defeat state. Remote pilot placement is rejected |
| `pause` | `paused: bool` | Set the clock's paused state |
| `step` | `ticks: 1..60000` | Advance a paused world by this many milliseconds |
| `screenshot` | `path: "file.png"` | Save the current rendered view and HUD; relative paths use the app's working directory; parent directory must exist |
| `key` | `key: string`, `pressed: bool` | Hold or release a game key |
| `mouse_button` | `button: "left"`, `"right"` or `"middle"`, `pressed: bool` | Press or release a button in gameplay and UI picking |
| `mouse_motion` | `dx`, `dy` | Relative mouse motion in pixels; aiming requires capture and no blocking panel |
| `cursor` | `x`, `y` | Set pointer position in logical pixels from the upper-left corner, for UI picking |
| `capture` | `captured: bool` | Set gameplay mouse capture for automation without grabbing the OS cursor; explicit capture bypasses OS focus blocking until release or disconnect |
| `release_inputs` | None | Release all console-held keys/buttons and gameplay capture |
| `inspect` | Optional `debug_panel`, `wireframe`, `render_stats` booleans, `collision_view: "hidden"`, `"overlay"` or `"alone"` | Set runtime inspection options |
| `world` | `command`: multiplayer protocol `Command` object | Submit a world command through the existing session, with its normal host permissions |
| `quit` | None | Reply, then exit |

Keys include case-insensitive letters `a`–`z`, digits `0`–`9`, `f1`–`f12`,
`space`, `escape`, `enter`, `tab`, `backspace`, `arrow_up`, `arrow_down`,
`arrow_left`, `arrow_right`, `shift`, `control`, `alt`, `shift_left`,
`shift_right`, `control_left`, `control_right`, and `alt_left`.
These are physical game controls, not text entry. Panels block automation input
the same way they block physical input. For example, press and release `f3` to
open the debug panel.

The `world` command uses the existing externally tagged protocol shape:

~~~json
{"type":"world","command":{"Referee":"StartMatch"}}
{"type":"world","command":{"Pause":{"paused":true}}}
~~~

`PlaceChassis` is reserved for the embedded owner and their own chassis. The
console does not grant remote referee privileges and never creates a robot for
a spectator or referee.

The typed `ConsoleCommand` handler runs on the app thread independently of
socket parsing. A future in-game console can translate text into these commands
and use the same completion handling.

The `state` reply includes `network.trace`: cumulative stage/class counters,
trace path, dropped observations, contention and writer errors. Host-produced
snapshot/owner byte counters are under `network.downstream_queues.encoding`.
See [network tracing](network-tracing.md) for recording and accounting details.
