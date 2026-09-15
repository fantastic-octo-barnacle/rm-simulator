<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# rm-simulator

A first-person RoboMaster field simulator in Rust with Bevy. It loads the
RMUC 2026 field from the extracted competition CAD, runs the rune, outpost and
projectile rules on an explicit 1 ms clock, referees a match on top of them, and
lets you drive an omni chassis over the terrain (or fly a free camera) and shoot
at armor. Workspace crates keep gameplay rules, physics, rendering, the headless
server, the interactive application and the rendering benchmark apart.

## Documentation map

Read the shortest document that answers the question before opening a PDF or a
source file.

| Question | Read |
|---|---|
| What the simulator is, install, controls, options | `README.md` |
| Complete CLI reference, bindings, HUD, presets | `docs/app-options.md` |
| Field package layout, discovery, composition, collision | `docs/field-package.md` |
| Build, test, release and commit workflow | `docs/development.md`, `CONTRIBUTING.md` |
| Rulebook clauses the referee implements | `docs/referee-rules.md` |
| Standalone gameplay engine and its live integration | `docs/gameplay.md` |
| Crate ownership, ticks, restore, ECS | `docs/architecture-refactor.md` |
| Physics reuse without a match or server | `docs/physics-reuse.md` |
| Open issues and measurement gaps | `KNOWN_ISSUES.md` |
| Licensing and third-party provenance | `NOTICE.md` |
| Every guide, indexed | `docs/README.md` |

The `README.md` is user-facing; this file is the engineering contract. Where the
two disagree, this file wins.

## Context

- **Rules source.** The RoboMaster 2026 University Championship Rule Manual
  (V2.1.0, 2026-07-17) is the main rules baseline. The live base damage
  table was checked against V2.2.0; see `docs/referee-rules.md` for that
  limited exception. Cite section, table or figure numbers in doc comments
  when a constant comes from it (for example Table 5-1 detection speeds,
  Table 5-2 damage, Figure 5-16 armor detection area, section 5.5.1 outpost).
  Dimensions that live only in figures were read off the drawings; say so.
- **Sibling repositories.** `../Vision/rm-vision-sim` (computer vision
  simulator) and the Embedded simulator share the same rules and the same
  field package; `../rm-map-tools` builds that package from the DJI STEP
  releases (`prototype/export_field_package.py`). Rune rules,
  outpost geometry, CAD scene handling and armor artwork masks were copied
  from `rm-vision-sim` with their provenance notices. Never add a path or git
  dependency on those crates; copy code and keep its provenance notice.
- **CAD colours.** The default package's arena comes from DJI's V1.2.0
  STEP, which styles every arena face (white surfaces, dark grey slab and
  plate tops, beige plate sides, red and blue marking sheets); those colours
  are glTF materials and must be shown as they are. The V2.0.0 STEP styles
  every arena solid and face in the exporter's unpainted yellow-tinted
  cream, with flush cream markings; the renderer's scenery override
  recolours that cream only (`is_unpainted`), never white or painted
  materials. A re-export of V2.0.0 cannot add colour the STEP lacks.
- **Chassis per player, a referee, two teams.** Every pilot gets its own
  four-wheel chassis with a stabilised gun pivot (omni Infantry or mecanum
  Hero preset, documented in `crates/rm-simulator-physics/src/chassis.rs`), added to the field when the player
  arrives and removed when they leave; spectators and the free-flying
  camera have none. The field keeps them in a list with ids that are never
  reused, and commands are addressed by id; the server assigns a chassis
  per client and refuses commands for any other. A client's role is pilot,
  spectator or referee: the referee is a spectator on no team with the
  match controls (`Referee`, `Pause`, `Step`), which a host grants to
  nobody else; never give the referee a robot. There is a gun; a `Fire`
  names the shooter's chassis, which a host checks. The host derives the muzzle pose and enforces its weapon
  configuration and simulation-time cadence; its aim
  travels in the chassis command and the snapshot carries the turret pose,
  so peers see where each pilot points. The
  referee (`referee.rs`) runs the match clock, rune opportunities and
  stages, rune buffs and a robot HP record per chassis (opened when the
  chassis joins with the kind its `ChassisPlacement` names, all with the one
  configured HP); four small armor modules on
  the chassis body score like outpost armor and a defeated robot cannot
  drive, aim or fire. Live bases have HP, shield, outpost protection and
  projectile-scoring plates; destruction ends a running match. Only the defense
  buff affects live damage, on bases, outposts and robots. Attack/cooling buffs,
  referee power limits, heat and full competition progression remain unenforced
  in the live simulation. The standalone gameplay engine has broader coverage;
  see `docs/gameplay.md` before changing either integration. The chassis has an assumed shared
  drivetrain power budget, requested separately from referee power enforcement.
  Do not add further rule enforcement without being asked.
  The rulebook does not describe how an activated rune looks beyond its
  arms being lit or how long a struck module flashes; the three 2 Hz blinks and
  the 50 ms grey flash are app settings, not rule constants.

## Assets

Four asset locations exist and are deliberately separate. Never merge them; the
loader and the licensing rules depend on the split.

| Path | Tracked | Role |
|---|---|---|
| `assets/` | yes | Build input embedded with `include_bytes!`: armor-atlas artwork masks, outpost/title art and their sources. Small and CAD-free. |
| `crates/rm-simulator-server/assets/` | yes | Build input embedded with `include_bytes!`: the JSON and binary protocol 32 checkpoint ZSTD dictionaries. |
| `local-assets/` | no | Gitignored development scratch: extracted `field` package, dated `field.before-*` backups, coarse/preview exports, harness reports. Never source; not referenced from committed code. |
| `~/dev/RM/assets/` | outside the repo | Home-directory field install and the legacy V2.0.0 extraction the loader falls back to. Not this repository's `assets/`. |

The exported runtime field package is versioned in `field/` through Git LFS with
its upstream ownership notice retained. It is selected by `--cad-assets` when
supplied; otherwise the loader prefers `field/` beside an installed `bin/`
directory, then the build checkout's `local-assets/field`, then the checkout's
LFS `field/`, then `~/dev/RM/assets/rm2026-field`. The earlier V2.0.0 package at
`~/dev/RM/assets/rm2026-extracted` still loads. Files are verified against the
SHA-256 in `manifest.json` and `equipment/manifest.json`. The manifest's
`floor_top_source_z_m` is the slab top in the arena frame; when absent the
V2.0.0 pad height applies. Keep original STEP files and intermediate exports
outside Git; only the approved runtime package belongs in LFS. Record package
updates in `scripts/release-field.json` and verify with `inspect_assets`.
Never bake geometry into code beyond the fitted physics and world constants.
See `docs/field-package.md` for composition, collision contracts and deployment.

## Layout

| Path | Contents |
|---|---|
| `crates/rm-simulator-gameplay` | Standalone deterministic match engine and `live::Resources` for the world referee's economy/allowance tracking. No physics, rendering or other simulator-crate dependency. |
| `crates/rm-simulator-physics` | Bevy-free, gameplay-free Rapier library. `chassis.rs` owns wheel/body dynamics, `projectile.rs` owns stepping and raw armor contacts, `geometry.rs` owns shared collision captures and clearance queries, and `motion.rs` owns prescribed rotor/rail motion and fitted armor geometry. No other simulator crate dependency. |
| `crates/rm-simulator-world` | Complete `Field` facade, rune activation, outpost/base HP, referee integration and `scoring.rs` detection/damage. Coordinates physics on explicit ticks and restores the whole world. Re-exports existing physical types through compatibility modules. |
| `crates/rm-simulator-render` | Bevy scene. `cad.rs` (glTF scenery and roles), `rune.rs` and `outpost.rs` (light overlays with lit and struck materials), `projectile.rs` (pooled projectile spheres), `chassis.rs` (body, armor lights, omni wheels, two-axis gimbal), `sync.rs` (`SceneState` in, transforms and materials out), `lighting.rs`. Never depends on the world crate. |
| `crates/rm-simulator-server` | Bevy-free glue and the `rm-simulator-server` binary. `cad_assets.rs` (manifest parsing, checksums, CAD frame to FLU poses), `collision_mesh.rs` (visual GLBs to named FLU triangle parts, ground lookup), `compression.rs` (selectable wire codec: DEFLATE or ZSTD with an embedded trained checkpoint dictionary, self-identifying frames), `math.rs` (wxyz quaternion and column-major matrix helpers, the glTF root pose convention), `layout.rs` (rune hubs, outpost origins, terrain into the field, team spawn slots and the `ChassisSpawner`, `FieldConfig` from options), `simulation.rs` (`Simulation`: paused flag, bounded real-time advance, command application, chassis spawning per player), `protocol.rs` (JSON-lines `ClientMessage`/`ServerMessage`, roles), `host.rs` (single-owner simulation worker, roster and command authority), `net.rs` (TCP sockets, in-process owner channels, peer delivery and `Client`), `network_trace.rs` (bounded metadata tracing and local counters), `udp_codec.rs` (the per-peer UDP codec with no socket in it: fragment framing, reassembly, input batches, delta coding and pacing, all on an explicit `now`), `gns_transport.rs` (the GNS sockets that drive that codec), `scripted_link.rs` (a deterministic datagram link with scripted loss, reordering, duplication, delay and blackouts, for tests), `http.rs` and `panel.html` (minimal HTTP/1.1 server and the referee page), `main.rs` (headless binary). Depends on the world crate; the only place besides the app that reads host time. |
| `crates/rm-simulator-app` | The `rm-simulator` binary. `main.rs` (app wiring), `args.rs` (clap arguments), `loading.rs` (the match lifecycle: a `JoinRequest` prepares a session on a worker behind a splash, `Ready` unlocks gameplay, a `LeaveRequest` or any failure tears the match down to the title screen), `title.rs` (the title screen and the remembered fields; every choice becomes the arguments a command line would have given), `session.rs` (`Session` over a `Client` for both embedded and remote hosts, prediction state and match keys), `controls.rs` (gimbal camera, drive and fly, gun, mouse capture), `scene.rs` (CAD instances, overlay spawning, world-to-scene adaptation and flashes), `hud.rs` (overlay text), `debug.rs` (collision wireframe view), `screenshot.rs` (`--screenshot`), `frames.rs` (FLU-to-Bevy conversions). |
| `crates/rm-simulator-bench` | Standalone fixed-camera visual CAD benchmark. Shares renderer and graphics presets; never depends on physics, world, server or gameplay. GPU timestamp readbacks, CPU frame distributions, settings cases and optional raw output. See `docs/render-benchmark.md`. |
| `scripts/check-module-dependencies.py` | Asserts the crate boundaries above. |
| `scripts/check-mpl-compliance.py` | Asserts every resolved MPL-2.0 dependency is allowlisted in `deny.toml`, named at its locked version in `NOTICE.md`, and unmodified. |
| `LICENSE-MIT`, `LICENSE-APACHE`, `LICENSES/MPL-2.0.txt` | The workspace's dual license and the vendored MPL-2.0 text. |
| `README.md`, `CHANGELOG.md`, `NOTICE.md` | User docs, user-visible changes, licensing and provenance of reused code. |
| `docs/referee-rules.md` | Digest of the rulebook clauses the referee implements (clock, rune opportunities, buffs, outposts), with section and table numbers; read it before the PDF and update it when a rule changes. |

## Architecture rules

These define the shape of the system. Changing one is an architectural change,
not a refactor.

- **Crate boundaries.** Physical dynamics and prescribed motion in physics,
  live rules in world, drawing in render, Bevy-free glue
  and networking in server, the window in app. The world and server crates
  never depend on Bevy; the render crate never depends on the world or
  server crate, or physics crate. Physics never depends on world, gameplay,
  server or Bevy. The app converts world snapshots into render `SceneState`
  and sends every input as a protocol `Command`, whether the simulation is
  local or remote, so multiplayer needs no new input path.
- **Hosted ownership.** `host.rs` owns each live `Simulation` and its roster on
  one worker. Local channels, TCP, UDP and HTTP submit typed requests through `HostHandle`; never
  restore shared mutable simulation access in a transport or app system.
  Queue confirmation snapshots and Pongs in the worker's application order;
  encode broadcasts on socket writers. Replace only unsent periodic snapshots
  in the bounded peer outbox, never confirmation snapshots or reliable messages.
  Encode snapshots only after dequeuing the selected frame; decode before client
  inbox coalescing. Confirmation snapshots stay full and precede Pong. Every TCP
  snapshot is independent of the peer's previously transmitted frames; only the
  acknowledged UDP baselines carry deltas, and their retained-baseline scheme is
  not a revision chain.
  `Server::bind` consumes the simulation.
  Per-peer codec state is transport-agnostic and clock-injected: `udp_codec.rs`
  takes an explicit `now` and never touches a socket or the wall clock, so the
  socket loops stay thin and the same codec runs under a scripted link in tests.
  Frame-facing debug requests must remain nonblocking and keep at most one
  dynamic geometry capture pending.
- **One world.** A client never builds a reduced physics world of its own.
  `Field::restore` rebuilds a whole `Field` from a `FieldSnapshot` plus the
  shared `StaticGeometry` and floor height, and prediction replays it through
  the ordinary `Field::step`, `command_chassis` and `fire` paths. Snapshots
  therefore carry the rules' hidden state (`FieldSnapshot::restore`); a decoder
  that drops it gives up restoring, not drawing. What a restore cannot carry is
  solver state: contact manifolds and warm starts are rebuilt. Rebuild only when
  the checkpoint changes, never once per frame.
- **Renderer is passive.** It applies caller-owned `SceneState` and never
  advances rules or reads the physics/world crates. Chassis ingestion resolves
  caller state into pose and light components before applying changes. Keep
  material swaps and transform writes conditional on changed values.

## Simulation rules

- **Coordinates.** The world uses metres in forward/left/up (FLU) with wxyz
  quaternions (`Pose`). Bevy is right/up/back. Convert only at the renderer
  boundary with `apply_pose`, `flu_position`, `flu_vector` and
  `pose_from_transform`. The playing floor top is at height zero. Red's
  half is +x and blue's is −x (the V1.2.0 CAD paints its red markings
  there); `layout::side_team`, `rune_team` and `outpost_team` derive
  ownership from that, and the render `TeamColor` follows it.
- **Time.** World time advances only through explicit ticks (`tick_ns()`,
  1 ms by default; `--physics-rate-hz` selects one of the offered 1000, 500,
  250 or 128 Hz rates, frozen once per process and shared by every peer in a
  match). Rule durations stay in nanoseconds, never tick counts. Never read
  host time in physics, world or gameplay code. Stepping must be deterministic
  and independent of how ticks are partitioned; there is a test for this.
  With no projectiles, chassis or referee, `Field::step` advances rune state
  directly to the target tick. A referee keeps the tick loop active even when
  physics is idle; active bodies also advance tick by tick.
- **Physics.** Projectiles are rapier dynamic balls with CCD, kept until the
  four-second flight limit, the arena bounds or the 64-ball cap removes them,
  or until `ProjectilePolicy` retires a spent one that has rested at or below 2 m/s
  on stationary scenery for 50 ms; armor housings
  are kinematic cuboids that follow the rule poses each tick; the ground is
  a static trimesh supplied by the server layout adapter from the manifest's
  visual GLBs, including sheets and markings. The rune and outpost contribute
  only their `static` nodes; moving targets remain rule-driven bodies.
  Bases, energy cores and extra scenery also use their visual triangles
  unless the asset explicitly declares `collision_method: source-tessellation-v1`.
  That contract selects a checksummed, independently tessellated mesh of the same
  source faces. Legacy unmarked proxies remain ignored. User-approved approximate collision
  meshes may replace exact tessellations when generated reproducibly by scripts
  here or in rm-map-tools. Preserve ramps, clearances and scoring behavior, and
  record provenance, deviation checks and placed triangle counts. A bare
  physical world has a catch half-space at `floor_height_m`, zero by default.
  The loaded CAD layout removes that plane and adds a perimeter boundary.
  Mesh contacts are two-sided.
  `Field::static_geometry` returns what the physics holds, for the app's
  wireframe. The chassis is a
  dynamic body with a cuboid for the chassis box and one for the turret,
  and four massless armor housings on its sides,
  whose wheels are ray casts with spring/damper suspension and ideal omni
  tyres; it steps in the same world as the projectiles, which score on the
  armor and bounce off the rest. `C` in the app draws the physics geometry as a
  wireframe of the client's presented scene. The debug view uses verified local
  geometry and sends no host capture requests. Raw contacts leave physics in
  order and are detected/scored by the world at the end of each tick.
  Scoring faces are `TargetFace` poses with +x the outward normal, +y width,
  +z height.
- **Ground.** The default (V1.2.0) floor slab is flat at height zero with
  the terrain plates standing on it. The V2.0.0 slab is crowned: about
  0.11 m below the reference height along the centre line, falling to about
  0.32 m below at the side walls, with height zero at the small level pads
  near (±12.8, ±3.0) m. Never assume a floor height; ask the collision mesh
  (`CollisionMesh::ground_height_below`). Both releases put a hex-marked
  truncated pyramid on the centre line at x ≈ ±6.3..8.5 m (1.5 × 1.0 m
  top, 0.15 m high, 17° ramps; the default spawn faces the red-side one).
  The 起伏路段 undulating road, a 2.4 × 2.1 m strip of waves about 70 mm
  high on the 0.2 m deck near each side wall (x ≈ 6.3..8.7, y ≈ 5.4..7.5 m
  and its point mirror), is modelled only in the V2.0.0 STEP; the default
  package carries those two V2.0.0 solids grafted onto its flat deck (the
  manifest's `grafted` entry), so both maps have it. The passages under the
  base highland decks have 0.65 m of clearance in both.

## Code rules

- **Units and naming.** Standard Rust naming with explicit physical units on
  public numeric fields and constants (`_m`, `_rad`, `_ns`, `_rad_s`,
  `_m_s`). Rulebook constants get a doc comment with the source.
- **Documentation.** `gameplay`, `physics`, `world`, `render` and `server` deny
  `missing_docs`, so every new public item there, including struct fields and
  enum variants, needs a doc comment. Say what the item does, its units and the
  invariant it keeps; cite the rulebook section or table a constant comes from.
  Add a runnable doctest when a caller can exercise the item without a GPU, a
  socket, a file or the wall clock, and hide setup behind `# ` lines. The `app`
  and `bench` binaries are not linted, but their module interfaces are
  documented the same way.
- **Licensing.** The workspace is dual-licensed `MIT OR Apache-2.0`. Every file
  starts with its SPDX identifier and the line
  `Copyright (c) 2026 hxyulin <hxyulin@proton.me>`. Keep provenance notices on
  copied code, and never assert this copyright over third-party artwork,
  recorded measurements or license texts. Dependencies must satisfy
  `deny.toml` (permissive licenses, plus the explicitly approved MPL-2.0
  exceptions for Flair's CSS dependencies). The MPL-2.0 source offer lives in
  `NOTICE.md` and is enforced by `scripts/check-mpl-compliance.py`.
- **Keep it small.** Avoid unsafe code and new dependencies unless the task
  needs them. Prefer extending an existing module over adding a crate.

## Workflow

- Follow the commit and PR convention in `CONTRIBUTING.md`: `type(scope): summary`,
  at most 72 characters, optional short body. Squash by default; rebase only
  independently passing commits. Keep main linear.
- Run `just verify` before opening a PR. It runs the hooks, formatting, check,
  clippy with warnings denied, all tests, the crate-boundary script, the MPL
  notice check and `cargo deny`. `just run <args>` runs the app; `just server
  <args>` the headless server; `just world-test` the fast rule tests while
  iterating. See `docs/development.md` for the full target list.
- `--screenshot PATH` renders the loaded scene to a PNG and exits; use it to
  check visuals without a display session. `--start-paused` and `F7` step the
  world by 16 ms for inspection.
- Record user-visible changes under `Unreleased` in `CHANGELOG.md` and keep
  the README option and control tables current.
- Tests live beside the code. Run `cargo test -p rm-simulator-physics --locked`
  for dynamics, geometry and raw-contact tests. World tests build a `Field` from a
  `FieldConfig` and step it; render tests run a headless `App` with the sync
  plugin and inspect components; server tests exercise the protocol, a
  loopback TCP server and the HTTP routes; app tests cover argument
  parsing, frame conversions, the flashes, the HUD line and a deterministic
  end-to-end network trace (`net_harness.rs`) that runs a real host and a real
  session over `scripted_link.rs` on a hand-advanced clock.
