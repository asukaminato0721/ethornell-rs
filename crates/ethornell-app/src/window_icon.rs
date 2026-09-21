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

    #[test]
    fn reads_png_icon_resources_from_pe32_and_pe64() {
        fn u16_at(bytes: &mut [u8], offset: usize, value: u16) {
            bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        fn u32_at(bytes: &mut [u8], offset: usize, value: u32) {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let expected = image::RgbaImage::from_pixel(16, 16, image::Rgba([20, 40, 60, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        expected
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let png = png.into_inner();
        for pe64 in [false, true] {
            let mut bytes = vec![0; 4096];
            bytes[..2].copy_from_slice(b"MZ");
            u32_at(&mut bytes, 60, 128);
            bytes[128..132].copy_from_slice(b"PE\0\0");
            u16_at(&mut bytes, 132, if pe64 { 0x8664 } else { 0x14c });
            u16_at(&mut bytes, 134, 1);
            let optional_size = if pe64 { 240 } else { 224 };
            u16_at(&mut bytes, 148, optional_size);
            u16_at(&mut bytes, 152, if pe64 { 0x20b } else { 0x10b });
            u32_at(&mut bytes, 184, 4096);
            u32_at(&mut bytes, 188, 512);
            u32_at(&mut bytes, 208, 8192);
            u32_at(&mut bytes, 212, 512);
            let directories = 152 + if pe64 { 112 } else { 96 };
            u32_at(&mut bytes, directories - 4, 16);
            u32_at(&mut bytes, directories + 16, 4096);
            u32_at(&mut bytes, directories + 20, 3584);
            let section = 152 + usize::from(optional_size);
            bytes[section..section + 5].copy_from_slice(b".rsrc");
            for (offset, value) in [(8, 3584), (12, 4096), (16, 3584), (20, 512)] {
                u32_at(&mut bytes, section + offset, value);
            }
            let resources = &mut bytes[512..];
            // Type -> ID -> language -> data for RT_ICON and RT_GROUP_ICON.
            u16_at(resources, 14, 2);
            for (offset, id, child) in [(16, 3, 32), (24, 14, 56), (48, 1, 80), (72, 1, 104)] {
                u32_at(resources, offset, id);
                u32_at(resources, offset + 4, 0x8000_0000 | child);
            }
            for offset in [32, 56, 80, 104] {
                u16_at(resources, offset + 14, 1);
            }
            for (offset, data) in [(96, 128), (120, 144)] {
                u32_at(resources, offset, 1033);
                u32_at(resources, offset + 4, data);
            }
            u32_at(resources, 128, 4096 + 192);
            u32_at(resources, 132, png.len() as u32);
            u32_at(resources, 144, 4096 + 160);
            u32_at(resources, 148, 20);
            let group = &mut resources[160..180];
            u16_at(group, 2, 1);
            u16_at(group, 4, 1);
            group[6] = 16;
            group[7] = 16;
            u16_at(group, 10, 1);
            u16_at(group, 12, 32);
            u32_at(group, 14, png.len() as u32);
            u16_at(group, 18, 1);
            resources[192..192 + png.len()].copy_from_slice(&png);
            assert_eq!(executable_icon(&bytes), Some(expected.clone()));
        }
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
