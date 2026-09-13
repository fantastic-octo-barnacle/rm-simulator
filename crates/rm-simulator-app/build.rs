// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Check external SDK overrides before linking the optional Steam build.
use sha2::{Digest, Sha256};
use std::{env, fs, path::Path};

fn main() {
    println!("cargo:rerun-if-env-changed=STEAM_SDK_LOCATION");
    println!("cargo:rerun-if-changed=../../scripts/steam-sdk-api.sha256");
    if env::var_os("CARGO_FEATURE_STEAM").is_none() {
        return;
    }
    if let Some(sdk) = env::var_os("STEAM_SDK_LOCATION") {
        let api = Path::new(&sdk).join("public/steam/steam_api.json");
        println!("cargo:rerun-if-changed={}", api.display());
        let data =
            fs::read(&api).expect("STEAM_SDK_LOCATION must contain public/steam/steam_api.json");
        let expected = include_str!("../../scripts/steam-sdk-api.sha256")
            .split_whitespace()
            .next()
            .unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(data)),
            expected,
            "Steam SDK API does not match the locked steamworks-sys bindings. Unset STEAM_SDK_LOCATION to use the crate's matching SDK. SDK 1.65 is not compatible with these bindings."
        );
    }
    // Package the matching runtime beside the executable. Windows uses the
    // executable directory automatically; macOS's SDK uses @loader_path.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
}
