//! User settings stored as TOML in `%APPDATA%\Saywell\settings.toml`.
//! M1 keeps this minimal; M3 adds the settings UI.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Folder holding the model files.
    pub models_dir: PathBuf,
    /// Whisper model file name inside `models_dir`.
    pub whisper_model: String,
    /// Push-to-talk key, as a Windows virtual-key code. Default: Right Ctrl (0xA3).
    pub hotkey_vk: u32,
    /// Recordings are cut off after this many seconds.
    pub max_record_secs: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            models_dir: data_dir().join("models"),
            whisper_model: "ggml-large-v3-turbo-q5_0.bin".into(),
            hotkey_vk: 0xA3,
            max_record_secs: 360,
        }
    }
}

impl Settings {
    pub fn whisper_model_path(&self) -> PathBuf {
        self.models_dir.join(&self.whisper_model)
    }

    /// Load settings, writing defaults to disk if the file does not exist yet.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            return toml::from_str(&text).with_context(|| format!("parsing {}", path.display()));
        }
        let s = Self::default();
        s.save(path)?;
        Ok(s)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// `%APPDATA%\Saywell` (falls back to the current directory if APPDATA is unset).
pub fn data_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Saywell")
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_partial_files() {
        let dir = std::env::temp_dir().join(format!("saywell-test-{}", std::process::id()));
        let path = dir.join("settings.toml");
        let _ = std::fs::remove_file(&path);

        let created = Settings::load_or_create(&path).unwrap();
        assert_eq!(created, Settings::default());
        assert_eq!(Settings::load_or_create(&path).unwrap(), created);

        // Missing keys fall back to defaults.
        std::fs::write(&path, "hotkey_vk = 165\n").unwrap();
        let s = Settings::load_or_create(&path).unwrap();
        assert_eq!(s.hotkey_vk, 165);
        assert_eq!(s.whisper_model, Settings::default().whisper_model);
        let _ = std::fs::remove_dir_all(dir);
    }
}
