use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Response from GET /config/providers endpoint
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProvidersResponse {
    pub providers: Vec<ProviderInfo>,
    #[serde(default)]
    pub default: HashMap<String, String>,
}

/// Provider information with available models
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub source: String,
    pub models: HashMap<String, ModelInfo>,
}

/// Detailed model information
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(rename = "providerID", default)]
    pub provider_id: String,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    #[serde(default)]
    pub cost: Option<ModelCost>,
    #[serde(default)]
    pub limit: ModelLimits,
}

/// Model capabilities (what the model can do)
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ModelCapabilities {
    #[serde(default)]
    pub temperature: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub attachment: bool,
    #[serde(default)]
    pub toolcall: bool,
    #[serde(default)]
    pub input: IOCapabilities,
    #[serde(default)]
    pub output: IOCapabilities,
}

/// Input/output capabilities (text, audio, image, video, pdf)
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct IOCapabilities {
    #[serde(default)]
    pub text: bool,
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub image: bool,
    #[serde(default)]
    pub video: bool,
    #[serde(default)]
    pub pdf: bool,
}

/// Model pricing information (per token)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelCost {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache: Option<CacheCost>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CacheCost {
    pub read: f64,
    pub write: f64,
}

/// Model context and output limits
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ModelLimits {
    #[serde(default)]
    pub context: u32,
    #[serde(default)]
    pub output: u32,
}

/// Model selection for a conversation tab
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelSelection {
    pub provider: String,
    pub model_id: String,
    pub name: String,
}

impl ModelSelection {
    pub fn new(
        provider: impl Into<String>,
        model_id: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            model_id: model_id.into(),
            name: name.into(),
        }
    }

    /// Format as "provider/model_id" for display
    pub fn display_id(&self) -> String {
        format!("{}/{}", self.provider, self.model_id)
    }
}

/// Request body for POST /session/:id/message with model selection
#[derive(Debug, Clone, Serialize)]
pub struct MessageRequest {
    pub parts: Vec<MessagePart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelIdentifier>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum MessagePart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "file")]
    File {
        mime: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        url: String, // Data URI or external URL
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelIdentifier {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(rename = "modelID")]
    pub model_id: String,
}

impl ModelIdentifier {
    pub fn new(provider_id: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
        }
    }
}
