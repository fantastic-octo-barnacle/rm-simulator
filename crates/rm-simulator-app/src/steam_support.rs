// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Optional Steam client lifetime. No gameplay, transport or account data depends
//! on it. Initialize before Bevy starts threads; dispatch callbacks on the main
//! thread, and let the sole Client shut Steam down when the app is dropped.
use bevy::prelude::*;

/// The optional Steam client, kept alive for the whole app. Steam is a
/// convenience only: no gameplay, transport or account state depends on it, and
/// a missing client leaves the simulator unchanged.
pub struct SteamRuntime {
    client: Option<steamworks::Client>,
}
impl SteamRuntime {
    /// Call once, before starting any worker threads. The upstream init_app
    /// implementation sets SteamAppId and SteamGameId in the process environment.
    pub fn initialize() -> Self {
        let configured = std::env::var("RM_SIMULATOR_STEAM_APP_ID").ok();
        let launched = std::env::var("SteamAppId").ok();
        let app_id = match select_app_id(
            configured.as_deref(),
            launched.as_deref(),
            cfg!(debug_assertions),
        ) {
            Ok(id) => id,
            Err(error) => {
                eprintln!("Steam disabled: {error}");
                return Self { client: None };
            }
        };
        match steamworks::Client::init_app(app_id) {
            Ok(client) => {
                eprintln!("Steam initialized for App ID {app_id}");
                Self {
                    client: Some(client),
                }
            }
            Err(error) => {
                eprintln!(
                    "Steam unavailable for App ID {app_id}: {error}; continuing without Steam"
                );
                Self { client: None }
            }
        }
    }
}

fn select_app_id(
    configured: Option<&str>,
    launched: Option<&str>,
    development: bool,
) -> Result<u32, &'static str> {
    match configured.or(launched) {
        Some(value) => {
            value.parse::<u32>().ok().filter(|id| *id != 0).ok_or(
                "App ID must be a nonzero integer in RM_SIMULATOR_STEAM_APP_ID or SteamAppId",
            )
        }
        None if development => Ok(480),
        None => {
            Err("set RM_SIMULATOR_STEAM_APP_ID to the production App ID, or launch through Steam")
        }
    }
}

/// Store the runtime as a non-send resource and pump its callbacks in `First`,
/// so every Steam call stays on the main thread.
pub fn install(app: &mut App, runtime: SteamRuntime) {
    app.insert_non_send(runtime)
        .add_systems(First, pump_callbacks);
}
fn pump_callbacks(runtime: NonSend<SteamRuntime>) {
    if let Some(client) = &runtime.client {
        client.run_callbacks();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn app_id_defaults_only_in_development() {
        assert_eq!(select_app_id(None, None, true), Ok(480));
        assert!(select_app_id(None, None, false).is_err());
        assert_eq!(
            select_app_id(Some("12345"), Some("54321"), false),
            Ok(12345)
        );
        assert_eq!(select_app_id(None, Some("54321"), false), Ok(54321));
        for bad in ["", "0", "-1", "abc", "4294967296"] {
            assert!(select_app_id(Some(bad), Some("54321"), true).is_err());
        }
    }
    #[test]
    fn unavailable_steam_does_not_prevent_frames() {
        let mut app = App::new();
        install(&mut app, SteamRuntime { client: None });
        app.update();
        app.update();
    }
}
