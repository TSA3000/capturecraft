//! Background mode: tray icon + global hotkey that opens the overlay.
//! Captures are saved to the output folder and copied to the clipboard.

use crate::config::{Config, config_dir, config_path};
use crate::overlay::{Outcome, Session};
use crate::{RgbaImage, autostart, copy_image};
use anyhow::{Context, Result, anyhow, bail};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::fs::File;
use std::path::{Path, PathBuf};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{
    Icon, MouseButton as TrayButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::WindowId;

pub struct TrayOptions {
    /// Hotkeys tried in order, e.g. `PrintScreen`, `ctrl+shift+s`.
    pub hotkeys: Vec<String>,
    pub output_dir: PathBuf,
    pub clipboard: bool,
}

impl From<Config> for TrayOptions {
    fn from(c: Config) -> Self {
        Self {
            hotkeys: c.hotkeys,
            output_dir: c.output_dir,
            clipboard: c.clipboard,
        }
    }
}

enum UserEvent {
    HotKey(GlobalHotKeyEvent),
    Tray(TrayIconEvent),
    Menu(MenuEvent),
}

/// Runs CaptureCraft in the background until "Quit" is chosen.
pub fn run(opts: TrayOptions) -> Result<()> {
    let lock = single_instance_lock()?;
    std::fs::create_dir_all(&opts.output_dir)
        .with_context(|| format!("failed to create {}", opts.output_dir.display()))?;

    #[allow(unused_mut)]
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        // No Dock icon, tray only.
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    let event_loop = builder.build()?;

    // Forward hotkey/tray/menu events into the winit loop so it wakes up.
    let proxy = event_loop.create_proxy();
    GlobalHotKeyEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::HotKey(e));
    }));
    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Tray(e));
    }));
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));

    let mut app = TrayApp {
        opts,
        _lock: lock,
        ui: None,
        session: None,
        clipboard: None,
        error: None,
    };
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

/// Holds an exclusive lock on `<config dir>/tray.lock` while running,
/// so autostart + a manual start don't fight over the hotkey.
fn single_instance_lock() -> Result<File> {
    std::fs::create_dir_all(config_dir())?;
    let path = config_dir().join("tray.lock");
    let file = File::create(&path).with_context(|| format!("failed to open {}", path.display()))?;
    if file.try_lock().is_err() {
        bail!("CaptureCraft is already running in the tray");
    }
    Ok(file)
}

/// Registers the first hotkey from `candidates` that is free.
/// Returns the registered hotkey and its text.
fn register_first(
    manager: &GlobalHotKeyManager,
    candidates: &[String],
) -> Option<(HotKey, String)> {
    for text in candidates {
        let hotkey: HotKey = match text.parse() {
            Ok(h) => h,
            Err(e) => {
                eprintln!("warning: invalid hotkey '{text}': {e}");
                continue;
            }
        };
        match manager.register(hotkey) {
            Ok(()) => return Some((hotkey, text.clone())),
            Err(e) => eprintln!("hotkey '{text}' unavailable: {e}"),
        }
    }
    None
}

/// Tray UI state, created once the event loop is running.
struct Ui {
    manager: GlobalHotKeyManager,
    hotkey: Option<(HotKey, String)>,
    _tray: TrayIcon,
    capture: MenuItem,
    folder: MenuItem,
    settings: MenuItem,
    reload: MenuItem,
    autostart: CheckMenuItem,
    quit: MenuItem,
}

impl Ui {
    fn capture_label(&self) -> String {
        match &self.hotkey {
            Some((_, text)) => format!("Capture  ({text})"),
            None => "Capture  (no hotkey available)".to_string(),
        }
    }
}

struct TrayApp {
    opts: TrayOptions,
    _lock: File,
    ui: Option<Ui>,
    session: Option<Session>,
    /// Kept open so clipboard content survives on Linux.
    clipboard: Option<arboard::Clipboard>,
    error: Option<anyhow::Error>,
}

impl TrayApp {
    fn init(&mut self) -> Result<()> {
        let manager = GlobalHotKeyManager::new().context("failed to create hotkey manager")?;
        let hotkey = register_first(&manager, &self.opts.hotkeys);
        match &hotkey {
            Some((_, text)) => println!("CaptureCraft running — press {text} to capture"),
            None => eprintln!(
                "warning: none of the hotkeys {:?} could be registered; use the tray icon.\n\
                 Edit {} to choose another.",
                self.opts.hotkeys,
                config_path().display()
            ),
        }

        let capture = MenuItem::new("Capture", true, None);
        let folder = MenuItem::new("Open captures folder", true, None);
        let settings = MenuItem::new("Edit settings…", true, None);
        let reload = MenuItem::new("Reload settings", true, None);
        let autostart = CheckMenuItem::new(autostart::label(), true, autostart::is_enabled(), None);
        let quit = MenuItem::new("Quit CaptureCraft", true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &capture,
            &folder,
            &PredefinedMenuItem::separator(),
            &settings,
            &reload,
            &autostart,
            &PredefinedMenuItem::separator(),
            &quit,
        ])?;

        let tray = TrayIconBuilder::new()
            .with_tooltip("CaptureCraft")
            .with_icon(app_icon()?)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .context("failed to create tray icon")?;

        let ui = Ui {
            manager,
            hotkey,
            _tray: tray,
            capture,
            folder,
            settings,
            reload,
            autostart,
            quit,
        };
        ui.capture.set_text(ui.capture_label());
        self.ui = Some(ui);
        self.apply_clipboard_setting();
        Ok(())
    }

    fn apply_clipboard_setting(&mut self) {
        if !self.opts.clipboard {
            self.clipboard = None;
        } else if self.clipboard.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => self.clipboard = Some(c),
                Err(e) => eprintln!("warning: clipboard unavailable: {e}"),
            }
        }
    }

    /// Re-reads config.toml and re-registers the hotkey.
    fn reload(&mut self) {
        let config = match Config::load_or_create() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("reload failed: {e:#}");
                return;
            }
        };
        self.opts = TrayOptions::from(config);
        if let Err(e) = std::fs::create_dir_all(&self.opts.output_dir) {
            eprintln!("warning: {}: {e}", self.opts.output_dir.display());
        }
        if let Some(ui) = &mut self.ui {
            if let Some((old, _)) = ui.hotkey.take() {
                let _ = ui.manager.unregister(old);
            }
            ui.hotkey = register_first(&ui.manager, &self.opts.hotkeys);
            ui.capture.set_text(ui.capture_label());
            println!("settings reloaded");
        }
        self.apply_clipboard_setting();
    }

    fn toggle_autostart(&self) {
        let Some(ui) = &self.ui else { return };
        // muda already flipped the check mark; revert it if the change fails.
        let wanted = ui.autostart.is_checked();
        if let Err(e) = autostart::set_enabled(wanted) {
            eprintln!("{e:#}");
            ui.autostart.set_checked(!wanted);
        }
    }

    fn start_capture(&mut self, event_loop: &ActiveEventLoop) {
        if self.session.is_some() {
            return;
        }
        match Session::start(event_loop) {
            Ok(s) => self.session = Some(s),
            Err(e) => eprintln!("capture failed: {e:#}"),
        }
    }

    fn handle_capture(&mut self, img: RgbaImage) {
        match save_capture(&img, &self.opts.output_dir) {
            Ok(path) => println!(
                "saved {} ({}x{})",
                path.display(),
                img.width(),
                img.height()
            ),
            Err(e) => eprintln!("save failed: {e:#}"),
        }
        if let Some(clipboard) = &mut self.clipboard
            && let Err(e) = copy_image(clipboard, &img)
        {
            eprintln!("warning: {e:#}");
        }
    }
}

impl ApplicationHandler<UserEvent> for TrayApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.ui.is_some() || self.error.is_some() {
            return;
        }
        if let Err(e) = self.init() {
            self.error = Some(e);
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(ui) = &self.ui else { return };
        match event {
            UserEvent::HotKey(e)
                if e.state() == HotKeyState::Pressed
                    && ui.hotkey.as_ref().is_some_and(|(h, _)| h.id() == e.id()) =>
            {
                self.start_capture(event_loop);
            }
            UserEvent::Tray(TrayIconEvent::Click {
                button: TrayButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }) => self.start_capture(event_loop),
            UserEvent::Menu(e) => {
                if e.id == ui.capture.id() {
                    self.start_capture(event_loop);
                } else if e.id == ui.folder.id() {
                    open_path(&self.opts.output_dir);
                } else if e.id == ui.settings.id() {
                    open_path(&config_path());
                } else if e.id == ui.reload.id() {
                    self.reload();
                } else if e.id == ui.autostart.id() {
                    self.toggle_autostart();
                } else if e.id == ui.quit.id() {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let outcome = match &mut self.session {
            Some(s) => s.window_event(id, event),
            None => None,
        };
        if let Some(outcome) = outcome {
            self.session = None; // closes the overlay windows
            if let Outcome::Selected(img) = outcome {
                self.handle_capture(img);
            }
        }
    }
}

/// Saves as `capture-YYYY-MM-DD_HH-MM-SS.png`, adding `-2`, `-3`… if taken.
pub fn save_capture(img: &RgbaImage, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let mut path = dir.join(format!("capture-{stamp}.png"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("capture-{stamp}-{n}.png"));
        n += 1;
    }
    img.save(&path)
        .with_context(|| format!("failed to save {}", path.display()))?;
    Ok(path)
}

/// Opens a folder or file with the OS default handler.
fn open_path(path: &Path) {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        // `explorer` opens folders; `cmd /c start` opens files with the
        // associated app (e.g. config.toml in your editor).
        let mut c = std::process::Command::new("cmd");
        c.args(["/c", "start", ""]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    if let Err(e) = cmd.arg(path).spawn() {
        eprintln!("failed to open {}: {e}", path.display());
    }
}

/// 32x32 viewfinder icon: blue rounded square with white corner brackets.
fn app_icon() -> Result<Icon> {
    const S: i32 = 32;
    let mut rgba = Vec::with_capacity((S * S * 4) as usize);
    for y in 0..S {
        for x in 0..S {
            // Rounded corners (radius 6)
            let (cx, cy) = (x.clamp(6, S - 7), y.clamp(6, S - 7));
            let inside = (x - cx).pow(2) + (y - cy).pow(2) <= 36;
            let near = |v: i32, edge: i32| (edge..edge + 3).contains(&v);
            let in_corner = |v: i32| (7..14).contains(&v) || (S - 14..S - 7).contains(&v);
            let bracket = (in_corner(x) && (near(y, 7) || near(y, S - 10)))
                || (in_corner(y) && (near(x, 7) || near(x, S - 10)));
            let px = match (inside, bracket) {
                (false, _) => [0, 0, 0, 0],
                (true, true) => [255, 255, 255, 255],
                (true, false) => [0x3B, 0x82, 0xF6, 255],
            };
            rgba.extend_from_slice(&px);
        }
    }
    Icon::from_rgba(rgba, S as u32, S as u32).map_err(|e| anyhow!("bad icon: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_capture_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("capturecraft-test-{}", std::process::id()));
        let img = RgbaImage::new(2, 2);
        let a = save_capture(&img, &dir).unwrap();
        let b = save_capture(&img, &dir).unwrap();
        assert_ne!(a, b);
        assert!(a.exists() && b.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn icon_builds() {
        assert!(app_icon().is_ok());
    }
}
