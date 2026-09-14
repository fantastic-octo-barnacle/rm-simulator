<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# rm-simulator-bench

`rm-simulator-bench` owns the standalone, fixed-camera CAD rendering benchmark. It
reuses the render crate's scenery, lighting and graphics presets but freezes
mechanisms at their exported poses, with no physics, collision loading, robots,
projectiles, networking or gameplay UI. It sits beside the app and depends only on
`rm-simulator-render`.

## Modules

| File | Owns |
|---|---|
| `src/main.rs` | One-run wiring: case loading, output directory creation, backend settings, and the render and report plugins. |
| `src/config.rs` | `Case` and `Args`: resolution, FLU camera, FOV, geometry substitution, stadium, durations and strict graphics overrides. Unknown JSON fields are rejected so a typo cannot silently measure different settings. |
| `src/assets.rs` | Visual-only manifest loading. No world, server, collider or referee dependency. |
| `src/scene.rs` | The fixed camera, lighting and geometry preparation for the frozen CAD scene. Nothing here advances gameplay or physics. |
| `src/gpu.rs` | Nonblocking GPU timestamps in one render submission, with contiguous stage intervals sharing one readback. |
| `src/report.rs` | The run state machine, capability checks and report writing. A run that loses a sample or a GPU timestamp fails instead of reporting a partial capture. |

`Case` validates the resolution (64..=8192 pixels per axis), finite camera
coordinates, a camera direction that is not parallel to world up, the FOV
(5..=150 degrees), the warmup and sample durations, and a timeout that leaves 5 s
beyond them. `--detail` selects `summary`, `passes` (the default) or `raw`.
The case's `geometry` field renders the authored `normal` meshes or substitutes
untextured ones with bounding `boxes` or `sparse` triangles; both substitutions
change coverage and shadows, so neither is a pure triangle-throughput measure.
VSync is forced off unless the case sets it.

## Dependencies

`rm-simulator-render`, `bevy`, `wgpu`, `clap`, `serde`, `serde_json`, `sha2` and
`anyhow`. The crate never depends on `rm-simulator-physics`, `rm-simulator-world`,
`rm-simulator-server`, `rm-simulator-gameplay` or `rm-simulator-app`.

## Testing

`cargo test -p rm-simulator-bench --locked` runs the in-module tests: `config.rs`
rejects invalid camera, resolution, sampling and unknown-field cases, `gpu.rs`
checks timestamp units, invalid results and that stage intervals partition the
whole GPU frame, and `report.rs` checks that percentiles use nearest rank and that
the one-percent low is the mean of the slowest samples. The crate exposes no
doctests or examples.

Build once on the machine being measured, then run a case:

```sh
cargo build --release -p rm-simulator-bench --locked
target/release/rm-simulator-bench --cad-assets local-assets/field-coarse \
  --backend vulkan --detail raw --screenshot --output /tmp/rm-render-high
```

The output directory must be new. Omitting `--case` uses the built-in High 1080p
defaults, and `--headless` renders to an image instead of a window. GPU timestamp
measurement requires a native Vulkan or DX12 machine; macOS is rejected unless
`--cpu-only` is set.

[docs/render-benchmark.md](../../docs/render-benchmark.md) documents the case and
sweep files, the output detail levels and their interpretation, and the effect
sweep. [scripts/benchmark-render.py](../../scripts/benchmark-render.py) drives
settings sweeps.
