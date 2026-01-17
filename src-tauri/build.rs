use std::fs;
use std::path::{Path, PathBuf};

fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.exists()).cloned()
}

fn write_rgba_png(src_path: &Path, dest_path: &Path, size: u32) {
    let img = image::open(src_path).expect("failed to open source icon");
    let resized = img.resize_exact(size, size, image::imageops::FilterType::Lanczos3);
    let rgba = resized.to_rgba8();
    let dyn_img = image::DynamicImage::ImageRgba8(rgba);
    dyn_img
        .save_with_format(dest_path, image::ImageFormat::Png)
        .expect("failed to write RGBA PNG");
}

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());

    // Prefer the favicon (best fit for app icons), fall back to the project assets and src-tauri icons.
    let source_icon = first_existing(&[
        manifest_dir
            .join("..")
            .join("public")
            .join("Ascent_Favicon.png"),
        manifest_dir
            .join("..")
            .join("src")
            .join("assets")
            .join("Ascent_Logo.png"),
        manifest_dir
            .join("..")
            .join("src")
            .join("assets")
            .join("Ascent_Favicon.png"),
        manifest_dir.join("icons").join("Ascent_Logo.png"),
        manifest_dir.join("icons").join("Ascent_Banner.png"),
    ]);

    let Some(source_icon) = source_icon else {
        // If no source icon exists, still run tauri build; the error will point at missing files.
        tauri_build::build();
        return;
    };

    let icons_dir = manifest_dir.join("icons");
    fs::create_dir_all(&icons_dir).expect("failed to create icons directory");

    // Always overwrite, so we don't get stuck with a previously generated non-RGBA PNG.
    write_rgba_png(&source_icon, &icons_dir.join("32x32.png"), 32);
    write_rgba_png(&source_icon, &icons_dir.join("128x128.png"), 128);
    write_rgba_png(&source_icon, &icons_dir.join("128x128@2x.png"), 256);

    // Some setups still look for this legacy path; keep it valid as well.
    write_rgba_png(&source_icon, &icons_dir.join("icon.png"), 256);

    tauri_build::build()
}
