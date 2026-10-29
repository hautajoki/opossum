//! Asset media as pure functions over bytes: the image pipeline (PNG, SVG, JPEG headers, bleed, resize, encode),
//! audio inspection (Ogg, WAV, MP3, FLAC) and the MIME table. No files, cache or config.

mod audio;
mod image;

pub use audio::{Audio, inspect_audio};
pub use image::{Fit, IMAGE_VERSION, Image, ImageOptions, MAX_SIDE, Preset, fit, pipeline, process};

/// The upload `Content-Type` of a file with this extension (lowercase, with its dot) when its own bytes are uploaded;
/// `None` for one opossum cannot upload. An `Image::Png` output is always `image/png`, so an SVG maps to that.
pub fn mime(ext: &str) -> Option<&'static str> {
    Some(match ext {
        ".png" | ".svg" => "image/png",
        ".jpg" | ".jpeg" => "image/jpeg",
        ".ogg" => "audio/ogg",
        ".mp3" => "audio/mpeg",
        ".wav" => "audio/wav",
        ".flac" => "audio/flac",
        ".mp4" => "video/mp4",
        ".mov" => "video/mov",
        ".glb" => "model/gltf-binary",
        ".gltf" => "model/gltf+json",
        ".fbx" => "model/fbx",
        ".rbxm" | ".rbxmx" => "model/x-rbxm",
        _ => return None,
    })
}
