use std::path::Path;
#[cfg(target_os = "windows")]
use std::process::Command;

fn ensure_parent(path: &Path) -> bool {
    path.parent()
        .map(|parent| std::fs::create_dir_all(parent).is_ok())
        .unwrap_or(true)
}

#[cfg(target_os = "windows")]
pub(crate) fn create_shortcut(destination: &Path, target: &str) -> bool {
    if !ensure_parent(destination) {
        return false;
    }
    let destination = if destination.extension().is_none() {
        destination.with_extension("lnk")
    } else {
        destination.to_path_buf()
    };
    let script = r#"
$wsh = New-Object -ComObject WScript.Shell
$link = $wsh.CreateShortcut($env:BGI_SHORTCUT_DESTINATION)
$link.TargetPath = $env:BGI_SHORTCUT_TARGET
$parent = [System.IO.Path]::GetDirectoryName($env:BGI_SHORTCUT_TARGET)
if ($parent) { $link.WorkingDirectory = $parent }
$link.Save()
if (Test-Path -LiteralPath $env:BGI_SHORTCUT_DESTINATION) { exit 0 }
exit 1
"#;
    for executable in ["powershell.exe", "pwsh.exe"] {
        if let Ok(status) = Command::new(executable)
            .args(["-NoProfile", "-STA", "-Command", script])
            .env("BGI_SHORTCUT_DESTINATION", &destination)
            .env("BGI_SHORTCUT_TARGET", target)
            .status()
        {
            return status.success();
        }
    }
    false
}

#[cfg(target_os = "macos")]
pub(crate) fn create_shortcut(destination: &Path, target: &str) -> bool {
    if !ensure_parent(destination) {
        return false;
    }
    let body = format!("#!/bin/sh\nexec {}\n", target);
    if std::fs::write(destination, body).is_err() {
        return false;
    }
    set_executable(destination)
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn create_shortcut(destination: &Path, target: &str) -> bool {
    if !ensure_parent(destination) {
        return false;
    }
    let name = destination
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("Ethornell application");
    let body = format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec={}\nTerminal=false\n",
        name.replace('\n', " "),
        target.replace('\n', " ")
    );
    if std::fs::write(destination, body).is_err() {
        return false;
    }
    set_executable(destination)
}

#[cfg(not(any(target_os = "windows", unix)))]
pub(crate) fn create_shortcut(_destination: &Path, _target: &str) -> bool {
    false
}

#[cfg(unix)]
fn set_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    std::fs::set_permissions(path, permissions).is_ok()
}

#[cfg(target_os = "windows")]
pub(crate) fn read_installed_folder(vendor: &str, product: &str) -> Option<String> {
    let key = format!(r"HKLM\Software\{}\{}", vendor, product);
    let output = Command::new("reg.exe")
        .args(["query", &key, "/v", "InstalledFolder"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().find_map(|line| {
        let marker = "REG_SZ";
        let position = line.find(marker)?;
        let value = line[position + marker.len()..].trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn read_installed_folder(_vendor: &str, _product: &str) -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
pub(crate) fn delete_installed_registry_key(vendor: &str, product: &str) -> bool {
    let key = format!(r"HKLM\Software\{}\{}", vendor, product);
    Command::new("reg.exe")
        .args(["delete", &key, "/f"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn delete_installed_registry_key(_vendor: &str, _product: &str) -> bool {
    false
}

#[cfg(target_os = "windows")]
pub(crate) fn register_file_association(
    extension: &str,
    class_name: &str,
    description: &str,
    icon: &str,
    open_command: &str,
) -> bool {
    let extension = extension.trim().trim_start_matches('.');
    if extension.is_empty() || class_name.trim().is_empty() {
        return false;
    }
    let extension_key = format!(r"HKCU\Software\Classes\.{}", extension);
    let class_key = format!(r"HKCU\Software\Classes\{}", class_name);
    let icon_key = format!(r"{}\DefaultIcon", class_key);
    let command_key = format!(r"{}\shell\open\command", class_key);
    let operations = [
        (&extension_key, class_name),
        (&class_key, description),
        (&icon_key, icon),
        (&command_key, open_command),
    ];
    operations.into_iter().all(|(key, value)| {
        Command::new("reg.exe")
            .args(["add", key, "/ve", "/t", "REG_SZ", "/d", value, "/f"])
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn register_file_association(
    _extension: &str,
    _class_name: &str,
    _description: &str,
    _icon: &str,
    _open_command: &str,
) -> bool {
    false
}
