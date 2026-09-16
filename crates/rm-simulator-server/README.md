<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# rm-simulator-server

`rm-simulator-server` owns the authoritative simulation and every way to reach it:
a verified CAD package loaded into a `Field`, the `Simulation` that paces it on
the host clock, the framed player transports, the referee HTTP panel and the
`rm-simulator-server` headless binary. It draws nothing and never depends on Bevy;
the app is one client of this crate, whether it hosts in-process or connects here.

## Modules

| File | Owns |
|---|---|
| `src/cad_assets.rs` | Manifest parsing, SHA-256 verification, and CAD-frame-to-FLU placements with the floor at height zero. |
| `src/collision_mesh.rs` | Visual GLBs to named FLU triangle parts, plus the ground-footprint height index. |
| `src/base_layout.rs` | Reproducible base scoring frames fitted at load time from checksum-verified CAD. |
| `src/layout.rs` | `FieldConfig` from `LayoutOptions`: rune hubs, outpost origins, terrain, team spawn slots and the `ChassisSpawner`. |
| `src/simulation.rs` | `Simulation`: the paused flag, bounded real-time advance, command application and chassis spawning per player. |
| `src/clock.rs` | The place host-side code reads the wall clock; tests hand it a `ManualTime` instead. |
| `src/host.rs` | The single-owner worker: roster, command ordering, snapshot capture and its bounded mailbox. |
| `src/net.rs` | TCP and GNS UDP transports, peer delivery, `Client`, and in-process owner channels. Its private submodules are `gns_transport.rs`, `outbox.rs` and `presentation_clock.rs`. |
| `src/udp_codec.rs`, `src/udp_snapshot.rs`, `src/snapshot_codec.rs`, `src/binary_snapshot/` | The per-peer codec with no socket in it, the acknowledged-baseline delta state machine, and the compact checkpoint and packed bitpacked encodings. |
| `src/protocol.rs` | The JSON-lines `ClientMessage`/`ServerMessage`, `Command`, roles, `PROTOCOL_VERSION`, the default ports and the weapon configuration. |
| `src/compression.rs` | The ZSTD wire codec for non-checkpoint messages; every frame is self-identifying. |
| `src/input_stream.rs`, `src/prediction.rs`, `src/view.rs`, `src/owner_stream.rs`, `src/pacing.rs` | Sequenced held input with a simulation-time lease, bounded replay contracts, remote pose history, owner anchors and byte pacing. |
| `src/scripted_link.rs` | A deterministic datagram link with scripted loss, reordering, duplication, delay and blackouts. |
| `src/network_stats.rs`, `src/network_trace.rs` | Local diagnostics, and bounded transport metadata tracing that never records payloads. |
| `src/http.rs`, `src/panel.html` | The HTTP/1.1 referee panel and JSON API: `GET /`, `GET /api/state`, `POST /api/referee` and `POST /api/command`. |
| `src/lobby.rs`, `src/lifecycle.rs`, `src/math.rs`, `src/semantics.rs` | LAN directory leases, owned listeners, wxyz quaternion and matrix helpers, and checksum-bound semantic articulation data. |
| `src/main.rs` | The headless binary: `--physics-rate-hz`, `--listen`, `--transport`, `--http` and the rune, referee, chassis and collision toggles. |

## Dependencies

`rm-simulator-world` plus `anyhow`, `clap`, `serde`, `serde_json`, `sha2`,
`game-networking-sockets`, `zstd` and `if-addrs`. The crate never
depends on Bevy, `rm-simulator-render`, `rm-simulator-app` or
`rm-simulator-gameplay`; the live resources arrive through the world crate. It is
the only place besides the app that reads host time.

`crates/rm-simulator-server/assets/` holds the tracked ZSTD checkpoint dictionary
embedded with `include_bytes!`. See [assets/README.md](assets/README.md) for its
bytes, hash, training run and regeneration commands; retrain it when an encoded
payload or precision changes.

## Testing

`cargo test -p rm-simulator-server --locked` runs the in-module tests and the
test-only `bandwidth_probe.rs` attribution probe. Coverage includes the protocol
and its version refusal, loopback TCP and UDP client flows, the HTTP routes, the
ZSTD wire codec and the snapshot delta state machine, the host worker's ordering
and bounded queues, the replaceable periodic outbox, input leases and their
renewal, and the scripted link's loss, reordering and duplication.

Many modules carry doctests, among them `protocol.rs`, `compression.rs`,
`math.rs`, `cad_assets.rs`, `collision_mesh.rs` and `scripted_link.rs`.

Examples cover the operational checks. `inspect_assets` verifies a package and
reports semantic bindings and collider counts; `physics_rate`, `match_cost`,
`prediction_replay` and `projectile_retirement` probe the loaded field;
`network_bandwidth` measures the wire; `train_binary_dictionaries` regenerates
the embedded checkpoint dictionary.

```sh
cargo run --locked -p rm-simulator-server --example inspect_assets -- <package>
```

Transport tracing is documented in
[docs/network-tracing.md](../../docs/network-tracing.md), the codec measurements
in [docs/bandwidth-experiments.md](../../docs/bandwidth-experiments.md), and the
engine cost checks in [docs/performance.md](../../docs/performance.md).
