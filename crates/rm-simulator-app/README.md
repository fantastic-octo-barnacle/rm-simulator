<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# rm-simulator-app

`rm-simulator-app` owns the `rm-simulator` binary: the window, the title screen and
match lifecycle, the player controls, the HUD, the world-to-scene adaptation, and
the local or remote session that turns every input into a protocol command. It is
the crate that holds the window, renderer, world and server together, and the only
place besides the server that reads host time.

## Modules

| File | Owns |
|---|---|
| `src/main.rs` | App wiring: plugins, systems and their ordering. |
| `src/args.rs` | The clap command line: `--connect`, `--listen`, `--console`, `--window-mode`, `--cad-assets`, `--robot` (Hero or one of two infantries), `--physics-rate-hz`, `--start-paused`, `--screenshot` and `--fly`. |
| `src/loading.rs` | The match lifecycle: a `JoinRequest` prepares a session on a worker behind a splash, `Ready` unlocks gameplay, and a `LeaveRequest` or any failure tears the match down to the title screen. |
| `src/title.rs` | The title screen and the remembered fields; every choice becomes the arguments a command line would have given. |
| `src/session.rs` | `Session` over a `Client` for embedded and remote hosts, prediction state and match keys; all gameplay input uses the same path. |
| `src/controls.rs`, `src/bindings.rs` | The gimbal camera, drive and fly, the gun, mouse capture, and the named action bindings. |
| `src/scene.rs` | World snapshots into the renderer's `SceneState`: CAD instances, overlay spawning, hit flashes and chassis visuals. |
| `src/prediction.rs`, `src/projectile_prediction.rs`, `src/session_shots.rs`, `src/interpolation.rs` | One replaceable local replay, provisional balls fired into a restored field, bounded shot retries, and remote view timing. |
| `src/hud.rs`, `src/hud/`, `src/network_hud.rs`, `src/minimap.rs` | The competitor overlay and panels, the bounded local network display, and the top-down map. |
| `src/debug.rs`, `src/debug_panel.rs` | The physics wireframe view and the developer and statistics panel. |
| `src/console.rs` | The automation console: one local controller, ordered JSON requests and nonblocking socket work. |
| `src/graphics.rs`, `src/preferences.rs` | Persisted graphics and user preferences applied to the passive renderer. |
| `src/auto_aim.rs`, `src/hit_feedback.rs`, `src/frames.rs`, `src/screenshot.rs`, `src/presentation.rs` | Ground-truth aim assist, receipt-timed contact effects, FLU conversions, `--screenshot`, and window presentation. |
| `src/steam_support.rs` | The optional Steam client lifetime behind the `steam` feature. |
| `src/net_harness.rs` | Test-only deterministic end-to-end network trace over `scripted_link`. |

## Dependencies

`rm-simulator-world`, `rm-simulator-render` and `rm-simulator-server`, plus Bevy,
`bevy_flair`, `clap`, `serde`, `serde_json` and `sha2`. The optional `steam`
feature adds `steamworks`; `rm-simulator-physics` is a dev-dependency only. The
renderer stays passive, the server owns the simulation, and the app submits every
input as a protocol `Command`, so multiplayer needs no separate input path.

## Testing

`cargo test -p rm-simulator-app --locked` covers argument parsing, frame
conversions, the hit flashes, the HUD line, preferences and console requests, and
the `net_harness.rs` trace. That trace runs a real host and a real client session
over `scripted_link.rs` on one hand-advanced clock, with no sockets, sleeps or CAD,
so a seed replays byte for byte.

The crate exposes no doctests. `examples/physical_scene.rs` is the optional Bevy
composition of the physical library with the renderer, without `Field`, the
referee, CAD loading or server APIs; it advances 16 simulation milliseconds per
rendered frame:

```sh
cargo run -p rm-simulator-app --example physical_scene --locked
```

`just run <args>` starts the app and `just server <args>` the headless host. The
window modes, the automation console and its commands are documented in
[docs/console.md](../../docs/console.md), the complete option and binding
reference in [docs/app-options.md](../../docs/app-options.md), and the
snapshot-to-scene and prediction contracts in
[docs/architecture-refactor.md](../../docs/architecture-refactor.md).
