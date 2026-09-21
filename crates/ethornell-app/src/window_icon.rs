//! Read game icons as data, including icons embedded in Windows executables.
use std::path::Path;
use winit::icon::RgbaIcon;
use winit::window::WindowAttributes;

pub(super) fn configure(attributes: WindowAttributes, root: &Path) -> WindowAttributes {
    let Some(rgba) = load(root) else {
        return attributes;
    };
    let icon = RgbaIcon::new(rgba.as_raw().clone(), rgba.width(), rgba.height())
        .ok()
        .map(Into::into);
    attributes.with_window_icon(icon)
}

fn load(root: &Path) -> Option<image::RgbaImage> {
    let mut candidates: Vec<_> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("ico") || ext.eq_ignore_ascii_case("exe")
            }) || path
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("icon.png"))
        })
        .collect();
    candidates.sort_by_key(|path| {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let rank = match name.as_str() {
            "icon.png" => 0,
            "icon.ico" => 1,
            _ if name.ends_with(".ico") => 2,
            // Helper executables are not the game's application icon.
            _ if name.contains("uninst") || name.contains("setup") || name == "bhvc.exe" => 4,
            _ => 3,
        };
        (rank, name)
    });
    for path in candidates {
        let rgba = if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
        {
            if !std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() <= 64 * 1024 * 1024) {
                continue;
            }
            std::fs::read(&path)
                .ok()
                .and_then(|bytes| executable_icon(&bytes))
        } else {
            image::open(&path).ok().map(image::DynamicImage::into_rgba8)
        };
        if let Some(rgba) = rgba {
            tracing::info!(path = %path.display(), "loaded game window icon");
            return Some(rgba);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_icon_falls_through_to_case_insensitive_game_ico() {
        let root = std::env::temp_dir().join(format!("ethornell-icons-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("ICON.PNG"), b"broken").unwrap();
        let expected = image::RgbaImage::from_pixel(32, 32, image::Rgba([12, 34, 56, 255]));
        expected
            .save_with_format(root.join("Game.ICO"), image::ImageFormat::Ico)
            .unwrap();
        assert_eq!(load(&root), Some(expected));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_executable_has_no_icon() {
        assert!(executable_icon(b"MZ").is_none());
        assert!(executable_icon(&[0; 1024]).is_none());
    }
}

fn executable_icon(bytes: &[u8]) -> Option<image::RgbaImage> {
    let pe = pelite::PeFile::from_bytes(bytes).ok()?;
    let resources = pe.resources().ok()?;
    for (_, group) in resources.icons().take(64).flatten() {
        let entries = group.entries();
        if entries.is_empty() || entries.len() > 64 {
            continue;
        }
        let size: Option<usize> = entries.iter().try_fold(0usize, |size, entry| {
            size.checked_add(group.image(entry.nId).ok()?.len())
        });
        if size.is_none_or(|size| size > 16 * 1024 * 1024) {
            continue;
        }
        let mut ico = Vec::new();
        if group.write(&mut ico).is_ok()
            && let Ok(image) = image::load_from_memory_with_format(&ico, image::ImageFormat::Ico)
        {
            return Some(image.into_rgba8());
        }
    }
    None
}
