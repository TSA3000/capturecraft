//! Background mode: tray icon + global hotkey that opens the overlay.
//! Captures are saved to the output folder and copied to the clipboard.

use crate::overlay::{Outcome, Session};
use crate::{RgbaImage, copy_image};
use anyhow::{Context, Result, anyhow};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::path::{Path, PathBuf};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{
    Icon, MouseButton as TrayButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::WindowId;

/// Default capture hotkey.
#[cfg(target_os = "macos")]
pub const DEFAULT_HOTKEY: &str = "super+shift+2";
/// Default capture hotkey.
#[cfg(not(target_os = "macos"))]
pub const DEFAULT_HOTKEY: &str = "PrintScreen";

pub struct TrayOptions {
    /// Hotkey string, e.g. `PrintScreen`, `ctrl+shift+s`, `super+shift+2`.
    pub hotkey: String,
    pub output_dir: PathBuf,
    pub clipboard: bool,
}

/// Default output folder: `<Pictures>/CaptureCraft`.
pub fn default_output_dir() -> PathBuf {
    dirs::picture_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("CaptureCraft")
}

enum UserEvent {
    HotKey(GlobalHotKeyEvent),
    Tray(TrayIconEvent),
    Menu(MenuEvent),
}

/// Runs CaptureCraft in the background until "Quit" is chosen.
pub fn run(opts: TrayOptions) -> Result<()> {
    let hotkey: HotKey = opts
        .hotkey
        .parse()
        .map_err(|e| anyhow!("invalid hotkey '{}': {e}", opts.hotkey))?;
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
        hotkey,
        started: false,
        _manager: None,
        _tray: None,
        menu: None,
        session: None,
        clipboard: None,
        error: None,
    };
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

#[derive(Clone)]
struct MenuIds {
    capture: MenuId,
    folder: MenuId,
    quit: MenuId,
}

struct TrayApp {
    opts: TrayOptions,
    hotkey: HotKey,
    started: bool,
    // Kept alive for the lifetime of the app.
    _manager: Option<GlobalHotKeyManager>,
    _tray: Option<TrayIcon>,
    menu: Option<MenuIds>,
    session: Option<Session>,
    /// Kept open so clipboard content survives on Linux.
    clipboard: Option<arboard::Clipboard>,
    error: Option<anyhow::Error>,
}

impl TrayApp {
    fn init(&mut self) -> Result<()> {
        let manager = GlobalHotKeyManager::new().context("failed to create hotkey manager")?;
        match manager.register(self.hotkey) {
            Ok(()) => println!(
                "CaptureCraft running — press {} to capture",
                self.opts.hotkey
            ),
            Err(e) => eprintln!(
                "warning: could not register hotkey '{}': {e}\n\
                 On Windows 11, turn off Settings > Accessibility > Keyboard > \
                 \"Use the Print screen key to open screen capture\", \
                 or start with e.g. --hotkey ctrl+shift+s. The tray icon still works.",
                self.opts.hotkey
            ),
        }
        self._manager = Some(manager);

        let capture = MenuItem::new(format!("Capture  ({})", self.opts.hotkey), true, None);
        let folder = MenuItem::new("Open captures folder", true, None);
        let quit = MenuItem::new("Quit CaptureCraft", true, None);
        let separator = PredefinedMenuItem::separator();
        let menu = Menu::new();
        menu.append_items(&[&capture, &folder, &separator, &quit])?;
        self.menu = Some(MenuIds {
            capture: capture.id().clone(),
            folder: folder.id().clone(),
            quit: quit.id().clone(),
        });

        let tray = TrayIconBuilder::new()
            .with_tooltip(format!("CaptureCraft — {}", self.opts.hotkey))
            .with_icon(app_icon()?)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .context("failed to create tray icon")?;
        self._tray = Some(tray);

        if self.opts.clipboard {
            match arboard::Clipboard::new() {
                Ok(c) => self.clipboard = Some(c),
                Err(e) => eprintln!("warning: clipboard unavailable: {e}"),
            }
        }
        Ok(())
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
        if self.started {
            return;
        }
        self.started = true;
        if let Err(e) = self.init() {
            self.error = Some(e);
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::HotKey(e)
                if e.id() == self.hotkey.id() && e.state() == HotKeyState::Pressed =>
            {
                self.start_capture(event_loop);
            }
            UserEvent::Tray(TrayIconEvent::Click {
                button: TrayButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }) => self.start_capture(event_loop),
            UserEvent::Menu(e) => {
                let Some(ids) = self.menu.clone() else { return };
                if e.id == ids.capture {
                    self.start_capture(event_loop);
                } else if e.id == ids.folder {
                    open_folder(&self.opts.output_dir);
                } else if e.id == ids.quit {
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

fn open_folder(dir: &Path) {
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let program = "xdg-open";
    if let Err(e) = std::process::Command::new(program).arg(dir).spawn() {
        eprintln!("failed to open {}: {e}", dir.display());
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
    fn default_hotkey_parses() {
        assert!(DEFAULT_HOTKEY.parse::<HotKey>().is_ok());
        assert!("ctrl+shift+s".parse::<HotKey>().is_ok());
    }

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
}
