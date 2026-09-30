<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Referee link

The referee link lets a RoboMaster custom client, such as trident-rm
`custom-client-27`, run against the simulator instead of the referee system.
The app embeds an MQTT broker and publishes the official custom-client state
topics for the robot it pilots, as raw Protobuf, one topic per message name,
and drives that robot from the client's `KeyboardMouseControl`.

It is opt-in twice: build with the `referee-link` feature (`just run-referee`
does), then pass `--referee-link`.

~~~sh
just run-referee --play --robot hero --referee-link
# The custom client connects as it would to the referee system:
./build/rm-client --broker tcp://127.0.0.1:3333 --client-id 1
~~~

`--referee-link` listens on `127.0.0.1:3333`. Pass an address, for example
`--referee-link 0.0.0.0:3333`, to accept a client on another machine; the
broker has no authentication. `--client-id` on the client must be this robot's
referee ID (red 1-7, blue 101-107) so it shows the right side.

## Topics

| Topic | Rate | Content |
|---|---|---|
| `GameStatus` | 5 Hz | Stage from the referee phase, stage clock, round, round wins as scores, pause |
| `GlobalUnitStatus` | 1 Hz | Base, shield and outpost HP of both sides; robot HP by number; damage dealt |
| `RobotStaticStatus` | 1 Hz | Referee ID, type, level, maximum HP, heat limit and cooling, chassis power |
| `RobotDynamicStatus` | 10 Hz | HP, heat, allowance for the robot's caliber, chassis energy, experience, shots, out of combat |
| `RobotPosition` | 1 Hz | Own chassis position |
| `RadarInfoToClient` | 1 Hz | Every numbered robot's position, opponent first |

Fields the simulation does not model stay absent rather than zero: series
length, base and outpost status codes, buffer energy, last launch speed and
yaw. Radar positions go to every client at 1 Hz, which is more than a real
client receives.

Referee IDs come from the roster's robot (Infantry 3, 4 or 5), or from the kind
where only one number fits. Robot numbers with no robot on the field report 0
HP and radar position 0, 0.

## Control

The link subscribes to `KeyboardMouseControl` and plays each message as local
input, the way the [app console](console.md) injects keys: the sixteen protocol
keys (W, S, A, D, Shift, Ctrl, Q, E, R, F, G, Z, X, C, V, B) are held as the
same keyboard keys, the three buttons as the mouse buttons, and mouse motion
aims (protocol y is up). The app's own bindings then apply, so W drives, Ctrl
is fast, R toggles spin and the left button fires unless they were rebound in
the settings. The wheel is ignored.

While messages arrive the link holds mouse capture, so the app aims and fires
even when its window is not focused; an open panel still blocks input. When no
message arrives for 200 ms, or the match ends, every held key and button is
released and capture ends. A client in menu mode sends neutral input, which
keeps capture but moves nothing.

The console's `state` reply shows the link under `referee_link`:
`controlling`, `controls_received` and the held `keys` and `buttons`.

## Coordinates

The protocol world frame is assumed to have its origin at the red-side corner
with x towards blue, in metres; radar positions are in centimetres. The
simulator puts red on +x with the origin at the field centre, so
`to_protocol_frame` turns positions a half turn about the centre. The custom
client uses the same assumption; it has not been confirmed against the V2.0.0
protocol.
