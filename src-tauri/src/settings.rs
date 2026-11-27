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
    /// Display name of the hotkey ("Right Ctrl").
    pub hotkey_label: String,
    /// Double-tap the hotkey to keep recording hands-free until the next tap.
    pub double_tap_hands_free: bool,
    /// Show a caption of the words above the bar while speaking (transcribes about once a second).
    pub preview_caption: bool,
    /// Keep a history of dictations (stored only on this PC).
    pub history_enabled: bool,
    /// Delete history older than this many days; 0 keeps it forever.
    pub history_days: u32,
    /// Microphone name; `None` means the Windows default input device.
    pub mic_device: Option<String>,
    /// Recordings are cut off after this many seconds.
    pub max_record_secs: u32,
    /// Run the local LLM over each transcript (fillers, self-corrections, punctuation).
    pub cleanup_enabled: bool,
    /// Cleanup LLM file name (GGUF) inside `models_dir`.
    pub llm_model: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            models_dir: data_dir().join("models"),
            whisper_model: "ggml-large-v3-turbo-q5_0.bin".into(),
            hotkey_vk: 0xA3,
            hotkey_label: "Right Ctrl".into(),
            double_tap_hands_free: true,
            preview_caption: true,
            history_enabled: true,
            history_days: 30,
            mic_device: None,
            max_record_secs: 360,
            cleanup_enabled: true,
            llm_model: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf".into(),
        }
    }
}

impl Settings {
    pub fn whisper_model_path(&self) -> PathBuf {
        self.model_path(&self.whisper_model)
    }

    pub fn llm_model_path(&self) -> PathBuf {
        self.model_path(&self.llm_model)
    }

    /// Path of a model file. Looks in `models_dir` first; debug builds also fall back to the
    /// repo's git-ignored `models/` folder so a fresh dev checkout works without editing settings.
    fn model_path(&self, file: &str) -> PathBuf {
        let primary = self.models_dir.join(file);
        if primary.exists() {
            return primary;
        }
        #[cfg(debug_assertions)]
        {
            let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../models").join(file);
            if dev.exists() {
                log::info!("model not in {}; using dev copy {}", self.models_dir.display(), dev.display());
                return dev;
            }
        }
        primary
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
