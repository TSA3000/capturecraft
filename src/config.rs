//! User settings in `<config dir>/CaptureCraft/config.toml`
//! (Windows: `%APPDATA%\CaptureCraft\config.toml`).

use crate::theme::ThemeKind;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const HEADER: &str = "\
# CaptureCraft settings. The window and tray reload this file automatically.
#
# selection:  \"region\" (snap while moving, pixel picking when still),
#             \"window\" (click a window) or \"fullscreen\".
# hotkeys:    tried in order; the first one that can be registered is used.
#             Examples: \"PrintScreen\", \"ctrl+shift+s\", \"alt+PrintScreen\", \"super+shift+2\"
# output_dir: where captures are saved.
# clipboard:  copy every capture to the clipboard.
# start_tray: start the background tray (and its hotkey) when the window opens.
# theme:      \"pro\", \"proMedium\", \"studio\" or \"studioLight\" (PhotoCraft's palettes).
# editor:     open the capture in the annotation editor instead of saving it
#             straight away (Snagit's \"Preview in Editor\").
#
# [overlay] — look and feel of the capture overlay
# zoom_delay_ms:  cursor must be still this long before the magnifier appears
#                 (Region mode). 0 = always show it.
# hide_delay_ms:  wait after hiding the window before the screen is frozen.
# dim:            brightness outside the selection, 0-255 (255 = no dimming).
# dim_on_drag:    true dims only once you start dragging a region; while you
#                 are just hovering, the screen stays bright (Region mode).
# border_color:   selection frame, \"#RRGGBB\".
# border_width:   selection frame thickness in px.
# zoom:           magnifier pixel size.
# zoom_radius:    magnifier shows (2*r+1)^2 pixels.
# zoom_offset:    distance from the cursor to the magnifier, px.
# label_scale:    size badge text scale.
# min_drag:       drags smaller than this count as a click, px.
# min_window:     windows smaller than this are not snap targets, px.

";

/// What the Capture button / hotkey does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelectMode {
    /// Drag a region, or click a window (snaps to windows).
    #[default]
    Region,
    /// Click a window.
    Window,
    /// Whole primary screen, no overlay.
    Fullscreen,
}

impl SelectMode {
    pub const ALL: [SelectMode; 3] = [
        SelectMode::Region,
        SelectMode::Window,
        SelectMode::Fullscreen,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SelectMode::Region => "Region",
            SelectMode::Window => "Window",
            SelectMode::Fullscreen => "Full screen",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub selection: SelectMode,
    pub hotkeys: Vec<String>,
    pub output_dir: PathBuf,
    pub clipboard: bool,
    pub start_tray: bool,
    pub editor: bool,
    pub theme: ThemeKind,
    pub overlay: Overlay,
}

/// Overlay appearance and timings. Every field has a default, so an older
/// or hand-trimmed config file still loads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overlay {
    pub zoom_delay_ms: u64,
    pub hide_delay_ms: u64,
    pub dim: u32,
    pub dim_on_drag: bool,
    pub border_color: String,
    pub border_width: u32,
    pub zoom: u32,
    pub zoom_radius: i32,
    pub zoom_offset: i32,
    pub label_scale: u32,
    pub min_drag: u32,
    pub min_window: u32,
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            zoom_delay_ms: 250,
            hide_delay_ms: 300,
            dim: 140,
            dim_on_drag: true,
            border_color: "#3B82F6".to_string(),
            border_width: 2,
            zoom: 8,
            zoom_radius: 7,
            zoom_offset: 24,
            label_scale: 2,
            min_drag: 3,
            min_window: 20,
        }
    }
}

impl Overlay {
    pub fn zoom_delay(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.zoom_delay_ms)
    }

    pub fn hide_delay(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.hide_delay_ms)
    }

    /// Border colour as 0x00RRGGBB; falls back to the default blue.
    pub fn border(&self) -> u32 {
        parse_hex_color(&self.border_color).unwrap_or(0x003B_82F6)
    }

    /// Dimming factor out of 256, clamped so the screen never goes black.
    pub fn dim_factor(&self) -> u32 {
        self.dim.clamp(16, 255)
    }
}

/// Parses `#RRGGBB` (or `RRGGBB`) into 0x00RRGGBB.
fn parse_hex_color(text: &str) -> Option<u32> {
    let hex = text.trim().trim_start_matches('#');
    (hex.len() == 6).then(|| u32::from_str_radix(hex, 16).ok())?
}

impl Default for Config {
    fn default() -> Self {
        Self {
            selection: SelectMode::Region,
            hotkeys: default_hotkeys(),
            output_dir: default_output_dir(),
            clipboard: true,
            start_tray: true,
            editor: false,
            theme: ThemeKind::default(),
            overlay: Overlay::default(),
        }
    }
}

/// Default hotkeys, in order of preference.
pub fn default_hotkeys() -> Vec<String> {
    let first = if cfg!(target_os = "macos") {
        "super+shift+2"
    } else {
        "PrintScreen"
    };
    vec![first.to_string(), "ctrl+shift+s".to_string()]
}

/// Default output folder: `<Pictures>/CaptureCraft`.
pub fn default_output_dir() -> PathBuf {
    dirs::picture_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("CaptureCraft")
}

/// `<config dir>/CaptureCraft`.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("CaptureCraft")
}

/// `<config dir>/CaptureCraft/config.toml`.
pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

impl Config {
    /// Loads the config file, creating it with defaults if it doesn't exist.
    pub fn load_or_create() -> Result<Self> {
        Self::load_or_create_at(&config_path())
    }

    pub fn load_or_create_at(path: &Path) -> Result<Self> {
        if !path.exists() {
            let config = Self::default();
            config.save_at(path)?;
            return Ok(config);
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let config = Self::parse(&text).with_context(|| format!("invalid {}", path.display()))?;

        // A file written by an older version is missing newer keys. Rewrite
        // it so every setting (and its comment) is visible and editable.
        if let Ok(canonical) = config.to_toml()
            && canonical.trim() != text.trim()
        {
            let _ = config.save_at(path);
        }
        Ok(config)
    }

    pub fn parse(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(format!("{HEADER}{}", toml::to_string(self)?))
    }

    pub fn save_at(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_toml()?)
            .with_context(|| format!("failed to write {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use global_hotkey::hotkey::HotKey;

    #[test]
    fn default_hotkeys_parse() {
        for hk in default_hotkeys() {
            assert!(hk.parse::<HotKey>().is_ok(), "bad default hotkey {hk}");
        }
    }

    #[test]
    fn roundtrip() {
        let config = Config::default();
        let text = config.to_toml().unwrap();
        assert!(text.starts_with("# CaptureCraft settings"));
        assert_eq!(Config::parse(&text).unwrap(), config);
    }

    #[test]
    fn missing_keys_use_defaults() {
        let config = Config::parse("clipboard = false").unwrap();
        assert!(!config.clipboard);
        assert_eq!(config.hotkeys, default_hotkeys());
    }

    #[test]
    fn selection_parses() {
        assert_eq!(
            Config::parse("selection = \"window\"").unwrap().selection,
            SelectMode::Window
        );
        assert_eq!(Config::parse("").unwrap().selection, SelectMode::Region);
        assert!(Config::parse("selection = \"blah\"").is_err());
    }

    #[test]
    fn overlay_defaults_and_parsing() {
        let o = Overlay::default();
        assert_eq!(o.zoom_delay_ms, 250);
        assert_eq!(o.border(), 0x003B_82F6);
        assert_eq!(parse_hex_color("#ff0000"), Some(0x00FF_0000));
        assert_eq!(parse_hex_color("00FF00"), Some(0x0000_FF00));
        assert_eq!(parse_hex_color("nope"), None);

        // A bad colour or an extreme dim value must not break the overlay.
        let c = Config::parse("[overlay]\nborder_color = \"nope\"\ndim = 0").unwrap();
        assert_eq!(c.overlay.border(), 0x003B_82F6);
        assert_eq!(c.overlay.dim_factor(), 16);
    }

    #[test]
    fn theme_parses() {
        assert_eq!(Config::default().theme, ThemeKind::ProMedium);
        assert_eq!(
            Config::parse("theme = \"studio\"").unwrap().theme,
            ThemeKind::Studio
        );
        assert!(Config::parse("theme = \"nope\"").is_err());
    }

    #[test]
    fn editor_defaults_off_and_parses() {
        assert!(!Config::default().editor);
        assert!(Config::parse("editor = true").unwrap().editor);
    }

    #[test]
    fn start_tray_defaults_true() {
        assert!(Config::default().start_tray);
        assert!(!Config::parse("start_tray = false").unwrap().start_tray);
    }

    #[test]
    fn dim_on_drag_defaults_true_and_parses() {
        assert!(Overlay::default().dim_on_drag);
        let c = Config::parse("[overlay]\ndim_on_drag = false").unwrap();
        assert!(!c.overlay.dim_on_drag);
    }

    #[test]
    fn overlay_partial_table_keeps_defaults() {
        let c = Config::parse("[overlay]\nzoom_delay_ms = 50").unwrap();
        assert_eq!(c.overlay.zoom_delay_ms, 50);
        assert_eq!(c.overlay.zoom, Overlay::default().zoom);
    }

    #[test]
    fn rejects_invalid_toml() {
        assert!(Config::parse("hotkeys = 5").is_err());
    }

    #[test]
    fn load_upgrades_an_old_file() {
        let dir = std::env::temp_dir().join(format!("capturecraft-upgrade-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "clipboard = false\n").unwrap();

        let config = Config::load_or_create_at(&path).unwrap();
        assert!(!config.clipboard);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[overlay]"), "overlay section not written");
        assert!(text.contains("zoom_delay_ms"));
        assert!(text.contains("start_tray"));
        assert!(text.contains("clipboard = false"), "setting not kept");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_or_create_writes_defaults_once() {
        let dir = std::env::temp_dir().join(format!("capturecraft-cfg-{}", std::process::id()));
        let path = dir.join("config.toml");
        let created = Config::load_or_create_at(&path).unwrap();
        assert!(path.exists());

        std::fs::write(&path, "clipboard = false").unwrap();
        let loaded = Config::load_or_create_at(&path).unwrap();
        assert!(created.clipboard && !loaded.clipboard);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
