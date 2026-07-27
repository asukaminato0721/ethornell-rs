use ethornell_archive::ResourceManager;
use std::path::{Path, PathBuf};

use super::find_runtime_resource;

pub(crate) fn game_root_path(manager: &ResourceManager) -> PathBuf {
    let root = manager.archives().root.as_path();
    if root.is_absolute() {
        return root.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(root))
        .unwrap_or_else(|_| root.to_path_buf())
}

pub(crate) fn find_runtime_file(
    manager: &ResourceManager,
    archive: &str,
    file: &str,
) -> Option<PathBuf> {
    if !is_empty_archive_arg(archive.trim()) {
        return None;
    }
    let normalized = file.replace('\\', std::path::MAIN_SEPARATOR_STR);
    let path = Path::new(&normalized);
    let windows_absolute = file.starts_with('\\')
        || file
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':');
    if path.is_absolute() || windows_absolute {
        return Some(path.to_path_buf());
    }

    // Native sub_4665C0 resolves ordinary relative names through the archive
    // complex. GetUserDataRoot paths are direct filesystem paths; accept their
    // relative form as well when the game itself was opened through one.
    if path.starts_with(manager.archives().root.as_path()) {
        return Some(path.to_path_buf());
    }
    manager.archives().archives.iter().find_map(|archive| {
        archive
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| name.eq_ignore_ascii_case(file))
            .map(|_| archive.path.clone())
    })
}

pub(crate) fn runtime_file_path(manager: &ResourceManager, file: &str) -> Option<PathBuf> {
    let normalized = file.replace('\\', std::path::MAIN_SEPARATOR_STR);
    if normalized.trim().is_empty() {
        return None;
    }
    let path = Path::new(&normalized);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else if path.starts_with(manager.archives().root.as_path()) {
        Some(path.to_path_buf())
    } else {
        Some(manager.archives().root.join(path))
    }
}

pub(crate) fn runtime_file_cache_key(archive: &str, file: &str) -> String {
    format!(
        "{}\0{}",
        archive.trim().to_ascii_lowercase(),
        file.replace('\\', "/").to_ascii_lowercase()
    )
}

pub(crate) fn read_runtime_bytes(
    manager: &ResourceManager,
    archive: &str,
    file: &str,
) -> Option<Vec<u8>> {
    if let Some(path) = find_runtime_file(manager, archive, file) {
        if let Ok(bytes) = std::fs::read(path) {
            return Some(bytes);
        }
    }
    let entry = find_runtime_resource(manager, archive, file)?;
    manager.read_by_entry_decoded(&entry).ok()
}

pub(crate) fn is_empty_archive_arg(archive: &str) -> bool {
    archive.is_empty() || archive == "0" || archive == "0x00000000"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn testcase_manager() -> ResourceManager {
        ResourceManager::open_game(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("testcase"),
        )
        .unwrap()
    }

    #[test]
    fn game_root_is_exposed_as_an_absolute_user_data_root() {
        let manager = testcase_manager();
        assert!(game_root_path(&manager).is_absolute());
        assert!(game_root_path(&manager).ends_with("testcase"));
    }

    #[test]
    fn get_user_data_root_paths_resolve_as_loose_files() {
        let manager = testcase_manager();
        let file = game_root_path(&manager).join("UserData").join("slot.sud");
        assert_eq!(
            find_runtime_file(&manager, "", &file.to_string_lossy()),
            Some(file)
        );
    }
}
