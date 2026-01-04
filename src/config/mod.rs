pub mod models;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FontSizePreset {
    Small,
    Standard,
    Large,
}

impl Default for FontSizePreset {
    fn default() -> Self {
        FontSizePreset::Standard
    }
}

impl FontSizePreset {
    pub fn offset(&self) -> f32 {
        match self {
            FontSizePreset::Small => -2.0,
            FontSizePreset::Standard => 0.0,
            FontSizePreset::Large => 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChatDensity {
    Compact,
    Normal,
    Comfortable,
}

impl Default for ChatDensity {
    fn default() -> Self {
        ChatDensity::Normal
    }
}

impl ChatDensity {
    pub fn message_spacing(&self) -> f32 {
        match self {
            ChatDensity::Compact => 4.0,
            ChatDensity::Normal => 8.0,
            ChatDensity::Comfortable => 12.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiPreferences {
    #[serde(default)]
    pub font_size: FontSizePreset,
    #[serde(default = "default_base_font_points")]
    pub base_font_points: f32,
    #[serde(default)]
    pub chat_density: ChatDensity,
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            font_size: FontSizePreset::default(),
            base_font_points: default_base_font_points(),
            chat_density: ChatDensity::default(),
        }
    }
}

fn default_base_font_points() -> f32 {
    14.0
}

impl UiPreferences {
    pub fn apply_to_context(&self, ctx: &eframe::egui::Context) {
        use eframe::egui::{FontFamily, FontId, TextStyle};

        let base = self.base_font_points + self.font_size.offset();

        ctx.style_mut(|style| {
            style.text_styles.insert(
                TextStyle::Heading,
                FontId::new(base + 4.0, FontFamily::Proportional),
            );

            style
                .text_styles
                .insert(TextStyle::Body, FontId::new(base, FontFamily::Proportional));

            style.text_styles.insert(
                TextStyle::Button,
                FontId::new(base, FontFamily::Proportional),
            );

            style.text_styles.insert(
                TextStyle::Small,
                FontId::new(base - 2.0, FontFamily::Proportional),
            );

            style.text_styles.insert(
                TextStyle::Monospace,
                FontId::new(base, FontFamily::Monospace),
            );
        });
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub last_base_url: Option<String>,
    #[serde(default = "default_auto_start")]
    pub auto_start: bool,
    pub directory_override: Option<String>,
}

fn default_auto_start() -> bool {
    true
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            last_base_url: None,
            auto_start: default_auto_start(),
            directory_override: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    #[serde(default = "default_push_to_talk_key")]
    pub push_to_talk_key: String,
    pub whisper_model_path: Option<String>,
}

fn default_push_to_talk_key() -> String {
    "AltRight".to_string()
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            push_to_talk_key: default_push_to_talk_key(),
            whisper_model_path: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub ui: UiPreferences,
    #[serde(default)]
    pub audio: AudioConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            ui: UiPreferences::default(),
            audio: AudioConfig::default(),
        }
    }
}

impl AppConfig {
    fn config_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "opencode-egui")
            .map(|dirs| dirs.config_dir().join("config.json"))
    }

    pub fn load() -> Self {
        if let Some(path) = Self::config_path() {
            if path.exists() {
                if let Ok(contents) = std::fs::read_to_string(&path) {
                    if let Ok(config) = serde_json::from_str(&contents) {
                        return config;
                    }
                }
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        if let Some(path) = Self::config_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(&path, json);
            }
        }
    }
}
