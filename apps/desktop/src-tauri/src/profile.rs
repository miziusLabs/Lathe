//! Device-local profile picture storage for the Settings account avatar.
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use tokio::{fs, sync::Mutex};
use uuid::Uuid;

use crate::store::get_home_app_dir;

const AVATAR_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "avif", "svg",
];
const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;
static PICTURE_LOCK: Mutex<()> = Mutex::const_new(());

async fn picture_directory() -> Result<PathBuf> {
    let path = get_home_app_dir()
        .await?
        .join("attachments")
        .join("profile");
    fs::create_dir_all(&path)
        .await
        .context("could not create profile picture directory")?;
    Ok(path)
}

fn supported_format(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" | "pngs" | "apng" => Some("png"),
        "jpg" | "jpgs" | "jpeg" | "jpe" | "jfif" => Some("jpeg"),
        "gif" => Some("gif"),
        "webp" => Some("webp"),
        "bmp" => Some("bmp"),
        "ico" => Some("ico"),
        "avif" => Some("avif"),
        "svg" => Some("svg"),
        _ => None,
    }
}

fn detected_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.get(..4) == Some(&b"RIFF"[..])
        && bytes.get(8..12) == Some(&b"WEBP"[..])
    {
        Some("webp")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else if bytes.starts_with(&[0, 0, 1, 0]) {
        Some("ico")
    } else if is_avif(bytes) {
        Some("avif")
    } else if is_svg(bytes) {
        Some("svg")
    } else {
        None
    }
}

fn is_avif(bytes: &[u8]) -> bool {
    if bytes.get(4..8) != Some(&b"ftyp"[..]) {
        return false;
    }
    let is_avif_brand = |brand: &[u8]| brand == b"avif" || brand == b"avis";
    if bytes.get(8..12).is_some_and(is_avif_brand) {
        return true;
    }
    let compatible_brands = bytes.get(16..bytes.len().min(64)).unwrap_or_default();
    compatible_brands
        .chunks_exact(4)
        .any(is_avif_brand)
}

fn is_svg(bytes: &[u8]) -> bool {
    let Ok(source) = std::str::from_utf8(bytes) else {
        return false;
    };
    source
        .trim_start_matches('\u{feff}')
        .trim_start()
        .chars()
        .take(4096)
        .collect::<String>()
        .contains("<svg")
}

/// Returns the saved picture's path for `convertFileSrc`, if one has been set.
pub async fn picture_path() -> Result<Option<String>> {
    let _guard = PICTURE_LOCK.lock().await;
    let directory = picture_directory().await?;

    for extension in AVATAR_EXTENSIONS {
        let path = directory.join(format!("avatar.{extension}"));
        match fs::metadata(&path).await {
            Ok(metadata)
                if metadata.is_file()
                    && metadata.len() > 0
                    && metadata.len() <= MAX_IMAGE_BYTES =>
            {
                return Ok(Some(path.to_string_lossy().into_owned()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("could not read profile picture metadata"),
        }
    }

    Ok(None)
}

/// Validates and copies a user-selected image into Lathe's private data directory.
pub async fn save_picture(source_path: &str) -> Result<String> {
    let source = Path::new(source_path);
    let format = supported_format(source).context(
        "Choose a PNG, JPEG, GIF, WebP, BMP, ICO, AVIF, or SVG image.",
    )?;
    let metadata = fs::metadata(source)
        .await
        .context("could not read the selected profile picture")?;
    if !metadata.is_file() {
        bail!("The selected profile picture is not a file.");
    }
    if metadata.len() == 0 || metadata.len() > MAX_IMAGE_BYTES {
        bail!("Choose an image no larger than 5 MB.");
    }

    let bytes = fs::read(source)
        .await
        .context("could not read the selected profile picture")?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        bail!("Choose an image no larger than 5 MB.");
    }
    if detected_format(&bytes) != Some(format) {
        bail!("The selected file is not a valid {format} image.");
    }

    let _guard = PICTURE_LOCK.lock().await;
    let directory = picture_directory().await?;
    let target = directory.join(format!("avatar.{format}"));
    let temporary = directory.join(format!(".avatar-{}.{}", Uuid::now_v7(), format));
    fs::write(&temporary, &bytes)
        .await
        .context("could not save the selected profile picture")?;

    for old_extension in AVATAR_EXTENSIONS {
        let old_path = directory.join(format!("avatar.{old_extension}"));
        match fs::remove_file(old_path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                let _ = fs::remove_file(&temporary).await;
                return Err(error).context("could not replace the existing profile picture");
            }
        }
    }

    if let Err(error) = fs::rename(&temporary, &target).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(error).context("could not finish saving the profile picture");
    }

    Ok(target.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_browser_renderable_image_formats() {
        assert_eq!(detected_format(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(detected_format(&[0xff, 0xd8, 0xff, 0]), Some("jpeg"));
        assert_eq!(detected_format(b"GIF89a"), Some("gif"));
        assert_eq!(detected_format(b"RIFF0000WEBPrest"), Some("webp"));
        assert_eq!(detected_format(b"BMrest"), Some("bmp"));
        assert_eq!(detected_format(&[0, 0, 1, 0, 0]), Some("ico"));
        assert_eq!(detected_format(b"\0\0\0\0ftypavif0000"), Some("avif"));
        assert_eq!(detected_format(b"<?xml version='1.0'?><svg/>") , Some("svg"));
        assert_eq!(detected_format(b"<script/>"), None);
    }

    #[test]
    fn supports_png_and_jpeg_suffix_aliases_and_common_formats() {
        assert_eq!(supported_format(Path::new("photo.PNG")), Some("png"));
        assert_eq!(supported_format(Path::new("photo.pngs")), Some("png"));
        assert_eq!(supported_format(Path::new("photo.apng")), Some("png"));
        assert_eq!(supported_format(Path::new("photo.jpg")), Some("jpeg"));
        assert_eq!(supported_format(Path::new("photo.jpgs")), Some("jpeg"));
        assert_eq!(supported_format(Path::new("photo.jfif")), Some("jpeg"));
        assert_eq!(supported_format(Path::new("photo.avif")), Some("avif"));
        assert_eq!(supported_format(Path::new("photo.heic")), None);
    }

    #[test]
    fn image_extension_must_match_its_content() {
        assert_eq!(detected_format(b"GIF89a"), Some("gif"));
        assert_ne!(detected_format(b"GIF89a"), supported_format(Path::new("photo.png")));
    }
}
