// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Shared quality presets for the game and rendering benchmark.
use serde::{Deserialize, Serialize};

/// Named quality level; each level resolves to concrete graphics values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum QualityPreset {
    /// Shadows off, one MSAA sample, projectile detail 1.
    Low,
    /// 1024 pixel shadow map, two MSAA samples, projectile detail 2.
    Medium,
    /// The default: 2048 pixel shadow map, four MSAA samples, projectile detail 3.
    #[default]
    High,
    /// 4096 pixel shadow map, two cascades, 60 m shadow distance, projectile detail 4.
    Ultra,
}
impl QualityPreset {
    /// The next preset in the cycle, wrapping from Ultra back to Low.
    pub fn next(self) -> Self {
        match self {
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::Ultra,
            Self::Ultra => Self::Low,
        }
    }
}

/// Optional per-field overrides; none keeps the preset value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsOverrides {
    /// Enable shadow maps.
    pub shadows: Option<bool>,
    /// Shadow map edge in pixels; one of 512, 1024, 2048 or 4096.
    pub shadow_map_size: Option<usize>,
    /// Far shadow distance in metres, clamped to 10 to 100.
    pub shadow_distance_m: Option<f32>,
    /// Number of shadow cascades, clamped to 1 to 4.
    pub shadow_cascades: Option<usize>,
    /// MSAA sample count; one of 1, 2, 4 or 8.
    pub msaa_samples: Option<u32>,
    /// Enable bloom.
    pub bloom: Option<bool>,
    /// Bloom intensity in [0, 1].
    pub bloom_intensity: Option<f32>,
    /// Camera exposure in EV100, clamped to -4 to 20.
    pub exposure_ev100: Option<f32>,
    /// Linear emission multiplier, clamped to 0 to 100000.
    pub emissive_strength: Option<f32>,
    /// Projectile icosphere subdivision, clamped to 1 to 4.
    pub projectile_detail: Option<u32>,
    /// Present with vertical sync.
    pub vsync: Option<bool>,
    /// Render a depth prepass.
    pub depth_prepass: Option<bool>,
    /// Enable occlusion culling.
    pub occlusion_culling: Option<bool>,
}
/// A preset plus the overrides applied on top of it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsSettings {
    /// Base quality level.
    pub preset: QualityPreset,
    /// Field overrides, if any.
    pub overrides: GraphicsOverrides,
}
/// Concrete graphics values after the preset and overrides are clamped.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ResolvedGraphics {
    /// Whether shadow maps are enabled.
    pub shadows: bool,
    /// Shadow map edge in pixels.
    pub shadow_map_size: usize,
    /// Far shadow distance in metres.
    pub shadow_distance_m: f32,
    /// Number of shadow cascades.
    pub shadow_cascades: usize,
    /// MSAA sample count.
    pub msaa_samples: u32,
    /// Whether bloom is enabled.
    pub bloom: bool,
    /// Bloom intensity in [0, 1].
    pub bloom_intensity: f32,
    /// Camera exposure in EV100.
    pub exposure_ev100: f32,
    /// Linear emission multiplier.
    pub emissive_strength: f32,
    /// Projectile icosphere subdivision level.
    pub projectile_detail: u32,
    /// Whether presentation waits for vertical sync.
    pub vsync: bool,
    /// Whether a depth prepass runs.
    pub depth_prepass: bool,
    /// Whether occlusion culling runs.
    pub occlusion_culling: bool,
}
impl GraphicsSettings {
    /// Resolve the preset, apply the overrides and clamp every value to its range.
    ///
    /// ```
    /// use rm_simulator_render::graphics_settings::{
    ///     GraphicsOverrides, GraphicsSettings, QualityPreset,
    /// };
    ///
    /// let settings = GraphicsSettings {
    ///     preset: QualityPreset::Low,
    ///     overrides: GraphicsOverrides {
    ///         shadows: Some(true),
    ///         ..Default::default()
    ///     },
    /// };
    /// let resolved = settings.resolved();
    /// // The override turns shadows back on for the Low preset.
    /// assert!(resolved.shadows);
    /// assert_eq!(resolved.msaa_samples, 1);
    ///
    /// // An unsupported shadow map size falls back to 2048.
    /// let clamped = GraphicsSettings {
    ///     overrides: GraphicsOverrides {
    ///         shadow_map_size: Some(777),
    ///         ..Default::default()
    ///     },
    ///     ..Default::default()
    /// };
    /// assert_eq!(clamped.resolved().shadow_map_size, 2048);
    /// ```
    pub fn resolved(&self) -> ResolvedGraphics {
        let mut value = ResolvedGraphics {
            shadows: true,
            shadow_map_size: 2048,
            shadow_distance_m: 40.,
            shadow_cascades: 1,
            msaa_samples: 4,
            bloom: true,
            bloom_intensity: 0.12,
            exposure_ev100: 9.,
            emissive_strength: 12000.,
            projectile_detail: 3,
            vsync: true,
            depth_prepass: false,
            occlusion_culling: false,
        };
        match self.preset {
            QualityPreset::Low => {
                value.shadows = false;
                value.msaa_samples = 1;
                // Preserve the same emissive armor appearance as the other presets.
                // Bloom participates in the HDR appearance, beyond the surrounding glow.
                value.projectile_detail = 1;
            }
            QualityPreset::Medium => {
                value.shadow_map_size = 1024;
                value.msaa_samples = 2;
                value.projectile_detail = 2;
            }
            QualityPreset::High => {}
            QualityPreset::Ultra => {
                value.shadow_map_size = 4096;
                value.shadow_cascades = 2;
                value.shadow_distance_m = 60.;
                value.projectile_detail = 4;
            }
        }
        let o = &self.overrides;
        macro_rules! apply { ($($field:ident),*) => { $(if let Some(v) = o.$field { value.$field = v; })* }; }
        apply!(
            shadows,
            shadow_map_size,
            shadow_distance_m,
            shadow_cascades,
            msaa_samples,
            bloom,
            bloom_intensity,
            exposure_ev100,
            emissive_strength,
            projectile_detail,
            vsync,
            depth_prepass,
            occlusion_culling
        );
        if ![512, 1024, 2048, 4096].contains(&value.shadow_map_size) {
            value.shadow_map_size = 2048;
        }
        if ![1, 2, 4, 8].contains(&value.msaa_samples) {
            value.msaa_samples = 4;
        }
        value.shadow_cascades = value.shadow_cascades.clamp(1, 4);
        value.projectile_detail = value.projectile_detail.clamp(1, 4);
        value.shadow_distance_m = finite_clamp(value.shadow_distance_m, 10., 100., 40.);
        value.bloom_intensity = finite_clamp(value.bloom_intensity, 0., 1., 0.12);
        value.exposure_ev100 = finite_clamp(value.exposure_ev100, -4., 20., 9.);
        value.emissive_strength = finite_clamp(value.emissive_strength, 0., 100000., 12000.);
        value
    }
}
/// Clamp a finite value into a range, or return the fallback when it is not finite.
fn finite_clamp(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}
