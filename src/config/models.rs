use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ModelConfigError {
    #[allow(dead_code)]
    #[error("Failed to read models.toml: {0}")]
    Read(String),

    #[allow(dead_code)]
    #[error("Failed to parse models.toml: {0}")]
    Parse(String),

    #[error("Failed to write models.toml: {0}")]
    Write(String),
}

/// Curated model entry in models.toml
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CuratedModel {
    pub name: String,
    pub provider: String,
    pub model_id: String,
}

impl CuratedModel {
    pub fn new(
        name: impl Into<String>,
        provider: impl Into<String>,
        model_id: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            provider: provider.into(),
            model_id: model_id.into(),
        }
    }
}

/// Provider configuration from [[providers]] section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub display_name: String,
    pub api_key_env: String,
    pub models_url: String,
    pub auth_type: String,
    #[serde(default)]
    pub auth_header: Option<String>,
    #[serde(default)]
    pub auth_param: Option<String>,
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
    pub response_format: ResponseFormat,
}

/// Response parsing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResponseFormat {
    pub models_path: String,
    pub model_id_field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id_strip_prefix: Option<String>,
    pub model_name_field: String,
}

/// Models configuration from models.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub models: ModelsSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsSection {
    #[serde(default = "default_model")]
    pub default_model: String,

    #[serde(default)]
    pub curated: Vec<CuratedModel>,
}

impl Default for ModelsSection {
    fn default() -> Self {
        Self {
            default_model: default_model(),
            curated: Vec::new(),
        }
    }
}

fn default_model() -> String {
    "openai/gpt-4".to_string()
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            models: ModelsSection::default(),
        }
    }
}

impl ModelsConfig {
    /// Get the path to models.toml relative to the executable directory.
    /// In development, this will be target/debug/config/models.toml
    /// In production, this will be ./config/models.toml (relative to executable)
    fn config_path() -> Option<PathBuf> {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
            .map(|exe_dir| exe_dir.join("config").join("models.toml"))
    }

    /// Load models configuration from models.toml.
    /// Returns default config if file doesn't exist or can't be parsed.
    pub fn load() -> Self {
        if let Some(path) = Self::config_path() {
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(contents) => match toml::from_str::<ModelsConfig>(&contents) {
                        Ok(config) => return config,
                        Err(e) => {
                            eprintln!("Failed to parse models.toml: {e}");
                        }
                    },
                    Err(e) => {
                        eprintln!("Failed to read models.toml: {e}");
                    }
                }
            }
        }
        Self::default()
    }

    /// Save models configuration to models.toml.
    pub fn save(&self) -> Result<(), ModelConfigError> {
        let path = Self::config_path().ok_or_else(|| {
            ModelConfigError::Write("Could not determine config path".to_string())
        })?;

        // Ensure config directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ModelConfigError::Write(format!("Failed to create config directory: {e}"))
            })?;
        }

        let toml_str = toml::to_string_pretty(self)
            .map_err(|e| ModelConfigError::Write(format!("Failed to serialize config: {e}")))?;

        std::fs::write(&path, toml_str)
            .map_err(|e| ModelConfigError::Write(format!("Failed to write file: {e}")))?;

        Ok(())
    }

    /// Add a model to the curated list (avoiding duplicates)
    pub fn add_curated_model(&mut self, model: CuratedModel) {
        // Check if model already exists (by provider + model_id)
        let exists = self
            .models
            .curated
            .iter()
            .any(|m| m.provider == model.provider && m.model_id == model.model_id);

        if !exists {
            self.models.curated.push(model);
        }
    }

    /// Remove a model from the curated list
    pub fn remove_curated_model(&mut self, provider: &str, model_id: &str) {
        self.models
            .curated
            .retain(|m| !(m.provider == provider && m.model_id == model_id));
    }

    /// Get all curated models
    pub fn get_curated_models(&self) -> &[CuratedModel] {
        &self.models.curated
    }

    /// Get provider configuration by name
    #[allow(dead_code)]
    pub fn get_provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.name == name)
    }

    /// Get all provider configurations
    pub fn get_providers(&self) -> &[ProviderConfig] {
        &self.providers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn given_empty_config_when_add_model_then_model_added() {
        // Given
        let mut config = ModelsConfig::default();
        let model = CuratedModel::new("GPT-4", "openai", "gpt-4");

        // When
        config.add_curated_model(model.clone());

        // Then
        assert_eq!(config.models.curated.len(), 1);
        assert_eq!(config.models.curated[0], model);
    }

    #[test]
    fn given_existing_model_when_add_duplicate_then_not_added() {
        // Given
        let mut config = ModelsConfig::default();
        let model = CuratedModel::new("GPT-4", "openai", "gpt-4");
        config.add_curated_model(model.clone());

        // When
        config.add_curated_model(model.clone());

        // Then
        assert_eq!(config.models.curated.len(), 1);
    }

    #[test]
    fn given_model_when_remove_then_model_removed() {
        // Given
        let mut config = ModelsConfig::default();
        let model = CuratedModel::new("GPT-4", "openai", "gpt-4");
        config.add_curated_model(model);

        // When
        config.remove_curated_model("openai", "gpt-4");

        // Then
        assert_eq!(config.models.curated.len(), 0);
    }
}
