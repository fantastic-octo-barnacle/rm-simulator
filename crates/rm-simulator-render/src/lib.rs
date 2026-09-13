// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Bevy rendering of the RoboMaster field from caller-owned scene state.
//! This crate never advances rules or reads a simulation clock.
#![deny(missing_docs)]
pub mod armor;
pub mod cad;
pub mod chassis;
mod equipment;
pub mod graphics_settings;
pub mod lighting;
pub mod outpost;
pub mod projectile;
pub mod quality;
pub mod rune;
pub mod sync;
use bevy::{
    camera::{Exposure, Hdr},
    core_pipeline::tonemapping::Tonemapping,
    post_process::bloom::Bloom,
    prelude::*,
};

/// Floor height in renderer Y-up metres. World FLU z=0 is the floor.
pub const FLOOR_Y_M: f32 = 0.0;

/// Lit field appearance and an explicit unlit diagnostic fixture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RenderingProfile {
    /// Unlit view: no HDR, exposure or bloom is added to the camera.
    #[default]
    Diagnostic,
    /// Photo-informed indoor venue look. Not sensor calibrated.
    Field,
}
/// Team colour of a rune or armor light.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TeamColor {
    /// Red team; the red half of the field is FLU +x.
    Red,
    /// Blue team; the blue half of the field is FLU -x.
    #[default]
    Blue,
}
impl TeamColor {
    /// Base colour and linear emission of a lit light bar or rune target.
    pub fn light(self) -> (Color, LinearRgba) {
        match self {
            TeamColor::Red => (
                Color::srgb(1.0, 0.12, 0.08),
                LinearRgba::rgb(1.0, 0.03, 0.01),
            ),
            TeamColor::Blue => (Color::srgb(0.0, 0.35, 1.0), LinearRgba::rgb(0.0, 0.2, 1.0)),
        }
    }
}

/// Camera and emissive appearance shared by every renderer plugin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderingConfig {
    /// Which look is applied, and therefore whether post-processing is added.
    pub profile: RenderingProfile,
    /// Camera exposure in EV100; read only for a lit profile.
    pub exposure_ev100: f32,
    /// Linear RGB emission multiplier, not measured LED radiance.
    pub emissive_strength: f32,
    /// Bloom intensity in [0, 1]; read only for a lit profile.
    pub bloom_intensity: f32,
}
impl Default for RenderingConfig {
    fn default() -> Self {
        Self {
            profile: RenderingProfile::Diagnostic,
            exposure_ev100: 9.0,
            emissive_strength: 1000.0,
            bloom_intensity: 0.15,
        }
    }
}
impl RenderingConfig {
    /// The lit venue profile: HDR, 9 EV100 exposure and a 0.12 bloom intensity.
    ///
    /// ```
    /// use rm_simulator_render::RenderingConfig;
    ///
    /// let field = RenderingConfig::field();
    /// assert!(field.is_lit());
    /// assert!(field.is_valid());
    /// // The diagnostic profile leaves the caller's camera untouched.
    /// assert!(!RenderingConfig::default().is_lit());
    /// // Values outside the supported ranges are rejected.
    /// let out_of_range = RenderingConfig {
    ///     bloom_intensity: 1.5,
    ///     ..field
    /// };
    /// assert!(!out_of_range.is_valid());
    /// ```
    pub fn field() -> Self {
        Self {
            profile: RenderingProfile::Field,
            exposure_ev100: 9.0,
            emissive_strength: 12000.0,
            bloom_intensity: 0.12,
        }
    }
    /// Whether exposure, emission and bloom are finite and inside their ranges.
    pub fn is_valid(self) -> bool {
        self.exposure_ev100.is_finite()
            && (-4.0..=20.0).contains(&self.exposure_ev100)
            && self.emissive_strength.is_finite()
            && (0.0..=100000.0).contains(&self.emissive_strength)
            && self.bloom_intensity.is_finite()
            && (0.0..=1.0).contains(&self.bloom_intensity)
    }
    /// Whether this profile adds HDR, fixed exposure and bloom to the camera.
    pub fn is_lit(self) -> bool {
        self.profile != RenderingProfile::Diagnostic
    }
}
/// Apply the shared camera appearance without resetting caller-owned `Camera` fields.
pub fn camera_appearance(camera: &mut bevy::ecs::system::EntityCommands, config: RenderingConfig) {
    assert!(config.is_valid(), "invalid rendering configuration");
    if config.is_lit() {
        let clear_color = ClearColorConfig::Custom(Color::srgb(0.055, 0.075, 0.11));
        camera
            .entry::<Camera>()
            .and_modify(move |mut camera| camera.clear_color = clear_color)
            .or_insert(Camera {
                clear_color,
                ..default()
            });
        camera.insert((
            Hdr,
            Exposure {
                ev100: config.exposure_ev100,
            },
            Bloom {
                intensity: config.bloom_intensity,
                ..default()
            },
            // Fixed exposure followed by output clipping preserves saturated LED cores.
            Tonemapping::None,
        ));
    }
}
/// A normalized rigid pose in forward/left/up coordinates, metres and wxyz.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PoseFlu {
    /// Translation in FLU metres.
    pub translation_m: [f64; 3],
    /// Normalized rotation in wxyz order.
    pub rotation_wxyz: [f64; 4],
}
impl Default for PoseFlu {
    fn default() -> Self {
        Self {
            translation_m: [0.0; 3],
            rotation_wxyz: [1.0, 0.0, 0.0, 0.0],
        }
    }
}
/// Convert an FLU position in metres to Bevy coordinates.
pub fn flu_position(p: [f64; 3]) -> Vec3 {
    Vec3::new(-p[1] as f32, p[2] as f32, -p[0] as f32)
}
/// Bevy vector to FLU components.
pub fn flu_vector(v: Vec3) -> [f64; 3] {
    [-v.z as f64, -v.x as f64, v.y as f64]
}
/// Rotation from FLU axes to Bevy axes: forward -Z, left -X, up +Y.
fn flu_conversion() -> Quat {
    Quat::from_mat3(&Mat3::from_cols(Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y))
}
/// Set the pose without affecting scale. Caller supplies a finite normalized pose.
///
/// ```
/// use bevy::prelude::*;
/// use rm_simulator_render::{PoseFlu, apply_pose, flu_position, pose_from_transform};
///
/// // The renderer converts FLU metres to Bevy units: forward +x becomes -Z.
/// assert_eq!(flu_position([1.0, 0.0, 0.0]), Vec3::NEG_Z);
/// assert_eq!(flu_position([0.0, 0.0, 1.0]), Vec3::Y);
///
/// let mut transform = Transform::from_scale(Vec3::splat(2.0));
/// apply_pose(
///     &mut transform,
///     PoseFlu {
///         translation_m: [3.0, 2.0, 1.0],
///         rotation_wxyz: [1.0, 0.0, 0.0, 0.0],
///     },
/// );
/// assert_eq!(transform.translation, Vec3::new(-2.0, 1.0, -3.0));
/// assert_eq!(transform.scale, Vec3::splat(2.0));
/// assert_eq!(pose_from_transform(&transform).translation_m, [3.0, 2.0, 1.0]);
/// ```
pub fn apply_pose(transform: &mut Transform, pose: PoseFlu) {
    let conversion = flu_conversion();
    let [w, x, y, z] = pose.rotation_wxyz;
    transform.translation = flu_position(pose.translation_m);
    transform.rotation =
        conversion * Quat::from_xyzw(x as f32, y as f32, z as f32, w as f32) * conversion.inverse();
}
/// Inverse of `apply_pose`: read a Bevy transform back as an FLU pose. Scale is ignored.
pub fn pose_from_transform(transform: &Transform) -> PoseFlu {
    let conversion = flu_conversion();
    let q = (conversion.inverse() * transform.rotation * conversion).normalize();
    PoseFlu {
        translation_m: flu_vector(transform.translation),
        rotation_wxyz: [q.w as f64, q.x as f64, q.y as f64, q.z as f64],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_validate_and_only_field_adds_hdr_postprocessing() {
        assert!(RenderingConfig::default().is_valid());
        assert!(RenderingConfig::field().is_valid());
        assert!(
            !RenderingConfig {
                exposure_ev100: f32::NAN,
                ..RenderingConfig::field()
            }
            .is_valid()
        );
        assert!(
            !RenderingConfig {
                bloom_intensity: 1.1,
                ..RenderingConfig::field()
            }
            .is_valid()
        );
        let mut app = App::new();
        app.add_systems(Startup, |mut commands: Commands| {
            let mut diagnostic = commands.spawn((Camera3d::default(), Name::new("diagnostic")));
            camera_appearance(&mut diagnostic, RenderingConfig::default());
            let mut arena = commands.spawn((
                Camera3d::default(),
                Name::new("field"),
                Camera {
                    order: 7,
                    ..default()
                },
            ));
            camera_appearance(&mut arena, RenderingConfig::field());
        });
        app.update();
        let world = app.world_mut();
        for (name, camera, hdr, bloom, exposure) in world
            .query::<(
                &Name,
                &Camera,
                Option<&Hdr>,
                Option<&Bloom>,
                Option<&Exposure>,
            )>()
            .iter(world)
        {
            if name.as_str() == "field" {
                assert!(hdr.is_some());
                assert_eq!(bloom.unwrap().intensity, 0.12);
                assert_eq!(exposure.unwrap().ev100, 9.0);
                assert_eq!(camera.order, 7);
                assert!(matches!(camera.clear_color, ClearColorConfig::Custom(_)));
            } else {
                assert!(hdr.is_none());
                assert!(bloom.is_none());
                assert!(matches!(camera.clear_color, ClearColorConfig::Default));
            }
        }
    }
    #[test]
    fn flu_forward_and_yaw_match_bevy_camera_axes() {
        assert_eq!(flu_position([1.0, 0.0, 0.0]), Vec3::NEG_Z);
        assert_eq!(flu_position([0.0, 1.0, 0.0]), Vec3::NEG_X);
        assert_eq!(flu_position([0.0, 0.0, 1.0]), Vec3::Y);
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let mut transform = Transform::from_scale(Vec3::splat(2.0));
        apply_pose(
            &mut transform,
            PoseFlu {
                translation_m: [3.0, 2.0, 1.0],
                rotation_wxyz: [h, 0.0, 0.0, h],
            },
        );
        assert!((transform.rotation * Vec3::NEG_Z - Vec3::NEG_X).length() < 1e-6);
        assert_eq!(transform.translation, Vec3::new(-2.0, 1.0, -3.0));
        assert_eq!(transform.scale, Vec3::splat(2.0));
    }
    #[test]
    fn pose_round_trips_through_bevy_transforms() {
        let h = std::f64::consts::FRAC_1_SQRT_2;
        for pose in [
            PoseFlu::default(),
            PoseFlu {
                translation_m: [3.0, 2.0, 1.0],
                rotation_wxyz: [h, 0.0, 0.0, h],
            },
            PoseFlu {
                translation_m: [-1.0, 0.5, 4.0],
                rotation_wxyz: [0.5, 0.5, 0.5, 0.5],
            },
        ] {
            let mut transform = Transform::from_scale(Vec3::splat(3.0));
            apply_pose(&mut transform, pose);
            let back = pose_from_transform(&transform);
            for i in 0..3 {
                assert!((back.translation_m[i] - pose.translation_m[i]).abs() < 1e-6);
            }
            // q and -q are the same rotation.
            let sign = if back.rotation_wxyz[0] * pose.rotation_wxyz[0] < 0.0 {
                -1.0
            } else {
                1.0
            };
            for i in 0..4 {
                assert!((sign * back.rotation_wxyz[i] - pose.rotation_wxyz[i]).abs() < 1e-6);
            }
        }
        // A Bevy transform facing +X reads as an FLU yaw of -90 degrees (facing -y, right).
        let facing_x =
            Transform::from_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2));
        let pose = pose_from_transform(&facing_x);
        let [w, _, _, z] = pose.rotation_wxyz;
        assert!((2.0 * z.atan2(w) + std::f64::consts::FRAC_PI_2).abs() < 1e-6);
    }
}
