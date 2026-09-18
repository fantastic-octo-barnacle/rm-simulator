<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Changelog

## Unreleased

- Show the 5 s countdown's own seconds on the HUD clock and the referee panel
  instead of the round clock; the referee view derives them from the phase
  start it already carries, so the wire format is unchanged. An overheated
  or locked barrel no longer begins local shots, so refused shots no longer
  appear as provisional balls; the client also holds a shot while the last
  snapshot's heat plus unconfirmed shots is past the limit.

- Draw barrel heat as a ring around the reticle, filled clockwise by a UI
  shader, and stop Auto Fire before a shot would pass the heat limit (manual
  fire can still overheat). A Fortress banner counts down to the Fortress
  opening at 3:00 and to the base opening during a capture, and team cards
  mark an expanded base `OPEN`.

- Detect the section 5.5.3 buff points on the loaded arena (protocol 45).
  Card areas read off Figure 5-24 report occupation to the gameplay engine,
  which now applies base, trapezoid, central highland, outpost and Fortress
  defense, resupply healing and respawn acceleration, the Road, Elevated
  Ground, Launch Ramp and Tunnel crossing buffs, the Fortress cooling bonus
  and reserve, and the 20 s Fortress capture that expands the opponent's
  Base Protective Armor and stops the capturer's outpost rotor. The outpost
  rebuild uses the Outpost Buff Point instead of a 1.5 m circle, and a
  destroyed outpost no longer opens its base armor. The HUD shows a ZONE
  line and the panel logs `TerrainCrossing` and `BaseArmorExpanded`.

- Run the remaining RMUC 2026 V2.1.0 match rules live (protocol 44). The world
  referee now runs the `rm-simulator-gameplay` engine as its rules authority in
  place of the resource tracker: robot damage follows Table 5-2 (20 HP per
  17 mm, 200 per 42 mm) with attack, defense and cooling buffs all applied;
  robots gain experience and levels with Hero and Infantry performance types
  (`--performance`); heat and allowance count in every match, with allowance
  enforcement a panel toggle; defeated robots respawn in place on the section
  5.2.2 timer, weakened and invincible, with paid instant respawn; armor
  collisions cost 2 HP; the outpost rotor spins up at the round start and
  stops at its first destruction or 3:00; a robot standing in its team's
  destroyed outpost zone rebuilds it; and a round ends with a result the
  referee can adjudicate when the rules leave it open. Idle stays free
  practice, and pilots can no longer revive themselves during a match. O
  now buys ten 17 mm rounds. The referee panel replaces its economy settings
  with the rule policy, per-robot performance, instant respawn, weakness and
  result controls; `Gameplay` referee commands are replaced by `SetGold`,
  `SetAllowance`, `SetPolicy`, `SetPerformance`, `BuyAmmo`, `Adjudicate`,
  `InstantRespawn` and `ClearWeakened`. The HUD shows level, heat, weakness,
  invincibility, respawn progress and the round result.

- Remove legacy protocol surface (protocol 43). `ShotScheduled` no longer
  carries an intended time; `Fire` and `FireAimed` no longer carry client
  timing diagnostics, so `GET /api/fire-records` drops its client timing and
  observed poses. A checkpoint's rule state always travels as the restore: a
  state without a restore matching its views is refused instead of sent
  explicitly. Hosts no longer accept the older `RMI2` input batch, and JSON
  decoding no longer fills in fields that predate protocol 38. Network traces
  now classify the current `RMO6` owner anchors and `RMA2` baseline feedback,
  which were counted as control. Owner anchor command and tyre speeds now use the documented 1 cm/s
  velocity scale (±327 m/s) instead of millimetre steps, which limited them to
  ±32.767 m/s and silently dropped anchors beyond that. The protocol 41 dictionary is kept; in
  `network_bandwidth` sent bytes change by under 0.2% with players (+44 bytes
  over four seconds with two) and +1.2% idle.

- Predict checkpoint baselines before differencing them (protocol 42). A
  delta is now coded against its pinned baseline dead-reckoned to the frame's
  tick: chassis and turret positions advance by the body velocity, rotations
  integrate the body and gimbal rates, wheels spin at their commanded speed and
  balls fly ballistically until their first contact, all in integer grid steps
  or basic IEEE 754 operations so host and client predict bit for bit. The tick
  lead rides in the delta header. A changed sequence now realigns on its
  elements' ids, so retired and newly fired projectiles no longer resend the
  whole projectile array, and a baseline may rotate after 12 frames instead of
  32. On the remote-cadence bandwidth probe, downstream bytes fall 29% firing
  and 24% with twelve chassis, stay flat driving and rise 1% idle (more
  baseline rotations); in `network_bandwidth`, sent bytes fall 21% with two
  players, 47% with twelve and 45% firing and rise 3% idle. Host encode costs
  up to about 9 µs more per frame and client decode up to 12 µs. The owner anchor is
  unchanged.

- Lay checkpoints out by change rate (protocol 41). Slow records (the header,
  shot results, hits, bases, rule state and each chassis' identity,
  configuration and command) come first, each byte-aligned so the dictionary
  matches them frame after frame; chassis motion and projectiles follow as
  dense bits. A chassis configuration equal to the Infantry or Hero preset
  travels as a one- or three-bit index instead of about 123 packed bytes, and
  the default projectile policy as one bit; anything else still travels
  exactly, so an independent checkpoint needs no earlier frame. Deltas under
  128 bytes are no longer offered to ZSTD, which never shrank them. The
  dictionary is retrained on independent frames and compressible deltas only.
  In `network_bandwidth`, independent checkpoint bytes fall 23% idle, 28% with
  two players, 23% with twelve and 12% firing (now below protocol 39 in every
  workload); sent bytes fall 2%, 5%, 3% and 2%.

- Tighten checkpoint float coding (protocol 40). The field path now implies
  each fixed-point grid, so no grid index travels; a delta codes the step
  difference as an exponential-Golomb number instead of a 6-bit width and raw
  bits; chassis pose and turret rotations are sent smallest-three (15-bit
  components, at most 0.13 mrad of rotation error). The owner anchor (`RMO6`)
  packs its quaternions the same way, 202 to 198 bytes. The dictionary is
  retrained. Sent checkpoint bytes in `network_bandwidth` fall 31% with two
  players, 40% with twelve and 13% firing; idle rises 2% (3,294 to 3,367).

- Drop derived data from UDP checkpoints (protocol 39). The field clock, the
  rune, outpost and referee views, wheel hubs and tyre targets are rebuilt on
  decode from the tick, the restore's rule state and each chassis' pose,
  configuration and command; only wheel spin travels per wheel. Every
  absolute timestamp (rune, Big Rune epoch and first-hit window, referee and
  buff times, outpost stop, detections, hits, shot results, projectiles)
  becomes a whole-tick code with an exact sub-tick remainder, so a restored
  Big Rune predicts identically. The dictionary is retrained. Sent checkpoint
  bytes fall 86% idle, 57% with two players, 44% with twelve and 31% firing
  in `network_bandwidth`.

- Remove JSON from the gameplay wire (protocol 38). Checkpoints are packed
  straight from the typed state by a positional bitpack (`RMB1`) that rounds
  fixed-point fields while serializing, with no JSON value tree, key strings
  or type tags. Control messages, acknowledgements and retirements use the
  same codec. The dictionary is retrained on the new frames. Host encode is
  about 5–7× faster and client decode 10–20× faster. Sent bytes fall 7–13%
  for idle, driving and firing workloads but rise 13% with twelve players.
  HTTP, lobby, traces and saved preferences stay JSON.

- Send small checkpoint frames uncompressed. The binary checkpoint compressor
  now returns a packed `RMB0` frame unchanged when dictionary compression
  would not shrink it, instead of emitting a larger `RMBZ` frame; the decoder
  already treats a bare `RMB0` frame as inflated, so the wire format is
  unchanged. The bandwidth probe reports these as `raw_fallback_frames`, and
  its section ablation now measures the live binary pipeline (fine fixed
  point, bitpack, dictionary) with packed and compressed shares per section.

- Compress packed checkpoints at ZSTD level 6 instead of 3. Independent
  frames measure 10–18% smaller for ~10–35 µs of host encode each against
  the 89–374 µs bitpack encode beside it, and about 2.5–3% on the live
  mixed delta stream; decompression is level-independent, so clients pay
  nothing. The level is not on the wire — ZSTD decodes any level against
  the same dictionary — so no protocol bump was needed. A new
  `net_codec` Criterion benchmark covers quantize/bitpack, compression
  across levels, decompression, bitpack decode and the full
  acknowledged-baseline round trip.

- Round projectile checkpoints to 1 mm and 1 mm/s instead of 0.01 mm and
  0.0001 m/s. Both fit the bitpack 18-bit grids, cutting the projectile
  share about 10% and the live firing stream about 6%; a new replay test
  restores balls from a compact checkpoint and holds four seconds of
  flight within 5 cm. No wire change: values stay f64, only coarser.

- Carry projectile timestamps as checkpoint-relative ages in nanoseconds
  instead of absolute simulation times (protocol 37). Ages stay under 2^33
  for a four-second ball; reconstruction is exact, so replays retire the
  same balls on the same ticks.

- Quantize the owner anchor (`RMO5`, protocol 37): millimetre positions,
  1/32767 quaternions, centimetre-per-second velocities,
  milliradian-per-second rates and 0.1 mrad aims (32-bit, since aims and
  wheel roll rotate without bound). The anchor drops from 444 bytes toward
  202, cutting the fixed owner floor from 111 kbps to about 50 kbps;
  out-of-range or non-finite dynamics fail at encode time so the size
  stays fixed. Combined with the checkpoint work above, the probe's remote
  workloads measure 87/114/287/299 kbps for idle/drive/fire/twelve, down
  from 151/178/412/422 kbps.

- Log per-peer network health on the server. The GNS host prints one `net
  peer=ID world=N skipped_congested=N replaced_unsent=N ...` line per admitted
  peer every 10 s, and appends snapshot totals to the disconnect line, so a
  peer starving its client's auto-aim observation is visible in the server log
  without a packet trace. The auto-aim HUD row now shows the observation age
  in ms, and the detailed network overlay breaks down the firing gate
  (`stale` / `tracking-only` / `blocked` / `firing` frame counters).

- Judge auto-aim freshness at presentation instead of at intended execution.
  The 150 ms tracking-only gate and the 300 ms stale gate used the age at
  `fire_time_ns`, which includes the input lead (rtt/2 + 32 ms, up to 150 ms):
  on a link with ~95 ms RTT the lead alone consumed most of the firing budget
  and the assist tracked without ever firing, even with checkpoints arriving
  every ~14 ms. Both gates now use the presentation-time snapshot age; the
  ballistic solver still predicts ahead to the execution time.

- Unify singleplayer and multiplayer on one wire. The embedded owner no longer
  uses a typed in-process channel: `Server::connect_owner` now drives the same
  per-peer codec a UDP client runs — the RMG1 fragment framing, the packed `RMB0`
  checkpoints and their acknowledged-baseline delta rotation, the RMI3 input
  batches, the RMO4 owner anchor and configuration handshake, and the byte pacer
  — over two in-process datagram channels, with compression skipped (`RMRW`
  frames instead of `RMZ1`/`RMBZ`). The owner still registers its spawn directly
  and keeps owner authority, and the client still reports transport `local`.
  `PROTOCOL_VERSION` is unchanged: the wire format did not change, only which
  transport carries it. Frame timings and cadence are unchanged.

- Remove the selectable physics rate. The `--physics-rate-hz` option and the
  `RM_SIM_TICK_NS` environment override are gone; every build, host and client
  now runs the fixed 128 Hz tick (7.8125 ms per tick), so `Hello` and `Welcome`
  no longer carry a tick length and a host no longer refuses a rate mismatch.
  The
  `physics_rate` measurement example and the `physics_rate_hz` layout option go
  with them. `PROTOCOL_VERSION` is 36, so both peers must update.

- Remove the TCP gameplay transport. Valve GameNetworkingSockets over UDP is now
  the only network transport, alongside the in-process owner link and the HTTP
  referee panel, so the shared `--transport` option is gone from both binaries and
  a host or guest that passed it must drop it. The JSON-lines wire helpers, the
  TCP client and server loops and the lobby's `tcp` advertisement are removed;
  discovery still reports `"transport": "gns"`, and a listing that names any other
  transport is refused. `Server::bind_udp` and `Server::bind_udp_suspended` are
  now `Server::bind` and `Server::bind_suspended`. `PROTOCOL_VERSION` is unchanged.

- Stop carrying the field package's pure build reports in the repository. The
  upstream zip's `build-provenance.json`, `deployment.json`,
  `mesh-simplification.json`, `road-marking-composition.json`,
  `asset-refresh.json`, `asset-refresh-simplify.json` and
  `ground-validation.json` are read by no simulator code, script or manifest
  descriptor, so they are listed under `excluded_build_reports` in
  `scripts/release-field.json` instead of being tracked through Git LFS. The
  runtime manifests, both `articulation.json` sidecars, `field-detail-build.json`
  and the minimap stay.

- Remove the wire-development selectors. Periodic checkpoints are always packed
  fine fixed point with the embedded trained dictionary, and every other message
  is always plain ZSTD; the DEFLATE codec, the JSON checkpoint path and the
  `RM_NET_CODEC`, `RM_NET_SNAPSHOT`, `RM_NET_DEFLATE_LEVEL` and
  `RM_NET_ZSTD_LEVEL` environment selectors are gone, along with the miniz_oxide
  dependency, the second dictionary asset and the three examples that existed
  only to compare the removed alternatives. The pacing, input-lead,
  input-redundancy and checkpoint-completeness overrides (`RM_NET_PROFILE`,
  `RM_NET_FIXED_INPUT_LEAD`, `RM_NET_INPUT_HISTORY`,
  `RM_NET_FULL_CHECKPOINTS`) are gone too, so the adaptive lead and the twelve
  redundant input frames are the only behaviour; `RM_NET_UP_KIB_S` and
  `RM_NET_DOWN_KIB_S` remain for impairment trials. Periodic checkpoints are now
  protocol 35, so both peers must update.

- Add a robot page to the title screen: Single Player, Join lobby / address
  and Create lobby open it before the match starts, with blue seats on the
  left, red on the right (Hero, Infantry 3, Infantry 4 or a spectating
  camera) and the referee seat below. The robot is remembered; Enter confirms
  and Escape goes back. The team and spectator checkboxes are gone.

- Let every pilot name its robot (protocol 33): `Hello` and the chassis
  assignment carry it, the roster announces it, and the host builds that
  chassis and fixes its caliber, 42 mm for the mecanum Hero and 17 mm for the
  omni infantries; a weapon update naming another caliber is refused. The
  two infantries differ only in the painted number, which every client shows
  and repaints once the roster arrives. `--robot` takes
  `hero|infantry-3|infantry-4` (`infantry` still means the Infantry 3), works
  with `--connect`, and `--projectile-mm` is gone from the app, the server
  and the host weapon settings. The server's `--robot` is gone too. Both
  peers must update.

- Share the host command line between the two binaries. The field, rune, weapon
  and chassis options now live in `rm_simulator_server::host_args::HostArgs`,
  which the app and the headless host both flatten, so a new option is declared
  once and the two cannot disagree about a default. Every option keeps its name,
  default and accepted values; `rm-simulator-server` now also accepts a bare
  `--listen`/`--http` (both resolve to the same defaults as before), and the app's
  `--connect` refuses every shared host option except `--robot`, which names the
  pilot rather than the host and travels in `Hello`.

- Remove the `ShotFinished` message, which no host ever produced: a shot's end is
  reported as a `ShotResult`. The client loop that read it could never iterate.
  The wire is now protocol 34, so both peers must update.

- Remove the stale `scripts/capture-rune-reference.py`, which still handshook
  with protocol 6. `just verify` now runs the same Python commands as CI (every
  module under `scripts/tests`, two of which its pattern list skipped) plus the
  field package inventory check; `just field-check` runs that check alone.

- Fix the 150 % centre-square bonus to round the same way on both scoring paths:
  a base plate and an outpost implemented the same rule with different rounding,
  so the odd Table 5-2 value (17 mm on a base's upper front) answered 8 through
  the base path and 7 through the outpost path. Both now round up.

- Read base cover from the field's outposts rather than the referee's copy of
  their destruction, so a base is immune exactly while a tower of its team still
  stands. The referee keeps that copy only to avoid repeating an
  `OutpostDestroyed` event.

- Derive the drawn muzzle offset from the physics barrel length, and share the
  gimbal pitch range and the provisional-ball id bit between the aim assist, the
  drawn shots and the shot retirement path, so each value has one definition.

- Move the deterministic bandwidth workload into `rm_simulator_server::workload`,
  used by the probe and all four measurement examples; the five copies of the
  field builder had to be edited together to stay comparable.

- Reject binary checkpoints that exceed the decoder’s traversal or frame-size
  limits before transmission.

- Default periodic UDP world checkpoints to bitpacked fine fixed point and the
  newly trained binary ZSTD dictionary (protocol 32). Retain acknowledged
  baseline recovery, exact confirmations, owner anchors and input commands.
  `RM_NET_SNAPSHOT=json` selects the previous checkpoint path for comparison.
  Both peers must update; the precision assumptions and the interactive driving
  test are recorded in `docs/bandwidth-experiments.md`.

- Add a standalone `binary_protocol_comparison` experiment comparing current
  compressed JSON checkpoints with lossless binary deltas, bitpacking and
  fixed-point motion fields. It checks decoding and short physics replays,
  including finer projectile precision and a chassis-only variant, and trains
  separate binary dictionaries on separate training workloads. The experiment is
  standalone; Protocol 32 checkpoint behavior is described above.

- Add a selectable wire compression codec: DEFLATE stays the production default,
  and `RM_NET_CODEC=zstd` or `zstd-dict` selects ZSTD, the latter with a trained
  checkpoint dictionary embedded in the binary. Frames are self-identifying
  (`RMZ1`/`RMZ2`), so a decoder reads either codec and the DEFLATE wire is
  unchanged. `RM_NET_DEFLATE_LEVEL` and `RM_NET_ZSTD_LEVEL` set the effort.
  Compare the candidates with `cargo run --release --locked -p
  rm-simulator-server --example compression_comparison`; the dictionary is
  regenerated by the `train_checkpoint_dictionary` example. Measured on the
  canonical probe, the dictionary cuts the selected checkpoint stream 30–58%
  and the full remote downstream 21–27% against DEFLATE, while DEFLATE stays the
  default; see `docs/bandwidth-experiments.md`.

- Remove the committed `benchmarks/` configuration and result files and the
  dated investigation, experiment-result and implementation records under
  `docs/`. Their measurements remain in Git history and in the pull requests
  that produced them; `docs/README.md` and `docs/bandwidth-experiments.md`
  remain the index and the surviving experiment summary. Protocol 32's
  `binary_snapshot` test now compares its embedded dictionary against
  `crates/rm-simulator-server/assets/binary-fixed-fine.zstd`, the dictionary
  that was already tracked there, instead of a copy under `docs/`. Document the
  roles of `assets/`, `crates/rm-simulator-server/assets/`, `local-assets/` and
  `~/dev/RM/assets/` in `AGENTS.md`.

- Rewrite the documentation. `README.md` is now a structured front door with a
  table of contents: the command-line reference, weapon settings, HUD and
  controls move to the new `docs/app-options.md`, the field package material to
  the new `docs/field-package.md`, and the developer workflow to the new
  `docs/development.md`. `AGENTS.md` gains a documentation map and separates
  architecture, simulation, and code rules. Every crate gains a `README.md`
  describing what it owns, its module layout, its permitted dependencies and how
  to test it. No command, option, default, citation or measurement was dropped;
  the README's stale claim that protocol 29 is current is corrected to 32, which
  is what `protocol.rs` defines.

- Drop the two dangling `benchmarks/` references from `field/CAD-NOTICE.md`,
  which described audit images and a whole-file conversion report that are not in
  this repository. The notice is checksum-pinned in `scripts/release-field.json`,
  so its recorded SHA-256 is updated with it; `stage-release-field.py
  --verify-only` passes, and the upstream import URL and archive hash are
  unchanged.

- Add `--physics-rate-hz` to the app and the headless server, offering the
  measured 1000, 500, 250 and 128 Hz rates (128 Hz is exactly 7,812,500 ns per
  tick) and refusing any other value. The title screen remembers it, the HUD
  shows the active rate, and a host refuses a client that predicts at another
  rate or states none. The paused `F7` step covers the first tick boundary at
  or after 16 ms, so it is never shorter than 16 ms at a reduced rate.

- Remove spent projectiles: a ball that has stayed at or below 2 m/s for 50 ms while
  resting on stationary scenery is retired instead of rolling until the
  four-second flight limit. A ball in free flight, or one that has not touched
  anything yet, is never retired, and a ball that is struck or pushed again
  speeds up and keeps its place. Measured over 60 s of sustained fire on the
  CAD field at a 20 Hz aggregate launch rate, this cuts the mean number of live
  balls by 34%, physics CPU per simulated second by 31% and projectile snapshot
  traffic by 35%. Restitution stays 0.45 and the flight limit, arena bounds and
  64-ball cap are unchanged. `--no-projectile-retirement` restores the earlier
  behaviour on the app and the server binary.

- Skip malformed optional network trace metrics before aggregation so diagnostic
  summaries still report valid rows and flag incomplete traces.

- Fix automation capture after focus loss, GNS link transport labels and completed
  world queue ages. Drain network trace writers on shutdown and preserve diagnostic
  reports when event histories reset or trace rows omit required fields.

- Add a reproducible selected-checkpoint deflate sweep and five-seed cadence/
  capacity trial scenarios, including a progressive manual networking profile.

- Classify protocol 29 owner anchors and input batches correctly in network
  tracing; their new wire tags previously appeared as control traffic.

- Measure individual shot confirmations, probe RTTs and complete-checkpoint
  arrival intervals with bounded event histories. Avoid repeated last-value
  latency samples and report overwritten measurements in network trials.

- Reduce UDP bandwidth with acknowledged owner configuration references and
  lossless input batch compaction. Preserve update cadence, input redundancy and
  full-precision dynamics; require matching protocol 29 builds.
- Add a deterministic production-codec bandwidth probe and record the isolated
  bandwidth experiments, their measured savings and outstanding acceptance work.

- Replace embedded owner TCP with bounded typed channels and avoid opening a
  gameplay listener in standalone play. Keep host ordering and authority intact.
- Add opt-in bounded networking traces, local traffic/work counters, host encoding
  diagnostics, trace status in the detailed overlay and an offline summary tool.
- Add a pinned Nix development shell and direnv integration for native build
  dependencies, with a separate `target/nix` build directory.

- Stream scored contacts during simulation advancement so long manual steps
  cannot discard reliable hit feedback when snapshot history expires.

- Disable incompatible lobby rows and show actionable version mismatch errors
  when connecting directly, including to the first release.

- Deliver confirmed armor-hit feedback independently of replaceable world
  snapshots and time each flash from receipt. Require matching protocol 27 builds.
- Sample current aim for every shot, submit prediction after controls, and acquire
  auto-aim targets from displayed poses. Report why auto-fire waits and stop it
  on stale target observations.
- Raise the default LAN allowances to 512 KiB/s downstream and 64 KiB/s
  upstream per peer, retain the
  40 KiB/s `limited` profile, bound packet bypass to preserve world progress,
  expose host queue diagnostics, and recover interpolation delay faster.
- Repair console capture across OS focus transitions and make the direct network
  trial fail unless the robot moves and rounds actually launch.

- Keep closed-PR metadata checks from cancelling main CI, and explicitly fetch
  PR commits before validating their messages after branch updates.

- Fix intermittent native release validation failures by waiting for deferred
  UDP socket cleanup, isolating lobby tests from process-wide packet impairment,
  and giving the TCP backpressure test a separate deadline after admission.

- Drop Intel macOS from native validation and release packages. Supported release
  targets are Windows x64, Linux x64 and macOS Apple Silicon.

- Unblock standalone Windows release validation by testing without the optional
  Steam feature, whose native library conflicts with static GameNetworkingSockets
  on MSVC. Keep an all-feature compile check on Windows and all-feature tests on
  Linux and macOS; Steam-enabled Windows linking remains unsupported.

- Version the approved runtime field in Git LFS, preserving the upstream CAD
  ownership notice and import provenance. Check file hashes and LFS pointers in
  lightweight CI, hydrate the package for native tests and releases, and stage
  it locally instead of downloading an rm-map-tools release. Checkouts can load
  `field/` directly after `git lfs pull`.

- Keep push and PR CI to formatting, repository policy and Python tests. Add
  manual four-platform validation reused by exact commit SHA for releases,
  release dry runs, and an opt-in main-branch PR ruleset. Share release-profile
  caches and remove rebuildable Windows vcpkg downloads and Git history from
  them. Local development keeps workspace debug information while omitting
  third-party debug information.

- Keep LAN discovery running when a probe reaches a closed UDP port, including
  Windows reporting a connection reset when no local lobby is advertised.

- Retry transient field-package download failures before compiling release
  packages. Split package compilation from testing and packaging, saving complete build
  caches before tests run so test failures preserve the Windows and Intel macOS
  build artifacts.

- Fix the console input test on Windows by checking body-relative sideways
  motion with a tolerance instead of the sign of a near-zero forward component.

- Keep network-harness readiness checks on loopback even when an HTTP proxy is
  configured. Fence prediction in deterministic tests so coverage does not depend
  on thread scheduling.

- Add Windows CI coverage and native ZIP packaging for Windows x64, Linux x64,
  macOS Apple Silicon and macOS Intel. Add a manual release workflow with a
  version, stable/alpha/beta/RC channel, and prerelease number. Packages include
  the app, server, runtime libraries, notices, checksums, and a pinned field
  package published separately in rm-map-tools.

- Add in-game weapon settings for each pilot: firing rate, muzzle speed,
  spread angle, uniform or Gaussian dispersion, and a repeatable spread seed.
  Starting values are 20 Hz, 25 m/s, +/-0.3 m/s speed variation and 0.3 degrees
  of spread. Host limits remain 30 Hz and 30 m/s for either caliber. The HUD
  displays the last bullet's actual server-confirmed launch speed. The in-game
  spread slider covers 0..2 degrees in 0.01-degree steps. Gaussian is the
  default angular distribution. A 0..1 m/s speed-variation slider selects a
  maximum +/- offset with a truncated Gaussian distribution inside host limits.
  Hosts choose speed and rate limits, caliber, and default spread from remembered
  title-screen fields or CLI options. Settings updates are reliable and affect
  subsequent launches without changing bullets already in flight. Protocol 26
  adds the per-pilot settings command.

- Add Git and editor defaults, ignore local credentials and generated caches,
  and provide GitHub issue/PR templates and a private security reporting policy.

- Default Windows direct joins and the optional lobby directory to localhost.
  Keep the remembered Windows server address out of version control.

- Relicense the workspace under `MIT OR Apache-2.0`. Every source file now
  starts with its SPDX identifier and the copyright line
  `Copyright (c) 2026 hxyulin <hxyulin@proton.me>`; add `LICENSE-MIT` and
  `LICENSE-APACHE` and remove the proprietary license. The `rm-vision-sim`
  provenance notice stays, and third-party artwork and recorded measurements
  keep their own terms.

- Record MPL-2.0 compliance for the five CSS crates Bevy Flair pulls in: the
  notice now lists each component at its locked version with the immutable
  crates.io source archive, ship the full text at `LICENSES/MPL-2.0.txt`, and
  add `scripts/check-mpl-compliance.py` to `just verify` so the notice,
  `deny.toml` and the resolved graph cannot drift apart.

- Document the public API and add runnable doctests to the pure crates.

- Split the main menu into Single Player and Multiplayer. Add LAN lobby
  discovery, named hosts, optional host-enforced passwords, direct joining,
  and a dismissible firewall tip. Add page and lobby-list scrolling, with
  stacked columns in narrow windows. Public lobbies remain disabled. Include an
  undeployed Docker directory service for future use. Protocol is now 25;
  clients and hosts must use the same build.

- Update architecture, asset discovery, controls and live-rule documentation;
  distinguish current networking behavior from historical plans and measurements.

- Extract `rm-simulator-physics` for chassis dynamics, ballistics, shared collision
  geometry and armor motion without gameplay, Bevy or server dependencies.
  Keep the complete `Field` facade and serialized checkpoints compatible.
  Add headless moving-armor and optional Bevy composition examples.
- Resolve chassis poses and lights into ECS components once per frame, applying
  only changed values. Group session prediction state and separate contact
  detection from damage handling. Record reproducible pre-refactor CPU baselines.

- Add spinning training bots through the singleplayer Pause menu and referee
  panel, with team/spin selection, removal, ordinary robot physics and HP.
- Make all seven base plates score projectile hits, including the moving dart
  plate as a training override. Add base HP/shield, match-time outpost protection,
  destruction, referee HP controls, armor feedback and auto-aim targeting.
  Fit scoring frames from the verified map at startup without changing meshes.
  Network protocol is now 24; hosts and clients must use the same build.

- Add separately rebindable Auto Aim and Auto Fire actions, sharing right mouse
  by default. Aim near a robot, outpost, or active rune to track its predicted
  scoring face with gravity/drag compensation and the existing motor response.
  Automatic fire waits for actual barrel alignment, impact speed, and clearance;
  releasing the button or opening a menu cancels it. Controls can unbind Auto Fire
  for aim-only use. Rune targeting uses the visible scoring face and own-team
  color, with a slower cadence and wait for blade confirmation between shots.
  If no target is near the crosshair, fall back to the closest visible enemy
  robot within 40 m. Ordinary host fire checks remain in effect.

- Replace aggregate team HP with mirrored per-robot health slots following the
  July 2026 competitor UI manual. Dim empty slots, distinguish defeated robots,
  and show extra slots for duplicate robot types in multiplayer test matches.
- Tab now lists both teams and spectators, including robots whose roster names
  have not arrived yet. Hide the hold-to-peek panel while Pause is open, and let
  the mouse wheel scroll long lists while the game has captured the cursor.

- Make the driving camera follow the visible gimbal tilt and roll when the
  chassis tips or flips upside down, while preserving its barrel aim.

- Escape opens a Pause menu with Resume, Settings, and Exit Match. Pause only
  singleplayer, preserve existing pauses, and return from Settings to Pause.
  Escape and Quit on the title screen now require an explicit quit confirmation.

- Keep bloom enabled on Low to preserve bright armor and target colors. Warn
  when bloom is manually disabled, since it affects more than surrounding glow.

- Warn prominently that custom light emission can alter armor and target colors
  or hide gameplay indicators. Low continues to retain full material emission.

- Reduce default shadow cascades from two to one on Medium/High and from four
  to two on Ultra, following sustained-load NVIDIA rendering tests. Keep shadow
  resolution, distance, bloom and MSAA; custom overrides still take precedence.

- Add hover and keyboard-focus help for every graphics setting, explaining its
  visual effect and performance tradeoff.
- Reject rendering benchmark runs with resized targets or dropped samples,
  enlarge the nonblocking timestamp readback pool for offscreen captures, and
  add a reproducible sweep of individual rendering effects at 1080p and 4K.

- Add a separate rendering benchmark with fixed cameras, shared graphics presets,
  GPU timestamp queries, CPU frame distributions, configurable raw/stage output,
  screenshots and reproducible settings sweeps. Extreme box/sparse geometry modes
  isolate rendering experiments from simulation and saved player preferences.

- Add a coarser V1.2/V2-road visual candidate with 355,661 placed triangles,
  retaining standard collision detail and the protected terrain and artwork.

- Add a reproducible, separate V2 terrain preview with V1.2 horizontal paint
  and atlas artwork projected onto its surface. The installed map is unchanged.

- Persist keyboard/mouse bindings, sensitivity, mouse inversion and display
  preferences across launches. Settings are available from the title screen and
  in matches, with conflict handling, capture cancellation and binding resets.
- Add Low, Medium, High and Ultra graphics presets with saved per-option overrides.
  Shadows, antialiasing, bloom, exposure, generated-light emission, projectile
  detail, VSync and optional depth prepass/occlusion culling update live. Unsupported
  GPU settings fall back while preserving the requested values.
- Add a reproducible standard-detail field generator, validation probe and
  reversible installer. The installed package has 497,886 visual and 312,749
  collision triangles including mechanisms. Terrain and markings remain exact;
  the collision budget is still above the 100k target.
- Add optional Steam client initialization and callbacks behind `--features steam`,
  with development runtime staging and SDK compatibility checks. The local
  SDK 1.65 stays ignored; the wrapper uses its matching bundled runtime.

- Gimbal motors now follow mouse targets on the 1 ms physics clock with speed
  and acceleration limits. Actual aim drives the camera, visible barrel and
  authoritative muzzle, and motor state survives prediction restore. Protocol
  23 adds motor rates to owner anchors; update hosts and clients together.
- Chassis drives share an assumed motor power budget including stall losses;
  suspension gains progressive bump stops and robot contacts have less rebound.
  Arena perimeter walls contain robots while absorbing projectiles.
- Cache conservative shape bounds before camera-clearance intersections. Add
  a loaded-CAD, concurrent-match CPU probe reporting median and tail step costs,
  snapshot/restore costs and batches that exceed their simulation-time budget.
- Make mouse aiming more responsive with a 25 ms gimbal motor response,
  12 rad/s speed limit and 240 rad/s² acceleration limit; retain physical
  motor motion and the existing mouse sensitivity.

- The title screen uses a darkened game screenshot without the gameplay HUD.
  Team and spectator checkboxes now visibly toggle both on and off.
- A dark low-poly stadium replaces the outdoor sky. Open wire mesh fencing
  and posts now mark the existing two-metre arena collision walls. Field
  lighting is slightly dimmer while preserving the CAD paint colours. The
  stadium stands and walls sit well back from the field in a larger hall;
  the fence and colliders follow grounded CAD scenery rather than the outer
  edge of the floor slab apron.

- The app opens on a title screen with a name, a connect address, a host
  address, team and spectator choices, and Connect, Host, Practice and Quit;
  the fields are remembered between launches. A lost connection, a host
  failure or a failed join now ends the match and returns to the title screen
  with the reason instead of closing the window; Escape with the cursor
  released and the toolbar's Leave match do the same on purpose. `--play`,
  `--connect`, `--listen`, `--screenshot` and `--console` still enter a match
  at once. The CAD package and scenery are kept across matches.
- A client no longer runs reduced physics worlds of its own. `Field::restore`
  rebuilds a whole field from a snapshot and the client's verified collision
  geometry, and both own-chassis reconciliation and provisional shots now replay
  through the ordinary `Field::step`, `command_chassis` and `fire` paths. Local
  movement is therefore predicted against the same rules, terrain, armor and
  moving equipment the host runs, and a provisional ball is corrected simply by
  the host's own ball arriving in the next snapshot. Measured 64 ms corrections
  on flat ground and on a ramp fell from millimetres to exactly zero.
- Snapshots now carry the rule state an observer does not need (rune activation
  progress and its seeded stream, outpost rotor stops, referee schedule and
  configuration, armor detection intervals, and the chassis id counter) so a
  client can rebuild the field from one. Projectiles carry the chassis credited
  with the shot.
- Protocol 22 removes the networking layers that no client used. The shooter-view
  compensated fire path is gone (`CompensatedFire`, `ShotView` and `InputEvidence`
  commands, the host's per-peer view archive and shot admission worker, and the
  `--max-compensation-ms` option on both binaries); firing stays server-authoritative
  from independent physics. The ordered TCP snapshot delta chain is gone, so the
  TCP transport now sends plain compact snapshots; acknowledged UDP baselines are
  unchanged. Input acknowledgements, which no client read, are dropped from
  snapshots and owner anchors. The unused `--predict` compatibility flag is removed;
  prediction is on by default and `--no-prediction` still turns it off.
- Fix a lost `Retired` answer pinning a UDP baseline forever and blocking new ones:
  an unanswered `Retire` is now resent.
- Fix owner anchors being dropped whenever a UDP peer's send backlog caused the
  world checkpoint to be skipped; the anchor is now queued first.
- Make UDP peer service order and snapshot shot-result order deterministic, so
  identical worlds produce identical bytes.
- Log the reason a UDP peer is disconnected instead of discarding it.
- Send pilot input on the documented 16 ms grid instead of once per rendered
  frame: a control transition still leaves immediately, while aim-only motion is
  coalesced and the frame at the grid boundary carries the newest aim. A 240 fps
  client now sends about 60 input frames a second instead of 240.
- Size the remote motion buffer's jitter window by time (two seconds) rather
  than by a fixed number of frames, so its delay estimate no longer depends on
  the client's frame rate.
- Host pacing and client session timing now read the wall clock through an
  injectable source, so tests can drive them with a clock they advance by hand.
- Split the per-peer UDP codec out of the socket reactor (`udp_codec.rs`), so the
  framing, reassembly, delta coding and pacing that a peer performs run against an
  explicit clock with no socket. The wire format is unchanged.
- Add a deterministic network test harness: a scripted datagram link with
  per-direction loss, reordering, duplication, delay and blackout rules
  (`scripted_link.rs`), and an end-to-end trace that drives the real host and a
  real client session across it on a hand-advanced clock, with no sockets,
  threads or sleeps. A seed now replays a whole impaired session exactly.
- Fix client message arrival times being stamped from the wall clock even when a
  clock source was injected, which defeated `Client::set_time_source` and made
  latency-dependent client behaviour untestable.

- Add F3 automatic/manual remote motion buffering, bounded delay controls, reset,
  and effective delay, view-age and underrun diagnostics. Local input stays immediate.
- Add correction context clues and orientation, velocity and aim metrics to network
  trial reports; compare cold restoration with retained solver state in contact tests.
- Fix a scheduled shot that arrived late rejecting the correctly spaced shot
  behind it as "weapon is cooling down": the host now enforces the gun's rate on
  the pilot's timeline (no two shots intended within one interval) instead of on
  the tick each fired on, so a lost, retried or reordered intent fires late
  rather than costing the next shot too.
- Draw runes, outposts and the dart target at the time a shot taken now leaves
  the gun (presentation time plus the input lead) instead of at the presentation
  time, so aiming at a rotating target no longer needs a latency lead.
- Network trials can now fail. Scenarios take an `expectations` block of named
  threshold checks, `--baseline` diffs a prior `summary.json` for regressions, and
  the report status is `passed`, `failed` or `completed` with a matching exit code.
  Summaries add the shot-timing, collision-context and prediction fields the console
  already published, compute the unavailable list from what was collected, hold the
  planned sample rate and record a timed-out console query as a gap.

- Fix uneven local chassis motion caused by discarding completed predictions
  whenever a newer owner snapshot arrived. Preserve generation resets and reject
  predictions that would rewind time or the correction baseline.
- Keep the prediction clock advancing when older world snapshots arrive after
  newer owner updates, while retaining pause and timeline resets.
- Add local prediction replay timing and accepted/discarded result counters to
  console network diagnostics for the chassis smoothness investigation.

- Protocol 21 adapts input lead from host arrival margins and repeats useful
  movement transitions within a bounded UDP packet. Schedule future shots on
  their movement tick with reliable scheduling receipts and terminal outcomes;
  reject cooldown-ineligible shots once instead of using retries as a scheduler.
  Bound each pilot's shot queue and cancel across pause, defeat and placement
  changes. Capture predicted muzzle poses at the intended tick and distinguish
  retry expiry from unresolved outcomes. Add lead/history comparison overrides;
  keep remote buffering controls in the stage 6 F3 plan.

- Protocol 20 uses acknowledged, retained UDP baselines for world deltas. Keep
  owner corrections independent, bound baseline caches, retire them explicitly
  and recover missing state without depending on the last transmitted frame.
  Pack owner state and redundant inputs without reducing f64 precision; compress
  shot intents and coalesce queued retries with expiry. Add codec loss/reordering/
  reset tests and a full-checkpoint comparison switch.

- Pace UDP application traffic per peer, replace unsent owner/input state, and
  give world transfers bounded service alongside ordered control messages. Cap
  queued bytes and transfer age, discard expired unreliable commands, and expose
  client queue/replacement counters. Default budgets are 10/40 KiB/s up/down.

- Protocol 19 sends independent full-precision owner corrections before optional
  world fragments. Keep world checkpoints as coherent collision context, track
  context age separately and reject stale prediction results and owner revisions.

- Protocol 18 adds bounded host input execution feedback and shot execution ticks.
  Measure owner corrections against exact-tick prediction history before replay;
  expose missing comparisons and extend harness summaries with these measurements.
  Input scheduling, firing and server authority are unchanged.

- Add local network stats to console and an Off/Compact/Detailed overlay, selected
  with `--network-stats` or P/F3. Sample GNS ping, rates and backlog at 4 Hz;
  retain bounded display history and distinguish unavailable loss from zero loss.

- Expand the networking roadmap into an implementation design for independent owner
  corrections, collision context recovery, paced traffic, acknowledged UDP baselines
  and scheduled inputs/shots. No runtime behavior changes.

- Add a standalone network harness with directional bandwidth caps, bounded queues,
  random/burst loss, duplication, reordering, blackouts and shared server egress.
  Run isolated scripted or manual clients, retain bounded logs/reports and clean
  up owned processes. Add scenario validation and model/UDP/lifecycle tests through
  `just network-test`. Game networking and the planned in-game stats remain unchanged.

- Replace earlier networking recommendations with a prioritized experiment roadmap.
  Document a shared measurement contract, repeatable bandwidth/loss/latency harness
  and optional in-game network stats display. These are designs, not runtime changes.

- Remove player bullet trails and contact-point markers. Protocol 17 reduces UDP
  checkpoint traffic with compact projectile vectors, reconstructed armor poses,
  and omission of wheel-contact and failed-hit diagnostics. Preserve authoritative
  combat state and full-precision chassis reconciliation. Pack the four redundant
  input samples into one compressed datagram. Add proxy byte counters.

- Align client collider outlines with the exact rendered poses every frame,
  including predicted gimbal aim and remote interpolation. Cache moving CAD
  wire meshes once instead of holding 20 Hz captures of world positions.

- Keep collider inspection entirely client-side without replacing predicted gameplay
  poses. Reuse one bounded background worker and avoid rebuilding physics worlds.
- Send full checkpoints at 31 Hz, bound repeated shot outcomes, and allow GNS
  congestion control above its default 256 KiB/s ceiling to avoid transport queues.
- Recover promptly from clock offsets caused by an initial retransmitted timing probe.
- Apply fresh late controls at receipt instead of discarding delivered inputs.
  Retry shots delayed by weapon cooldown without bypassing cadence or charging twice.
  Separate unacknowledged shots from accepted bullets so sustained fire stays available.
  During prediction, update only moving scenery instead of rewriting fixed CAD
  collider poses each millisecond.

- Restore the C collider shortcut and enable remote collider inspection from verified
  field assets and client presentation, including spectators.

- Keep client physics running through snapshot gaps for up to one second, then
  reconcile from authoritative checkpoints and discard finalized input intervals.
  Protocol 16 schedules redundant controls and replaces normal shooter-view
  evidence with retried exact-aim shots and independent live server projectiles.
  Local contacts can be overturned. Preserve authoritative damage, ammunition,
  cadence and lifecycle checks; blend small camera corrections where space permits.
- Show small toasts for high latency, interrupted updates and confirmed disconnects.
  A disconnected client keeps its window open. Add timed, directional UDP blackouts
  to the local impairment proxy. Releases remain paused.

- Enable local chassis and gimbal prediction by default for matching field packages.
  Protocol 15 adds redundant unreliable pilot controls with a 250 ms stop lease,
  exact-aim shot IDs, compact shot acknowledgements and shooter-view projectile
  validation throughout flight. The host still validates ammunition and cadence
  and applies all damage. Tune the admitted view age with `--max-compensation-ms`
  from 0 to 500; `--no-prediction` retains the authoritative comparison path.
  Provisional shots may end when evidence is stale or incomplete. Revival changes
  the chassis revision so late contacts cannot cross robot lives.

- Pause release packaging and publishing during development. Add a local UDP
  proxy for configurable packet loss and delay; the severe-loss trial remains
  unsuitable for responsive play.

- Default multiplayer to Valve GameNetworkingSockets over UDP, with compressed
  independent periodic snapshots and ordered reliable commands and confirmations.
  Protocol 14 retains TCP through `--transport tcp`. Standalone play works offline;
  Windows packages include a `Play Local.cmd` launcher.

- Include red/blue Windows launch scripts that prompt for the server address
  remember the address in `server.txt`, and generate player names from the
  computer name plus a random suffix.

- Keep robots inside the arena with invisible boundary walls and remove the
  catch plane beneath loaded CAD terrain. Add a procedural sky background.
- Open base shields when the owning outpost is destroyed, and include moving
  shield geometry in synchronized collision inspection. Correct equipment paint
  to follow red/blue ownership without changing friendly-fire rules.
- Resolve relative asset paths from the working directory before loading either
  manifests or glTF meshes, including Windows packages.
- Add an explicit defeat-menu respawn button and an F3 reset-to-spawn button.
  Protocol 13 checks ownership for both commands.

- Add O/I pilot purchases for one 17 mm/42 mm round from team gold, at the
  existing default prices of 1/10. Protocol 12 checks ownership on the host.
- Restore defeated pilots to full HP in place on request, retaining position,
  identity, ammo and gold. This training shortcut has no timer or invulnerability.

- Make host-driven chassis, gimbal and driving-camera poses the default, so
  network delay affects both driving and aiming. Keep commanded mouse angles
  separate from displayed aim. `--predict` opts into the previous local prediction
  and immediate-aim behavior; `--no-prediction` explicitly selects the default.

- Pair local collision wireframes with visual snapshots from the same host
  capture. Collision inspection uses captured robot and mechanism poses instead
  of mixing asynchronous physics geometry with predicted or interpolated models.

- Predict local chassis movement with shared 1 ms drive and suspension physics,
  host-acknowledged input replay, and bounded background work. Reset on pause,
  placement, defeat and rejection; fall back when snapshots grow stale or the
  field package differs. Add `--no-prediction` for baseline comparisons.
- Protocol 11 adds numbered chassis inputs, application-time acknowledgements
  and a prediction terrain contract. Firing and all gameplay outcomes remain
  host-authoritative. Add reproducible jitter/stall reconciliation tests and a
  CAD replay cost benchmark.

- Reduce snapshot traffic with protocol 10 structural deltas generated after
  outbox coalescing, deterministic mechanism reconstruction, and periodic full
  recovery checkpoints. Keep confirmation snapshots full and ordered before Pong.
  Add a reproducible bandwidth and codec-cost benchmark.
- Interpolate remote chassis poses and wheels through a bounded 64 ms buffer.
  Add explicit placement revisions and reset stale history on discontinuities;
  local chassis, authoritative rules and screenshot confirmations are unchanged.

- Reconstruct outpost and rune motion between authoritative checkpoints with a
  bounded client simulation-clock estimate, and advance displayed match/buff
  countdowns locally. Pauses and screenshots retain exact snapshot poses.
- Protocol 9 adds independent clock probes and optional client fire timing and
  observed chassis/muzzle poses. Keep accepted-shot diagnostics in a 256-entry
  host journal exposed at `/api/fire-records`; launch pose, cadence and hits
  remain authoritative, with no backdating.
- Add a bounded local TCP impairment proxy for visual inspection without
  changing system network settings.

- Add deterministic multiplayer delivery tests for latency, jitter, TCP recovery
  stalls and bounded confirmation backlogs, plus a networking investigation
  covering interpolation, prediction, UDP and Steam integration.

- Derive pilot shots from the host's turret pose and weapon configuration, with
  a per-chassis firing interval measured in simulation time. Protocol 8 carries
  shooter-only firing intent and announces the host weapon. Keep free-camera
  projectile placement restricted to the embedded owner and operator API.
- Coalesce periodic snapshots before adding them to each bounded peer outbox,
  while preserving reliable messages and snapshot-before-Pong confirmations.
- Share income schedules and ammo accounting between live resources and the
  standalone gameplay engine. Skip inactive gameplay clock spans and avoid
  cloning the full game for commands that validate before mutation.

- Give each hosted simulation one worker that orders clock advances, TCP and
  HTTP commands, player admission and removal, and snapshot confirmations.
  Encode shared broadcasts on socket writers and capture collision geometry
  through the host mailbox. The Rust hosting API now consumes `Simulation`
  and exposes a `HostHandle` instead of a shared simulation mutex.

- Reduce repeated allocations in the 1 ms physics loop by reusing target-pose,
  wheel-force and projectile-velocity buffers. Read robot state and chassis
  positions directly for defeat synchronization, spawning and placement.

- Add a localhost JSON-lines app console with ordered replies for runtime camera,
  spawn, keyboard/mouse input, pause/step, inspection, state and repeated screenshot
  commands. Include a Python client and command reference.
- Add normal, unfocused and headless rendering modes. Keep the existing runtime
  CLI flags as launch shortcuts and use the same screenshot command in every mode.
- Add owner-only pilot placement with ground lookup, preserving robot ID and HP;
  multiplayer protocol version is now 7.

- Capture screenshots from the visible game window instead of redirecting the
  gameplay camera to an offscreen image. `--screenshot PATH` still waits for the
  scene to settle, saves a PNG and exits.

- Add an F3 debug panel with a three-state collider selector, visual mesh
  wireframe and rendering statistics. Collider debugging includes live chassis,
  armor and projectiles in local sessions. Replace the C shortcut with the panel
  and add `--debug-panel`, `--render-stats` and `--wireframe` for startup inspection.

- Apply passive white base artwork to the moving dart plate, three upper armor
  modules and three lower modules revealed by opening the shields.

- Move both base dart targets back and forth on their exported rails, with
  matching collision geometry and a four-second simulation-time cycle.
- Refresh local semantic assets with all reference joints and bindings; record
  the composition inputs and updated simplifier configuration for reproduction.

- Give chassis and outpost armor light bars white plastic off-state diffusers,
  rounded covers, and brighter emission centers with colored edge falloff,
  guided by real armor photos from the Yolo-Detector corpus. Printed
  identifiers remain passive white.

- Replace procedural chassis numbers and the separate outpost texture with a
  shared SVG-derived armor atlas. Include 1–5, guard, outpost and base sprites
  with their small/large variants and preserve aspect and orientation. Patterns
  are passive white printing and remain white when armor lights switch off.

- Add per-team referee HTTP and panel controls for dart doors and base shields,
  using exported joints for both drawing and collision.
- Disable destroyed outpost armor lights and hit
  processing, while retaining its housing and stopped rotor. Share disabled
  light-state handling with chassis armor.

- Redesign the competitor overlay with responsive Bevy UI and embedded Flair
  styling, following the July 2026 student client manual. Add Feathers settings
  checkboxes, a sensitivity slider, reset, and a mouse-driven panel toolbar.
  Click the minimap to expand it. Panels release the cursor and block drive,
  aim, fire and match shortcuts; closing never captures or fires through the UI.

- Remove the extracted arena and base artwork from source colliders, then
  simplify their backing geometry. Static collision drops from 927,028 to
  828,829 triangles; native textured visuals and joint bindings are retained.

- Support the native-texture field export with embedded artwork on simplified
  arena and base geometry. Preserve textured CAD materials during scenery
  adjustment, including their alpha masks, tint and PBR settings. The local
  package reduces placed visual triangles by 25.1%; collision is unchanged.
  Omit transparent artwork quads from the geometry-only minimap projection.

- Replace the debug text HUD with a simplified July 2026 competitor interface:
  team and outpost status, match clock, robot HP, ammo, gold and a teammate map.
  Generate an optional CAD-derived top-down background with rm-map-tools,
  keeping its PNG and coordinate metadata in the external field package.
  Add hold-Tab status, P settings, M large map and hold-F12 help. Move match
  start/reset, pause and step to F5/F6/F7; block drive and fire in modal panels.

- Flatten and lengthen the VTM case above the barrel, with top cooling slots
  and a forward lens. Keep the first-person eye aligned with the lower lens.

- Extend the launcher by 100 mm behind the speed monitor, keeping the firing
  point aligned with the longer muzzle and the centered VT03 camera mount.

- Add a host-selectable mecanum Hero prototype and update the omni Infantry
  with open chassis frames and approximate AM02, LI01, FI02, SM01/SM11 and VT03
  equipment. LI01 segments follow referee HP; equipment LEDs go dark on defeat.
  Center VT03 above the barrel, align the driving eye with its lens, and draw grounded wheels at their
  suspension contacts. Add a CAD-free robot comparison with staged hit/defeat
  views. `--robot hero` selects the larger chassis and defaults local fire to
  42 mm; HP and other referee policies remain unchanged.

- Remove dark gaps in the rune activation waterfall by continuously moving all
  arrow rows instead of blinking alternating groups off.

- Fix semantic rune artwork that stayed coloured while inactive, and give both
  centre emblems their owning team's colour. Correct the emblem's occlusion by
  its backing disk and stop rear-face lights showing through arm openings.
- Animate rune arrows from simulation time, align target overlays with the CAD
  front plane, draw Big Rune progress along the arm outlines, and separate the
  completed-arm pattern from the fully activated framing and centre rings.
  Big Rune progress now shares the configured activation blink.
- Use neutral scene lamps to preserve CAD paint colours. Install the map-tools
  composition that removes two obsolete wordmarks beneath the grafted bumpy
  roads from both visual and collision scenes, retaining a local package backup.
  Record per-issue decisions and visual fixtures in `docs/scene-reference-audit.md`.

- Connect live projectile counts, allowances and team economy to the referee
  clock. Add HTTP controls for enforcement, initial ammo, income, prices,
  balances, counters and referee resupply; show resources in the HUD.
- Add HTTP outpost HP and rune opportunity/buff overrides. Match start/reset
  restores outposts and round resources. Protocol 6 carries resource snapshots
  and edits, with network edits restricted to the referee. Base scoring and the
  standalone engine's other rules remain unconnected.

- Add the standalone `rm-simulator-gameplay` crate with deterministic match and
  round lifecycle, economy, purchases, respawns, base protection, outpost
  rebuilding, heat, experience caps and result confirmation. Include a rule
  coverage register and observation records for unsupported equipment, plus
  `just gameplay-test` and `just gameplay-demo`. The app and server continue to
  use the world referee, now with the live resource component.

- Fix timing-dependent black PNG screenshots by rendering to a dedicated
  image and waiting for GPU pipeline compilation and settled scene frames.
  Retry blank readbacks and report failure if capture cannot finish.

- Run standalone simulation on an embedded server worker, using the multiplayer
  client path for commands and snapshots. Hold the clock until scenery is ready,
  preserve custom spawns, initial pause, local match controls and owner free-camera
  shooting. Local snapshots target 4 ms; remote broadcasts remain at 16 ms.
- Capture collision shapes on the wireframe worker without waiting on physics
  in a frame update. Screenshots wait for discrete command results to reach the
  scene. Protocol version 5 adds a snapshot-before-Pong command barrier; clients
  and hosts need matching versions.

- Tie TCP and HTTP listener lifetimes to their host handles. Ending a host closes
  connections, removes its players and joins server workers. Add an owned
  background server clock with error reporting and protection against duplicate
  clocks, ready for embedded hosting.

- Accept checksummed `meshopt-simplification-v1` colliders with validated producer
  error and border metadata. Deploy simplified semantic assets as the local
  default: 3.02 million visual / 2.17 million static collider triangles, with
  median local scene-ready time reduced from 15.50 to 5.73 seconds in three runs.

- Accept independently configured visual and collider meshes with simplified open
  borders through the checksummed `meshopt-boundary-simplification-v1` contract.
  Validate the border declaration and sampling budget. The local default uses
  1.18 million visual and 0.94 million static collider triangles, retaining
  semantic joints and identified artwork. Installed binaries find the package
  beside their `bin` directory, independent of the build checkout.


- Update the default local CAD package with reduced resource-zone and tech-core
  tessellation, preserving checksum-pinned semantic equipment. The deployed
  package contains 7,527,046 visual and 5,968,039 static collider triangles.

- Index terrain triangle footprints once during loading to accelerate chassis
  spawn ground-height queries, preserving slopes, stacked surfaces and edge
  tolerances. Collision geometry and wheel physics are unchanged.

- Move multiplayer client writes off the gameplay thread. Preserve command
  order in a bounded queue, keep only the newest incoming snapshot and roster,
  and bound unread notices and rejections. Queue overflow disconnects with a
  visible error instead of stalling frames.

- Share CAD-to-simulation construction between the app and headless server,
  including terrain loading, physics setup and optional chassis spawning.
  The app's loading screen receives progress from the shared constructor.

- Load checksum-bound rm-map-tools articulation sidecars. Semantic joint IDs,
  pivots and hierarchy drive rune/outpost rendering, gameplay pivots and fixed
  collision selection; fixed rune logos and shaft caps need no counterrotation.
  Legacy packages remain supported. Base shield/gate preview joints stay at rest.
- Prefer the repository's ignored `local-assets/field` package when present and add
  a compositor plus `inspect_assets` verification/count report. See
  `docs/semantic-assets.md` for the mixed optimized/reference asset snapshot.

- Capture collision-debug geometry only when the wireframe is first requested.
  Triangle expansion and edge building run off the UI thread without holding
  the simulation lock. Show build status in the HUD, retain scenery while
  preparing, and reuse the finished wireframe across view changes. Screenshot
  mode waits for requested wireframes and reports unavailable geometry or errors.

- Load independently tessellated collision meshes only for assets declaring the
  `source-tessellation-v1` contract. Older packages continue using visual geometry.

- Added a startup splash with a stage progress bar, current loading activity,
  and visible errors. File verification, terrain loading, physics construction
  and connection setup run on a worker thread. CAD instances are requested
  across frames; local gameplay waits until scenery is ready.

- Static CAD collision now uses the visual triangles for every package,
  including markings and stationary equipment. Removed arena simplification
  and the equipment voxel fallback; legacy hull and silhouette proxies are
  no longer loaded.

- Chassis armor and robots: every chassis carries four small armor modules
  (front, left, back, right; the outpost's module leaning back 15°, on the
  body sides) that detect strikes like the outpost's armor (Figure 5-16
  area, Table 5-1 speeds, section 5.1.1 intervals) and cost the robot 10 HP
  (17 mm) or 100 HP (42 mm) through the defense buff. The referee opens a
  robot record per chassis when it joins and drops it when it leaves
  (`RefereeConfig::robot` is the one template; `RobotJoined`/`RobotLeft`
  events); a robot at zero HP is defeated: its drive is cut, its aim
  freezes and it cannot fire until revived. `ArmorTarget::Chassis`,
  `ArmorHit::shooter` and `RobotDamaged::shooter` name the chassis that
  fired; `Command::Fire` carries `shooter`, which a host requires to be the
  sender's own chassis. `ChassisSnapshot::defeated`; the HUD marks defeated
  robots and names armor hits. Protocol version 4.
- Robot visuals: omni wheels with a hub, rim and eight tangential rollers;
  a two-axis gimbal bolted to the body (turntable, pedestal and fork
  turning about the body's up axis, a cradle with the barrel, muzzle,
  feeder and camera pitching between the arms, the angles solved so the
  barrel follows the stabilised aim on any slope; the first-person eye
  rides the cradle's camera block, so the view rolls with the gimbal); armor modules with team-coloured light bars that flash grey when
  struck and go dark when defeated; corner bumpers.
- A ground lookup tolerates a surface at exactly the searched height, so a
  chassis spawned at floor level is set on the floor, not the slab's
  underside.

- Load additional placed scenery from the field manifest’s `static_assets` list,
  including its verified collision proxies. The deployment utility in
  `rm-map-tools` combines existing exports to add the centre platform, dart
  stations, resource zones and outpost footings without duplicating animated
  equipment; the previous asset installation is retained as a backup.

- Roles and the gun aim on the wire: `Hello` names a role (`Pilot`,
  `Spectator` or `Referee`) instead of a `spectate` flag. The referee has
  no chassis and no team, and alone may send `Referee`, `Pause` and `Step`;
  a host rejects them from pilots and spectators ("only the referee runs the
  match"). The app's `--referee` joins that way (and hides the match keys
  from everyone else on a remote host); `Welcome` and the roster carry the
  role and an optional team. `ChassisCommand` gains `aim_yaw_rad` and
  `aim_pitch_rad` (the stabilised gun's world heading and elevation, held
  exactly), and `ChassisSnapshot` a `turret` pose, so every client draws
  other players' guns where they point. Protocol version 3.
- Multiplayer infrastructure: the field holds any number of chassis, each
  with an id, a team and its configuration in the snapshot; `Field::
  add_chassis` and `remove_chassis` put them on and take them off a running
  field, and chassis commands name the id. A host spawns one chassis per
  pilot in the next free slot beside the team's spawn point (`layout::
  spawn_slot`, `ChassisSpawner`), removes it when the client leaves, and
  accepts chassis commands only from its owner. `Hello` carries a
  `spectate` flag (the app's `--fly` on a remote host): spectators get no
  chassis and cannot fire. A client that names no team is put on the team
  with fewer pilots. The host broadcasts a `Roster` (name, team, chassis of
  every client) whenever it changes; the HUD shows it. The renderer draws
  every chassis with a team-coloured bar. Protocol version 2. The server's `--spawn` and
  `--spawn-yaw-deg` are gone (spawns are per team); `--no-chassis` makes
  every client a spectator.
- The app binary is split into modules (`args`, `session`, `controls`,
  `scene`, `hud`, `debug`, `screenshot`, `frames`); no behaviour change.
- Referee system: a new `referee` module in the world crate runs an RMUC
  2026 match on the field clock (section 5.5.2 and section 6.6 rune
  opportunities and small rune buff, Tables 5-16 and 5-17 big rune buff by
  average ring and arms lit, Figure 5-18 ring geometry): a 5 s countdown,
  a 7 min round, two teams with rune opportunities at 0:00 and 1:30 (small)
  and 3:00, 4:15 and 5:30 (big), the 20 s Activating window, rune detection
  limited to rings 4..10 after one big activation and 7..10 after two, and
  defense buffs applied to that team's outposts. Buffs and the activation
  window run on the round clock, so `SkipTo` (capped at the end of the
  round) ages them. Robots are HP records
  (damage, defeat, revive) for the referee commands; attack and cooling
  buffs are reported but nothing consumes them yet. The field config's
  `referee` entry enables it; the app builds one by default (`--no-referee`
  turns it off) with the rune and both outposts assigned red and blue
  alternately.
- Rune states: a rune now starts `Inactive` (arms dark) until the referee
  or `ResetMatch` activates it; from `StartMatch` the target order and the
  big rune motion are drawn from a seed so a match replays identically on
  every peer, while an idle field keeps the training order. Hits on a
  ring the referee has disabled are rejected as `DisabledRing`.
- Server crate: new `rm-simulator-server` crate and `rm-simulator-server`
  binary (`just server`). It loads the CAD, builds the field, runs the
  1 ms clock and speaks a JSON-lines protocol over TCP (port 7700): hello,
  chassis, fire, referee, pause and step commands in, full snapshots out at
  60 Hz. The first client that connects while the chassis is free drives it;
  later clients watch. An HTTP referee panel (port 7780, `--http none`
  disables it) serves `/api/state`, `/api/command` and `/api/referee` and a
  small page with match, rune, robot and outpost controls. Lines, request
  heads and bodies are capped and a client that stops reading is dropped, so
  a misbehaving peer cannot exhaust the host.
- Multiplayer-ready app: `--connect HOST:PORT` runs the window as a client
  of a server, `--listen [ADDR]` hosts one from the app's own world, and
  `--http [ADDR]` opens the panel; `--team red|blue` and `--name` identify
  the pilot. Every input already travels as a protocol command, locally or
  remotely.
- Match controls: `M` starts the match (or resets a finished one) and `F`
  activates the rune for your team; the HUD shows the clock, stage, each
  team's opportunities, buffs and robots, and the recent referee events.
- Team sides: the referee now assigns the rune faces and outposts by field
  half instead of by index. Red is the +x half, where the CAD paints its
  red markings, so red owns the outpost at x > 0 and the rune face looking
  towards +x; blue owns the other pair. Rune targets and outpost light
  bars glow in their owner's colour (both were blue before). The default
  spawn follows `--team`: red starts at x = +9 facing −x, blue at x = −9
  facing +x, unless `--spawn` or `--spawn-yaw-deg` says otherwise.
- Flashes: an activated rune's arms blink three times at 2 Hz and then stay
  lit (`--rune-flash-hz`, `--rune-flashes`; the rulebook only shows the
  arms lit) and a struck
  armor module turns grey for 50 ms instead of 120 ms (`--hit-flash-ms`).
- Module moves: `cad_assets.rs`, `collision_mesh.rs` and `terrain.rs` now
  live in the server crate (Bevy-free, with a small quaternion and matrix
  helper in `math.rs`), together with the field layout (`layout.rs`) that
  the app and the server share.
- Field package: the default map is now `~/dev/RM/assets/rm2026-field`,
  built by the sibling `rm-map-tools` project from DJI's RMUC2026 V1.2.0
  STEP, whose arena carries real face colours. The floor slab, plates,
  walls and every marking now render in the CAD's own colours (dark grey
  slab and plate tops, beige plate sides, white lettering, red and blue
  lines) instead of a grey override. The manifest's new
  `floor_top_source_z_m` puts that slab's flat top at height zero; the
  earlier V2.0.0 extraction (`rm2026-extracted`) still loads through
  `--cad-assets` and keeps its pad reference. The rune, outposts, bases and
  tech cores are unchanged.
- Undulating road: the V1.2.0 STEP has a flat deck where V2.0.0 models the
  起伏路段 (a 2.4 × 2.1 m strip of 70 mm waves near each side wall), so the
  package now grafts those two V2.0.0 solids onto its deck in V1.2.0's plate
  colours (the manifest lists them under `grafted`) and the chassis feels
  them through its ray-cast wheels. The hex-marked 0.15 m plateau on the
  centre line, which earlier notes mislabelled as the bump road, is the same
  truncated pyramid in both releases.
- Base pedestals: the regenerated package carries the pedestal under each
  base and the step behind it, which V1.2.0 keeps as top-level products the
  exporter had skipped.
- Scenery colours: the unpainted-cream override no longer catches pure
  white, so white line markings stay white.
- Ground contact: the world now carries the field's collision proxies
  (`floor-collision.glb`, `arena-static-collision.glb`, checksum-verified)
  plus the base and tech core meshes as one static trimesh from start-up,
  with two-sided contacts and a catch plane 0.45 m below zero. The crowned
  floor slab, the 起伏路段 undulating roads, plateaus, ramps, highlands and covered passages
  are all real surfaces. `--no-field-collision` now leaves only a flat floor.
- Collision view: `C` cycles between the scenery, the scenery with the
  physics geometry drawn as a green wireframe, and the wireframe alone
  (`--collision-view overlay|alone` starts there). The wireframe is the
  geometry the physics holds.
- Scenery colours: the floor and arena overrides now recolour only the CAD's
  unpainted cream and leave painted materials alone. (The arena STEP carries
  no colour for its markings, so they still show only as relief.)
- Omni chassis: by default you drive a four-wheel omni chassis (ray-cast
  wheels with suspension, motor-limited ideal omni tyres, a turret collider so
  low roofs stop it) placed on the ground at the spawn point. WASD drives relative to the mouse aim, the chassis
  heading follows the aim, `R` spins it, `V` toggles a third-person view
  (`--third-person` to start there) and the gun fires from a stabilised turret
  on the chassis; the eye sits above and ahead of the turret and the
  third-person camera trails over the right shoulder so the body stays out of
  the aim. `--fly` restores the free camera. The HUD reports chassis speed,
  ground slope and wheel contact. The default spawn moved to just before the
  red-side centre-line plateau.
- Outpost armor now scores strikes across the Figure 5-16 effective detection
  area (101 × 94 mm) instead of only the 55 mm light bar band, so shots that
  visibly land on the plate register.
- Projectiles: hold the left mouse button to fire 17 mm or 42 mm balls
  (`--projectile-mm`, `--muzzle-speed-m-s`, `--fire-rate-hz`). Flight and
  collision run on a `rapier3d` world stepped with the field clock, with
  gravity, quadratic drag, floor, field CAD (`--no-field-collision` to skip)
  and armor housing colliders. Outpost and rune armor detect strikes per the
  RMUC 2026 normal-speed, detection-interval and caliber rules; outposts carry
  1500 HP with the centre bonus and freeze their rotor when destroyed. Struck
  lights flash grey, projectiles draw with trails and impact marks, a crosshair
  marks the muzzle line and the HUD shows gun, HP and last-hit state.
- RMUC 2026 field from the extracted competition CAD: floor, terrain, the
  two-faced rune, both outposts, bases and tech cores are placed from the
  manifests (checksum-verified at start-up). Rune faces and outpost rotors
  follow the world rules; rune targets and outpost armor are emissive overlays
  on the imported geometry. Outposts now take a full base pose and the field
  holds one rune per CAD face. The procedural arena, rune wheel and tower
  proxies are removed. `--spawn`, `--spawn-yaw-deg` and `--spawn-pitch-deg`
  set the start view.
- Initial workspace: `rm-simulator-world` (rune and outpost rules on an
  explicit 1 ms clock), `rm-simulator-render` (rune and outpost visuals with
  passive scene synchronization), and the `rm-simulator` first-person
  application with a fly camera, pause/step, and HUD. Rune rules, outpost
  geometry, rune and outpost visuals, and armor artwork masks are reused from
  `rm-vision-sim`.
