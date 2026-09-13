// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Optional top-down artwork generated from the external field package.
use bevy::{
    asset::RenderAssetUsages,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    prelude::*,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

/// The optional top-down artwork for the current field, with the world FLU
/// rectangle it covers. It exists only when the package ships artwork that
/// passes its checksums, and the map falls back to its schematic without it.
#[derive(Resource)]
pub struct Minimap {
    /// The decoded PNG, registered with Bevy's image assets.
    pub image: Handle<Image>,
    /// x minimum, y minimum, x maximum, y maximum in world FLU metres.
    pub bounds_m: [f64; 4],
}
#[derive(Deserialize)]
struct Metadata {
    schema_version: u32,
    image: String,
    image_sha256: String,
    bounds_flu_m: [f64; 4],
    orientation: String,
    manifest_sha256: BTreeMap<String, String>,
}
fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Read on the loading worker. Missing or stale optional artwork falls back
/// to the schematic; it never prevents joining a field.
pub fn load(root: &Path) -> Option<(Image, [f64; 4])> {
    if !root.join("minimap.json").exists() {
        return None;
    }
    match read(root) {
        Ok(map) => Some(map),
        Err(error) => {
            warn!("minimap unavailable: {error:#}");
            None
        }
    }
}
fn read(root: &Path) -> anyhow::Result<(Image, [f64; 4])> {
    let metadata: Metadata = serde_json::from_slice(&std::fs::read(root.join("minimap.json"))?)?;
    anyhow::ensure!(
        metadata.schema_version == 1
            && metadata.orientation == "x-left-y-down"
            && metadata.image == "minimap.png",
        "unsupported minimap format"
    );
    let [xmin, ymin, xmax, ymax] = metadata.bounds_flu_m;
    anyhow::ensure!(
        metadata.bounds_flu_m.iter().all(|v| v.is_finite()) && xmax > xmin && ymax > ymin,
        "invalid minimap bounds"
    );
    for file in ["manifest.json", "equipment/manifest.json"] {
        anyhow::ensure!(
            metadata.manifest_sha256.get(file) == Some(&checksum(&std::fs::read(root.join(file))?)),
            "minimap belongs to a different field package"
        );
    }
    let bytes = std::fs::read(root.join("minimap.png"))?;
    anyhow::ensure!(
        checksum(&bytes) == metadata.image_sha256,
        "minimap image checksum mismatch"
    );
    let image = Image::from_buffer(
        &bytes,
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::linear(),
        RenderAssetUsages::default(),
    )?;
    Ok((image, metadata.bounds_flu_m))
}

/// A world FLU position in metres as percentages into the artwork, measured
/// from its left and top edges. The image is stored x-left-y-down, so red's +x
/// half sits on the left. Both values are clamped to 1..99 to keep a marker
/// inside the picture.
pub fn position(position_m: [f64; 3], bounds_m: [f64; 4]) -> (f32, f32) {
    let [xmin, ymin, xmax, ymax] = bounds_m;
    (
        ((xmax - position_m[0]) / (xmax - xmin) * 100.).clamp(1., 99.) as f32,
        ((position_m[1] - ymin) / (ymax - ymin) * 100.).clamp(1., 99.) as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn artwork_requires_matching_image_and_both_package_manifests() {
        let root = std::env::temp_dir().join(format!("rm-minimap-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("equipment")).unwrap();
        let png = include_bytes!("../../../assets/outpost-mask/digit.png");
        std::fs::write(root.join("minimap.png"), png).unwrap();
        for file in ["manifest.json", "equipment/manifest.json"] {
            std::fs::write(root.join(file), b"{}").unwrap();
        }
        let metadata = serde_json::json!({
            "schema_version": 1, "image": "minimap.png", "image_sha256": checksum(png),
            "bounds_flu_m": [-15., -9., 15., 9.], "orientation": "x-left-y-down",
            "manifest_sha256": { "manifest.json": checksum(b"{}"), "equipment/manifest.json": checksum(b"{}") }
        });
        std::fs::write(root.join("minimap.json"), metadata.to_string()).unwrap();
        assert!(read(&root).is_ok());
        std::fs::write(root.join("equipment/manifest.json"), b"changed").unwrap();
        assert!(
            read(&root)
                .unwrap_err()
                .to_string()
                .contains("different field")
        );
        std::fs::write(root.join("equipment/manifest.json"), b"{}").unwrap();
        std::fs::write(root.join("minimap.png"), b"changed").unwrap();
        assert!(
            read(&root)
                .unwrap_err()
                .to_string()
                .contains("image checksum")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn image_registration_uses_manifest_bounds_and_red_is_left() {
        let bounds = [-15., -9., 15., 9.];
        assert_eq!(position([0., 0., 0.], bounds), (50., 50.));
        assert_eq!(position([7.5, -4.5, 4.], bounds), (25., 25.));
        assert_eq!(position([-7.5, 4.5, -1.], bounds), (75., 75.));
    }
}
