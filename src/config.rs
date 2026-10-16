//! User settings in `<config dir>/CaptureCraft/config.toml`
//! (Windows: `%APPDATA%\CaptureCraft\config.toml`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const HEADER: &str = "\
# CaptureCraft settings. Use \"Reload settings\" in the tray menu after editing.
#
# hotkeys:    tried in order; the first one that can be registered is used.
#             Examples: \"PrintScreen\", \"ctrl+shift+s\", \"alt+PrintScreen\", \"super+shift+2\"
# output_dir: where tray captures are saved.
# clipboard:  copy every capture to the clipboard.

";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkeys: Vec<String>,
    pub output_dir: PathBuf,
    pub clipboard: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkeys: default_hotkeys(),
            output_dir: default_output_dir(),
            clipboard: true,
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
        if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            return Self::parse(&text).with_context(|| format!("invalid {}", path.display()));
        }
        let config = Self::default();
        config.save_at(path)?;
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
    fn rejects_invalid_toml() {
        assert!(Config::parse("hotkeys = 5").is_err());
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
