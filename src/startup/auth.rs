use std::collections::HashMap;
use std::env;
use thiserror::Error;


#[derive(Debug, Clone)]
pub struct AuthSyncState {
    pub status: AuthSyncStatus,
    pub synced_providers: Vec<String>,
    pub failed_providers: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthSyncStatus {
    NotStarted,
    InProgress,
    Complete,
    Failed(String),
}

#[derive(Debug, Error)]
pub enum AuthSyncError {
    #[error("Failed to load .env file: {0}")]
    EnvLoad(String),

    #[error("HTTP request failed for provider '{provider}': {message}")]
    Http { provider: String, message: String },

    #[error("Failed to parse response from server: {0}")]
    Parse(String),
}

impl Default for AuthSyncState {
    fn default() -> Self {
        Self {
            status: AuthSyncStatus::NotStarted,
            synced_providers: Vec::new(),
            failed_providers: Vec::new(),
        }
    }
}

/// Extract provider name from environment variable name.
/// Example: "OPENAI_API_KEY" -> "openai"
fn extract_provider_name(env_var: &str) -> Option<String> {
    if env_var.ends_with("_API_KEY") {
        let provider = env_var.strip_suffix("_API_KEY")?;
        Some(provider.to_lowercase())
    } else {
        None
    }
}

/// Sync API keys from .env file to the OpenCode server.
///
/// This function:
/// 1. Loads the .env file from the executable directory
/// 2. Extracts all *_API_KEY environment variables
/// 3. Sends each key to the server via PUT /auth/{provider}
/// 4. Returns the sync state with success/failure information
pub async fn sync_api_keys_to_server(server_url: &str) -> AuthSyncState {
    let mut state = AuthSyncState {
        status: AuthSyncStatus::InProgress,
        synced_providers: Vec::new(),
        failed_providers: Vec::new(),
    };

    // Load .env file from the executable directory (or current directory in dev)
    if let Err(e) = dotenvy::dotenv() {
        // .env file not found is not a fatal error - user might not have set up keys yet
        state.status = AuthSyncStatus::Failed(format!("No .env file found: {e}"));
        return state;
    }

    // Collect all API keys from environment variables
    let mut api_keys: HashMap<String, String> = HashMap::new();
    for (key, value) in env::vars() {
        if let Some(provider) = extract_provider_name(&key) {
            // Only include non-empty keys (not just the placeholder from .env.example)
            if !value.is_empty() && !value.contains("...") {
                api_keys.insert(provider, value);
            }
        }
    }

    if api_keys.is_empty() {
        state.status = AuthSyncStatus::Failed("No API keys found in .env file".to_string());
        return state;
    }

    // Create HTTP client
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            state.status = AuthSyncStatus::Failed(format!("Failed to create HTTP client: {e}"));
            return state;
        }
    };

    // Check if Anthropic already has OAuth tokens in server's auth.json
    let skip_anthropic_oauth = if let Ok(Some(crate::auth::AuthInfo::OAuth { .. })) = 
        crate::auth::AnthropicAuth::read_from_server() {
        true
    } else {
        false
    };

    // Sync each API key to the server
    for (provider, key) in api_keys {
        // Skip Anthropic if it already has OAuth configured
        if provider == "anthropic" && skip_anthropic_oauth {
            eprintln!("ℹ️  Skipping Anthropic API key sync - OAuth tokens detected");
            continue;
        }
        
        let url = format!("{server_url}/auth/{provider}");
        let body = serde_json::json!({
            "type": "api",
            "key": key,
        });

        match client.put(&url).json(&body).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    state.synced_providers.push(provider.clone());
                } else {
                    let error = format!("HTTP {}", resp.status());
                    state.failed_providers.push((provider.clone(), error));
                }
            }
            Err(e) => {
                state
                    .failed_providers
                    .push((provider.clone(), e.to_string()));
            }
        }
    }

    // Set final status
    if state.failed_providers.is_empty() {
        state.status = AuthSyncStatus::Complete;
    } else if state.synced_providers.is_empty() {
        state.status = AuthSyncStatus::Failed("All providers failed to sync".to_string());
    } else {
        state.status = AuthSyncStatus::Complete;
    }

    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn given_openai_env_var_when_extract_provider_then_returns_openai() {
        // Given
        let env_var = "OPENAI_API_KEY";

        // When
        let result = extract_provider_name(env_var);

        // Then
        assert_eq!(result, Some("openai".to_string()));
    }

    #[test]
    fn given_anthropic_env_var_when_extract_provider_then_returns_anthropic() {
        // Given
        let env_var = "ANTHROPIC_API_KEY";

        // When
        let result = extract_provider_name(env_var);

        // Then
        assert_eq!(result, Some("anthropic".to_string()));
    }

    #[test]
    fn given_non_api_key_env_var_when_extract_provider_then_returns_none() {
        // Given
        let env_var = "PATH";

        // When
        let result = extract_provider_name(env_var);

        // Then
        assert_eq!(result, None);
    }
}

