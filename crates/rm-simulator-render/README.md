<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# rm-simulator-render

`rm-simulator-render` owns the Bevy scene for the RoboMaster field: CAD scenery,
light overlays, pooled projectiles, chassis visuals, lighting and the
pose/material synchronization from caller-owned `SceneState`. It is passive, never
advances rules and never reads a simulation clock; the app and the benchmark crate
build on it.

## Modules

| File | Owns |
|---|---|
| `src/lib.rs` | `RenderingConfig` and `RenderingProfile`, `TeamColor`, `PoseFlu`, `camera_appearance`, and the FLU-to-Bevy conversions `flu_position`, `flu_vector`, `apply_pose` and `pose_from_transform`. |
| `src/cad.rs` | Extracted RMUC CAD scenery: `CadScenePlugin`, `CadSceneStatus`, and the static and articulated instances with their scene roles. |
| `src/rune.rs` | Emissive rune overlays: the active target, the activated outline and Big Rune progress lights on each blade. |
| `src/outpost.rs` | Outpost armor optics overlaid on the imported tower. |
| `src/projectile.rs` | The pooled projectile spheres and `ProjectileVisualsPlugin`. |
| `src/chassis.rs` | `ChassisVisualsPlugin` and `ArmorOptics`: body, omni and mecanum wheels, two-axis gimbal, armor lights and LI01 HP segments. |
| `src/armor.rs` | `ArmorAtlas` and `DiffuserProfile`: SVG-derived armor artwork loaded from the tracked `assets/armor-atlas` sources. |
| `src/equipment.rs` | Hand-built referee equipment approximations. |
| `src/sync.rs` | `SceneState` in, transforms and materials out: `SceneSyncPlugin`, `SceneSyncSet` and the chassis ingestion components. |
| `src/lighting.rs` | Ambient light plus a shadowed key and an unshadowed fill. |
| `src/graphics_settings.rs` | `GraphicsSettings` quality presets shared with the benchmark. |
| `src/quality.rs` | Runtime adjustment of explicitly registered simulator lights; authored CAD materials stay untouched. |

## Dependencies

`bevy`, `serde` and `serde_json`. The crate never depends on
`rm-simulator-world`, `rm-simulator-server` or `rm-simulator-physics`; a caller
converts its own world state into `SceneState`. `rm-simulator-app` and
`rm-simulator-bench` are its consumers.

## Testing

`cargo test -p rm-simulator-render --locked` runs headless `App` tests that spawn
the sync and visual plugins and inspect components. Coverage includes rendering
profile validation and camera post-processing, FLU/Bevy pose round trips, armor
artwork geometry, CAD team paint and joint behaviour, chassis spawn and removal,
armor flashes and defeat, and rune and outpost lights following published
`SceneState`.

`lib.rs`, `sync.rs`, `cad.rs` and `graphics_settings.rs` carry doctests for the
coordinate conversions, the rendering profiles and the shared presets.

`examples/robots.rs` renders a static, CAD-free robot fixture and captures it to a
PNG, with HP staged as `healthy`, `hit` or `defeated`:

```sh
cargo run -p rm-simulator-render --example robots -- out.png healthy
```

Reference equipment dimensions and limits are in
[docs/robot-equipment.md](../../docs/robot-equipment.md).
