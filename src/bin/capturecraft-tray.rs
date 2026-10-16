//! Windowless tray launcher: no console window on Windows. Used by autostart.
//! Everything that would go to the console is written to
//! `<config dir>/CaptureCraft/tray.log` instead, including panics.
#![cfg_attr(windows, windows_subsystem = "windows")]

use capturecraft::config::{Config, config_dir};
use capturecraft::tray::{self, TrayOptions};
use std::io::Write;
use std::sync::Mutex;

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

fn log(msg: &str) {
    if let Ok(mut guard) = LOG.lock()
        && let Some(file) = guard.as_mut()
    {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(file, "{stamp} {msg}");
    }
}

fn main() {
    let _ = std::fs::create_dir_all(config_dir());
    let path = config_dir().join("tray.log");
    if let Ok(file) = std::fs::File::create(&path) {
        *LOG.lock().unwrap() = Some(file);
    }
    std::panic::set_hook(Box::new(|info| log(&format!("PANIC: {info}"))));

    log(&format!(
        "start: {}",
        std::env::current_exe().unwrap_or_default().display()
    ));
    let result = Config::load_or_create().and_then(|c| {
        log(&format!("config: {c:?}"));
        tray::run(TrayOptions::from(c))
    });
    match result {
        Ok(()) => log("exit: ok"),
        Err(e) => {
            log(&format!("ERROR: {e:#}"));
            std::process::exit(1);
        }
    }
}
