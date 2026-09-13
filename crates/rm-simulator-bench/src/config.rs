// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Case-file and command-line configuration for one benchmark run.
//! Unknown JSON fields are rejected so a typo cannot silently measure different settings.
use bevy::prelude::Resource;
use rm_simulator_render::graphics_settings::GraphicsSettings;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Graphics backend a measurement run uses.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// Accept whichever backend wgpu selects for the platform.
    #[default]
    Auto,
    /// Native Vulkan.
    Vulkan,
    /// Apple Metal.
    Metal,
    /// Direct3D 12.
    Dx12,
    /// OpenGL.
    Gl,
}
/// How much of the measurement a run writes out.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    /// Write `report.json` only.
    Summary,
    /// Add CPU and GPU statistics per render stage.
    #[default]
    Passes,
    /// Add `render-frames.jsonl` and `cpu-frames.csv`.
    Raw,
}
/// Mesh substitution applied to untextured CAD meshes before sampling.
///
/// Substitution lowers triangle count but also changes coverage, overdraw and shadows,
/// so a `Boxes` or `Sparse` run is not a pure measure of triangle processing cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Geometry {
    /// Render the authored meshes.
    #[default]
    Normal,
    /// Replace each detailed untextured mesh with its local bounding box.
    Boxes,
    /// Keep every hundredth triangle of each detailed untextured mesh.
    Sparse,
}
/// One benchmark run: camera, resolution, durations and graphics settings.
///
/// Loaded from the `--case` JSON file or from the defaults, then recorded in `report.json`.
#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Case {
    /// Label recorded with the run and shown in the window title.
    pub name: String,
    /// Render target size in physical pixels, validated to 64..=8192 per axis.
    pub resolution: [u32; 2],
    /// Shared graphics preset and per-field overrides, checked against the known keys.
    /// `main` forces `vsync` off unless the case sets it, so runs are uncapped by default.
    #[serde(deserialize_with = "strict_graphics")]
    pub graphics: GraphicsSettings,
    /// Camera position in field forward/left/up metres.
    pub camera_position_flu_m: [f64; 3],
    /// Point the camera looks at in field forward/left/up metres.
    pub camera_target_flu_m: [f64; 3],
    /// Vertical field of view in degrees, validated to 5..=150.
    pub vertical_fov_deg: f32,
    /// Mesh substitution applied to untextured CAD meshes.
    pub geometry: Geometry,
    /// Add the venue shell of stands, fencing, lamps and room panels around the CAD bounds.
    pub stadium: bool,
    /// Seconds rendered after pipelines settle and before sampling opens.
    pub warmup_seconds: f64,
    /// Seconds of sampling after warmup.
    pub sample_seconds: f64,
    /// Wall-clock limit for the whole run, from process start to readback drain.
    pub timeout_seconds: f64,
}
impl Default for Case {
    fn default() -> Self {
        Self {
            name: "default".into(),
            resolution: [1920, 1080],
            graphics: GraphicsSettings::default(),
            camera_position_flu_m: [10., 0., 0.7],
            camera_target_flu_m: [0., 0., 0.7],
            vertical_fov_deg: 60.,
            geometry: Geometry::Normal,
            stadium: false,
            warmup_seconds: 10.,
            sample_seconds: 20.,
            timeout_seconds: 120.,
        }
    }
}
impl Case {
    /// Reject a case that cannot produce a comparable measurement.
    ///
    /// Checks the resolution range, finite camera coordinates within one million metres,
    /// a camera direction that is not parallel to world up, the field of view, the warmup
    /// and sample durations, and a timeout that leaves 5 s beyond warmup plus sampling.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.resolution.iter().all(|n| (64..=8192).contains(n)),
            "resolution must be 64..8192 pixels per axis"
        );
        anyhow::ensure!(
            self.camera_position_flu_m
                .iter()
                .chain(&self.camera_target_flu_m)
                .all(|v| v.is_finite() && v.abs() <= 1_000_000.),
            "camera coordinates must be finite and within one million metres"
        );
        anyhow::ensure!(
            (self.camera_position_flu_m[0] - self.camera_target_flu_m[0])
                .hypot(self.camera_position_flu_m[1] - self.camera_target_flu_m[1])
                > 1e-6,
            "camera direction must not be parallel to world up"
        );
        anyhow::ensure!(
            self.vertical_fov_deg.is_finite() && (5. ..=150.).contains(&self.vertical_fov_deg),
            "invalid camera FOV"
        );
        anyhow::ensure!(
            self.warmup_seconds.is_finite()
                && self.warmup_seconds >= 0.
                && self.sample_seconds.is_finite()
                && (0.1..=3600.).contains(&self.sample_seconds),
            "invalid warmup/sample duration"
        );
        anyhow::ensure!(
            self.timeout_seconds.is_finite()
                && self.timeout_seconds > self.warmup_seconds + self.sample_seconds + 5.,
            "timeout must allow startup, warmup, sampling and drain"
        );
        Ok(())
    }
}
/// Command-line arguments for one `rm-simulator-bench` process.
#[derive(clap::Parser, Resource, Clone, Debug)]
#[command(
    about = "Rendering-only field benchmark: GPU timestamps, settings sweeps and structured output"
)]
pub struct Args {
    /// CAD package root holding `manifest.json` and `equipment/manifest.json`.
    #[arg(long)]
    pub cad_assets: PathBuf,
    /// Case file to measure; the built-in case is used when omitted.
    #[arg(long)]
    pub case: Option<PathBuf>,
    /// New output directory; existing results are never overwritten.
    #[arg(long)]
    pub output: PathBuf,
    /// Report detail level.
    #[arg(long, value_enum, default_value = "passes")]
    pub detail: Detail,
    /// Backend to measure on; `auto` accepts the platform default.
    #[arg(long, value_enum, default_value = "auto")]
    pub backend: Backend,
    /// Explicitly allow CPU-only smoke tests; GPU timestamps are otherwise required.
    #[arg(long)]
    pub cpu_only: bool,
    /// Render to an offscreen image at the case resolution instead of opening a window.
    #[arg(long)]
    pub headless: bool,
    /// Save a screenshot after sampling, outside the timing window.
    #[arg(long)]
    pub screenshot: bool,
}

// Preferences accept future fields, but a benchmark typo must not silently run
// a different experiment. Derive known keys from the shared serialized type.
fn strict_graphics<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<GraphicsSettings, D::Error> {
    use serde::de::Error;
    let value = serde_json::Value::deserialize(deserializer)?;
    let known = serde_json::to_value(GraphicsSettings::default()).map_err(D::Error::custom)?;
    for (input, defaults) in [(&value, &known), (&value["overrides"], &known["overrides"])] {
        if let Some(fields) = input.as_object() {
            for key in fields.keys() {
                if defaults.get(key).is_none() {
                    return Err(D::Error::custom(format!("unknown graphics field: {key}")));
                }
            }
        }
    }
    serde_json::from_value(value).map_err(D::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_camera_and_sampling_are_rejected() {
        let mut case = Case::default();
        assert!(case.validate().is_ok());
        case.camera_target_flu_m = case.camera_position_flu_m;
        assert!(case.validate().is_err());
        case.camera_target_flu_m[2] += 1.;
        assert!(case.validate().is_err());
        case = Case::default();
        case.resolution = [0, 1080];
        assert!(case.validate().is_err());
        case = Case::default();
        case.sample_seconds = f64::NAN;
        assert!(case.validate().is_err());
        assert!(serde_json::from_str::<Case>(r#"{"unknown_resolution":[1920,1080]}"#).is_err());
        assert!(
            serde_json::from_str::<Case>(r#"{"graphics":{"overrides":{"blloom":false}}}"#).is_err()
        );
    }
}
