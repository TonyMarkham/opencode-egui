use crate::config::models::{ProviderConfig, ResponseFormat};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("HTTP request failed: {0}")]
    Http(String),

    #[error("Failed to parse response: {0}")]
    Parse(String),

    #[error("Invalid auth configuration: {0}")]
    Auth(String),

    #[error("Model field not found: {0}")]
    FieldNotFound(String),
}

/// A model discovered from a provider's API
#[derive(Debug, Clone, Deserialize)]
pub struct DiscoveredModel {
    pub id: String,
    pub name: String,
}

/// Client for calling provider APIs directly to discover models
pub struct ProviderClient {
    client: reqwest::Client,
}

impl ProviderClient {
    pub fn new() -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Http(e.to_string()))?;

        Ok(Self { client: client })
    }

    /// Discover models from a provider's API using config from models.toml
    pub async fn discover_models(
        &self,
        provider_config: &ProviderConfig,
        api_key: &str,
    ) -> Result<Vec<DiscoveredModel>, ProviderError> {
        // Build HTTP request based on auth_type
        let mut request = match provider_config.auth_type.as_str() {
            "bearer" => self
                .client
                .get(&provider_config.models_url)
                .header("Authorization", format!("Bearer {api_key}")),
            "header" => {
                let header_name = provider_config.auth_header.as_ref().ok_or_else(|| {
                    ProviderError::Auth("auth_header required for header auth type".to_string())
                })?;

                self.client
                    .get(&provider_config.models_url)
                    .header(header_name, api_key)
            }
            "query_param" => {
                let param_name = provider_config.auth_param.as_ref().ok_or_else(|| {
                    ProviderError::Auth("auth_param required for query_param auth type".to_string())
                })?;

                let url = format!("{}?{param_name}={api_key}", provider_config.models_url);
                self.client.get(&url)
            }
            other => {
                return Err(ProviderError::Auth(format!("Unknown auth_type: {other}")));
            }
        };

        // Apply extra headers if configured
        for (header_name, header_value) in &provider_config.extra_headers {
            request = request.header(header_name, header_value);
        }

        // Make request
        let response = request
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;

        if !response.status().is_success() {
            return Err(ProviderError::Http(format!("HTTP {}", response.status())));
        }

        let json: Value = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        // Parse using response_format config
        parse_provider_response(json, &provider_config.response_format)
    }
}

impl Default for ProviderClient {
    fn default() -> Self {
        Self::new().expect("Failed to create ProviderClient")
    }
}

/// Parse provider API response using response_format configuration
fn parse_provider_response(
    json: Value,
    format: &ResponseFormat,
) -> Result<Vec<DiscoveredModel>, ProviderError> {
    // Extract models array using models_path
    let models_array = json
        .get(&format.models_path)
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            ProviderError::Parse(format!(
                "Models array not found at path: {}",
                format.models_path
            ))
        })?;

    let mut discovered_models = Vec::new();

    for model in models_array {
        // Extract model ID
        let mut model_id = model
            .get(&format.model_id_field)
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::FieldNotFound(format.model_id_field.clone()))?
            .to_string();

        // Strip prefix if configured
        if let Some(prefix) = &format.model_id_strip_prefix {
            if let Some(stripped) = model_id.strip_prefix(prefix) {
                model_id = stripped.to_string();
            }
        }

        // Extract model name
        let model_name = model
            .get(&format.model_name_field)
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::FieldNotFound(format.model_name_field.clone()))?
            .to_string();

        discovered_models.push(DiscoveredModel {
            id: model_id,
            name: model_name,
        });
    }

    Ok(discovered_models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn given_openai_response_when_parse_then_extracts_models() {
        // Given
        let json = json!({
            "data": [
                { "id": "gpt-4", "created": 1234 },
                { "id": "gpt-3.5-turbo", "created": 5678 }
            ]
        });

        let format = ResponseFormat {
            models_path: "data".to_string(),
            model_id_field: "id".to_string(),
            model_id_strip_prefix: None,
            model_name_field: "id".to_string(),
        };

        // When
        let result = parse_provider_response(json, &format);

        // Then
        assert!(result.is_ok());
        let models = result.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-4");
        assert_eq!(models[1].id, "gpt-3.5-turbo");
    }

    #[test]
    fn given_google_response_when_parse_with_prefix_strip_then_removes_prefix() {
        // Given
        let json = json!({
            "models": [
                { "name": "models/gemini-pro", "displayName": "Gemini Pro" },
                { "name": "models/gemini-flash", "displayName": "Gemini Flash" }
            ]
        });

        let format = ResponseFormat {
            models_path: "models".to_string(),
            model_id_field: "name".to_string(),
            model_id_strip_prefix: Some("models/".to_string()),
            model_name_field: "displayName".to_string(),
        };

        // When
        let result = parse_provider_response(json, &format);

        // Then
        assert!(result.is_ok());
        let models = result.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gemini-pro");
        assert_eq!(models[0].name, "Gemini Pro");
        assert_eq!(models[1].id, "gemini-flash");
        assert_eq!(models[1].name, "Gemini Flash");
    }
}
