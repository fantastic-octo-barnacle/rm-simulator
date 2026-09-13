<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Changelog

## Unreleased

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
