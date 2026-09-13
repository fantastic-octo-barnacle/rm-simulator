// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Runtime adjustment of explicitly registered simulator lights. Authored CAD
//! materials are never registered and retain their original colors and emission.
use bevy::prelude::*;
use std::collections::HashMap;

/// Emission multiplier applied to every registered simulator light.
#[derive(Resource)]
pub struct EmissionStrength(pub f32);
impl Default for EmissionStrength {
    fn default() -> Self {
        Self(12000.0)
    }
}
/// Normalized emission per generated material asset; never holds CAD materials.
#[derive(Resource, Default)]
struct EmissionMaterials(HashMap<AssetId<StandardMaterial>, LinearRgba>);

/// Re-applies the configured emission strength to registered light materials.
pub struct MaterialQualityPlugin;
impl Plugin for MaterialQualityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EmissionStrength>()
            .init_resource::<EmissionMaterials>()
            .add_systems(Last, update_emission);
    }
}

/// Remember emission per unit of configured strength, including materials that
/// are currently hidden by an armor flash. Only call for generated light assets.
pub fn register_emission(
    commands: &mut Commands,
    materials: &Assets<StandardMaterial>,
    handle: &Handle<StandardMaterial>,
    strength: f32,
) {
    if strength <= 0.0 {
        return;
    }
    let Some(material) = materials.get(handle) else {
        return;
    };
    let normalized = material.emissive / strength;
    let id = handle.id();
    commands.queue(move |world: &mut World| {
        world.init_resource::<EmissionMaterials>();
        world
            .resource_mut::<EmissionMaterials>()
            .0
            .insert(id, normalized);
    });
}

/// Re-apply the configured strength to registered materials whose value moved.
fn update_emission(
    strength: Res<EmissionStrength>,
    mut registered: ResMut<EmissionMaterials>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !strength.is_changed() && !registered.is_changed() {
        return;
    }
    // New assets can arrive after a preset is selected. Compare values to avoid
    // writing unchanged material assets and causing unnecessary GPU uploads.
    registered.0.retain(|id, normalized| {
        let Some(material) = materials.get(*id) else {
            return false;
        };
        let emission = *normalized * strength.0;
        if material.emissive != emission {
            materials.get_mut(*id).unwrap().emissive = emission;
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn emission_recovers_after_zero_without_changing_cad() {
        let mut app = App::new();
        app.init_resource::<Assets<StandardMaterial>>()
            .add_plugins(MaterialQualityPlugin);
        let generated = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                emissive: LinearRgba::rgb(12000., 6000., 0.),
                ..default()
            });
        let cad = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                emissive: LinearRgba::rgb(3., 2., 1.),
                ..default()
            });
        app.world_mut()
            .resource_mut::<EmissionMaterials>()
            .0
            .insert(generated.id(), LinearRgba::rgb(1., 0.5, 0.));
        app.world_mut().resource_mut::<EmissionStrength>().0 = 0.;
        app.update();
        app.world_mut().resource_mut::<EmissionStrength>().0 = 24000.;
        app.update();
        let assets = app.world().resource::<Assets<StandardMaterial>>();
        assert_eq!(
            assets.get(&generated).unwrap().emissive,
            LinearRgba::rgb(1., 0.5, 0.) * 24000.
        );
        assert_eq!(
            assets.get(&cad).unwrap().emissive,
            LinearRgba::rgb(3., 2., 1.)
        );
    }
}
