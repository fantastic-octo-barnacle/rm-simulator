<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Rendering benchmark

`rm-simulator-bench` is a separate executable using the game's CAD renderer,
lighting and shared graphics presets. It loads checksummed visual GLBs and
freezes mechanisms at their exported poses. It has a fixed camera and optional
stadium, with no physics, collision loading, robots, projectiles, networking or
gameplay UI. The renderer's normal GPU light clustering remains enabled. It never
depends on physics, world, server or gameplay; see
[`crates/rm-simulator-bench`](../crates/rm-simulator-bench/README.md) and
[simulation architecture](architecture-refactor.md) for the crate split.

This guide is the canonical reference for the benchmark: commands, case input,
sweep files and report interpretation. It is not a full-game frame-rate claim;
[performance](performance.md) covers CPU probes, and
[reproducible field detail](field-detail.md) covers the package under test.

## Build and run

Build once on the machine being measured, before starting any captures:

```sh
cargo build --release -p rm-simulator-bench --locked
```

On Linux or Windows, use native Vulkan or DX12 for GPU timestamp measurements:

```sh
target/release/rm-simulator-bench --cad-assets local-assets/field-coarse \
  --backend vulkan --detail raw --screenshot --output /tmp/rm-render-high
```

Omitting `--case` uses the built-in High 1080p defaults. For another case, write a
JSON file and pass it as `--case /tmp/render-case.json`:

```json
{
  "name": "high-1080p",
  "resolution": [1920, 1080],
  "graphics": {"preset": "High", "overrides": {"vsync": false}},
  "camera_position_flu_m": [10, 0, 0.7],
  "camera_target_flu_m": [0, 0, 0.7],
  "vertical_fov_deg": 60,
  "geometry": "normal",
  "stadium": true,
  "warmup_seconds": 10,
  "sample_seconds": 20,
  "timeout_seconds": 120
}
```

On Windows, the executable has an `.exe` suffix. Adjust output paths as needed.
Copy the same external CAD package to the test machine; it is not part of the
source checkout. The report records its manifest and visual-file hashes.

Mac runs support CPU-only measurements:

```sh
target/release/rm-simulator-bench --cad-assets local-assets/field-coarse \
  --backend metal --cpu-only \
  --detail raw --screenshot --output /tmp/rm-render-metal
```

GPU timestamp runs are rejected on macOS. There are no MoltenVK workarounds,
SDK environment injection, forced CPU light clustering, or synthetic calibration
workloads in the benchmark. Mac CPU-only runs disable GPU timestamp queries.

The output directory must be new. Defaults are High at 1920×1080 physical pixels,
10 seconds of warmup and 20 seconds of sampling. Window scale is fixed at 1 to
avoid silently rendering at Retina resolution. VSync defaults off; an explicit
`graphics.overrides.vsync` value takes precedence. `--headless` renders to an
image without presentation. Compare offscreen and windowed results separately.
`--screenshot` saves `scene.png` after sampling and readback drain.

### Geometry modes

`normal` uses authored geometry. `boxes` replaces detailed untextured CAD meshes
with their local bounding boxes. `sparse` retains every hundredth triangle of
those meshes. Both retain entities, materials, transforms, textured artwork and
meshes of 12 or fewer triangles. They also change coverage, overdraw and shadows,
so their speed difference is not a pure measure of triangle processing cost.
They never modify asset files.

## Settings sweeps

Write a sweep file and pass it to the driver:

```sh
python3 scripts/benchmark-render.py /tmp/render-1080p.json \
  --binary target/release/rm-simulator-bench \
  --cad-assets local-assets/field-coarse --backend vulkan \
  --detail raw --output /tmp/rm-render-sweep
```

For Mac CPU-only sweeps, replace the backend with `--backend metal --cpu-only`.
`--require-ac` checks AC power before and after each run on macOS.

The example tests High/Ultra × normal/boxes/sparse geometry, twice per case.
Each case uses a fresh process with its own warmup. A recorded seed shuffles
execution order; `--cooldown` controls the pause between runs. The driver saves
commands, binary hash, configuration, logs, exit status and power state where
available. `comparison.csv` keeps repetitions separate, and `sweep.json` records
the run order. Failed runs remain visible and cause a nonzero exit code.
`--list` previews a sweep without running it.

Sweep files contain a `base` case, dotted-field `sweep` axes, optional explicit
`cases`, `repeats`, and `seed`. For example:

```json
{
  "base": {"resolution": [1920, 1080], "stadium": true},
  "sweep": {
    "graphics.preset": ["High", "Ultra"],
    "graphics.overrides.shadow_map_size": [1024, 2048, 4096],
    "graphics.overrides.bloom": [false, true]
  },
  "repeats": 3,
  "seed": 2026
}
```

Other axes include resolution, FLU camera position/target, vertical FOV, stadium,
geometry, and shared graphics overrides for shadows, shadow distance/cascades,
MSAA, bloom intensity, exposure, depth prepass and occlusion culling. The report
includes resolved settings. An override value outside the supported set is
clamped to the preset fallback — an unsupported MSAA count becomes 4× — whereas a
setting the adapter cannot provide (an unsupported sample count, a shadow map
over the device limit, or unavailable occlusion culling) fails the run.
Occlusion requires depth prepass.
Projectile detail and generated-light emission have no workload to affect in
this frozen CAD scene. Unknown configuration fields are rejected.

### Effect sweep

A useful effect sweep changes one option at a time from the original Ultra
settings at 1080p and 4K, twice in a seeded shuffled order. It covers shadow
resolution, cascade count and distance, shadows off, MSAA, bloom, depth prepass,
occlusion culling, sparse geometry and the stadium. It uses the same fixed
camera throughout. Sparse geometry changes surface coverage as well as triangle
count, and disabling the stadium changes lighting geometry and shadows; neither
is a pure geometry throughput test.

```sh
python3 scripts/benchmark-render.py /tmp/render-effects.json \
  --binary target/release/rm-simulator-bench \
  --cad-assets local-assets/field-coarse --backend vulkan \
  --headless --detail passes --output /tmp/rm-render-effects
```

Use offscreen rendering consistently when the requested resolution exceeds the
desktop. Record power, temperatures and clocks alongside the sweep. Laptop
thermal limits can dominate variation; confirm promising changes with repeated
baseline/candidate runs before using them to change a preset. The static scene
cannot measure projectile detail, and appearance-only controls such as exposure
and emission strength are not performance switches.

No sweep result is committed. Treat a single sweep as a hypothesis rather than a
preset decision, and keep the raw reports with the change that used them.

## Output and interpretation

### Detail levels

| Detail | Output |
|---|---|
| `summary` | `report.json`: case, adapter, asset hashes, geometry counts, CPU frame and whole-render GPU distributions, readback health. |
| `passes` | Summary plus CPU/GPU statistics for four render stages. |
| `raw` | Passes plus `render-frames.jsonl` and `cpu-frames.csv`. |

Warmup starts after CAD preparation and pending pipelines settle. New pipelines
reset warmup; a pipeline becoming pending during sampling fails the run.
GPU readbacks drain for up to five seconds after sampling. The fixed readback
ring has 256 slots and never blocks waiting for a free slot. Dropped samples,
missing or invalid GPU timestamps fail the run rather than admitting a partial
capture into comparisons. `actual_resolution` records the camera's physical
target size. After pipeline warmup, a window constrained to a different size fails with instructions
to use `--headless`; case dimensions alone are not proof of render resolution.
Native Vulkan captures have been exercised on an RTX 3070 Ti Laptop GPU.

### What the numbers measure

GPU timestamps bracket render commands in one queue submission. They exclude
CPU work, presentation and the final query resolve/copy. Stage intervals divide
the submission into preparation/shadows, main rendering, early post-processing,
and final post-processing/upscaling. These are stage groups, not individual
draw-call timings. Raw render rows contain frame IDs, query ticks and CPU/GPU
stage durations. CPU stage times cover render-thread encoding and scheduling.
The separate CPU frame CSV measures application pacing between `Last` schedules;
do not join it to render rows by row number. CPU-only reports have no GPU samples.

Percentiles use nearest rank. `rate_from_mean_hz` is the reciprocal of mean
milliseconds; for GPU timings it is a throughput estimate, not game FPS.
`one_percent_low_hz` is the reciprocal of the mean of the slowest 1% of samples.
Compare runs at the same detail level because instrumentation adds overhead.

Use the stronger test machine to compare relative changes at a fixed camera,
resolution, backend and build. Those gains may differ on the Mac because the
bottleneck can change. Results do not establish the Mac's 120 FPS/60 FPS targets,
and this rendering-only workload does not establish full-game FPS on either box.
