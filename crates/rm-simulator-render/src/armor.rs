// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! SVG-derived armor artwork. Sources and the offline recipe live in
//! `assets/armor-atlas`; no rule state or SVG toolchain is needed at runtime.
use crate::{RenderingConfig, TeamColor};
use bevy::{
    asset::RenderAssetUsages,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    mesh::VertexAttributeValues,
    prelude::*,
};

/// The caller selects the physical identifier, independently of chassis IDs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArmorPattern {
    /// Hero artwork, identifier 1.
    One,
    /// Engineer artwork, identifier 2.
    Two,
    /// Small infantry armor, identifier 3.
    #[default]
    Three,
    /// Small infantry armor, identifier 4.
    Four,
    /// Small infantry armor, identifier 5.
    Five,
    /// Small guard or sentry armor.
    GuardSmall,
    /// Large guard or sentry armor.
    GuardLarge,
    /// Outpost armor.
    Outpost,
    /// Small base armor.
    BaseSmall,
    /// Large base armor.
    BaseLarge,
    /// Large infantry armor, identifier 3.
    ThreeLarge,
    /// Large infantry armor, identifier 4.
    FourLarge,
    /// Large infantry armor, identifier 5.
    FiveLarge,
}
impl ArmorPattern {
    /// Every pattern, in atlas order.
    pub const ALL: [Self; 13] = [
        Self::One,
        Self::Two,
        Self::Three,
        Self::Four,
        Self::Five,
        Self::GuardSmall,
        Self::GuardLarge,
        Self::Outpost,
        Self::BaseSmall,
        Self::BaseLarge,
        Self::ThreeLarge,
        Self::FourLarge,
        Self::FiveLarge,
    ];
    /// Atlas sprite key, for example `3` or `Gs`.
    pub fn name(self) -> &'static str {
        match self {
            Self::One => "1",
            Self::Two => "2",
            Self::Three => "3",
            Self::Four => "4",
            Self::Five => "5",
            Self::GuardSmall => "Gs",
            Self::GuardLarge => "Gb",
            Self::Outpost => "O",
            Self::BaseSmall => "Bs",
            Self::BaseLarge => "Bb",
            Self::ThreeLarge => "B3",
            Self::FourLarge => "B4",
            Self::FiveLarge => "B5",
        }
    }
}

/// Labels artwork entities for inspection without confusing them with light bars.
#[derive(Component)]
pub struct ArmorArtwork(pub ArmorPattern);

/// Shared smooth emission profile under the armor's white plastic diffuser.
/// The shape of the profile is an appearance choice, not measured radiometry.
#[derive(Resource)]
pub struct DiffuserProfile(pub Handle<Image>);
impl FromWorld for DiffuserProfile {
    fn from_world(world: &mut World) -> Self {
        use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
        let (width, height) = (32, 128);
        let mut pixels = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                let u = 2. * (x as f32 + 0.5) / width as f32 - 1.;
                let v = 2. * (y as f32 + 0.5) / height as f32 - 1.;
                // Soft bright centre, fading into the reflective white rim.
                let across = (1. - u * u).max(0.).powf(1.8);
                let ends = ((1. - v.abs()) / 0.16).clamp(0., 1.);
                let intensity = (255. * across * ends * ends * (3. - 2. * ends)).round() as u8;
                pixels.extend_from_slice(&[intensity, intensity, intensity, 255]);
            }
        }
        let mut image = Image::new(
            Extent3d {
                width: width as u32,
                height: height as u32,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            pixels,
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::default(),
        );
        image.sampler = ImageSampler::linear();
        Self(world.resource_mut::<Assets<Image>>().add(image))
    }
}

/// Opaque milky plastic, including when powered off. The supplied AM12 2019
/// STEP is a shape reference; color and roughness follow the user's appearance
/// correction and are not material measurements from that file.
pub fn diffuser_material(rendering: RenderingConfig) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::srgb(0.84, 0.86, 0.85),
        metallic: 0.,
        perceptual_roughness: 0.32,
        reflectance: 0.4,
        emissive: LinearRgba::BLACK,
        unlit: !rendering.is_lit(),
        ..default()
    }
}

/// Team-coloured emission through the smooth diffuser profile when lit, and the
/// team base colour when the profile is diagnostic. The bright centre is a
/// qualitative fit to photographs, not calibrated LED radiance.
pub fn powered_diffuser_material(
    rendering: RenderingConfig,
    team: TeamColor,
    profile: &DiffuserProfile,
) -> StandardMaterial {
    let mut material = diffuser_material(rendering);
    if rendering.is_lit() {
        // Internal colored light also tints the diffuser body; leaving its
        // reflected component white would wash out team color under arena lights.
        material.base_color = match team {
            TeamColor::Red => Color::srgb(0.95, 0.16, 0.10),
            TeamColor::Blue => Color::srgb(0.08, 0.28, 0.95),
        };
        // Qualitative fit to real Yolo-Detector corpus photographs: bright
        // centers approach white through tone mapping, with colored dimmer edges.
        // These RGB values include a camera appearance fit, not LED spectra.
        let emission = match team {
            TeamColor::Red => LinearRgba::rgb(1., 0.08, 0.035),
            TeamColor::Blue => LinearRgba::rgb(0.035, 0.18, 1.),
        };
        material.emissive = emission * (rendering.emissive_strength * 2.0);
        material.emissive_texture = Some(profile.0.clone());
        material.emissive_exposure_weight = 1.;
    } else {
        material.base_color = team.light().0;
    }
    material
}

/// Rounded, shallow diffuser with a smooth front and visible plastic edges.
/// Existing optical spans are retained; the curvature/depth are visual fits.
pub fn diffuser_mesh(width_m: f32, length_m: f32, depth_m: f32) -> Mesh {
    let mut mesh = Mesh::from(Capsule3d::new(width_m / 2., length_m - width_m))
        .scaled_by(Vec3::new(1., 1., depth_m / width_m));
    let Some(VertexAttributeValues::Float32x3(points)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        unreachable!("capsule positions")
    };
    let uvs: Vec<[f32; 2]> = points
        .iter()
        .map(|p| [p[0] / width_m + 0.5, p[1] / length_m + 0.5])
        .collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh
}

/// Embedded white artwork mask and the sprite rectangles that index it.
#[derive(Resource)]
pub struct ArmorAtlas {
    /// The decoded 2048 by 2048 atlas texture.
    pub image: Handle<Image>,
    /// Parsed `atlas.json`, keyed by [`ArmorPattern::name`].
    layout: serde_json::Value,
}
impl FromWorld for ArmorAtlas {
    fn from_world(world: &mut World) -> Self {
        let image = Image::from_buffer(
            include_bytes!("../../../assets/armor-atlas/atlas.png"),
            ImageType::Extension("png"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::linear(),
            RenderAssetUsages::default(),
        )
        .expect("embedded armor atlas decodes");
        Self {
            image: world.resource_mut::<Assets<Image>>().add(image),
            layout: serde_json::from_str(include_str!("../../../assets/armor-atlas/atlas.json"))
                .expect("embedded armor atlas layout"),
        }
    }
}
impl ArmorAtlas {
    /// Full source canvas, centered on the armor face. Preserve the artwork's
    /// native aspect and whitespace; cropping to the glyph would shift its fit.
    /// `canvas_height_m` is a caller-supplied visual fit, not a rule dimension.
    pub fn mesh(&self, pattern: ArmorPattern, canvas_height_m: f32) -> Mesh {
        let sprite = &self.layout["sprites"][pattern.name()];
        let n = |value: &serde_json::Value| value.as_f64().expect("atlas dimension") as f32;
        let source = &sprite["source_size_px"];
        let mut mesh = Mesh::from(Rectangle::new(
            canvas_height_m * n(&source[0]) / n(&source[1]),
            canvas_height_m,
        ));
        let rect = &sprite["rect_px"];
        let size = &self.layout["size_px"];
        if let Some(VertexAttributeValues::Float32x2(uvs)) =
            mesh.attribute_mut(Mesh::ATTRIBUTE_UV_0)
        {
            for uv in uvs {
                uv[0] = (n(&rect[0]) + uv[0] * n(&rect[2])) / n(&size[0]);
                uv[1] = (n(&rect[1]) + uv[1] * n(&rect[3])) / n(&size[1]);
            }
        }
        mesh
    }

    /// Passive white printing: receives scene lighting and never emits light.
    /// Damage and defeat change the light bars, not this printed pattern.
    pub fn material(&self, rendering: RenderingConfig) -> StandardMaterial {
        StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(self.image.clone()),
            emissive: LinearRgba::BLACK,
            perceptual_roughness: 0.8,
            alpha_mode: AlphaMode::Blend,
            unlit: !rendering.is_lit(),
            ..default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffuser_keeps_optical_dimensions_and_white_plastic_when_off() {
        let mesh = diffuser_mesh(0.010, 0.056, 0.004);
        let VertexAttributeValues::Float32x3(points) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
        else {
            panic!("positions")
        };
        for (axis, expected) in [0.010, 0.056, 0.004].into_iter().enumerate() {
            let extent = points.iter().map(|p| p[axis]).reduce(f32::max).unwrap()
                - points.iter().map(|p| p[axis]).reduce(f32::min).unwrap();
            assert!((extent - expected).abs() < 1e-6);
        }
        let off = diffuser_material(RenderingConfig::field());
        assert_eq!(off.emissive, LinearRgba::BLACK);
        assert_eq!(off.alpha_mode, AlphaMode::Opaque);
        assert_eq!(off.metallic, 0.);
        let color = off.base_color.to_srgba();
        assert!(color.red > 0.8 && color.green > 0.8 && color.blue > 0.8);
        let mut world = World::new();
        world.init_resource::<Assets<Image>>();
        let profile = DiffuserProfile::from_world(&mut world);
        let image = world.resource::<Assets<Image>>().get(&profile.0).unwrap();
        let pixels = image.data.as_ref().unwrap();
        let sample = |x: usize, y: usize| pixels[(y * 32 + x) * 4];
        assert!(sample(16, 64) > sample(1, 64));
        assert!(sample(16, 64) > sample(16, 1));
        for team in [TeamColor::Red, TeamColor::Blue] {
            let on = powered_diffuser_material(RenderingConfig::field(), team, &profile);
            assert_ne!(on.emissive, LinearRgba::BLACK);
            assert_eq!(on.emissive_texture, Some(profile.0.clone()));
        }
    }

    #[test]
    fn every_sprite_preserves_aspect_and_uses_only_its_atlas_rectangle() {
        let mut world = World::new();
        world.init_resource::<Assets<Image>>();
        let atlas = ArmorAtlas::from_world(&mut world);
        let image = world.resource::<Assets<Image>>().get(&atlas.image).unwrap();
        assert_eq!(image.width(), 2048);
        assert_eq!(image.height(), 2048);
        for pattern in ArmorPattern::ALL {
            let mesh = atlas.mesh(pattern, 0.113);
            let sprite = &atlas.layout["sprites"][pattern.name()];
            let values = |key: &str| -> Vec<f32> {
                sprite[key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap() as f32)
                    .collect()
            };
            let rect = values("rect_px");
            let source = values("source_size_px");
            let VertexAttributeValues::Float32x2(uvs) =
                mesh.attribute(Mesh::ATTRIBUTE_UV_0).unwrap()
            else {
                panic!("UVs")
            };
            for uv in uvs {
                assert!((rect[0]..=rect[0] + rect[2]).contains(&(uv[0] * 2048.)));
                assert!((rect[1]..=rect[1] + rect[3]).contains(&(uv[1] * 2048.)));
            }
            let VertexAttributeValues::Float32x3(points) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
            else {
                panic!("positions")
            };
            let width = points.iter().map(|p| p[0]).reduce(f32::max).unwrap()
                - points.iter().map(|p| p[0]).reduce(f32::min).unwrap();
            assert!((width / 0.113 - source[0] / source[1]).abs() < 1e-5);
            assert!(points.iter().all(|p| p[2] == 0.));
            let pixels = image.data.as_ref().unwrap();
            let (x, y, w, h) = (
                rect[0] as usize,
                rect[1] as usize,
                rect[2] as usize,
                rect[3] as usize,
            );
            assert!(
                (y..y + h)
                    .any(|row| (x..x + w).any(|col| pixels[(row * 2048 + col) * 4 + 3] == 255))
            );
            // Empty gutters prevent neighbouring symbols leaking through linear sampling.
            assert_eq!(pixels[(y * 2048 + x - 1) * 4 + 3], 0);
        }
    }

    #[test]
    fn artwork_is_passive_white_printing() {
        let mut world = World::new();
        world.init_resource::<Assets<Image>>();
        let atlas = ArmorAtlas::from_world(&mut world);
        let material = atlas.material(RenderingConfig::field());
        assert_eq!(material.base_color, Color::WHITE);
        assert_eq!(material.base_color_texture, Some(atlas.image.clone()));
        assert_eq!(material.alpha_mode, AlphaMode::Blend);
        assert_eq!(material.emissive, LinearRgba::BLACK);
        assert!(material.emissive_texture.is_none());
        assert!(!material.unlit);
    }
}
