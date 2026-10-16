//! "Start with Windows / at login" via the `auto-launch` crate:
//! Windows: HKCU Run key, macOS: LaunchAgent, Linux: XDG autostart .desktop.

use anyhow::{Result, anyhow};
use auto_launch::{AutoLaunch, AutoLaunchBuilder, MacOSLaunchMode, WindowsEnableMode};
use std::path::PathBuf;

/// Menu label for the current OS.
pub fn label() -> &'static str {
    if cfg!(target_os = "windows") {
        "Start with Windows"
    } else {
        "Open at login"
    }
}

/// What autostart launches: the windowless `capturecraft-tray` binary if it
/// sits next to the current exe (no console window on Windows), otherwise
/// the current exe with `--tray`.
fn launch_target() -> Result<(PathBuf, Vec<String>)> {
    let exe = std::env::current_exe()?;
    let tray = exe.with_file_name(format!("capturecraft-tray{}", std::env::consts::EXE_SUFFIX));
    if tray.exists() {
        Ok((tray, Vec::new()))
    } else {
        Ok((exe, vec!["--tray".to_string()]))
    }
}

fn launcher() -> Result<AutoLaunch> {
    let (path, args) = launch_target()?;
    AutoLaunchBuilder::new()
        .set_app_name("CaptureCraft")
        .set_app_path(&path.to_string_lossy())
        .set_args(&args)
        .set_windows_enable_mode(WindowsEnableMode::CurrentUser)
        .set_macos_launch_mode(MacOSLaunchMode::LaunchAgent)
        .build()
        .map_err(|e| anyhow!("autostart: {e}"))
}

pub fn is_enabled() -> bool {
    launcher()
        .and_then(|l| l.is_enabled().map_err(|e| anyhow!("{e}")))
        .unwrap_or(false)
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    let launcher = launcher()?;
    let result = if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    };
    result.map_err(|e| anyhow!("autostart: {e}"))
}
