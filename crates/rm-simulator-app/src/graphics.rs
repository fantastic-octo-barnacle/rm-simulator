// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Persisted graphics preferences and live application to the passive renderer.
use crate::{controls::PlayerCamera, preferences::Preferences};
use bevy::{
    camera::Exposure,
    core_pipeline::prepass::DepthPrepass,
    feathers::{controls::FeathersButton, theme::ThemedText},
    input_focus::{InputFocus, InputFocusVisible},
    light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap},
    picking::hover::Hovered,
    post_process::bloom::Bloom,
    prelude::*,
    render::{
        batching::gpu_preprocessing::GpuPreprocessingSupport,
        occlusion_culling::OcclusionCulling,
        render_resource::{TextureFormat, WgpuFeatures},
        renderer::{RenderAdapter, RenderDevice},
    },
    ui_widgets::{
        Activate, ActivateOnPress,
        popover::{Popover, PopoverAlign, PopoverPlacement, PopoverSide},
    },
    window::{PresentMode, PrimaryWindow},
};
use rm_simulator_render::{
    projectile::ProjectileDetail,
    quality::{EmissionStrength, MaterialQualityPlugin},
};

/// The renderer's quality preset, its per-field overrides and the resolved
/// result, re-exported so the preferences document and the graphics menu name
/// one set of types.
pub use rm_simulator_render::graphics_settings::{
    GraphicsOverrides, GraphicsSettings, ResolvedGraphics,
};

#[derive(Resource, Default)]
struct AppliedGraphics(Option<ResolvedGraphics>);
#[derive(Resource, Default)]
struct GraphicsNotice(String);
#[derive(Resource, Default)]
struct CullingSupport {
    ready: bool,
    supported: bool,
}

fn refresh_capabilities(world: &mut World) {
    let Some(device) = world.get_resource_ref::<RenderDevice>() else {
        return;
    };
    if world.resource::<CullingSupport>().ready && !device.is_changed() {
        return;
    }
    if !world.contains_resource::<RenderAdapter>() {
        return;
    }
    let supported = GpuPreprocessingSupport::from_world(world).is_culling_supported();
    *world.resource_mut::<CullingSupport>() = CullingSupport {
        ready: true,
        supported,
    };
}

/// Adds the material quality plugin and applies the saved graphics preferences.
/// The preset is resolved against adapter limits and support, then only changed
/// values are written. Also keeps the settings menu labels and notices current.
pub struct GraphicsPlugin;
impl Plugin for GraphicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialQualityPlugin)
            .init_resource::<AppliedGraphics>()
            .init_resource::<GraphicsNotice>()
            .init_resource::<CullingSupport>()
            .add_systems(Update, (refresh_capabilities, apply_graphics).chain())
            .add_systems(Update, graphics_tooltips)
            .add_systems(PostUpdate, refresh_labels);
    }
}

fn effective_settings(
    requested: ResolvedGraphics,
    supported_samples: &[u32],
    max_texture_size: usize,
) -> ResolvedGraphics {
    let mut effective = requested;
    effective.msaa_samples = supported_samples
        .iter()
        .copied()
        .filter(|s| *s <= requested.msaa_samples)
        .max()
        .unwrap_or(1);
    effective.shadow_map_size = [512, 1024, 2048, 4096]
        .into_iter()
        .filter(|s| *s <= max_texture_size && *s <= requested.shadow_map_size)
        .max()
        .unwrap_or(512);
    effective
}

#[allow(clippy::too_many_arguments)]
fn apply_graphics(
    mut commands: Commands,
    preferences: Res<Preferences>,
    adapter: Option<Res<RenderAdapter>>,
    device: Option<Res<RenderDevice>>,
    culling: Res<CullingSupport>,
    mut applied: ResMut<AppliedGraphics>,
    mut notice: ResMut<GraphicsNotice>,
    mut cameras: Query<(Entity, &mut Exposure, &mut Msaa), With<PlayerCamera>>,
    new_cameras: Query<(), Added<PlayerCamera>>,
    mut lights: Query<
        (Entity, &mut DirectionalLight),
        With<rm_simulator_render::lighting::ShadowKeyLight>,
    >,
    new_lights: Query<(), Added<rm_simulator_render::lighting::ShadowKeyLight>>,
    mut shadow_map: ResMut<DirectionalLightShadowMap>,
    mut emission: ResMut<EmissionStrength>,
    mut detail: ResMut<ProjectileDetail>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if !preferences.is_changed()
        && !culling.is_changed()
        && new_cameras.is_empty()
        && new_lights.is_empty()
    {
        return;
    }
    let requested = preferences.graphics.resolved();
    let mut settings = if let (Some(adapter), Some(device)) = (adapter, device) {
        let format_features = |format: TextureFormat| {
            if device
                .features()
                .contains(WgpuFeatures::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
            {
                adapter.get_texture_format_features(format)
            } else {
                format.guaranteed_format_features(device.features())
            }
        };
        let color = format_features(TextureFormat::Rgba16Float);
        let depth = format_features(TextureFormat::Depth32Float);
        let supported: Vec<_> = [1, 2, 4, 8]
            .into_iter()
            .filter(|samples| {
                color.flags.sample_count_supported(*samples)
                    && depth.flags.sample_count_supported(*samples)
            })
            .collect();
        effective_settings(
            requested,
            &supported,
            device.limits().max_texture_dimension_2d as usize,
        )
    } else {
        // The renderer can initialize asynchronously. Use universally supported
        // one-sample rendering until the adapter is available, then resolve again.
        effective_settings(requested, &[1], 2048)
    };
    settings.occlusion_culling &= culling.supported;
    settings.depth_prepass |= settings.occlusion_culling;
    notice.0 = if settings.msaa_samples != requested.msaa_samples
        || settings.shadow_map_size != requested.shadow_map_size
    {
        format!(
            "GPU fallback: {}× MSAA, {} px shadows. Saved choices are retained.",
            settings.msaa_samples, settings.shadow_map_size
        )
    } else {
        "All graphics changes apply immediately.".into()
    };
    if !settings.bloom {
        notice.0 = format!(
            "WARNING: Bloom is disabled. Bright armor plates and target lights may change apparent color. Enable Bloom for the normal appearance. {}",
            notice.0
        );
    }
    if settings.emissive_strength != 12000. {
        notice.0 = format!(
            "WARNING: Custom light emission changes armor plate and target colors and can hide gameplay indicators. Restore 12000 for normal appearance. {}",
            notice.0
        );
    }
    if requested.occlusion_culling && !settings.occlusion_culling {
        notice.0.push_str(
            " Occlusion culling is unavailable on this GPU; using ordinary visibility culling.",
        );
    } else if settings.occlusion_culling {
        notice
            .0
            .push_str(" Occlusion culling also enables depth prepass.");
    }
    if applied.0 == Some(settings) && new_cameras.is_empty() && new_lights.is_empty() {
        return;
    }
    for (entity, mut exposure, mut msaa) in &mut cameras {
        if exposure.ev100 != settings.exposure_ev100 {
            exposure.ev100 = settings.exposure_ev100;
        }
        let samples = Msaa::from_samples(settings.msaa_samples);
        if *msaa != samples {
            *msaa = samples;
        }
        if settings.depth_prepass {
            commands.entity(entity).insert(DepthPrepass);
        } else {
            commands.entity(entity).remove::<DepthPrepass>();
        }
        if settings.occlusion_culling {
            commands.entity(entity).insert(OcclusionCulling);
        } else {
            commands.entity(entity).remove::<OcclusionCulling>();
        }
        if settings.bloom {
            commands.entity(entity).insert(Bloom {
                intensity: settings.bloom_intensity,
                ..default()
            });
        } else {
            commands.entity(entity).remove::<Bloom>();
        }
    }
    for (entity, mut light) in &mut lights {
        if light.shadow_maps_enabled != settings.shadows {
            light.shadow_maps_enabled = settings.shadows;
        }
        commands.entity(entity).insert(
            CascadeShadowConfigBuilder {
                num_cascades: settings.shadow_cascades,
                first_cascade_far_bound: (settings.shadow_distance_m * 0.5).min(10.),
                maximum_distance: settings.shadow_distance_m,
                ..default()
            }
            .build(),
        );
    }
    if shadow_map.size != settings.shadow_map_size {
        shadow_map.size = settings.shadow_map_size;
    }
    if emission.0 != settings.emissive_strength {
        emission.0 = settings.emissive_strength;
    }
    if detail.0 != settings.projectile_detail {
        detail.0 = settings.projectile_detail;
    }
    for mut window in &mut windows {
        let mode = if settings.vsync {
            PresentMode::AutoVsync
        } else {
            PresentMode::AutoNoVsync
        };
        if window.present_mode != mode {
            window.present_mode = mode;
        }
    }
    applied.0 = Some(settings);
}

#[derive(Component, Clone, Copy, Default)]
enum GraphicsAction {
    #[default]
    Preset,
    Reset,
    Shadows,
    ShadowSize,
    ShadowDistance,
    Cascades,
    Msaa,
    Bloom,
    BloomIntensity,
    Exposure,
    Emission,
    Projectile,
    Vsync,
    DepthPrepass,
    Occlusion,
}
#[derive(Component, Clone, Default)]
struct GraphicsLabel(GraphicsAction);
#[derive(Component)]
struct NoticeLabel;

#[derive(Component, Default)]
struct GraphicsTooltip {
    hovered_seconds: f32,
}

fn graphics_tooltips(
    time: Res<Time>,
    ui: Res<crate::hud::HudState>,
    focus: Res<InputFocus>,
    focus_visible: Res<InputFocusVisible>,
    buttons: Query<(&Hovered, &ComputedNode), With<GraphicsAction>>,
    mut tips: Query<(&ChildOf, &mut Node, &mut GraphicsTooltip)>,
) {
    for (parent, mut node, mut tip) in &mut tips {
        let active = ui.settings
            && buttons.get(parent.parent()).is_ok_and(|(hover, computed)| {
                !computed.is_empty()
                    && (hover.0 || (focus_visible.0 && focus.get() == Some(parent.parent())))
            });
        tip.hovered_seconds = if active {
            tip.hovered_seconds + time.delta_secs()
        } else {
            0.
        };
        let display = if active && tip.hovered_seconds >= 0.35 {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
}

/// Spawns the graphics section of the settings menu under `parent`: one button per
/// option with a hover tooltip, plus the notice line. Each button writes an override
/// into `Preferences`; `settings` only supplies the initial labels.
pub fn spawn_graphics_controls(
    commands: &mut Commands,
    parent: Entity,
    settings: &GraphicsSettings,
) {
    commands.spawn((
        ChildOf(parent),
        Text::new("Graphics"),
        TextFont {
            font_size: 18.0.into(),
            ..default()
        },
    ));
    for action in [
        GraphicsAction::Preset,
        GraphicsAction::Reset,
        GraphicsAction::Shadows,
        GraphicsAction::ShadowSize,
        GraphicsAction::ShadowDistance,
        GraphicsAction::Cascades,
        GraphicsAction::Msaa,
        GraphicsAction::Bloom,
        GraphicsAction::BloomIntensity,
        GraphicsAction::Exposure,
        GraphicsAction::Emission,
        GraphicsAction::Projectile,
        GraphicsAction::Vsync,
        GraphicsAction::DepthPrepass,
        GraphicsAction::Occlusion,
    ] {
        let label = action.label(settings);
        let accessible = label.clone();
        let button = commands.spawn_scene(bsn! {
            @FeathersButton { @caption: bsn! { Text(label) ThemedText GraphicsLabel(action) } }
            ActivateOnPress
            AccessibleLabel(accessible)
            on(move |_: On<Activate>, mut preferences: ResMut<Preferences>| { action.apply(&mut preferences.graphics); })
        }).insert((ChildOf(parent), action)).id();
        commands.spawn((
            ChildOf(button),
            GraphicsTooltip::default(),
            Text::new(action.description()),
            TextFont {
                font_size: 14.0.into(),
                ..default()
            },
            TextColor(Color::srgb(0.94, 0.96, 0.98)),
            BackgroundColor(Color::srgb(0.025, 0.04, 0.055)),
            BorderColor::all(Color::srgb(0.3, 0.6, 0.65)),
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                width: px(360),
                max_width: vw(90),
                padding: UiRect::all(px(12)),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            GlobalZIndex(500),
            bevy::ui::OverrideClip,
            Pickable::IGNORE,
            Popover {
                positions: [
                    PopoverSide::Right,
                    PopoverSide::Left,
                    PopoverSide::Bottom,
                    PopoverSide::Top,
                ]
                .map(|side| PopoverPlacement {
                    side,
                    align: PopoverAlign::Center,
                    gap: 12.,
                })
                .to_vec(),
                window_margin: 12.,
            },
        ));
    }
    commands.spawn((
        ChildOf(parent),
        NoticeLabel,
        Text::new("All graphics changes apply immediately."),
        TextFont {
            font_size: 12.0.into(),
            ..default()
        },
    ));
}
fn cycle<T: Copy + PartialEq>(value: T, choices: &[T]) -> T {
    let index = choices
        .iter()
        .position(|v| *v == value)
        .unwrap_or(choices.len() - 1);
    choices[(index + 1) % choices.len()]
}
impl GraphicsAction {
    fn description(self) -> &'static str {
        match self {
            Self::Preset => {
                "Selects a starting quality level and clears custom overrides. Higher levels trade rendering time and GPU memory for image quality."
            }
            Self::Reset => {
                "Restores every graphics option to the selected quality preset. Your control bindings are kept."
            }
            Self::Shadows => {
                "Adds shadows from the main light. High GPU cost in the field scene, especially at 1080p. Turning this off removes shadow rendering and its GPU memory cost."
            }
            Self::ShadowSize => {
                "Sets the resolution of each shadow cascade. Higher values sharpen shadow edges but use more GPU memory and rendering time. Doubling the size quadruples the texels per cascade. Has no effect when shadows are off."
            }
            Self::ShadowDistance => {
                "Sets how far shadows extend from the camera. Shorter distances can avoid drawing distant objects into shadow maps, but distant shadows disappear. Has no effect when shadows are off."
            }
            Self::Cascades => {
                "Splits the shadow range into separate maps to keep nearby shadows sharp. High GPU impact: more cascades mean more shadow rendering and GPU memory. Has no effect when shadows are off."
            }
            Self::Msaa => {
                "Smooths polygon edges. Moderate GPU impact at 4K in field tests. More samples use more GPU memory and bandwidth. 1x disables MSAA. It does not smooth every texture or shader edge."
            }
            Self::Bloom => {
                "WARNING: Disabling bloom can change the apparent colors of bright armor plates and target lights. All presets keep it enabled. Small to moderate GPU cost from extra full-screen processing and image buffers."
            }
            Self::BloomIntensity => {
                "Changes glow strength while bloom is enabled. Lower intensity does not skip bloom processing. Disabling bloom saves rendering time but can alter armor and target colors."
            }
            Self::Exposure => {
                "Changes image brightness. Higher EV100 makes the image darker. This is an appearance control, with no expected meaningful performance saving."
            }
            Self::Emission => {
                "WARNING: Reducing or disabling emission changes armor plate and target colors and can hide gameplay indicators. Keep 12000 for normal appearance, including on Low. Changing emission offers no expected meaningful performance saving."
            }
            Self::Projectile => {
                "Changes the smoothness of projectile spheres. Cost depends on how much firing is happening. The static rendering benchmark does not measure this setting."
            }
            Self::Vsync => {
                "Synchronizes presentation with the display to reduce tearing. Can cap frame rate and add input latency. Turning it off removes the display refresh cap, but does not reduce rendering work."
            }
            Self::DepthPrepass => {
                "Draws depth before shading. It can reduce repeated shading of overlapping surfaces, but adds another geometry pass. Performance depends on the scene and GPU. Occlusion culling requires it."
            }
            Self::Occlusion => {
                "Uses depth to skip objects hidden behind other geometry. Requires a depth prepass and GPU support. Its extra passes can cost more than they save in open views; test your usual camera positions."
            }
        }
    }
    fn apply(self, settings: &mut GraphicsSettings) {
        let value = settings.resolved();
        let o = &mut settings.overrides;
        match self {
            Self::Preset => {
                settings.preset = settings.preset.next();
                settings.overrides = default();
            }
            Self::Reset => settings.overrides = default(),
            Self::Shadows => o.shadows = Some(!value.shadows),
            Self::ShadowSize => {
                o.shadow_map_size = Some(cycle(value.shadow_map_size, &[512, 1024, 2048, 4096]))
            }
            Self::ShadowDistance => {
                o.shadow_distance_m = Some(cycle(value.shadow_distance_m, &[20., 40., 60., 100.]))
            }
            Self::Cascades => o.shadow_cascades = Some(cycle(value.shadow_cascades, &[1, 2, 3, 4])),
            Self::Msaa => o.msaa_samples = Some(cycle(value.msaa_samples, &[1, 2, 4, 8])),
            Self::Bloom => o.bloom = Some(!value.bloom),
            Self::BloomIntensity => {
                o.bloom_intensity = Some(cycle(
                    value.bloom_intensity,
                    &[0., 0.06, 0.12, 0.2, 0.4, 1.],
                ))
            }
            Self::Exposure => {
                o.exposure_ev100 = Some(cycle(value.exposure_ev100, &[7., 8., 9., 10., 11.]))
            }
            Self::Emission => {
                o.emissive_strength = Some(cycle(
                    value.emissive_strength,
                    &[0., 3000., 6000., 12000., 24000., 48000.],
                ))
            }
            Self::Projectile => {
                o.projectile_detail = Some(cycle(value.projectile_detail, &[1, 2, 3, 4]))
            }
            Self::Vsync => o.vsync = Some(!value.vsync),
            Self::DepthPrepass => o.depth_prepass = Some(!value.depth_prepass),
            Self::Occlusion => o.occlusion_culling = Some(!value.occlusion_culling),
        }
    }
    fn label(self, settings: &GraphicsSettings) -> String {
        let v = settings.resolved();
        let on = |value| if value { "On" } else { "Off" };
        match self {
            Self::Preset => format!(
                "Quality: {:?}{}",
                settings.preset,
                if settings.overrides == GraphicsOverrides::default() {
                    ""
                } else {
                    " • Custom"
                }
            ),
            Self::Reset => "Reset graphics to selected preset".into(),
            Self::Shadows => format!("Shadows: {}", on(v.shadows)),
            Self::ShadowSize => format!("Shadow resolution: {} px", v.shadow_map_size),
            Self::ShadowDistance => format!("Shadow distance: {} m", v.shadow_distance_m),
            Self::Cascades => format!("Shadow cascades: {}", v.shadow_cascades),
            Self::Msaa => format!("Antialiasing: {}× MSAA", v.msaa_samples),
            Self::Bloom => format!("Bloom: {}", on(v.bloom)),
            Self::BloomIntensity => format!("Bloom intensity: {:.2}", v.bloom_intensity),
            Self::Exposure => format!("Exposure: {} EV100", v.exposure_ev100),
            Self::Emission => format!("Light emission (WARNING): {}", v.emissive_strength),
            Self::Projectile => format!("Projectile detail: {} / 4", v.projectile_detail),
            Self::Vsync => format!("VSync: {}", on(v.vsync)),
            Self::DepthPrepass => format!(
                "Depth prepass: {}{}",
                on(v.depth_prepass),
                if v.occlusion_culling {
                    " (required by occlusion)"
                } else {
                    ""
                }
            ),
            Self::Occlusion => format!("Occlusion culling: {}", on(v.occlusion_culling)),
        }
    }
}
fn refresh_labels(
    preferences: Res<Preferences>,
    notice: Res<GraphicsNotice>,
    mut labels: Query<(&GraphicsLabel, &mut Text), Without<NoticeLabel>>,
    mut notices: Query<&mut Text, With<NoticeLabel>>,
    buttons: Query<(Entity, &GraphicsAction, &AccessibleLabel)>,
    mut commands: Commands,
) {
    for (entity, action, label) in &buttons {
        let value = action.label(&preferences.graphics);
        if label.0 != value {
            commands.entity(entity).insert(AccessibleLabel(value));
        }
    }
    for (action, mut text) in &mut labels {
        let label = action.0.label(&preferences.graphics);
        if text.0 != label {
            text.0 = label;
        }
    }
    for mut text in &mut notices {
        if text.0 != notice.0 {
            text.0.clone_from(&notice.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_simulator_render::graphics_settings::QualityPreset;
    #[test]
    fn tooltip_waits_for_hover_supports_keyboard_and_hides_with_menu() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<crate::hud::HudState>()
            .init_resource::<InputFocus>()
            .init_resource::<InputFocusVisible>()
            .add_systems(Update, graphics_tooltips);
        let button = app
            .world_mut()
            .spawn((
                GraphicsAction::Msaa,
                Hovered(true),
                ComputedNode {
                    size: Vec2::new(200., 32.),
                    ..default()
                },
            ))
            .id();
        let tip = app
            .world_mut()
            .spawn((
                ChildOf(button),
                GraphicsTooltip::default(),
                Node {
                    display: Display::None,
                    ..default()
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<crate::hud::HudState>()
            .settings = true;
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(200));
        app.update();
        assert_eq!(app.world().get::<Node>(tip).unwrap().display, Display::None);
        app.update();
        assert_eq!(app.world().get::<Node>(tip).unwrap().display, Display::Flex);
        app.world_mut().entity_mut(button).insert(Hovered(false));
        app.update();
        assert_eq!(app.world().get::<Node>(tip).unwrap().display, Display::None);
        *app.world_mut().resource_mut::<InputFocus>() = InputFocus::from_entity(button);
        app.world_mut().resource_mut::<InputFocusVisible>().0 = true;
        app.update();
        app.update();
        assert_eq!(app.world().get::<Node>(tip).unwrap().display, Display::Flex);
        app.world_mut()
            .resource_mut::<crate::hud::HudState>()
            .settings = false;
        app.update();
        assert_eq!(app.world().get::<Node>(tip).unwrap().display, Display::None);
        app.world_mut()
            .resource_mut::<crate::hud::HudState>()
            .settings = true;
        app.world_mut()
            .get_mut::<ComputedNode>(button)
            .unwrap()
            .size = Vec2::ZERO;
        app.update();
        app.update();
        assert_eq!(app.world().get::<Node>(tip).unwrap().display, Display::None);
    }
    #[test]
    fn live_changes_remove_effects_and_leave_fill_unshadowed() {
        let mut app = App::new();
        app.init_resource::<Preferences>()
            .init_resource::<AppliedGraphics>()
            .init_resource::<GraphicsNotice>()
            .init_resource::<CullingSupport>()
            .init_resource::<DirectionalLightShadowMap>()
            .init_resource::<EmissionStrength>()
            .init_resource::<ProjectileDetail>()
            .add_systems(Update, apply_graphics);
        let camera = app
            .world_mut()
            .spawn((
                PlayerCamera,
                Exposure { ev100: 9. },
                Msaa::Sample4,
                Bloom::default(),
            ))
            .id();
        let key = app
            .world_mut()
            .spawn((
                rm_simulator_render::lighting::ShadowKeyLight,
                DirectionalLight::default(),
            ))
            .id();
        let fill = app
            .world_mut()
            .spawn(DirectionalLight {
                shadow_maps_enabled: false,
                ..default()
            })
            .id();
        app.world_mut()
            .resource_mut::<Preferences>()
            .graphics
            .preset = QualityPreset::Low;
        app.update();
        assert_eq!(app.world().resource::<EmissionStrength>().0, 12000.);
        assert!(app.world().get::<Bloom>(camera).is_some());
        assert!(
            !app.world()
                .get::<DirectionalLight>(key)
                .unwrap()
                .shadow_maps_enabled
        );
        app.world_mut()
            .resource_mut::<Preferences>()
            .graphics
            .preset = QualityPreset::High;
        app.world_mut()
            .resource_mut::<Preferences>()
            .graphics
            .overrides
            .depth_prepass = Some(true);
        app.update();
        assert!(app.world().get::<Bloom>(camera).is_some());
        assert!(app.world().get::<DepthPrepass>(camera).is_some());
        assert!(
            app.world()
                .get::<DirectionalLight>(key)
                .unwrap()
                .shadow_maps_enabled
        );
        assert!(
            !app.world()
                .get::<DirectionalLight>(fill)
                .unwrap()
                .shadow_maps_enabled
        );
        app.world_mut()
            .resource_mut::<Preferences>()
            .graphics
            .overrides
            .depth_prepass = Some(false);
        app.world_mut()
            .resource_mut::<Preferences>()
            .graphics
            .overrides
            .occlusion_culling = Some(true);
        app.update();
        assert!(app.world().get::<OcclusionCulling>(camera).is_none());
        assert!(app.world().get::<DepthPrepass>(camera).is_none());
        app.world_mut().resource_mut::<CullingSupport>().supported = true;
        app.update();
        assert!(app.world().get::<OcclusionCulling>(camera).is_some());
        assert!(app.world().get::<DepthPrepass>(camera).is_some());
        app.world_mut()
            .resource_mut::<Preferences>()
            .graphics
            .overrides
            .occlusion_culling = Some(false);
        app.update();
        assert!(app.world().get::<OcclusionCulling>(camera).is_none());
        assert!(app.world().get::<DepthPrepass>(camera).is_none());
    }
    #[test]
    fn customization_persists_and_reset_keeps_preset() {
        let mut settings = GraphicsSettings {
            preset: QualityPreset::Low,
            ..default()
        };
        GraphicsAction::Shadows.apply(&mut settings);
        assert!(settings.resolved().shadows);
        let saved = serde_json::to_string(&settings).unwrap();
        assert_eq!(
            serde_json::from_str::<GraphicsSettings>(&saved).unwrap(),
            settings
        );
        GraphicsAction::Reset.apply(&mut settings);
        assert_eq!(settings.preset, QualityPreset::Low);
        assert!(!settings.resolved().shadows);
    }
    #[test]
    fn invalid_numbers_and_gpu_limits_are_safe() {
        let settings = GraphicsSettings {
            overrides: GraphicsOverrides {
                exposure_ev100: Some(f32::NAN),
                shadow_cascades: Some(usize::MAX),
                shadow_map_size: Some(4096),
                msaa_samples: Some(8),
                ..default()
            },
            ..default()
        };
        let resolved = settings.resolved();
        assert_eq!(resolved.exposure_ev100, 9.);
        assert_eq!(resolved.shadow_cascades, 4);
        let effective = effective_settings(resolved, &[1, 4], 2048);
        assert_eq!(effective.msaa_samples, 4);
        assert_eq!(effective.shadow_map_size, 2048);
        assert_eq!(settings.overrides.msaa_samples, Some(8));
    }
}
