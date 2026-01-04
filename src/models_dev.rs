use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Models.dev API response structure
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelsDevProvider {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub npm: Option<String>,
    #[serde(default)]
    pub api: Option<String>,
    pub models: HashMap<String, ModelsDevModel>,
}

/// Individual model from models.dev
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelsDevModel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub attachment: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub tool_call: bool,
    #[serde(default)]
    pub temperature: bool,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub cost: Option<ModelCost>,
    #[serde(default)]
    pub limit: Option<ModelLimit>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelCost {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelLimit {
    pub context: u64,
    pub output: u64,
}

/// Fetches models from models.dev and returns the full provider map
pub async fn fetch_models_dev() -> Result<HashMap<String, ModelsDevProvider>, String> {
    let url = "https://models.dev/api.json";
    
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("Failed to fetch models.dev: {}", e))?;
    
    if !response.status().is_success() {
        return Err(format!("models.dev returned status: {}", response.status()));
    }
    
    let data: HashMap<String, ModelsDevProvider> = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse models.dev JSON: {}", e))?;
    
    Ok(data)
}

/// Finds the latest Haiku model from Anthropic provider
/// Returns (provider_id, model_id) tuple
pub fn find_latest_haiku(providers: &HashMap<String, ModelsDevProvider>) -> Option<(String, String)> {
    let anthropic = providers.get("anthropic")?;
    
    // Filter for Haiku models
    let mut haiku_models: Vec<(&String, &ModelsDevModel)> = anthropic
        .models
        .iter()
        .filter(|(id, model)| {
            // Match models with "haiku" in the ID or name
            let id_lower = id.to_lowercase();
            let name_lower = model.name.to_lowercase();
            id_lower.contains("haiku") || name_lower.contains("haiku")
        })
        .collect();
    
    if haiku_models.is_empty() {
        return None;
    }
    
    // Sort by release date (newest first), then by model ID
    haiku_models.sort_by(|a, b| {
        // First try to sort by release date
        match (&a.1.release_date, &b.1.release_date) {
            (Some(date_a), Some(date_b)) => {
                // Reverse order for newest first
                date_b.cmp(date_a)
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => {
                // Fall back to ID comparison (prefer "latest" suffix)
                if a.0.contains("latest") && !b.0.contains("latest") {
                    std::cmp::Ordering::Less
                } else if !a.0.contains("latest") && b.0.contains("latest") {
                    std::cmp::Ordering::Greater
                } else {
                    // Reverse for newer versions (higher numbers)
                    b.0.cmp(a.0)
                }
            }
        }
    });
    
    // Return the first (newest) Haiku model
    haiku_models.first().map(|(id, _)| {
        ("anthropic".to_string(), (*id).clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_latest_haiku_prefers_latest_suffix() {
        let mut providers = HashMap::new();
        let mut models = HashMap::new();
        
        models.insert(
            "claude-haiku-4-5".to_string(),
            ModelsDevModel {
                id: "claude-haiku-4-5".to_string(),
                name: "Claude Haiku 4.5".to_string(),
                family: Some("haiku".to_string()),
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: Some("2025-10-01".to_string()),
                cost: None,
                limit: None,
            },
        );
        
        models.insert(
            "claude-3-5-haiku-20241022".to_string(),
            ModelsDevModel {
                id: "claude-3-5-haiku-20241022".to_string(),
                name: "Claude Haiku 3.5".to_string(),
                family: Some("haiku".to_string()),
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: Some("2024-10-22".to_string()),
                cost: None,
                limit: None,
            },
        );
        
        providers.insert(
            "anthropic".to_string(),
            ModelsDevProvider {
                id: "anthropic".to_string(),
                name: "Anthropic".to_string(),
                env: vec!["ANTHROPIC_API_KEY".to_string()],
                npm: None,
                api: None,
                models,
            },
        );
        
        let result = find_latest_haiku(&providers);
        assert!(result.is_some());
        let (provider, model_id) = result.unwrap();
        assert_eq!(provider, "anthropic");
        // Should pick the newer release date
        assert_eq!(model_id, "claude-haiku-4-5");
    }
}
