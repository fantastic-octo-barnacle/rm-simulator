// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! User preferences outlive matches. Writes replace a complete JSON document.
use crate::{bindings::ControlsSettings, graphics::GraphicsSettings, hud::HudState};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Everything the app remembers between launches, stored as `settings.json`
/// beside `title.json`. Every write replaces the whole document.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// On-disk format version. `Preferences::load` accepts only 1; any other
    /// value is an error and leaves the file unchanged.
    pub version: u32,
    /// Bindings for the named client actions.
    pub controls: ControlsSettings,
    /// Mouse look gain: yaw radians per unit of raw mouse motion. `validate`
    /// clamps it to 0.0005..=0.005 and replaces a non-finite value with
    /// 0.0025; the settings slider maps 0.0025 to 100 percent.
    pub sensitivity: f32,
    /// Draw the minimap.
    pub show_map: bool,
    /// Draw the aiming reticle.
    pub show_reticle: bool,
    /// Which network statistics overlay to show; `Off` hides it.
    pub network_stats: crate::network_hud::NetworkStatsMode,
    /// Quality preset and per-field overrides for the passive renderer.
    pub graphics: GraphicsSettings,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            controls: default(),
            sensitivity: 0.0025,
            show_map: true,
            show_reticle: true,
            network_stats: default(),
            graphics: default(),
        }
    }
}
impl Preferences {
    fn validate(&mut self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!(
                "Settings version {} is unsupported; file left unchanged",
                self.version
            ));
        }
        self.controls.validate();
        self.sensitivity = if self.sensitivity.is_finite() {
            self.sensitivity.clamp(0.0005, 0.005)
        } else {
            0.0025
        };
        Ok(())
    }
    /// Read the settings from `path`. A missing file gives the defaults, so a
    /// first launch needs no file at all; a parse error or an unsupported
    /// version gives `Err` and leaves the file for the user to fix.
    pub fn load(path: &Path) -> Result<Self, String> {
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(default()),
            Err(error) => return Err(format!("Cannot read settings: {error}")),
        };
        let mut preferences: Self = serde_json::from_str(&contents)
            .map_err(|error| format!("Cannot read settings: {error}; file left unchanged"))?;
        preferences.validate()?;
        Ok(preferences)
    }
    /// Write the settings to `path` as pretty JSON, creating the parent
    /// directory when it is missing. The bytes go to a `.tmp` sibling that is
    /// synced and renamed over `path`, and a failed write removes that sibling.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
        let result = (|| {
            use std::io::Write;
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
            std::fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }
}
/// Runtime bookkeeping for the settings file: where it lives, the last saved
/// and observed copies, when the last edit happened and whether writes are
/// allowed. `save_changes` uses it to debounce and to report failures.
#[derive(Resource)]
pub struct PreferenceStatus {
    path: Option<PathBuf>,
    last_saved: Preferences,
    last_observed: Preferences,
    changed_at: std::time::Duration,
    /// A readable explanation shown in settings; corrupt files are never overwritten.
    pub message: String,
    writable: bool,
}
/// Loads `settings.json` beside `title.json` at startup, seeds `HudState` from
/// it and adds the `PostUpdate` system that saves later edits. A load failure
/// keeps the defaults, blocks writes and reports the error in
/// `PreferenceStatus::message`.
pub struct PreferencesPlugin;
impl Plugin for PreferencesPlugin {
    fn build(&self, app: &mut App) {
        let path = crate::title::remembered_path().map(|path| path.with_file_name("settings.json"));
        let loaded = path
            .as_deref()
            .map(Preferences::load)
            .unwrap_or_else(|| Err("No configuration directory; settings cannot be saved".into()));
        let (settings, message, writable) = match loaded {
            Ok(settings) => (settings, String::new(), true),
            Err(error) => {
                warn!("{error}");
                (Preferences::default(), error, false)
            }
        };
        if let Some(mut ui) = app.world_mut().get_resource_mut::<HudState>() {
            ui.controls = settings.controls.clone();
            ui.sensitivity = settings.sensitivity;
            ui.show_map = settings.show_map;
            ui.show_reticle = settings.show_reticle;
            if ui.network_stats == crate::network_hud::NetworkStatsMode::Off {
                ui.network_stats = settings.network_stats;
            }
        }
        app.insert_resource(PreferenceStatus {
            path,
            last_saved: settings.clone(),
            last_observed: settings.clone(),
            changed_at: std::time::Duration::ZERO,
            message,
            writable,
        })
        .insert_resource(settings)
        .add_systems(PostUpdate, save_changes);
    }
}
fn save_changes(
    ui: Res<HudState>,
    mut settings: ResMut<Preferences>,
    mut status: ResMut<PreferenceStatus>,
    time: Res<Time<Real>>,
    mut exit: MessageReader<AppExit>,
) {
    // HudState remains the runtime source for input and existing headless app systems.
    if ui.controls != settings.controls {
        settings.controls = ui.controls.clone();
    }
    if ui.sensitivity != settings.sensitivity {
        settings.sensitivity = ui.sensitivity;
    }
    if ui.show_map != settings.show_map {
        settings.show_map = ui.show_map;
    }
    if ui.show_reticle != settings.show_reticle {
        settings.show_reticle = ui.show_reticle;
    }
    if ui.network_stats != settings.network_stats {
        settings.network_stats = ui.network_stats;
    }
    let exiting = exit.read().next().is_some();
    if *settings != status.last_observed {
        status.last_observed = settings.clone();
        status.changed_at = time.elapsed();
    }
    if *settings == status.last_saved || !status.writable {
        return;
    }
    if ui.settings
        && !exiting
        && time
            .elapsed()
            .saturating_sub(status.changed_at)
            .as_secs_f32()
            < 0.3
    {
        return;
    }
    let Some(path) = status.path.as_deref() else {
        return;
    };
    match settings.save(path) {
        Ok(()) => {
            status.message.clear();
            status.last_saved = settings.clone();
        }
        Err(error) => {
            status.message = format!("Settings could not be saved: {error}");
            warn!("{}", status.message);
            // Retry after the next user edit, not every frame.
            status.last_saved = settings.clone();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_fields_defaults_and_settings_round_trip() {
        let root = std::env::temp_dir().join(format!("rm-settings-{}", std::process::id()));
        let path = root.join("nested/settings.json");
        let mut settings = Preferences::default();
        settings.controls.invert_y = true;
        settings.controls.set(
            crate::bindings::InputAction::Fire,
            0,
            Some(crate::bindings::Binding::Key(KeyCode::KeyZ)),
        );
        settings.sensitivity = 0.004;
        settings.save(&path).unwrap();
        assert_eq!(Preferences::load(&path).unwrap(), settings);
        std::fs::write(&path, "{}").unwrap();
        assert_eq!(Preferences::load(&path).unwrap(), Preferences::default());
        std::fs::write(&path, "broken").unwrap();
        assert!(Preferences::load(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken");
        let obstruction = root.join("file");
        std::fs::write(&obstruction, "keep").unwrap();
        assert!(settings.save(&obstruction.join("settings.json")).is_err());
        assert_eq!(std::fs::read_to_string(&obstruction).unwrap(), "keep");
        std::fs::write(&path, r#"{"version":99}"#).unwrap();
        assert!(Preferences::load(&path).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
