use std::path::PathBuf;
use std::time::Duration;

use crate::error::api::ApiError;
use crate::types::agent::AgentInfo;
use crate::types::models::{MessagePart, MessageRequest, ModelIdentifier};
use reqwest::Url;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub directory: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub time: Option<SessionTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTime {
    pub created: i64,
    pub updated: i64,
}

#[derive(Clone)]
pub struct OpencodeClient {
    base: Url,
    http: reqwest::Client,
    pub directory: Option<PathBuf>,
    pub oauth_token: Option<String>,
}

impl OpencodeClient {
    pub fn new(base_url: &str) -> Result<Self, ApiError> {
        let base = Url::parse(base_url).map_err(|e| ApiError::Url(e.to_string()))?;
        Ok(Self {
            base,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| ApiError::Http(e.to_string()))?,
            directory: None,
            oauth_token: None,
        })
    }
    
    pub fn set_oauth_token(&mut self, token: String) {
        self.oauth_token = Some(token);
    }

    fn prepare_request(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(dir) = &self.directory {
            if let Some(d) = dir.to_str() {
                req = req.header("x-opencode-directory", d);
            }
        }
        
        if let Some(token) = &self.oauth_token {
             req = req.header("Authorization", format!("Bearer {}", token));
        }
        
        req
    }

    pub async fn doc(&self) -> Result<String, ApiError> {
        let url = self
            .base
            .join("doc")
            .map_err(|e| ApiError::Url(e.to_string()))?;
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?;
        let text = resp
            .text()
            .await
            .map_err(|e| ApiError::Decode(e.to_string()))?;
        Ok(text)
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>, ApiError> {
        let url = self
            .base
            .join("session")
            .map_err(|e| ApiError::Url(e.to_string()))?;
        let resp = self
            .prepare_request(self.http.get(url))
            .send()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?;
        let data = resp
            .json::<Vec<SessionInfo>>()
            .await
            .map_err(|e| ApiError::Decode(e.to_string()))?;
        Ok(data)
    }

    pub async fn list_agents(&self) -> Result<Vec<AgentInfo>, ApiError> {
        let url = self
            .base
            .join("agent")
            .map_err(|e| ApiError::Url(e.to_string()))?;
        let resp = self
            .prepare_request(self.http.get(url))
            .send()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?;
        let data = resp
            .json::<Vec<AgentInfo>>()
            .await
            .map_err(|e| ApiError::Decode(e.to_string()))?;
        Ok(data)
    }

    pub async fn create_session(&self, title: Option<&str>) -> Result<SessionInfo, ApiError> {
        let url = self
            .base
            .join("session")
            .map_err(|e| ApiError::Url(e.to_string()))?;
        let body = match title {
            Some(t) => serde_json::json!({"title": t}),
            None => serde_json::json!({}),
        };
        let resp = self
            .prepare_request(self.http.post(url).json(&body))
            .send()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?;
        let data = resp
            .json::<SessionInfo>()
            .await
            .map_err(|e| ApiError::Decode(e.to_string()))?;
        Ok(data)
    }

    pub async fn delete_session(&self, id: &str) -> Result<bool, ApiError> {
        let url = self
            .base
            .join(&format!("session/{id}"))
            .map_err(|e| ApiError::Url(e.to_string()))?;
        let resp = self
            .prepare_request(self.http.delete(url))
            .send()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?;
        Ok(resp.status().is_success())
    }

    pub async fn send_message(
        &self,
        session_id: &str,
        parts: Vec<MessagePart>,
        model: Option<(String, String)>,
        agent: Option<String>,
    ) -> Result<(), ApiError> {
        let url = self
            .base
            .join(&format!("session/{session_id}/message"))
            .map_err(|e| ApiError::Url(e.to_string()))?;

        let body = MessageRequest {
            parts,
            model: model.map(|(provider_id, model_id)| ModelIdentifier::new(provider_id, model_id)),
            agent: agent.clone(),
        };

        let resp = self
            .prepare_request(self.http.post(url).json(&body))
            .send()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ApiError::Http(format!("Status {}", resp.status())));
        }
        Ok(())
    }

    pub async fn respond_permission(
        &self,
        session_id: &str,
        permission_id: &str,
        response: &str,
    ) -> Result<(), ApiError> {
        let url = self
            .base
            .join(&format!("session/{session_id}/permissions/{permission_id}"))
            .map_err(|e| ApiError::Url(e.to_string()))?;
        let body = serde_json::json!({ "response": response });
        let resp = self
            .prepare_request(self.http.post(url).json(&body))
            .send()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ApiError::Http(format!("Status {}", resp.status())));
        }
        Ok(())
    }

    pub async fn abort_session(&self, session_id: &str) -> Result<bool, ApiError> {
        let url = self
            .base
            .join(&format!("session/{session_id}/abort"))
            .map_err(|e| ApiError::Url(e.to_string()))?;
        let resp = self
            .prepare_request(self.http.post(url))
            .send()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ApiError::Http(format!("Status {}", resp.status())));
        }
        Ok(true)
    }

    pub async fn get_provider_status(&self) -> Result<ProviderStatus, ApiError> {
        let url = self
            .base
            .join("provider")
            .map_err(|e| ApiError::Url(e.to_string()))?;
        let resp = self
            .prepare_request(self.http.get(url))
            .send()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?;
        resp.json()
            .await
            .map_err(|e| ApiError::Decode(e.to_string()))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderStatus {
    pub connected: Vec<String>,
}
