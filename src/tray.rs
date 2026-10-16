//! Background mode: tray icon + global hotkey that opens the overlay.
//! Captures are saved to the output folder and copied to the clipboard.

use crate::config::{Config, Overlay, SelectMode, config_dir, config_path};
use crate::overlay::{Outcome, Session};
use crate::{RgbaImage, autostart, copy_image, icon_rgba};
use anyhow::{Context, Result, anyhow, bail};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton as TrayButton, TrayIcon, TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

pub struct TrayOptions {
    pub selection: SelectMode,
    pub editor: bool,
    pub overlay: Overlay,
    /// Hotkeys tried in order, e.g. `PrintScreen`, `ctrl+shift+s`.
    pub hotkeys: Vec<String>,
    pub output_dir: PathBuf,
    pub clipboard: bool,
}

impl From<Config> for TrayOptions {
    fn from(c: Config) -> Self {
        Self {
            selection: c.selection,
            editor: c.editor,
            overlay: c.overlay,
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
    let _ = std::fs::remove_file(quit_marker());
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
        config_mtime: config_mtime(),
        last_config_check: Instant::now(),
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

/// True if a tray instance currently holds the lock.
pub fn is_running() -> bool {
    let path = config_dir().join("tray.lock");
    match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
    {
        Ok(file) => file.try_lock().is_err(),
        Err(_) => false,
    }
}

fn config_mtime() -> Option<SystemTime> {
    std::fs::metadata(config_path())
        .and_then(|m| m.modified())
        .ok()
}

/// How often the tray checks config.toml for changes (e.g. from the window).
const CONFIG_POLL: Duration = Duration::from_secs(2);

/// Marker a window writes to ask a running tray to quit. A file keeps this
/// portable: the window cannot post a message into the tray's event loop.
fn quit_marker() -> PathBuf {
    config_dir().join("quit")
}

/// Asks a running tray to exit, and closes the CaptureCraft windows.
/// Used by File › Quit in both windows.
pub fn request_quit() {
    if is_running() {
        let _ = std::fs::write(quit_marker(), b"quit");
    }
    close_app_window();
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
    window: MenuItem,
    capture: MenuItem,
    editor: MenuItem,
    last: MenuItem,
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
    config_mtime: Option<SystemTime>,
    last_config_check: Instant,
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

        let window = MenuItem::new("Open CaptureCraft…", true, None);
        let capture = MenuItem::new("Capture", true, None);
        let editor = MenuItem::new("Open editor", true, None);
        let last = MenuItem::new("Open last capture", true, None);
        let folder = MenuItem::new("Open captures folder", true, None);
        let settings = MenuItem::new("Edit settings…", true, None);
        let reload = MenuItem::new("Reload settings", true, None);
        let autostart = CheckMenuItem::new(autostart::label(), true, autostart::is_enabled(), None);
        let quit = MenuItem::new("Quit CaptureCraft", true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &window,
            &capture,
            &editor,
            &last,
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
            window,
            capture,
            editor,
            last,
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
        self.config_mtime = config_mtime();
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
            ui.autostart.set_checked(autostart::is_enabled());
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
        if self.opts.selection == SelectMode::Fullscreen {
            match crate::capture_primary() {
                Ok(img) => self.handle_capture(img),
                Err(e) => eprintln!("capture failed: {e:#}"),
            }
            return;
        }
        match Session::start(event_loop, self.opts.selection, self.opts.overlay.clone()) {
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
            // Double click opens the window; a single left click would fire
            // first and start a capture, so it is deliberately not bound.
            UserEvent::Tray(TrayIconEvent::DoubleClick {
                button: TrayButton::Left,
                ..
            }) => open_window(),
            UserEvent::Menu(e) => {
                if e.id == ui.window.id() {
                    open_window();
                } else if e.id == ui.capture.id() {
                    self.start_capture(event_loop);
                } else if e.id == ui.editor.id() {
                    match crate::app::newest_capture(&self.opts.output_dir) {
                        Some(path) => crate::open_editor(&path),
                        None => eprintln!("no captures yet to edit"),
                    }
                } else if e.id == ui.last.id() {
                    match crate::app::newest_capture(&self.opts.output_dir) {
                        Some(path) => open_path(&path),
                        None => eprintln!("no captures yet in {}", self.opts.output_dir.display()),
                    }
                } else if e.id == ui.folder.id() {
                    open_path(&self.opts.output_dir);
                } else if e.id == ui.settings.id() {
                    open_path(&config_path());
                } else if e.id == ui.reload.id() {
                    self.reload();
                } else if e.id == ui.autostart.id() {
                    self.toggle_autostart();
                } else if e.id == ui.quit.id() {
                    // Quit means the whole app, so take the window with it.
                    close_app_window();
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Pick up changes made in the window (or by hand) to config.toml.
        if self.ui.is_some() && self.last_config_check.elapsed() >= CONFIG_POLL {
            self.last_config_check = Instant::now();
            if config_mtime() != self.config_mtime {
                self.reload();
            }
        }
        // A window asked everything to quit.
        if self.ui.is_some() && quit_marker().exists() {
            let _ = std::fs::remove_file(quit_marker());
            close_app_window();
            event_loop.exit();
            return;
        }
        // An open overlay may need an earlier wake-up than the config poll.
        let overlay_wake = self.session.as_mut().and_then(Session::tick);
        let config_wake = Instant::now() + CONFIG_POLL;
        let wake = overlay_wake.map_or(config_wake, |at| at.min(config_wake));
        event_loop.set_control_flow(ControlFlow::WaitUntil(wake));
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

/// Starts the settings window (`capturecraft-app` next to this exe).
fn open_window() {
    let exe = crate::sibling_exe("capturecraft-app");
    let mut cmd = std::process::Command::new(&exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    if let Err(e) = cmd.spawn() {
        eprintln!("failed to start {}: {e}", exe.display());
    }
}

/// Asks every open CaptureCraft window to close, so "Quit" ends everything:
/// the capture window and any editor windows.
#[cfg(windows)]
fn close_app_window() {
    use windows::Win32::Foundation::{HWND, LPARAM, TRUE, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, IsWindowVisible, PostMessageW, WM_CLOSE,
    };
    // BOOL is a core type; only the TRUE constant lives in Foundation.
    use windows::core::BOOL;

    // Editor windows are titled per file, so match on the prefix instead of
    // looking one window up by its exact title.
    extern "system" fn close_matching(hwnd: HWND, _: LPARAM) -> BOOL {
        let mut buffer = [0u16; 256];
        // SAFETY: `buffer` is a valid, correctly sized slice for the call,
        // and `hwnd` comes from EnumWindows.
        let len = unsafe { GetWindowTextW(hwnd, &mut buffer) };
        if len > 0 && unsafe { IsWindowVisible(hwnd) }.as_bool() {
            let title = String::from_utf16_lossy(&buffer[..len as usize]);
            if title.starts_with("CaptureCraft") {
                // SAFETY: posting WM_CLOSE to a window we just enumerated.
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
        }
        // Keep enumerating.
        TRUE
    }

    // SAFETY: the callback only reads the handle it is given and posts to it.
    unsafe {
        let _ = EnumWindows(Some(close_matching), LPARAM(0));
    }
}

#[cfg(not(windows))]
fn close_app_window() {}

/// Opens a folder, file or URL with the OS default handler.
///
/// On Windows this goes through the shell API rather than `cmd /c start`,
/// which would flash a console window.
#[cfg(windows)]
pub(crate) fn open_path(path: &Path) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, w};
    let file = HSTRING::from(path.as_os_str());
    let result = unsafe { ShellExecuteW(None, w!("open"), &file, None, None, SW_SHOWNORMAL) };
    // ShellExecuteW reports failure as a "handle" of 32 or less.
    if result.0 as usize <= 32 {
        eprintln!("failed to open {}", path.display());
    }
}

#[cfg(not(windows))]
pub(crate) fn open_path(path: &Path) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    if let Err(e) = std::process::Command::new(program).arg(path).spawn() {
        eprintln!("failed to open {}: {e}", path.display());
    }
}

fn app_icon() -> Result<Icon> {
    let (rgba, w, h) = icon_rgba();
    Icon::from_rgba(rgba, w, h).map_err(|e| anyhow!("bad icon: {e}"))
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
    fn quit_marker_lives_in_the_config_dir() {
        let marker = quit_marker();
        assert!(marker.starts_with(config_dir()));
        assert_eq!(marker.file_name().and_then(|n| n.to_str()), Some("quit"));
    }

    #[test]
    fn icon_builds() {
        assert!(app_icon().is_ok());
    }
}
