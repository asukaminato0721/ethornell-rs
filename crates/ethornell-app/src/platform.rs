use std::path::Path;

pub(crate) fn test_path_writable(path: &Path) -> bool {
    let directory = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    if !directory.is_dir() {
        return false;
    }

    let probe = directory.join(format!(
        ".ethornell-write-probe-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("runtime")
    ));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            std::fs::remove_file(probe).is_ok()
        }
        Err(_) => false,
    }
}

pub(crate) fn shell_open(target: &str) -> bool {
    if std::env::var_os("ETHORNELL_GUI_HEADLESS").is_some() {
        return false;
    }

    shell_open_platform(target)
}

#[cfg(target_os = "macos")]
fn shell_open_platform(target: &str) -> bool {
    std::process::Command::new("open")
        .arg(target)
        .spawn()
        .is_ok()
}

#[cfg(target_os = "windows")]
fn shell_open_platform(target: &str) -> bool {
    std::process::Command::new("cmd")
        .args(["/C", "start", "", target])
        .spawn()
        .is_ok()
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn shell_open_platform(target: &str) -> bool {
    std::process::Command::new("xdg-open")
        .arg(target)
        .spawn()
        .is_ok()
}
