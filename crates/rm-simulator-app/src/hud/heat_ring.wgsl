// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
// Heat ring: a circle stroke inscribed in the node, filled clockwise from the
// top up to `fraction`, antialiased across the stroke and the fill edge.
#import bevy_ui::ui_vertex_output::UiVertexOutput

struct HeatRing {
    fill: vec4<f32>,
    track: vec4<f32>,
    fraction: f32,
    thickness_px: f32,
}

@group(1) @binding(0)
var<uniform> ring: HeatRing;

const TAU: f32 = 6.28318530718;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    // Pixels from the node centre, y down.
    let p = (in.uv - vec2<f32>(0.5)) * in.size;
    let radius = 0.5 * min(in.size.x, in.size.y) - 0.5 * ring.thickness_px - 1.0;
    let r = length(p);
    let stroke = clamp(0.5 * ring.thickness_px + 0.5 - abs(r - radius), 0.0, 1.0);
    if stroke <= 0.0 {
        discard;
    }
    // Clockwise angle from the top, in 0..TAU.
    var angle = atan2(p.x, -p.y);
    if angle < 0.0 {
        angle += TAU;
    }
    let fraction = clamp(ring.fraction, 0.0, 1.0);
    var filled = clamp((fraction * TAU - angle) * radius + 0.5, 0.0, 1.0);
    if fraction >= 1.0 {
        filled = 1.0;
    } else if fraction <= 0.0 {
        filled = 0.0;
    }
    let color = mix(ring.track, ring.fill, filled);
    return vec4<f32>(color.rgb, color.a * stroke);
}
