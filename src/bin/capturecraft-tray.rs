//! Windowless tray launcher: no console window on Windows. Used by autostart.
//! Errors are written to `<config dir>/CaptureCraft/error.log`.
#![cfg_attr(windows, windows_subsystem = "windows")]

use capturecraft::config::{Config, config_dir};
use capturecraft::tray::{self, TrayOptions};

fn main() {
    let result = Config::load_or_create().and_then(|c| tray::run(TrayOptions::from(c)));
    if let Err(e) = result {
        let log = config_dir().join("error.log");
        let _ = std::fs::create_dir_all(config_dir());
        let _ = std::fs::write(&log, format!("{e:#}\n"));
        std::process::exit(1);
    }
}
