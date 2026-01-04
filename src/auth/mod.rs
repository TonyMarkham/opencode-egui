use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use dotenvy;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum AuthInfo {
    #[serde(rename = "oauth")]
    OAuth {
        access: String,
        refresh: String,
        expires: u64,
    },
    #[serde(rename = "api")]
    ApiKey { key: String },
}

#[derive(Debug, Clone)]
pub struct AnthropicAuth {
    pub oauth: Option<OAuthTokens>,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OAuthTokens {
    pub access: String,
    pub refresh: String,
    pub expires: u64,
}

impl AnthropicAuth {
    /// Read Anthropic auth from server's auth.json
    pub fn read_from_server() -> Result<Option<AuthInfo>, Box<dyn std::error::Error>> {
        let home = directories::BaseDirs::new()
            .ok_or("Could not determine home directory")?
            .home_dir()
            .to_path_buf();
        let auth_path = home.join(".local/share/opencode/auth.json");

        if !auth_path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&auth_path)?;
        let auth_data: serde_json::Value = serde_json::from_str(&content)?;

        if let Some(anthropic) = auth_data.get("anthropic") {
            let auth_info: AuthInfo = serde_json::from_value(anthropic.clone())?;
            Ok(Some(auth_info))
        } else {
            Ok(None)
        }
    }

    /// Write OAuth tokens to egui's .env file
    pub fn cache_oauth_to_env(
        oauth: &OAuthTokens,
        env_path: &PathBuf,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Read existing .env content
        let existing_content = if env_path.exists() {
            fs::read_to_string(env_path)?
        } else {
            String::new()
        };

        let mut lines: Vec<String> = existing_content.lines().map(|s| s.to_string()).collect();

        // Remove old OAuth entries
        lines.retain(|line| {
            !line.starts_with("ANTHROPIC_OAUTH_ACCESS=")
                && !line.starts_with("ANTHROPIC_OAUTH_REFRESH=")
                && !line.starts_with("ANTHROPIC_OAUTH_EXPIRES=")
        });

        // Add new OAuth entries
        lines.push(format!("ANTHROPIC_OAUTH_ACCESS={}", oauth.access));
        lines.push(format!("ANTHROPIC_OAUTH_REFRESH={}", oauth.refresh));
        lines.push(format!("ANTHROPIC_OAUTH_EXPIRES={}", oauth.expires));

        // Write back
        fs::write(env_path, lines.join("\n"))?;
        Ok(())
    }

    /// Read OAuth tokens from egui's .env file
    pub fn read_oauth_from_env(env_path: &PathBuf) -> Result<Option<OAuthTokens>, Box<dyn std::error::Error>> {
        if !env_path.exists() {
            return Ok(None);
        }

        // Load the specific .env file into environment variables
        dotenvy::from_path(env_path)?;
        
        let access = std::env::var("ANTHROPIC_OAUTH_ACCESS").ok();
        let refresh = std::env::var("ANTHROPIC_OAUTH_REFRESH").ok();
        let expires = std::env::var("ANTHROPIC_OAUTH_EXPIRES")
            .ok()
            .and_then(|s| s.parse::<u64>().ok());

        match (access, refresh, expires) {
            (Some(access), Some(refresh), Some(expires)) => Ok(Some(OAuthTokens {
                access,
                refresh,
                expires,
            })),
            _ => Ok(None),
        }
    }

    /// Check if OAuth token is expired
    pub fn is_oauth_expired(expires: u64) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        expires <= now
    }

    /// Format time remaining until expiration
    pub fn format_time_remaining(expires: u64) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        if expires <= now {
            return "⚠️ Expired".to_string();
        }

        let remaining_ms = expires - now;
        let remaining_secs = remaining_ms / 1000;
        let minutes = remaining_secs / 60;
        let seconds = remaining_secs % 60;

        if minutes > 60 {
            let hours = minutes / 60;
            let mins = minutes % 60;
            format!("{}h {}m", hours, mins)
        } else {
            format!("{}m {}s", minutes, seconds)
        }
    }
}
