// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Barrel heat ring around the reticle: one UI node drawn by `heat_ring.wgsl`
//! as an antialiased circle that fills clockwise from the top.
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

/// Uniform block of [`HeatRingMaterial`], laid out as `HeatRing` in the shader.
#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub struct HeatRingUniform {
    /// Linear RGBA of the filled arc.
    pub fill: Vec4,
    /// Linear RGBA of the unfilled track.
    pub track: Vec4,
    /// Filled fraction of the circle, clamped to 0..=1 by the shader.
    pub fraction: f32,
    /// Stroke width, in logical pixels.
    pub thickness_px: f32,
}

/// UI material for the heat ring; the node's size sets the ring's outer diameter.
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct HeatRingMaterial {
    /// Colours, fill fraction and stroke width.
    #[uniform(0)]
    pub ring: HeatRingUniform,
}

impl UiMaterial for HeatRingMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://rm_simulator/hud/heat_ring.wgsl".into()
    }
}

/// Register the embedded shader and the material pipeline.
pub(super) fn register(app: &mut App) {
    bevy::asset::embedded_asset!(app, "heat_ring.wgsl");
    app.add_plugins(UiMaterialPlugin::<HeatRingMaterial>::default());
}
