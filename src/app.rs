use arboard;
use base64;
use base64::Engine;
use eframe::egui;
use image::ExtendedColorType;
use image::ImageEncoder;
use image::codecs::png::PngEncoder;
use serde::Deserialize;
use std::sync::{Arc, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;

use crate::discovery::process::{ServerInfo, check_health, discover, stop_pid};
use crate::discovery::spawn::spawn_and_wait;
use crate::startup::auth::{AuthSyncState, sync_api_keys_to_server};
use crate::types::agent::AgentInfo;

fn dbg_log(msg: impl AsRef<str>) {
    eprintln!("[egui-debug] {}", msg.as_ref());
}

pub struct OpenCodeApp {
    // Multi-session tabs (server-backed sessions in later milestones)
    tabs: Vec<Tab>,
    active: usize,

    // Server state
    server: Option<ServerInfo>,
    server_error: Option<String>,
    server_in_flight: bool,
    discovery_started: bool,

    // Async runtime + UI channel
    runtime: Option<Arc<Runtime>>,
    ui_rx: Option<mpsc::Receiver<UiMsg>>,
    ui_tx: Option<mpsc::Sender<UiMsg>>,

    // API client
    client: Option<crate::client::api::OpencodeClient>,
    oauth_token: Option<String>,

    // Auth sync state
    auth_sync_state: AuthSyncState,
    connected_providers: Vec<String>,
    
    // OAuth toggle state
    anthropic_subscription_mode: bool,
    anthropic_oauth_expires: Option<u64>,

    // Audio task
    audio_tx: Option<mpsc::Sender<AudioCmd>>,
    audio_enabled: bool,
    recording_state: RecordingState,

    // Rename state
    renaming_tab: Option<usize>,
    rename_buffer: String,
    rename_text_selected: bool,

    // Config and settings
    config: crate::config::AppConfig,
    models_config: crate::config::models::ModelsConfig,
    show_settings: bool,
    base_url_input: String,
    directory_input: String,

    // models.dev data
    models_dev_data: Option<std::collections::HashMap<String, crate::models_dev::ModelsDevProvider>>,
    oauth_default_model: Option<(String, String)>,

    // Model discovery UI state
    show_model_discovery: bool,
    discovery_provider: Option<String>,
    discovery_models: Vec<crate::client::providers::DiscoveredModel>,
    discovery_error: Option<String>,
    discovery_in_progress: bool,
    discovery_search: String,

    // Permission handling
    pending_permissions: Vec<PermissionInfo>,

    // Agents
    agents: Vec<AgentInfo>,
    show_subagents: bool,
    agents_pane_collapsed: bool,
    default_agent: String,

    // Markdown rendering
    commonmark_cache: egui_commonmark::CommonMarkCache,
}

#[derive(Default, Clone)]
pub(crate) struct Tab {
    title: String,
    session_id: Option<String>,
    session_version: Option<String>,
    directory: Option<String>,
    messages: Vec<DisplayMessage>,
    active_assistant: Option<String>,
    input: String,
    selected_model: Option<(String, String)>, // (provider, model_id)
    pub(crate) selected_agent: Option<String>,
    cancelled_messages: Vec<String>,
    cancelled_calls: Vec<String>,
    cancelled_after: Option<i64>,
    suppress_incoming: bool,
    last_send_at: i64,
    pending_attachments: Vec<PendingAttachment>,
}

#[derive(Clone)]
struct PendingAttachment {
    data: Vec<u8>,
    mime: String,
}

#[derive(Clone)]
struct DisplayMessage {
    message_id: String,
    role: String,
    text_parts: Vec<String>,
    reasoning_parts: Vec<String>,
    tokens_input: Option<u64>,
    tokens_output: Option<u64>,
    tokens_reasoning: Option<u64>,
    tool_calls: Vec<ToolCall>,
}

#[derive(Clone)]
struct ToolCall {
    id: String,
    name: String,
    status: String,
    call_id: Option<String>,
    input: serde_json::Value,
    output: Option<String>,
    error: Option<String>,
    metadata: serde_json::Map<String, serde_json::Value>,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    logs: Vec<String>,
}

enum UiMsg {
    ServerConnected(ServerInfo),
    ServerError(String),
    AttachmentAdded(Vec<u8>, String),
    SessionCreated {
        tab_idx: usize,
        id: String,
        title: String,
        directory: String,
        version: Option<String>,
    },
    GlobalEvent(serde_json::Value),
    #[allow(dead_code)]
    PermissionRequest(PermissionInfo),
    // Auth sync events
    AuthSyncComplete(AuthSyncState),
    // Model discovery events
    ModelsDiscovered(Vec<crate::client::providers::DiscoveredModel>),
    ModelDiscoveryError(String),
    // Provider status
    ProviderStatus(Vec<String>),
    // Agent events
    AgentsLoaded(Vec<AgentInfo>),
    AgentsFailed(String),
    // Audio events
    RecordingStarted,
    RecordingStopped,
    Transcription(String),
    AudioError(String),
    // models.dev events
    ModelsDevFetched(std::collections::HashMap<String, crate::models_dev::ModelsDevProvider>),
}

#[derive(Clone, Debug, Deserialize)]
struct PermissionInfo {
    id: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    perm_type: String,
    #[allow(dead_code)]
    pattern: Option<Vec<String>>,
    #[serde(rename = "sessionID")]
    session_id: String,
    #[serde(rename = "messageID")]
    message_id: String,
    #[serde(rename = "callID")]
    call_id: Option<String>,
    #[allow(dead_code)]
    title: String,
    #[allow(dead_code)]
    metadata: serde_json::Value,
    time: PermissionTime,
}

#[derive(Clone, Debug, Deserialize)]
struct PermissionTime {
    created: u64,
}

enum AudioCmd {
    StartRecording,
    StopRecording,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordingState {
    Idle,
    Recording,
}

impl OpenCodeApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Install image loaders for colored emoji support via egui-twemoji
        egui_extras::install_image_loaders(&cc.egui_ctx);

        // Load config and apply UI preferences
        let config = crate::config::AppConfig::load();
        let models_config = crate::config::models::ModelsConfig::load();
        config.ui.apply_to_context(&cc.egui_ctx);

        // Initialize OAuth toggle state by reading server's auth.json
        let (oauth_token, anthropic_subscription_mode, anthropic_oauth_expires) = {
            match crate::auth::AnthropicAuth::read_from_server() {
                Ok(Some(crate::auth::AuthInfo::OAuth { access, refresh, expires })) => {
                    // Cache OAuth tokens to .env next to executable
                    let env_path = std::env::current_exe()
                        .ok()
                        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                        .unwrap_or_else(|| std::env::current_dir().unwrap())
                        .join(".env");
                    
                    let oauth_tokens = crate::auth::OAuthTokens {
                        access: access.clone(),
                        refresh,
                        expires,
                    };
                    
                    let _ = crate::auth::AnthropicAuth::cache_oauth_to_env(&oauth_tokens, &env_path);
                    
                    (Some(access), true, Some(expires))
                }
                Ok(Some(crate::auth::AuthInfo::ApiKey { .. })) => {
                    (None, false, None)
                }
                _ => {
                    (None, false, None)
                }
            }
        };
        
        Self {
            tabs: Vec::new(),
            active: 0,
            server: None,
            server_error: None,
            server_in_flight: false,
            discovery_started: false,
            runtime: None,
            ui_rx: None,
            ui_tx: None,
            client: None,
            oauth_token,
            auth_sync_state: AuthSyncState::default(),
            connected_providers: Vec::new(),
            anthropic_subscription_mode,
            anthropic_oauth_expires,
            audio_tx: None,
            audio_enabled: false,
            recording_state: RecordingState::Idle,
            renaming_tab: None,
            rename_buffer: String::new(),
            rename_text_selected: false,
            config: config.clone(),
            models_config: models_config,
            show_settings: false,
            base_url_input: config.server.last_base_url.unwrap_or_default(),
            directory_input: config.server.directory_override.clone().unwrap_or_default(),
            show_model_discovery: false,
            discovery_provider: None,
            discovery_models: Vec::new(),
            discovery_error: None,
            discovery_in_progress: false,
            discovery_search: String::new(),
            pending_permissions: Vec::new(),
            agents: Vec::new(),
            show_subagents: false,
            agents_pane_collapsed: false,
            default_agent: "build".to_string(),
            commonmark_cache: egui_commonmark::CommonMarkCache::default(),
            models_dev_data: None,
            oauth_default_model: None,
        }
    }

    fn start_server_discovery(&mut self, ctx: &egui::Context) {
        // Lazy-init runtime on first call
        if self.runtime.is_none() {
            let rt = Arc::new(Runtime::new().expect("tokio runtime"));
            let (tx, rx) = mpsc::channel();
            self.runtime = Some(rt.clone());
            self.ui_rx = Some(rx);
            self.ui_tx = Some(tx.clone());

            // Start audio task if model is configured or auto-detected
            let model_path = if let Some(configured_path) = &self.config.audio.whisper_model_path {
                Some(std::path::PathBuf::from(configured_path))
            } else {
                // Auto-detect model relative to executable (for cargo make dev)
                std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
                    .map(|exe_dir| exe_dir.join("models").join("ggml-base.en.bin"))
                    .filter(|path| path.exists())
            };

            if let Some(path) = model_path {
                eprintln!("Starting audio task with model: {}", path.display());
                self.start_audio_task(&rt, tx.clone(), path, ctx);
            } else {
                eprintln!("No Whisper model found. Run 'cargo make dev' to auto-setup.");
            }

            // Fetch models.dev data for dynamic model selection
            let tx_models = tx.clone();
            let egui_ctx_models = ctx.clone();
            rt.spawn(async move {
                match crate::models_dev::fetch_models_dev().await {
                    Ok(data) => {
                        let _ = tx_models.send(UiMsg::ModelsDevFetched(data));
                        egui_ctx_models.request_repaint();
                    }
                    Err(e) => {
                        eprintln!("Failed to fetch models.dev: {}", e);
                    }
                }
            });
        }

        self.server_in_flight = true;
        self.discovery_started = true;

        let tx = self.ui_tx.as_ref().unwrap().clone();
        let rt = self.runtime.as_ref().unwrap().clone();
        let egui_ctx = ctx.clone();

        rt.spawn(async move {
            let msg = try_discover_or_spawn().await;
            let _ = tx.send(msg);
            egui_ctx.request_repaint();
        });
    }

    fn start_audio_task(
        &mut self,
        runtime: &Arc<Runtime>,
        ui_tx: mpsc::Sender<UiMsg>,
        model_path: std::path::PathBuf,
        ctx: &egui::Context,
    ) {
        let (audio_tx, audio_rx) = mpsc::channel::<AudioCmd>();
        self.audio_tx = Some(audio_tx);

        let egui_ctx = ctx.clone();
        runtime.spawn(async move {
            run_audio_task(audio_rx, ui_tx, model_path, egui_ctx).await;
        });
    }

    /// Drain the UI message channel fed by background tasks (e.g., SSE subscription).
    /// This is not network polling; it only drains already-received events.
    fn drain_ui_msgs(&mut self, ctx: &egui::Context) {
        let mut auto_rejects: Vec<(String, String)> = Vec::new();

        if let Some(rx) = &self.ui_rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    UiMsg::ServerConnected(info) => {
                        let base = info.base_url.clone();
                        match crate::client::api::OpencodeClient::new(&base) {
                            Ok(mut c) => {
                                if let Some(dir) = &self.config.server.directory_override {
                                    c.directory = Some(std::path::PathBuf::from(dir));
                                } else {
                                    // Auto-detect current working directory if no override configured
                                    if let Ok(cwd) = std::env::current_dir() {
                                        c.directory = Some(cwd);
                                    }
                                }
                                
                                if let Some(token) = &self.oauth_token {
                                    c.set_oauth_token(token.clone());
                                }
                                
                                self.client = Some(c)
                            }
                            Err(e) => self.server_error = Some(e.to_string()),
                        }

                        // Fetch provider status to check for OAuth subscriptions
                        if let (Some(client), Some(rt)) = (&self.client, &self.runtime) {
                            let client_clone = client.clone();
                            let tx = self.ui_tx.as_ref().unwrap().clone();
                            let egui_ctx = ctx.clone();
                            rt.spawn(async move {
                                if let Ok(status) = client_clone.get_provider_status().await {
                                    let _ = tx.send(UiMsg::ProviderStatus(status.connected));
                                    egui_ctx.request_repaint();
                                }
                            });
                        }

                        if let Some(rt) = &self.runtime {
                            let tx2 = self.ui_tx.as_ref().unwrap().clone();
                            let egui_ctx = ctx.clone();
                            let base_for_sse = base.clone();
                            rt.spawn(async move {
                                if let Ok(mut rx) =
                                    crate::client::events::subscribe_global(&base_for_sse).await
                                {
                                    while let Some(ev) = rx.recv().await {
                                        let _ = tx2.send(UiMsg::GlobalEvent(ev.payload.clone()));
                                        egui_ctx.request_repaint();
                                    }
                                }
                            });
                        }

                        self.config.server.last_base_url = Some(base.clone());
                        self.base_url_input = base;
                        self.config.save();

                        self.server = Some(info.clone());
                        self.server_error = None;
                        self.server_in_flight = false;

                        if let (Some(rt), Some(tx_agents), Some(client)) =
                            (&self.runtime, &self.ui_tx, &self.client)
                        {
                            let c = client.clone();
                            let tx = tx_agents.clone();
                            let egui_ctx_agents = ctx.clone();
                            rt.spawn(async move {
                                match c.list_agents().await {
                                    Ok(list) => {
                                        let _ = tx.send(UiMsg::AgentsLoaded(list));
                                    }
                                    Err(e) => {
                                        let _ = tx.send(UiMsg::AgentsFailed(e.to_string()));
                                    }
                                }
                                egui_ctx_agents.request_repaint();
                            });
                        }

                        if let Some(rt) = &self.runtime {
                            let tx3 = self.ui_tx.as_ref().unwrap().clone();
                            let egui_ctx2 = ctx.clone();
                            let server_url = info.base_url.clone();
                            rt.spawn(async move {
                                let state = sync_api_keys_to_server(&server_url).await;
                                let _ = tx3.send(UiMsg::AuthSyncComplete(state));
                                egui_ctx2.request_repaint();
                            });
                        }
                    }
                    UiMsg::ServerError(err) => {
                        self.server_error = Some(err);
                        self.server = None;
                        self.server_in_flight = false;
                    }
                    UiMsg::SessionCreated {
                        tab_idx,
                        id,
                        title,
                        directory,
                        version,
                    } => {
                        if let Some(tab) = self.tabs.get_mut(tab_idx) {
                            tab.title = title;
                            tab.session_id = Some(id);
                            tab.session_version = version;
                            tab.directory = Some(directory);
                        }
                    }
                    UiMsg::GlobalEvent(payload) => {
                        let event_type = payload.get("type").and_then(|v| v.as_str());
                        match event_type {
                            Some("permission.updated") => {
                                if let Some(props) = payload.get("properties") {
                                    if let Ok(info) =
                                        serde_json::from_value::<PermissionInfo>(props.clone())
                                    {
                                        let mut is_cancelled = false;
                                        if let Some(tab) = self.tabs.iter().find(|t| {
                                            t.session_id.as_deref()
                                                == Some(info.session_id.as_str())
                                        }) {
                                            if let Some(call_id) = info.call_id.as_deref() {
                                                if tab.cancelled_calls.iter().any(|c| c == call_id)
                                                {
                                                    is_cancelled = true;
                                                }
                                            }

                                            if !is_cancelled {
                                                if tab
                                                    .cancelled_messages
                                                    .iter()
                                                    .any(|m| m == &info.message_id)
                                                {
                                                    is_cancelled = true;
                                                }
                                            }

                                            if !is_cancelled {
                                                if let Some(cutoff) = tab.cancelled_after {
                                                    if info.time.created as i64 <= cutoff {
                                                        is_cancelled = true;
                                                    }
                                                }
                                            }

                                            if !is_cancelled {
                                                if info.time.created as i64 <= tab.last_send_at {
                                                    is_cancelled = true;
                                                }
                                            }

                                            if !is_cancelled {
                                                if tab.suppress_incoming {
                                                    is_cancelled = true;
                                                }
                                            }

                                            if !is_cancelled {
                                                if tab
                                                    .cancelled_messages
                                                    .iter()
                                                    .any(|m| m == &info.message_id)
                                                {
                                                    is_cancelled = true;
                                                }
                                            }
                                            if !is_cancelled {
                                                if let Some(call_id) = info.call_id.as_deref() {
                                                    if tab
                                                        .cancelled_calls
                                                        .iter()
                                                        .any(|c| c == call_id)
                                                    {
                                                        is_cancelled = true;
                                                    }
                                                }
                                            }

                                            if !is_cancelled {
                                                if tab.suppress_incoming {
                                                    is_cancelled = true;
                                                }
                                            }

                                            // if cancelled and skip_tools_for matched, text is already set in part handler
                                        }

                                        if is_cancelled {
                                            dbg_log(&format!(
                                                "perm auto-reject: sid={} mid={} call={:?} created={}",
                                                info.session_id,
                                                info.message_id,
                                                info.call_id,
                                                info.time.created
                                            ));
                                            auto_rejects
                                                .push((info.session_id.clone(), info.id.clone()));
                                        } else {
                                            dbg_log(&format!(
                                                "perm queued: sid={} mid={} call={:?} created={}",
                                                info.session_id,
                                                info.message_id,
                                                info.call_id,
                                                info.time.created
                                            ));
                                            self.pending_permissions.push(info);
                                        }
                                    }
                                }
                                continue;
                            }
                            Some("permission.replied") => {
                                if let Some(props) = payload.get("properties") {
                                    let pid = props.get("permissionID").and_then(|v| v.as_str());
                                    let sid = props.get("sessionID").and_then(|v| v.as_str());
                                    let _resp = props
                                        .get("response")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("?");
                                    if let (Some(pid), Some(sid)) = (pid, sid) {
                                        if let Some(idx) = self
                                            .pending_permissions
                                            .iter()
                                            .position(|p| p.id == pid && p.session_id == sid)
                                        {
                                            self.pending_permissions.remove(idx);
                                        }
                                    }
                                }
                                continue;
                            }
                            _ => {}
                        }

                        let sid_opt = payload
                            .get("properties")
                            .and_then(|p| p.get("part"))
                            .and_then(|part| part.get("sessionID"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| {
                                payload
                                    .get("properties")
                                    .and_then(|p| p.get("info"))
                                    .and_then(|info| info.get("sessionID"))
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                            });

                        if let Some(sid) = sid_opt {
                            if let Some(tab) = self
                                .tabs
                                .iter_mut()
                                .find(|t| t.session_id.as_deref() == Some(&sid))
                            {
                                Self::handle_event(tab, &payload, ctx);
                            }
                        }
                    }
                    UiMsg::RecordingStarted => {
                        self.audio_enabled = true;
                        if let Some(tab) = self.tabs.get_mut(self.active) {
                            tab.messages.push(DisplayMessage {
                                message_id: format!(
                                    "audio_rec_{}",
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap()
                                        .as_millis()
                                ),
                                role: "system".to_string(),
                                text_parts: vec!["🎙 Recording...".to_string()],
                                reasoning_parts: Vec::new(),
                                tokens_input: None,
                                tokens_output: None,
                                tokens_reasoning: None,
                                tool_calls: Vec::new(),
                            });
                        }
                    }
                    UiMsg::RecordingStopped => {
                        if let Some(tab) = self.tabs.get_mut(self.active) {
                            tab.messages.push(DisplayMessage {
                                message_id: format!(
                                    "audio_proc_{}",
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap()
                                        .as_millis()
                                ),
                                role: "system".to_string(),
                                text_parts: vec!["Processing audio...".to_string()],
                                reasoning_parts: Vec::new(),
                                tokens_input: None,
                                tokens_output: None,
                                tokens_reasoning: None,
                                tool_calls: Vec::new(),
                            });
                        }
                    }
                    UiMsg::Transcription(text) => {
                        self.audio_enabled = false;
                        if let Some(tab) = self.tabs.get_mut(self.active) {
                            if !tab.input.is_empty() {
                                tab.input.push(' ');
                            }
                            tab.input.push_str(&text);
                            tab.messages.push(DisplayMessage {
                                message_id: format!(
                                    "audio_done_{}",
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap()
                                        .as_millis()
                                ),
                                role: "system".to_string(),
                                text_parts: vec!["✅ Transcription complete".to_string()],
                                reasoning_parts: Vec::new(),
                                tokens_input: None,
                                tokens_output: None,
                                tokens_reasoning: None,
                                tool_calls: Vec::new(),
                            });
                        }
                    }
                    UiMsg::PermissionRequest(info) => {
                        self.pending_permissions.push(info);
                    }
                    UiMsg::AuthSyncComplete(state) => {
                        self.auth_sync_state = state;
                    }
                    UiMsg::ModelsDiscovered(models) => {
                        self.discovery_models = models;
                        self.discovery_in_progress = false;
                        self.discovery_error = None;
                    }
                    UiMsg::ModelDiscoveryError(error) => {
                        self.discovery_error = Some(error);
                        self.discovery_in_progress = false;
                    }
                    UiMsg::ProviderStatus(connected) => {
                        self.connected_providers = connected;
                    }
                    UiMsg::AgentsLoaded(list) => {
                        self.agents = list;
                        let filtered = Self::filtered_agents(self.show_subagents, &self.agents);
                        let fallback = filtered
                            .first()
                            .map(|agent| agent.name.clone())
                            .unwrap_or_else(|| "build".to_string());
                        self.default_agent = fallback.clone();
                        let default_agent = self.default_agent.clone();
                        for tab in &mut self.tabs {
                            Self::ensure_tab_agent(&default_agent, tab, &filtered);
                        }
                    }
                    UiMsg::AgentsFailed(err) => {
                        dbg_log(&format!("agent fetch failed: {err}"));
                        if let Some(tab) = self.tabs.get_mut(self.active) {
                            let msg_id = format!(
                                "agent_err_{}",
                                SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis()
                            );
                            tab.messages.push(DisplayMessage {
                                message_id: msg_id,
                                role: "system".to_string(),
                                text_parts: vec![format!("⚠ Agents: {err}")],
                                reasoning_parts: Vec::new(),
                                tokens_input: None,
                                tokens_output: None,
                                tokens_reasoning: None,
                                tool_calls: Vec::new(),
                            });
                        }
                    }
                    UiMsg::AttachmentAdded(data, mime) => {
                        if let Some(tab) = self.tabs.get_mut(self.active) {
                            tab.pending_attachments
                                .push(PendingAttachment { data, mime });
                        }
                    }
                    UiMsg::AudioError(err) => {
                        self.audio_enabled = false;
                        self.recording_state = RecordingState::Idle;
                        if let Some(tab) = self.tabs.get_mut(self.active) {
                            tab.messages.push(DisplayMessage {
                                message_id: format!(
                                    "audio_err_{}",
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap()
                                        .as_millis()
                                ),
                                role: "system".to_string(),
                                text_parts: vec![format!("⚠ Audio: {}", err)],
                                reasoning_parts: Vec::new(),
                                tokens_input: None,
                                tokens_output: None,
                                tokens_reasoning: None,
                                tool_calls: Vec::new(),
                            });
                        }
                    }
                    UiMsg::ModelsDevFetched(data) => {
                        // Find the latest Haiku model for OAuth default
                        if let Some((provider, model_id)) = crate::models_dev::find_latest_haiku(&data) {
                            eprintln!("✓ models.dev: Using {} for OAuth default", model_id);
                            self.oauth_default_model = Some((provider, model_id));
                        }
                        self.models_dev_data = Some(data);
                    }
                }
            }
        }

        for (sid, pid) in auto_rejects {
            self.action_respond_permission(sid, pid, "reject");
        }
    }

    fn action_reconnect(&mut self, ctx: &egui::Context) {
        if self.server_in_flight || self.runtime.is_none() {
            return;
        }
        self.server_in_flight = true;
        let tx = self.ui_tx.as_ref().unwrap().clone();
        let rt = self.runtime.as_ref().unwrap().clone();
        let egui_ctx = ctx.clone();
        rt.spawn(async move {
            let msg = try_discover_or_spawn().await;
            let _ = tx.send(msg);
            egui_ctx.request_repaint();
        });
    }

    pub(crate) fn filtered_agents(show_subagents: bool, agents: &[AgentInfo]) -> Vec<AgentInfo> {
        if show_subagents {
            return agents.to_vec();
        }
        agents
            .iter()
            .filter(|agent| agent.mode.as_deref() != Some("subagent"))
            .cloned()
            .collect()
    }

    pub(crate) fn ensure_tab_agent(default_agent: &str, tab: &mut Tab, filtered: &[AgentInfo]) {
        if let Some(name) = tab.selected_agent.clone() {
            if filtered.iter().any(|agent| agent.name == name) {
                return;
            }
        }
        tab.selected_agent = Some(default_agent.to_string());
    }

    #[cfg(test)]
    pub(crate) fn test_tab_with_agent(agent: Option<String>) -> Tab {
        Tab {
            selected_agent: agent,
            ..Tab::default()
        }
    }

    fn agent_color(hex: &str) -> Option<egui::Color32> {
        let trimmed = hex.strip_prefix('#').unwrap_or(hex);
        if trimmed.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&trimmed[0..2], 16).ok()?;
        let g = u8::from_str_radix(&trimmed[2..4], 16).ok()?;
        let b = u8::from_str_radix(&trimmed[4..6], 16).ok()?;
        Some(egui::Color32::from_rgb(r, g, b))
    }

    fn normalize_code_fences(input: &str) -> String {
        let mut out = String::with_capacity(input.len() + 8);
        let mut start = 0;

        while let Some(rel) = input[start..].find("```") {
            let fence_start = start + rel;
            let prev_is_newline = if fence_start == 0 {
                true
            } else {
                input[..fence_start].chars().rev().next() == Some('\n')
            };

            out.push_str(&input[start..fence_start]);

            if !prev_is_newline {
                out.push('\n');
            }

            out.push_str("```");
            start = fence_start + 3;
        }

        out.push_str(&input[start..]);
        out
    }

    fn handle_event(tab: &mut Tab, payload: &serde_json::Value, ctx: &egui::Context) {
        let event_type = payload.get("type").and_then(|v| v.as_str());

        match event_type {
            Some("message.updated") => {
                // New message started - only create if ID doesn't exist
                if let Some(props) = payload.get("properties") {
                    if let Some(info) = props.get("info") {
                        let message_id = info
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let role = info
                            .get("role")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let finish = info.get("finish").and_then(|v| v.as_str());
                        let created = info
                            .get("time")
                            .and_then(|t| t.get("created"))
                            .and_then(|v| v.as_i64())
                            .unwrap_or(i64::MAX);

                        if tab.cancelled_messages.iter().any(|m| m == &message_id) {
                            dbg_log(&format!(
                                "message.updated drop: msg={} cancelled",
                                message_id
                            ));
                            return;
                        }

                        if let Some(cutoff) = tab.cancelled_after {
                            if created <= cutoff {
                                dbg_log(&format!(
                                    "message.updated drop: msg={} created={} cutoff={} (cancelled)",
                                    message_id, created, cutoff
                                ));
                                return;
                            }
                        }
                        if created < tab.last_send_at {
                            dbg_log(&format!(
                                "message.updated drop: msg={} created={} last_send_at={}",
                                message_id, created, tab.last_send_at
                            ));
                            return;
                        }

                        if role == "assistant" {
                            if finish.is_some() {
                                tab.active_assistant = None;

                                // Collapse reasoning panel when assistant finishes
                                let id: egui::Id = format!("reasoning-{}", message_id).into();
                                let mut state =
                                     egui::collapsing_header::CollapsingState::load_with_default_open(
                                         ctx,
                                         id,
                                         false,
                                     );
                                state.set_open(false);
                                state.store(ctx);
                            } else {
                                tab.active_assistant = Some(message_id.clone());
                            }
                        }
                        if role == "user" {
                            tab.suppress_incoming = false;
                        }

                        let mut tokens_input = None;
                        let mut tokens_output = None;
                        let mut tokens_reasoning = None;
                        if role == "assistant" {
                            if let Some(tokens) = info.get("tokens") {
                                tokens_input = tokens.get("input").and_then(|v| v.as_u64());
                                tokens_output = tokens.get("output").and_then(|v| v.as_u64());
                                tokens_reasoning = tokens.get("reasoning").and_then(|v| v.as_u64());
                            }
                        }

                        if let Some(existing) =
                            tab.messages.iter_mut().find(|m| m.message_id == message_id)
                        {
                            if role == "assistant" {
                                existing.tokens_input = tokens_input;
                                existing.tokens_output = tokens_output;
                                existing.tokens_reasoning = tokens_reasoning;
                            }
                        } else {
                            dbg_log(&format!(
                                "message.updated accept: msg={} role={} created={} finish={:?}",
                                message_id, role, created, finish
                            ));
                            tab.messages.push(DisplayMessage {
                                message_id: message_id.clone(),
                                role: role.clone(),
                                text_parts: Vec::new(),
                                reasoning_parts: Vec::new(),
                                tokens_input,
                                tokens_output,
                                tokens_reasoning,
                                tool_calls: Vec::new(),
                            });
                        }
                    }
                }
            }
            Some("message.part.updated") => {
                // Part of a message - text events contain full accumulated content, not deltas
                if let Some(props) = payload.get("properties") {
                    if let Some(part) = props.get("part") {
                        let message_id = part.get("messageID").and_then(|v| v.as_str());
                        if let Some(mid) = message_id {
                            if tab.cancelled_messages.iter().any(|m| m == mid) {
                                dbg_log(&format!("part drop: msg={} because cancelled", mid));
                                return;
                            }
                        } else {
                            dbg_log("part drop: missing message_id");
                            return;
                        }

                        let part_type = part.get("type").and_then(|v| v.as_str());
                        let role = tab
                            .messages
                            .iter()
                            .find(|m| m.message_id.as_str() == message_id.unwrap_or(""))
                            .map(|m| m.role.clone())
                            .unwrap_or_else(|| "unknown".to_string());

                        if let Some(mid) = message_id {
                            if tab.cancelled_messages.iter().any(|m| m == mid) {
                                dbg_log(&format!("part drop: msg={} cancelled", mid));
                                return;
                            }
                        }

                        if tab.suppress_incoming {
                            if role == "assistant" {
                                if part_type == Some("text") {
                                    if let Some(mid) = message_id {
                                        dbg_log(&format!(
                                            "part clearing suppress on assistant text msg={}",
                                            mid
                                        ));
                                    }
                                    tab.suppress_incoming = false;
                                } else {
                                    if let Some(mid) = message_id {
                                        dbg_log(&format!(
                                            "part drop: suppress active for assistant msg={} type={:?}",
                                            mid, part_type
                                        ));
                                    }
                                    return;
                                }
                            } else {
                                if let Some(mid) = message_id {
                                    dbg_log(&format!(
                                        "part drop: suppress active for msg={} role={} type={:?}",
                                        mid, role, part_type
                                    ));
                                }
                                return;
                            }
                        }

                        if let Some(call) = part.get("callID").and_then(|v| v.as_str()) {
                            if tab.cancelled_calls.iter().any(|c| c == call) {
                                dbg_log(&format!("part drop: call={} cancelled", call));
                                return;
                            }
                        }

                        if part_type == Some("text") {
                            let text = part.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            if let Some(mid) = message_id {
                                if let Some(msg) =
                                    tab.messages.iter_mut().find(|m| m.message_id == mid)
                                {
                                    msg.text_parts.clear();
                                    msg.text_parts.push(text.to_string());
                                }
                            }
                        } else if part_type == Some("reasoning") {
                            let text = part.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            if let Some(mid) = message_id {
                                if let Some(msg) =
                                    tab.messages.iter_mut().find(|m| m.message_id == mid)
                                {
                                    msg.reasoning_parts.clear();
                                    msg.reasoning_parts.push(text.to_string());
                                }
                            }
                        } else if part_type == Some("tool") {
                            let tool_id =
                                part.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
                            let tool_name = part
                                .get("tool")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let call_id = part
                                .get("callID")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let state = part.get("state");
                            let status = state
                                .and_then(|v| v.get("status"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let output = state
                                .and_then(|v| v.get("output"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let error = state
                                .and_then(|v| v.get("error"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let logs = state
                                .and_then(|v| v.get("logs"))
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|l| l.as_str().map(|s| s.to_string()))
                                        .collect::<Vec<String>>()
                                })
                                .unwrap_or_default();
                            let metadata = state
                                .and_then(|v| v.get("metadata"))
                                .and_then(|v| v.as_object())
                                .cloned()
                                .unwrap_or_default();
                            let started_at = state
                                .and_then(|v| v.get("started_at"))
                                .and_then(|v| v.as_i64());
                            let finished_at = state
                                .and_then(|v| v.get("finished_at"))
                                .and_then(|v| v.as_i64());
                            let input = part
                                .get("input")
                                .or_else(|| state.and_then(|s| s.get("input")))
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);

                            if let Some(mid) = message_id {
                                if let Some(msg) =
                                    tab.messages.iter_mut().find(|m| m.message_id == mid)
                                {
                                    if let Some(call) = call_id.as_deref() {
                                        if tab.cancelled_calls.iter().any(|c| c == call) {
                                            dbg_log(&format!(
                                                "tool part drop: msg={} call={} cancelled",
                                                mid, call
                                            ));
                                            return;
                                        }
                                    }

                                    let existing = msg.tool_calls.iter_mut().find(|t| {
                                        t.id == tool_id
                                            || t.call_id.as_deref() == call_id.as_deref()
                                    });

                                    match existing {
                                        Some(tool) => {
                                            tool.name = tool_name.to_string();
                                            tool.status = status.to_string();
                                            if tool.call_id.is_none() {
                                                tool.call_id = call_id.clone();
                                            }
                                            if !input.is_null() {
                                                tool.input = input.clone();
                                            }
                                            if let Some(val) = output {
                                                tool.output = Some(val);
                                            }
                                            if let Some(val) = error {
                                                tool.error = Some(val);
                                            }
                                            if !metadata.is_empty() {
                                                tool.metadata = metadata.clone();
                                            }
                                            if let Some(start) = started_at {
                                                tool.started_at = Some(start);
                                            }
                                            if let Some(end) = finished_at {
                                                tool.finished_at = Some(end);
                                            }
                                            if !logs.is_empty() {
                                                tool.logs = logs.clone();
                                            }
                                        }
                                        None => {
                                            msg.tool_calls.push(ToolCall {
                                                id: tool_id.to_string(),
                                                name: tool_name.to_string(),
                                                status: status.to_string(),
                                                call_id: call_id,
                                                input: input,
                                                output: output,
                                                error: error,
                                                metadata: metadata,
                                                started_at: started_at,
                                                finished_at: finished_at,
                                                logs: logs,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn cancel_active_response(tab: &mut Tab) {
        if let Some(active_id) = tab.active_assistant.clone() {
            let now_ms = match SystemTime::now().duration_since(UNIX_EPOCH) {
                Ok(dur) => dur.as_millis() as i64,
                Err(_) => 0,
            };

            dbg_log(&format!(
                "stop: active_id={} cancelled_after={}",
                active_id, now_ms
            ));

            if let Some(msg) = tab.messages.iter_mut().find(|m| m.message_id == active_id) {
                if msg.text_parts.is_empty() {
                    msg.text_parts.push("✖ Cancelled".to_string());
                }

                for tool in &mut msg.tool_calls {
                    if tool.status != "success"
                        && tool.status != "error"
                        && tool.status != "completed"
                        && tool.status != "cancelled"
                    {
                        dbg_log(&format!(
                            "stop: cancelling tool id={} status was {}",
                            tool.id, tool.status
                        ));
                        tool.status = "cancelled".to_string();
                        if tool.finished_at.is_none() {
                            tool.finished_at = Some(now_ms);
                        }
                    }

                    if let Some(call_id) = &tool.call_id {
                        if !tab.cancelled_calls.iter().any(|c| c == call_id) {
                            tab.cancelled_calls.push(call_id.clone());
                        }
                    }
                }
            }

            if !tab.cancelled_messages.iter().any(|m| m == &active_id) {
                tab.cancelled_messages.push(active_id.clone());
            }

            tab.cancelled_after = Some(now_ms);
            tab.suppress_incoming = true;
            tab.active_assistant = None;
        }
    }

    fn render_message(
        &mut self,
        ui: &mut egui::Ui,
        msg: &DisplayMessage,
        session_id: Option<&str>,
    ) {
        let _message_id = msg.message_id.clone();
        let available_width = ui.available_width();
        let bubble_max_width = available_width * 0.75;

        // Determine colors and alignment
        let (bg_color, align_right) = match msg.role.as_str() {
            "user" => (egui::Color32::from_rgb(60, 100, 180), true),
            "assistant" => (egui::Color32::from_rgb(70, 70, 70), false),
            _ => (egui::Color32::from_rgb(100, 70, 120), false),
        };

        ui.add_space(8.0);

        // Combine text parts into a single markdown string
        let raw_text = msg.text_parts.join("");
        let full_text = if msg.role == "assistant" {
            OpenCodeApp::normalize_code_fences(&raw_text)
        } else {
            raw_text
        };
        let reasoning_text = msg.reasoning_parts.join("");

        ui.horizontal(|ui| {
            if align_right {
                // User messages: right-aligned
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    egui::Frame::new()
                        .fill(bg_color)
                        .corner_radius(10)
                        .inner_margin(12.0)
                        .show(ui, |ui| {
                            ui.set_max_width(bubble_max_width);
                            if !full_text.is_empty() {
                                egui_commonmark::CommonMarkViewer::new().show(
                                    ui,
                                    &mut self.commonmark_cache,
                                    &full_text,
                                );
                            }
                        });

                    ui.add_space(6.0);

                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(full_text.clone());
                    }
                });
            } else {
                // Assistant/system messages: left-aligned
                egui::Frame::new()
                    .fill(bg_color)
                    .corner_radius(10)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_max_width(bubble_max_width);

                        // Use a single vertical column so text and
                        // any tool calls share the same full-width layout.
                        ui.vertical(|ui| {
                            let column_width = ui.available_width();
                            ui.set_width(column_width);

                            if msg.role == "assistant" && !reasoning_text.trim().is_empty() {
                                egui::Frame::new()
                                    .fill(egui::Color32::from_rgb(45, 45, 45))
                                    .corner_radius(6)
                                    .inner_margin(6.0)
                                    .show(ui, |ui| {
                                        egui::collapsing_header::CollapsingState::load_with_default_open(
                                            ui.ctx(),
                                            format!("reasoning-{}", msg.message_id).into(),
                                            full_text.is_empty(),
                                        )
                                        .show_header(ui, |ui| {
                                            ui.label("Reasoning");
                                        })
                                        .body(|ui| {
                                            egui::Frame::new()
                                                .fill(egui::Color32::from_rgb(40, 40, 40))
                                                .corner_radius(4)
                                                .inner_margin(8.0)
                                                .show(ui, |ui| {
                                                    ui.label(reasoning_text.clone());
                                                });
                                        });
                                    });

                                ui.add_space(8.0);
                            }

                            if !full_text.is_empty() {
                                 // For system messages, use EmojiLabel to render colored emojis
                                 // For assistant messages, use CommonMarkViewer for markdown support
                                 if msg.role == "system" {
                                     egui_twemoji::EmojiLabel::new(&full_text).show(ui);
                                 } else {
                                     egui_commonmark::CommonMarkViewer::new().show(
                                         ui,
                                         &mut self.commonmark_cache,
                                         &full_text,
                                     );
                                 }
                             } else if msg.role == "assistant" && msg.tool_calls.is_empty() {
                                 // Show spinner only when no text AND no tools (truly waiting for response)
                                 ui.horizontal(|ui| {
                                     ui.spinner();
                                     ui.label("Thinking...");
                                 });
                             }

                            if msg.role == "assistant" {
                                if msg.tokens_input.is_some()
                                    || msg.tokens_output.is_some()
                                    || msg.tokens_reasoning.is_some()
                                {
                                    ui.add_space(4.0);
                                    let mut parts = Vec::new();
                                    if let Some(input) = msg.tokens_input {
                                        parts.push(format!("in {input}"));
                                    }
                                    if let Some(output) = msg.tokens_output {
                                        parts.push(format!("out {output}"));
                                    }
                                    if let Some(reasoning) = msg.tokens_reasoning {
                                        parts.push(format!("reason {reasoning}"));
                                    }
                                    if !parts.is_empty() {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "tokens: {}",
                                                parts.join(", "),
                                            ))
                                            .small()
                                            .weak(),
                                        );
                                    }
                                }
                            }
                             // Tool calls (collapsible), stacked vertically under the text
                             if !msg.tool_calls.is_empty() {

                                ui.add_space(8.0);
                                for tool in &msg.tool_calls {
                                    self.render_warp_tool_block(ui, tool, session_id);
                                }
                            }
                        });
                    });

                ui.add_space(6.0);

                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(full_text.clone());
                }
            }
        });

        ui.add_space(4.0);
    }

    fn render_warp_tool_block(
        &mut self,
        ui: &mut egui::Ui,
        tool: &ToolCall,
        session_id: Option<&str>,
    ) {
        let is_running = tool.status != "success"
            && tool.status != "completed"
            && tool.status != "error"
            && tool.status != "cancelled";
        let has_error = tool.error.is_some();
        let tool_id = tool.id.clone();

        // Check for permission
        let perm_opt = if let (Some(sid), Some(call_id)) = (session_id, &tool.call_id) {
            self.pending_permissions
                .iter()
                .find(|p| p.session_id == sid && p.call_id.as_deref() == Some(call_id.as_str()))
                .cloned()
        } else {
            None
        };
        let has_permission = perm_opt.is_some();

        let id = ui.make_persistent_id(&tool_id);
        let default_open = is_running || has_permission || has_error;
        let mut is_expanded = ui.data(|d| d.get_temp::<bool>(id).unwrap_or(default_open));

        ui.push_id(id, |ui| {
            ui.vertical(|ui| {
                // -- Header --
                let header_rounding = if is_expanded {
                    egui::CornerRadius {
                        nw: 6,
                        ne: 6,
                        sw: 0,
                        se: 0,
                    }
                } else {
                    egui::CornerRadius::same(6)
                };

                egui::Frame::new()
                    .fill(egui::Color32::from_gray(45))
                    .corner_radius(header_rounding)
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(60)))
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            // Header Row (Clickable)
                            let mut toggle_requested = false;

                            ui.horizontal(|ui| {
                                ui.style_mut().spacing.item_spacing.x = 8.0;

                                // Right Side (Duration) - Render first to stick to right
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if let (Some(start), Some(end)) =
                                            (tool.started_at, tool.finished_at)
                                        {
                                            let duration_ms = end - start;
                                            let text =
                                                format!("{:.1}s", duration_ms as f64 / 1000.0);
                                            if ui
                                                .add(
                                                    egui::Label::new(
                                                        egui::RichText::new(text).weak(),
                                                    )
                                                    .sense(egui::Sense::click()),
                                                )
                                                .clicked()
                                            {
                                                toggle_requested = true;
                                            }
                                        } else if tool.started_at.is_some() {
                                            if ui
                                                .add(
                                                    egui::Label::new(
                                                        egui::RichText::new("...").weak(),
                                                    )
                                                    .sense(egui::Sense::click()),
                                                )
                                                .clicked()
                                            {
                                                toggle_requested = true;
                                            }
                                        }

                                        // Left Side + Middle (Path) - Fills remaining space
                                        ui.with_layout(
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                // Status Icon
                                                let status_icon = match tool.status.as_str() {
                                                    "success" | "completed" => "✅",
                                                    "error" => "❌",
                                                    "cancelled" => "🚫",
                                                    _ => "⏳",
                                                };
                                                if ui
                                                    .add(
                                                        egui::Label::new(status_icon)
                                                            .sense(egui::Sense::click()),
                                                    )
                                                    .clicked()
                                                {
                                                    toggle_requested = true;
                                                }

                                                // Name
                                                let name_text =
                                                    egui::RichText::new(format!("({})", tool.name))
                                                        .strong()
                                                        .color(egui::Color32::WHITE);
                                                if ui
                                                    .add(
                                                        egui::Label::new(name_text)
                                                            .sense(egui::Sense::click()),
                                                    )
                                                    .clicked()
                                                {
                                                    toggle_requested = true;
                                                }

                                                // Separator
                                                let sep_text = egui::RichText::new("  -  ")
                                                    .color(egui::Color32::from_gray(100));
                                                if ui
                                                    .add(
                                                        egui::Label::new(sep_text)
                                                            .sense(egui::Sense::click()),
                                                    )
                                                    .clicked()
                                                {
                                                    toggle_requested = true;
                                                }

                                                // Command Summary
                                                let parsed_input_store;
                                                let effective_input = if let Some(s) =
                                                    tool.input.as_str()
                                                {
                                                    if let Ok(val) =
                                                        serde_json::from_str::<serde_json::Value>(s)
                                                    {
                                                        parsed_input_store = val;
                                                        &parsed_input_store
                                                    } else {
                                                        &tool.input
                                                    }
                                                } else {
                                                    &tool.input
                                                };

                                                let get_arg = |key: &str| -> Option<String> {
                                                    Self::extract_field_as_string(
                                                        effective_input,
                                                        key,
                                                    )
                                                    .or_else(|| {
                                                        effective_input.get("parameters").and_then(
                                                            |p| {
                                                                Self::extract_field_as_string(
                                                                    p, key,
                                                                )
                                                            },
                                                        )
                                                    })
                                                };

                                                let summary_text =
                                                    if let Some(command) = get_arg("command") {
                                                        Some(command)
                                                    } else if let Some(path) = get_arg("filePath")
                                                        .or_else(|| get_arg("path"))
                                                        .or_else(|| get_arg("file_path"))
                                                        .or_else(|| get_arg("filename"))
                                                    {
                                                        Some(path)
                                                    } else if let Some(url) = get_arg("url") {
                                                        Some(url)
                                                    } else if let Some(prompt) = get_arg("prompt") {
                                                        Some(prompt)
                                                    } else {
                                                        None
                                                    };

                                                if let Some(text) = summary_text {
                                                    // Scroll area for full path
                                                    let available = ui.available_width();
                                                    egui::ScrollArea::horizontal()
                                                        .max_width(available)
                                                        .show(ui, |ui| {
                                                            ui.label(
                                                                egui::RichText::new(text)
                                                                    .monospace()
                                                                    .color(
                                                                        egui::Color32::from_gray(
                                                                            180,
                                                                        ),
                                                                    ),
                                                            );
                                                        });
                                                } else {
                                                    // Fallback
                                                    ui.label(
                                                        egui::RichText::new("Run")
                                                            .monospace()
                                                            .color(egui::Color32::from_gray(180)),
                                                    );
                                                }
                                            },
                                        );
                                    },
                                );
                            });

                            if toggle_requested {
                                is_expanded = !is_expanded;
                                ui.data_mut(|d| d.insert_temp(id, is_expanded));
                            }

                            // Permission Row (Inside Header)
                            if let Some(perm) = perm_opt {
                                ui.add_space(6.0);
                                egui::Frame::default()
                                    .fill(egui::Color32::from_rgba_premultiplied(60, 20, 20, 255))
                                    .stroke(egui::Stroke::new(
                                        1.0,
                                        egui::Color32::from_rgb(180, 50, 50),
                                    ))
                                    .corner_radius(4)
                                    .inner_margin(8.0)
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            if ui.button("❌ Reject").clicked() {
                                                self.action_respond_permission(
                                                    perm.session_id.clone(),
                                                    perm.id.clone(),
                                                    "reject",
                                                );
                                                if let Some(idx) = self
                                                    .pending_permissions
                                                    .iter()
                                                    .position(|p| p.id == perm.id)
                                                {
                                                    self.pending_permissions.remove(idx);
                                                }
                                            }
                                            if ui.button("✅ Allow Once").clicked() {
                                                self.action_respond_permission(
                                                    perm.session_id.clone(),
                                                    perm.id.clone(),
                                                    "once",
                                                );
                                                if let Some(idx) = self
                                                    .pending_permissions
                                                    .iter()
                                                    .position(|p| p.id == perm.id)
                                                {
                                                    self.pending_permissions.remove(idx);
                                                }
                                            }
                                            if ui.button("✅ Always Allow").clicked() {
                                                self.action_respond_permission(
                                                    perm.session_id.clone(),
                                                    perm.id.clone(),
                                                    "always",
                                                );
                                                if let Some(idx) = self
                                                    .pending_permissions
                                                    .iter()
                                                    .position(|p| p.id == perm.id)
                                                {
                                                    self.pending_permissions.remove(idx);
                                                }
                                            }
                                        });
                                    });
                            }
                        });
                    });

                // -- Body --
                if is_expanded {
                    egui::Frame::new()
                        .fill(egui::Color32::BLACK)
                        .corner_radius(egui::CornerRadius {
                            nw: 0,
                            ne: 0,
                            sw: 6,
                            se: 6,
                        })
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(60)))
                        .inner_margin(12.0)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());

                            // Command
                            if let Some(command) =
                                Self::extract_field_as_string(&tool.input, "command")
                            {
                                ui.label(
                                    egui::RichText::new("COMMAND")
                                        .small()
                                        .color(egui::Color32::from_gray(120)),
                                );
                                ui.add_space(2.0);
                                let mut text = command.as_str();
                                let cmd_layout = egui::TextEdit::multiline(&mut text)
                                    .font(egui::TextStyle::Monospace)
                                    .code_editor()
                                    .interactive(false)
                                    .desired_width(f32::INFINITY);
                                ui.add(cmd_layout);
                                ui.add_space(8.0);
                            }

                            // Input Arguments (if not just command)
                            // Actually show full input if complex?
                            // Let's hide specific fields if we showed them specially
                            let mut display_input = tool.input.clone();
                            if let serde_json::Value::Object(ref mut map) = display_input {
                                map.remove("command");
                            }
                            if !display_input.is_null()
                                && display_input
                                    != serde_json::Value::Object(serde_json::Map::new())
                            {
                                ui.label(
                                    egui::RichText::new("INPUT")
                                        .small()
                                        .color(egui::Color32::from_gray(120)),
                                );
                                ui.add_space(2.0);
                                ui.monospace(Self::format_json_value(&display_input));
                                ui.add_space(8.0);
                            }

                            // Output
                            if let Some(output) = &tool.output {
                                ui.label(
                                    egui::RichText::new("OUTPUT")
                                        .small()
                                        .color(egui::Color32::from_gray(120)),
                                );
                                ui.add_space(2.0);

                                egui::ScrollArea::vertical()
                                    .max_height(300.0)
                                    .show(ui, |ui| {
                                        let mut text = output.as_str();
                                        ui.add(
                                            egui::TextEdit::multiline(&mut text)
                                                .font(egui::TextStyle::Monospace)
                                                .code_editor()
                                                .desired_width(f32::INFINITY)
                                                .interactive(false),
                                        );
                                    });
                                ui.add_space(8.0);
                            }

                            // Error
                            if let Some(error) = &tool.error {
                                ui.label(
                                    egui::RichText::new("ERROR")
                                        .small()
                                        .color(egui::Color32::RED),
                                );
                                ui.add_space(2.0);
                                ui.colored_label(egui::Color32::RED, error);
                                ui.add_space(8.0);
                            }

                            // Logs
                            if !tool.logs.is_empty() {
                                ui.label(
                                    egui::RichText::new("LOGS")
                                        .small()
                                        .color(egui::Color32::from_gray(120)),
                                );
                                egui::ScrollArea::vertical()
                                    .max_height(150.0)
                                    .show(ui, |ui| {
                                        for log in &tool.logs {
                                            ui.monospace(log);
                                        }
                                    });
                            }
                        });
                }

                ui.add_space(8.0); // Spacing between blocks
            });
        });
    }

    fn extract_field_as_string(value: &serde_json::Value, key: &str) -> Option<String> {
        value
            .as_object()
            .and_then(|obj| obj.get(key))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn format_json_value(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::String(s) => format!("\"{s}\""),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Null => "null".to_string(),
            other => format!("{other}"),
        }
    }

    fn action_start_only(&mut self, ctx: &egui::Context) {
        if self.server_in_flight || self.runtime.is_none() {
            return;
        }
        self.server_in_flight = true;
        let tx = self.ui_tx.as_ref().unwrap().clone();
        let rt = self.runtime.as_ref().unwrap().clone();
        let egui_ctx = ctx.clone();
        rt.spawn(async move {
            let msg = match spawn_and_wait().await {
                Ok(info) => UiMsg::ServerConnected(info),
                Err(e) => UiMsg::ServerError(e.to_string()),
            };
            let _ = tx.send(msg);
            egui_ctx.request_repaint();
        });
    }
}

impl OpenCodeApp {
    fn action_respond_permission(&mut self, session_id: String, perm_id: String, response: &str) {
        if let (Some(client), Some(rt)) = (&self.client, &self.runtime) {
            let c = client.clone();
            let resp = response.to_string();
            rt.spawn(async move {
                let _ = c.respond_permission(&session_id, &perm_id, &resp).await;
            });
        }
    }
    
    fn toggle_anthropic_auth_mode(&mut self, enable_subscription: bool) {
        if enable_subscription {
            // Switch to subscription mode
            if let Some(rt) = &self.runtime {
                // Get server URL
                let server_url = if let Some(server) = &self.server {
                    server.base_url.clone()
                } else {
                    eprintln!("⚠️ No server connected");
                    return;
                };
                
                // Read OAuth tokens from .env cache
                let env_path = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join(".env");
                
                match crate::auth::AnthropicAuth::read_oauth_from_env(&env_path) {
                    Ok(Some(oauth)) => {
                        if crate::auth::AnthropicAuth::is_oauth_expired(oauth.expires) {
                            eprintln!("⚠️ OAuth token expired. Run: opencode auth login");
                            return;
                        }
                        
                        let oauth_clone = oauth.clone();
                        let rt_clone = rt.clone();
                        let server_url_clone = server_url.clone();
                        
                        rt_clone.spawn(async move {
                            eprintln!("🔧 Starting OAuth switch...");
                            let client = reqwest::Client::new();
                            
                            // Send OAuth to server
                            let url = format!("{}/auth/anthropic", server_url_clone);
                            eprintln!("🔧 Sending PUT to {}", url);
                            let result = client.put(&url)
                                .json(&serde_json::json!({
                                    "type": "oauth",
                                    "access": oauth_clone.access,
                                    "refresh": oauth_clone.refresh,
                                    "expires": oauth_clone.expires
                                }))
                                .send()
                                .await;
                            
                            match result {
                                Ok(resp) => {
                                    let status = resp.status();
                                    eprintln!("🔧 Got response: {}", status);
                                    if status.is_success() {
                                        // Reload server state
                                        let dispose_url = format!("{}/instance/dispose", server_url_clone);
                                        eprintln!("🔧 Sending POST to {}", dispose_url);
                                        let _ = client.post(&dispose_url)
                                            .send()
                                            .await;
                                        println!("✓ Switched to Subscription mode");
                                    } else {
                                        let body = resp.text().await.unwrap_or_default();
                                        eprintln!("❌ Failed to switch to subscription: {} - {}", status, body);
                                    }
                                }
                                Err(e) => {
                                    eprintln!("❌ HTTP request failed: {}", e);
                                }
                            }
                        });
                        
                        self.anthropic_subscription_mode = true;
                        self.anthropic_oauth_expires = Some(oauth.expires);
                    }
                    Ok(None) => {
                        eprintln!("⚠️ No OAuth tokens cached. Run: opencode auth login, then click Refresh");
                    }
                    Err(e) => {
                        eprintln!("❌ Failed to read OAuth tokens: {}", e);
                    }
                }
            }
        } else {
            // Switch to API key mode
            if let Some(rt) = &self.runtime {
                // Get server URL
                let server_url = if let Some(server) = &self.server {
                    server.base_url.clone()
                } else {
                    eprintln!("⚠️ No server connected");
                    return;
                };
                
                // Read API key from .env file next to executable
                let env_path = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                    .unwrap_or_else(|| std::env::current_dir().unwrap())
                    .join(".env");
                
                let api_key = if let Ok(content) = std::fs::read_to_string(&env_path) {
                    content.lines()
                        .find(|line| line.starts_with("ANTHROPIC_API_KEY="))
                        .and_then(|line| line.strip_prefix("ANTHROPIC_API_KEY="))
                        .map(|s| s.to_string())
                } else {
                    None
                };
                
                if let Some(api_key) = api_key {
                    let api_key_clone = api_key.clone();
                    let rt_clone = rt.clone();
                    let server_url_clone = server_url.clone();
                    
                    rt_clone.spawn(async move {
                        eprintln!("🔧 Starting API key switch...");
                        let client = reqwest::Client::new();
                        
                        // Send API key to server
                        let url = format!("{}/auth/anthropic", server_url_clone);
                        eprintln!("🔧 Sending PUT to {}", url);
                        let result = client.put(&url)
                            .json(&serde_json::json!({
                                "type": "api",
                                "key": api_key_clone
                            }))
                            .send()
                            .await;
                        
                        match result {
                            Ok(resp) => {
                                let status = resp.status();
                                eprintln!("🔧 Got response: {}", status);
                                if status.is_success() {
                                    // Reload server state
                                    let dispose_url = format!("{}/instance/dispose", server_url_clone);
                                    eprintln!("🔧 Sending POST to {}", dispose_url);
                                    let _ = client.post(&dispose_url)
                                        .send()
                                        .await;
                                    println!("✓ Switched to API Key mode");
                                } else {
                                    let body = resp.text().await.unwrap_or_default();
                                    eprintln!("❌ Failed to switch to API key: {} - {}", status, body);
                                }
                            }
                            Err(e) => {
                                eprintln!("❌ HTTP request failed: {}", e);
                            }
                        }
                    });
                    
                    self.anthropic_subscription_mode = false;
                    self.anthropic_oauth_expires = None;
                } else {
                    eprintln!("⚠️ No API key found in .env");
                }
            }
        }
    }
    
    fn refresh_oauth_tokens(&mut self) {
        // Re-read server's auth.json and update cache
        match crate::auth::AnthropicAuth::read_from_server() {
            Ok(Some(crate::auth::AuthInfo::OAuth { access, refresh, expires })) => {
                // Update .env cache next to executable
                let env_path = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                    .unwrap_or_else(|| std::env::current_dir().unwrap())
                    .join(".env");
                
                let oauth_tokens = crate::auth::OAuthTokens { access, refresh, expires };
                
                match crate::auth::AnthropicAuth::cache_oauth_to_env(&oauth_tokens, &env_path) {
                    Ok(_) => {
                        self.anthropic_oauth_expires = Some(expires);
                        println!("✓ OAuth tokens refreshed");
                    }
                    Err(e) => {
                        eprintln!("❌ Failed to cache OAuth tokens: {}", e);
                    }
                }
            }
            Ok(Some(crate::auth::AuthInfo::ApiKey { .. })) => {
                eprintln!("⚠️ Server is in API key mode, not OAuth. Run: opencode auth login");
            }
            Ok(None) => {
                eprintln!("⚠️ No Anthropic auth found in server. Run: opencode auth login");
            }
            Err(e) => {
                eprintln!("❌ Failed to read server auth: {}", e);
            }
        }
    }
}

async fn try_discover_or_spawn() -> UiMsg {
    match discover() {
        Ok(Some(info)) => {
            if check_health(&info.base_url).await {
                UiMsg::ServerConnected(info)
            } else {
                match spawn_and_wait().await {
                    Ok(info) => UiMsg::ServerConnected(info),
                    Err(e) => UiMsg::ServerError(e.to_string()),
                }
            }
        }
        Ok(None) => match spawn_and_wait().await {
            Ok(info) => UiMsg::ServerConnected(info),
            Err(e) => UiMsg::ServerError(e.to_string()),
        },
        Err(e) => UiMsg::ServerError(e.to_string()),
    }
}

impl eframe::App for OpenCodeApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // Push-to-talk state machine
        // State transitions:
        // - (Idle, key_down) -> Recording + send StartRecording
        // - (Recording, key_up) -> Idle + send StopRecording
        // - All other transitions ignored (prevents double-triggers)

        // Only process if audio task is running
        if self.audio_tx.is_none() {
            // Debug: Check if AltRight is being pressed
            for event in &raw_input.events {
                if let egui::Event::Key { key, pressed, .. } = event {
                    let key_name = format!("{:?}", key);
                    if key_name == "AltRight" && *pressed {
                        eprintln!(
                            "AltRight pressed but audio task not running (no model configured)"
                        );
                    }
                }
            }
            return;
        }

        for event in &raw_input.events {
            if let egui::Event::Key {
                key,
                pressed,
                repeat,
                ..
            } = event
            {
                // Ignore key repeats
                if *repeat {
                    continue;
                }

                // Check if this is our push-to-talk key
                let key_name = format!("{:?}", key);
                if key_name != self.config.audio.push_to_talk_key {
                    continue;
                }

                // State machine
                match (self.recording_state, *pressed) {
                    (RecordingState::Idle, true) => {
                        // Key pressed - start recording
                        self.recording_state = RecordingState::Recording;
                        if let Some(tx) = &self.audio_tx {
                            let _ = tx.send(AudioCmd::StartRecording);
                        }
                    }
                    (RecordingState::Recording, false) => {
                        // Key released - stop recording
                        self.recording_state = RecordingState::Idle;
                        if let Some(tx) = &self.audio_tx {
                            let _ = tx.send(AudioCmd::StopRecording);
                        }
                    }
                    _ => {
                        // Ignore other transitions (key repeats, etc.)
                    }
                }
            }
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Request repaint every second for OAuth countdown timer
        if self.anthropic_subscription_mode && self.anthropic_oauth_expires.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
        
        // Start server discovery on first frame (lazy init)
        if !self.discovery_started {
            self.start_server_discovery(ctx);
        }

        // Drain async messages (SSE-fed channel)
        self.drain_ui_msgs(ctx);

        // Auto-create first tab when client is ready
        if self.tabs.is_empty()
            && self.client.is_some()
            && self.runtime.is_some()
            && self.ui_tx.is_some()
        {
            let tab_idx = 0;
            
            // If OAuth is enabled, use the latest Haiku from models.dev
            let default_model = if self.oauth_token.is_some() {
                self.oauth_default_model.clone()
            } else {
                None
            };

            self.tabs.push(Tab {
                title: "(creating…)".to_string(),
                session_id: None,
                session_version: None,
                directory: None,
                messages: Vec::new(),
                active_assistant: None,
                input: String::new(),
                selected_model: default_model,
                selected_agent: Some(self.default_agent.clone()),
                cancelled_messages: Vec::new(),
                cancelled_calls: Vec::new(),
                cancelled_after: None,
                suppress_incoming: false,
                last_send_at: 0,
                pending_attachments: Vec::new(),
            });

            self.active = 0;

            let txc = self.ui_tx.as_ref().unwrap().clone();
            let c = self.client.as_ref().unwrap().clone();
            let egui_ctx = ctx.clone();
            let rt = self.runtime.as_ref().unwrap().clone();

            rt.spawn(async move {
                match c.create_session(None).await {
                    Ok(info) => {
                        let _ = txc.send(UiMsg::SessionCreated {
                            tab_idx,
                            id: info.id,
                            title: info.title,
                            directory: info.directory,
                            version: info.version.clone(),
                        });
                    }
                    Err(e) => {
                        let _ = txc.send(UiMsg::ServerError(e.to_string()));
                    }
                }
                egui_ctx.request_repaint();
            });
        }

        // Top: Tabs + Server panel
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Tabs
                let mut to_close: Option<usize> = None;
                let mut rename_action: Option<(usize, String)> = None;
                let mut cancel_rename = false;

                for (i, tab) in self.tabs.iter().enumerate() {
                    let selected = self.active == i;

                    // Group tab label, model selector, and close button together
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;

                            // Check if this tab is being renamed
                            if self.renaming_tab == Some(i) {
                                let text_edit = egui::TextEdit::singleline(&mut self.rename_buffer);
                                let response = text_edit.show(ui).response;

                                // Request focus and select all text on first frame only
                                let id = response.id;
                                response.request_focus();
                                if response.has_focus() && !self.rename_text_selected {
                                    // Select all text (only once)
                                    if let Some(mut state) =
                                        egui::TextEdit::load_state(ui.ctx(), id)
                                    {
                                        let text_len = self.rename_buffer.len();
                                        state.cursor.set_char_range(Some(
                                            egui::text::CCursorRange::two(
                                                egui::text::CCursor::new(0),
                                                egui::text::CCursor::new(text_len),
                                            ),
                                        ));
                                        state.store(ui.ctx(), id);
                                        self.rename_text_selected = true;
                                    }
                                }

                                // Confirm on Enter or Tab
                                let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
                                let tab_pressed = ui.input(|i| i.key_pressed(egui::Key::Tab));
                                // Confirm on losing focus (click elsewhere)
                                let lost_focus = response.lost_focus();
                                // Cancel on Escape
                                let escape_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape));

                                if escape_pressed {
                                    cancel_rename = true;
                                } else if enter_pressed || tab_pressed || lost_focus {
                                    if !self.rename_buffer.trim().is_empty() {
                                        rename_action = Some((i, self.rename_buffer.clone()));
                                    }
                                    cancel_rename = true;
                                }
                            } else {
                                let response = ui.selectable_label(selected, &tab.title);

                                if response.clicked() {
                                    self.active = i;
                                }

                                // Right-click context menu
                                response.context_menu(|ui| {
                                    if ui.button("Rename").clicked() {
                                        self.renaming_tab = Some(i);
                                        self.rename_buffer = tab.title.clone();
                                        self.rename_text_selected = false;
                                        ui.close();
                                    }
                                });
                            }

                            if ui.small_button("X").clicked() {
                                to_close = Some(i);
                            }
                        });
                    });
                }

                // Apply deferred actions
                if let Some((idx, new_title)) = rename_action {
                    if let Some(tab) = self.tabs.get_mut(idx) {
                        tab.title = new_title;
                    }
                }
                if cancel_rename {
                    self.renaming_tab = None;
                    self.rename_buffer.clear();
                    self.rename_text_selected = false;
                }
                if let Some(idx) = to_close {
                    self.tabs.remove(idx);
                    if self.active >= self.tabs.len() && self.active > 0 {
                        self.active = self.tabs.len() - 1;
                    }
                    // Cancel rename if we closed the tab being renamed
                    if self.renaming_tab == Some(idx) {
                        self.renaming_tab = None;
                        self.rename_buffer.clear();
                        self.rename_text_selected = false;
                    } else if let Some(r) = self.renaming_tab {
                        if r > idx {
                            self.renaming_tab = Some(r - 1);
                        }
                    }
                }
                if ui.button("+").clicked() {
                    let tab_idx = self.tabs.len();
                    self.tabs.push(Tab {
                        title: "(creating…)".to_string(),
                        session_id: None,
                        session_version: None,
                        directory: None,
                        messages: Vec::new(),
                        active_assistant: None,
                        input: String::new(),
                        selected_model: None,
                        selected_agent: Some(self.default_agent.clone()),
                        cancelled_messages: Vec::new(),
                        cancelled_calls: Vec::new(),
                        cancelled_after: None,
                        suppress_incoming: false,
                        last_send_at: 0,
                        pending_attachments: Vec::new(),
                    });
                    self.active = tab_idx;
                    if let (Some(rt), Some(tx), Some(client)) =
                        (&self.runtime, &self.ui_tx, &self.client)
                    {
                        let txc = tx.clone();
                        let c = client.clone();
                        let egui_ctx = ctx.clone();
                        rt.spawn(async move {
                            match c.create_session(None).await {
                                Ok(info) => {
                                    let _ = txc.send(UiMsg::SessionCreated {
                                        tab_idx,
                                        id: info.id,
                                        title: info.title,
                                        directory: info.directory,
                                        version: info.version.clone(),
                                    });
                                }
                                Err(e) => {
                                    let _ = txc.send(UiMsg::ServerError(e.to_string()));
                                }
                            }
                            egui_ctx.request_repaint();
                        });
                    }
                }
            });
        });

        // Settings Window - handle actions with deferred execution
        let mut reconnect_requested = false;
        let mut start_requested = false;
        let mut stop_requested = false;
        let mut clear_other_sessions_requested = false;

        if self.show_settings {
            egui::Window::new("Settings")
                .open(&mut self.show_settings)
                .default_width(600.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        // Server Preferences Section
                        ui.collapsing("Server Preferences", |ui| {
                            ui.heading("Server Connection");
                            ui.separator();

                            // Manual URL override
                            ui.horizontal(|ui| {
                                ui.label("Base URL:");
                                ui.text_edit_singleline(&mut self.base_url_input);
                            });
                            ui.small("Leave empty for auto-discovery");

                            ui.add_space(8.0);

                            // Directory override
                            ui.horizontal(|ui| {
                                ui.label("Directory override:");
                                ui.text_edit_singleline(&mut self.directory_input);
                            });
                            ui.small("Optional. Sends as x-opencode-directory header.");

                            ui.add_space(8.0);

                            // Auto-start toggle
                            ui.checkbox(
                                &mut self.config.server.auto_start,
                                "Auto-start server on launch",
                            );

                            ui.add_space(8.0);
                            ui.separator();

                            // Discovery diagnostics
                            if let Some(info) = &self.server {
                                ui.label(format!(
                                    "Connected: {} (PID {})",
                                    info.base_url, info.pid
                                ));
                                ui.label(format!("Owned: {}", info.owned));
                            } else {
                                ui.label("Status: Not connected");
                            }
                            let dir_label = if self.directory_input.trim().is_empty() {
                                "(none)".to_string()
                            } else {
                                self.directory_input.clone()
                            };
                            ui.label(format!("Directory header: {}", dir_label));

                            ui.add_space(8.0);

                            // Server actions
                            ui.horizontal(|ui| {
                                if ui.button("Reconnect").clicked() {
                                    reconnect_requested = true;
                                }

                                if ui.button("Start Server").clicked() {
                                    start_requested = true;
                                }

                                if let Some(info) = &self.server {
                                    if info.owned && ui.button("Stop Server").clicked() {
                                        stop_requested = true;
                                    }
                                }
                            });

                            ui.add_space(8.0);

                            if ui.button("Delete all other sessions").clicked() {
                                clear_other_sessions_requested = true;
                            }

                            ui.small("Keeps only the current tab's session for this directory.");

                            ui.add_space(8.0);

                            // Save button for server settings
                            if ui.button("Save Server Settings").clicked() {
                                // Update config from input
                                if self.base_url_input.trim().is_empty() {
                                    self.config.server.last_base_url = None;
                                } else {
                                    self.config.server.last_base_url =
                                        Some(self.base_url_input.clone());
                                }
                                // Update directory override
                                if self.directory_input.trim().is_empty() {
                                    self.config.server.directory_override = None;
                                } else {
                                    self.config.server.directory_override =
                                        Some(self.directory_input.clone());
                                }
                                // Apply to live client
                                if let Some(c) = &mut self.client {
                                    c.directory = self
                                        .config
                                        .server
                                        .directory_override
                                        .as_ref()
                                        .map(|s| std::path::PathBuf::from(s));
                                }
                                self.config.save();
                            }
                        });

                        ui.add_space(16.0);

                        // UI Preferences Section
                        ui.collapsing("UI Preferences", |ui| {
                            ui.heading("Appearance");
                            ui.separator();

                            // Font size preset
                            ui.label("Font Size:");
                            let mut font_changed = false;
                            ui.horizontal(|ui| {
                                if ui
                                    .radio_value(
                                        &mut self.config.ui.font_size,
                                        crate::config::FontSizePreset::Small,
                                        "Small",
                                    )
                                    .clicked()
                                {
                                    font_changed = true;
                                }
                                if ui
                                    .radio_value(
                                        &mut self.config.ui.font_size,
                                        crate::config::FontSizePreset::Standard,
                                        "Standard",
                                    )
                                    .clicked()
                                {
                                    font_changed = true;
                                }
                                if ui
                                    .radio_value(
                                        &mut self.config.ui.font_size,
                                        crate::config::FontSizePreset::Large,
                                        "Large",
                                    )
                                    .clicked()
                                {
                                    font_changed = true;
                                }
                            });

                            // Apply font changes immediately
                            if font_changed {
                                self.config.ui.apply_to_context(ctx);
                                self.config.save();
                            }

                            ui.add_space(8.0);

                            // Base font size (pt)
                            ui.label("Base font (pt):");
                            let resp = ui.add(
                                egui::Slider::new(
                                    &mut self.config.ui.base_font_points,
                                    10.0..=24.0,
                                )
                                .text("Base (pt)"),
                            );
                            if resp.changed() {
                                self.config.ui.apply_to_context(ctx);
                                self.config.save();
                            }

                            ui.add_space(8.0);

                            // Chat density
                            ui.label("Chat Density:");
                            let mut density_changed = false;
                            ui.horizontal(|ui| {
                                if ui
                                    .radio_value(
                                        &mut self.config.ui.chat_density,
                                        crate::config::ChatDensity::Compact,
                                        "Compact",
                                    )
                                    .clicked()
                                {
                                    density_changed = true;
                                }
                                if ui
                                    .radio_value(
                                        &mut self.config.ui.chat_density,
                                        crate::config::ChatDensity::Normal,
                                        "Normal",
                                    )
                                    .clicked()
                                {
                                    density_changed = true;
                                }
                                if ui
                                    .radio_value(
                                        &mut self.config.ui.chat_density,
                                        crate::config::ChatDensity::Comfortable,
                                        "Comfortable",
                                    )
                                    .clicked()
                                {
                                    density_changed = true;
                                }
                            });

                            // Save density changes
                            if density_changed {
                                self.config.save();
                            }

                            ui.add_space(8.0);

                            let prev_subagents = self.show_subagents;
                            ui.checkbox(&mut self.show_subagents, "Show subagents in agent list");
                            if self.show_subagents != prev_subagents {
                                let filtered =
                                    Self::filtered_agents(self.show_subagents, &self.agents);
                                let fallback = filtered
                                    .first()
                                    .map(|agent| agent.name.clone())
                                    .unwrap_or_else(|| "build".to_string());
                                self.default_agent = fallback.clone();
                                let default_agent = self.default_agent.clone();
                                for tab in &mut self.tabs {
                                    Self::ensure_tab_agent(&default_agent, tab, &filtered);
                                }
                            }
                        });

                        ui.add_space(16.0);

                        // Models Section
                        ui.collapsing("Models", |ui| {
                            ui.heading("Curated Models");
                            ui.separator();

                            ui.label("Your curated models:");
                            ui.add_space(8.0);

                            // Display curated models with remove buttons
                            let mut model_to_remove: Option<(String, String)> = None;
                            for model in self.models_config.get_curated_models() {
                                ui.horizontal(|ui| {
                                    ui.label(format!(
                                        "{}  ({}/{})",
                                        model.name, model.provider, model.model_id
                                    ));
                                    if ui.small_button("✖").clicked() {
                                        model_to_remove =
                                            Some((model.provider.clone(), model.model_id.clone()));
                                    }
                                });
                            }

                            // Remove model if requested (deferred to avoid borrow issues)
                            if let Some((provider, model_id)) = model_to_remove {
                                self.models_config
                                    .remove_curated_model(&provider, &model_id);
                                let _ = self.models_config.save();
                            }

                            ui.add_space(8.0);

                            // Add Model button
                            if ui.button("+ Add Model").clicked() {
                                self.show_model_discovery = true;
                            }

                            ui.add_space(16.0);
                            ui.separator();

                            // Default model selector
                            ui.label("Default model for new tabs:");
                            let current_default = self.models_config.models.default_model.clone();
                            let curated_models = self.models_config.get_curated_models().to_vec();
                            egui::ComboBox::from_id_salt("default_model_selector")
                                .selected_text(&current_default)
                                .show_ui(ui, |ui| {
                                    for model in &curated_models {
                                        let model_id =
                                            format!("{}/{}", model.provider, model.model_id);
                                        if ui
                                            .selectable_value(
                                                &mut self.models_config.models.default_model,
                                                model_id.clone(),
                                                &model.name,
                                            )
                                            .clicked()
                                        {
                                            let _ = self.models_config.save();
                                        }
                                    }
                                });

                            ui.add_space(8.0);

                            // Auth sync status display
                            ui.separator();
                            ui.label("API Key Sync Status:");
                            match &self.auth_sync_state.status {
                                crate::startup::auth::AuthSyncStatus::NotStarted => {
                                    ui.label("⏸ Not started");
                                }
                                crate::startup::auth::AuthSyncStatus::InProgress => {
                                    ui.label("⏳ Syncing keys to server...");
                                }
                                crate::startup::auth::AuthSyncStatus::Complete => {
                                    ui.label("✅ Complete");
                                    if !self.auth_sync_state.synced_providers.is_empty() {
                                        ui.small(format!(
                                            "Synced: {}",
                                            self.auth_sync_state.synced_providers.join(", ")
                                        ));
                                    }
                                    if !self.auth_sync_state.failed_providers.is_empty() {
                                        for (provider, error) in
                                            &self.auth_sync_state.failed_providers
                                        {
                                            ui.colored_label(
                                                egui::Color32::from_rgb(255, 100, 100),
                                                format!("❌ {provider}: {error}"),
                                            );
                                        }
                                    }
                                }
                                crate::startup::auth::AuthSyncStatus::Failed(err) => {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(255, 100, 100),
                                        format!("❌ Failed: {err}"),
                                    );
                                }
                            }
                        });
                    });
                });
        }

        // Model Discovery Window
        if self.show_model_discovery {
            let mut close_requested = false;
            egui::Window::new("Add Model")
                .default_width(500.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        // Step 1: Select Provider (if not selected yet)
                        if self.discovery_provider.is_none() {
                            ui.heading("Select a provider:");
                            ui.separator();
                            ui.add_space(8.0);

                            for provider in self.models_config.get_providers() {
                                if ui.button(&provider.display_name).clicked() {
                                    self.discovery_provider = Some(provider.name.clone());
                                    self.discovery_in_progress = true;
                                    self.discovery_error = None;
                                    self.discovery_models.clear();

                                    // Spawn async task to discover models
                                    if let (Some(rt), Some(tx)) = (&self.runtime, &self.ui_tx) {
                                        let provider_config = provider.clone();
                                        let tx = tx.clone();
                                        let egui_ctx = ctx.clone();

                                        // Get API key from environment
                                        if let Ok(api_key) =
                                            std::env::var(&provider_config.api_key_env)
                                        {
                                            rt.spawn(async move {
                                                let provider_client =
                                                    crate::client::providers::ProviderClient::new()
                                                        .unwrap();
                                                match provider_client
                                                    .discover_models(&provider_config, &api_key)
                                                    .await
                                                {
                                                    Ok(models) => {
                                                        let _ = tx
                                                            .send(UiMsg::ModelsDiscovered(models));
                                                    }
                                                    Err(e) => {
                                                        let _ =
                                                            tx.send(UiMsg::ModelDiscoveryError(
                                                                e.to_string(),
                                                            ));
                                                    }
                                                }
                                                egui_ctx.request_repaint();
                                            });
                                        } else {
                                            self.discovery_error = Some(format!(
                                                "API key not found: {}",
                                                provider_config.api_key_env
                                            ));
                                            self.discovery_in_progress = false;
                                        }
                                    }
                                }
                            }

                            ui.add_space(8.0);
                            if ui.button("Cancel").clicked() {
                                close_requested = true;
                            }
                        }
                        // Step 2: Display discovered models
                        else {
                            let provider_name = self.discovery_provider.as_ref().unwrap().clone();
                            ui.heading(format!("Add Model from {provider_name}"));
                            ui.separator();
                            ui.add_space(8.0);

                            // Show loading spinner or error
                            if self.discovery_in_progress {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label("Discovering models...");
                                });
                            } else if let Some(error) = &self.discovery_error {
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 100, 100),
                                    format!("Error: {error}"),
                                );
                            } else if !self.discovery_models.is_empty() {
                                // Search box
                                ui.horizontal(|ui| {
                                    ui.label("Search:");
                                    ui.text_edit_singleline(&mut self.discovery_search);
                                });
                                ui.add_space(8.0);

                                // Filter models by search
                                let search_lower = self.discovery_search.to_lowercase();
                                let filtered_models: Vec<_> = self
                                    .discovery_models
                                    .iter()
                                    .filter(|m| {
                                        search_lower.is_empty()
                                            || m.id.to_lowercase().contains(&search_lower)
                                            || m.name.to_lowercase().contains(&search_lower)
                                    })
                                    .cloned()
                                    .collect();

                                ui.label(format!("{} models found:", filtered_models.len()));
                                ui.separator();

                                let mut model_to_add: Option<
                                    crate::client::providers::DiscoveredModel,
                                > = None;
                                egui::ScrollArea::vertical()
                                    .max_height(300.0)
                                    .show(ui, |ui| {
                                        for model in &filtered_models {
                                            ui.horizontal(|ui| {
                                                if ui.button("+").clicked() {
                                                    model_to_add = Some(model.clone());
                                                }
                                                ui.label(format!("{} ({})", model.name, model.id));
                                            });
                                        }
                                    });

                                // Add model if requested (deferred to avoid borrow issues)
                                if let Some(model) = model_to_add {
                                    let curated_model = crate::config::models::CuratedModel::new(
                                        model.name.clone(),
                                        provider_name.clone(),
                                        model.id.clone(),
                                    );
                                    self.models_config.add_curated_model(curated_model);
                                    let _ = self.models_config.save();

                                    // Show success and close
                                    close_requested = true;
                                    self.discovery_provider = None;
                                    self.discovery_models.clear();
                                    self.discovery_search.clear();
                                }
                            }

                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                if ui.button("Back").clicked() {
                                    self.discovery_provider = None;
                                    self.discovery_models.clear();
                                    self.discovery_error = None;
                                    self.discovery_in_progress = false;
                                    self.discovery_search.clear();
                                }
                                if ui.button("Cancel").clicked() {
                                    close_requested = true;
                                    self.discovery_provider = None;
                                    self.discovery_models.clear();
                                    self.discovery_error = None;
                                    self.discovery_in_progress = false;
                                    self.discovery_search.clear();
                                }
                            });
                        }
                    });

                    // Close button
                    ui.horizontal(|ui| {
                        if ui.button("✖ Close").clicked() {
                            close_requested = true;
                        }
                    });
                });

            if close_requested {
                self.show_model_discovery = false;
            }
        }

        // Execute deferred actions
        if reconnect_requested {
            self.action_reconnect(ctx);
        }
        if start_requested {
            self.action_start_only(ctx);
        }
        if stop_requested {
            if let Some(info) = &self.server {
                if stop_pid(info.pid) {
                    self.server = None;
                    self.server_in_flight = false;
                }
            }
        }
        if clear_other_sessions_requested {
            if let (Some(rt), Some(client)) = (&self.runtime, &self.client) {
                if let Some(tab) = self.tabs.get(self.active) {
                    if let Some(current_id) = &tab.session_id {
                        let current_id = current_id.clone();
                        let client = client.clone();
                        let tx = self.ui_tx.clone();
                        let egui_ctx = ctx.clone();

                        rt.spawn(async move {
                            let result = async {
                                let sessions = client.list_sessions().await?;
                                for session in sessions {
                                    if session.id != current_id {
                                        let _ = client.delete_session(&session.id).await;
                                    }
                                }
                                Ok::<(), crate::error::api::ApiError>(())
                            }
                            .await;

                            if let Err(e) = result {
                                if let Some(tx) = tx {
                                    let _ = tx.send(UiMsg::ServerError(e.to_string()));
                                }
                            }

                            egui_ctx.request_repaint();
                        });
                    }
                }
            }
        }

        let filtered_agents = Self::filtered_agents(self.show_subagents, &self.agents);
        let has_agents = !self.agents.is_empty();

        // Global footer: spans full width under agents and chat
        egui::TopBottomPanel::bottom("footer_panel")
            .resizable(false)
            .min_height(24.0)
            .default_height(28.0)
            .show(ctx, |ui| {
                ui.set_min_height(24.0);

                if self.tabs.is_empty() {
                    return;
                }

                if let Some(tab) = self.tabs.get_mut(self.active) {
                    // Deferred actions
                    let mut toggle_to: Option<bool> = None;
                    let mut do_refresh = false;
                    
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;

                        // Left side: OAuth toggle, model selector and active agent label
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            // OAuth Subscription Toggle (only for Anthropic)
                            let mut subscription_checked = self.anthropic_subscription_mode;
                            if ui.checkbox(&mut subscription_checked, "").changed() {
                                toggle_to = Some(subscription_checked);
                            }
                            
                            // Show countdown timer when in subscription mode
                            if self.anthropic_subscription_mode {
                                if let Some(expires) = self.anthropic_oauth_expires {
                                    let time_str = crate::auth::AnthropicAuth::format_time_remaining(expires);
                                    let color = if time_str.contains("Expired") {
                                        egui::Color32::RED
                                    } else if time_str.starts_with("0m") || time_str.starts_with("1m") || time_str.starts_with("2m") || time_str.starts_with("3m") || time_str.starts_with("4m") {
                                        egui::Color32::YELLOW
                                    } else {
                                        egui::Color32::GREEN
                                    };
                                    ui.colored_label(color, format!("⏱ {}", time_str));
                                } else {
                                    ui.label("⏱ --");
                                }
                                
                                // Refresh button
                                if ui.small_button("🔄").on_hover_text("Refresh OAuth tokens from server").clicked() {
                                    do_refresh = true;
                                }
                            } else {
                                ui.label("API Key");
                            }
                            
                            ui.separator();
                            
                            if !self.models_config.get_curated_models().is_empty() {
                                let current_display =
                                    if let Some((provider, model_id)) = &tab.selected_model {
                                        // Check if this provider is using OAuth subscription
                                        let is_oauth = self.connected_providers.contains(provider);
                                        let base_display = self.models_config
                                            .get_curated_models()
                                            .iter()
                                            .find(|m| {
                                                &m.provider == provider && &m.model_id == model_id
                                            })
                                            .map(|m| m.name.clone())
                                            .unwrap_or_else(|| format!("{provider}/{model_id}"));
                                        
                                        if is_oauth && provider == "anthropic" {
                                            format!("🟢 {} (Subscription)", base_display)
                                        } else {
                                            base_display
                                        }
                                    } else {
                                        // No model selected, check if anthropic OAuth is available
                                        if self.connected_providers.contains(&"anthropic".to_string()) {
                                            // Show the actual OAuth default model if available
                                            if let Some((provider, model_id)) = &self.oauth_default_model {
                                                let model_name = if let Some(dev_data) = &self.models_dev_data {
                                                    dev_data.get(provider)
                                                        .and_then(|p| p.models.get(model_id))
                                                        .map(|m| m.name.clone())
                                                        .unwrap_or_else(|| model_id.clone())
                                                } else {
                                                    model_id.clone()
                                                };
                                                format!("🟢 {} (Subscription)", model_name)
                                            } else {
                                                "🟢 (Anthropic Subscription)".to_string()
                                            }
                                        } else {
                                            match &self.auth_sync_state.status {
                                                crate::startup::auth::AuthSyncStatus::InProgress => {
                                                    "\u{23F3}".to_string()
                                                }
                                                crate::startup::auth::AuthSyncStatus::Complete => {
                                                    "(default)".to_string()
                                                }
                                                crate::startup::auth::AuthSyncStatus::Failed(_) => {
                                                    "\u{274C}".to_string()
                                                }
                                                _ => "...".to_string(),
                                            }
                                        }
                                    };

                                egui::ComboBox::from_id_salt("footer_model_selector")
                                    .selected_text(current_display)
                                    .width(160.0)
                                    .show_ui(ui, |ui| {
                                        if ui
                                            .selectable_label(
                                                tab.selected_model.is_none(),
                                                "(use default)",
                                            )
                                            .clicked()
                                        {
                                            tab.selected_model = None;
                                        }

                                        ui.separator();

                                        for model in self.models_config.get_curated_models() {
                                            let is_selected = tab
                                                .selected_model
                                                .as_ref()
                                                .map(|(p, m)| {
                                                    p == &model.provider && m == &model.model_id
                                                })
                                                .unwrap_or(false);

                                            if ui
                                                .selectable_label(is_selected, &model.name)
                                                .clicked()
                                            {
                                                tab.selected_model = Some((
                                                    model.provider.clone(),
                                                    model.model_id.clone(),
                                                ));
                                            }
                                        }

                                        ui.separator();

                                        if ui.small_button("\u{2699} Manage Models").clicked() {
                                            self.show_settings = true;
                                            ui.close();
                                        }
                                    });
                            }

                            let agent_display = tab
                                .selected_agent
                                .as_deref()
                                .unwrap_or(self.default_agent.as_str());
                            ui.small(format!("agent: {agent_display}"));

                            let current_dir: Option<&str> = if let Some(override_dir) =
                                self.config.server.directory_override.as_deref()
                            {
                                Some(override_dir)
                            } else {
                                tab.directory.as_deref()
                            };
                            if let Some(dir) = current_dir {
                                ui.separator();
                                ui.small("CWD");
                                ui.separator();
                                ui.small(dir);
                            }
                        });

                        // Right side: server status and settings button
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("\u{2699} Settings").clicked() {
                                self.show_settings = !self.show_settings;
                            }

                            if let Some(ver) = tab.session_version.as_ref() {
                                ui.small(format!("v{}", ver));
                                ui.separator();
                            }

                            if let Some(info) = &self.server {
                                ui.small(format!("Server: {} (PID {})", info.base_url, info.pid));
                            } else if self.server_in_flight {
                                ui.small("Server: connecting…");
                            } else {
                                ui.small("Server: not connected");
                            }
                        });
                    });
                    
                    // Execute deferred actions after UI is rendered
                    if let Some(enabled) = toggle_to {
                        self.toggle_anthropic_auth_mode(enabled);
                    }
                    if do_refresh {
                        self.refresh_oauth_tokens();
                    }
                }
            });

        if !self.tabs.is_empty() && has_agents {
            egui::SidePanel::left("agents_pane").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Agents");
                    if ui
                        .small_button(if self.agents_pane_collapsed {
                            "▸"
                        } else {
                            "▾"
                        })
                        .clicked()
                    {
                        self.agents_pane_collapsed = !self.agents_pane_collapsed;
                    }
                });

                if self.agents_pane_collapsed {
                    return;
                }

                if filtered_agents.is_empty() {
                    ui.label("No primary agents. Enable subagents in Settings.");
                    return;
                }

                if let Some(tab) = self.tabs.get_mut(self.active) {
                    for agent in &filtered_agents {
                        let is_selected = tab
                            .selected_agent
                            .as_deref()
                            .map(|name| name == agent.name.as_str())
                            .unwrap_or(false);
                        let is_sub = agent.mode.as_deref() == Some("subagent");
                        let mut label_text = egui::RichText::new(&agent.name);
                        if is_sub {
                            label_text = label_text.color(egui::Color32::from_gray(150));
                        }
                        ui.horizontal(|ui| {
                            let response = ui.selectable_label(is_selected, label_text);
                            if let Some(color_hex) = &agent.color {
                                if let Some(color) = Self::agent_color(color_hex) {
                                    ui.colored_label(color, "⬤");
                                }
                            }
                            if agent.built_in {
                                ui.small("built-in");
                            }
                            if is_sub {
                                ui.small("subagent");
                            }
                            if response.clicked() {
                                tab.selected_agent = Some(agent.name.clone());
                                dbg_log(&format!("agent selected: {}", agent.name));
                            }
                        });
                    }
                }
            });
        }

        // Bottom: Input area (resizable split pane)
        egui::TopBottomPanel::bottom("input_panel")
            .resizable(true)
            .min_height(72.0)
            .default_height(72.0)
            .show(ctx, |ui| {
                ui.set_min_height(72.0);
                ui.take_available_height();

                if self.tabs.is_empty() {
                    return;
                }

                if let Some(tab) = self.tabs.get_mut(self.active) {
                    let has_session = tab.session_id.is_some();
                    let session_id = tab.session_id.clone();
                    let blocked = session_id
                        .as_ref()
                        .and_then(|sid| {
                            self.pending_permissions
                                .iter()
                                .find(|p| p.session_id == *sid)
                        })
                        .is_some();
                    let streaming = tab.active_assistant.is_some();

                    ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                        let panel_height = ui.available_height();
                        let side_width = 200.0;
                        let side_size = egui::vec2(side_width, panel_height);

                        // Left column: attachments panel
                        ui.allocate_ui(side_size, |ui| {
                            ui.vertical(|ui| {
                                if ui.button("📋 Paste Image").clicked() {
                                    if let Some(tx) = self.ui_tx.clone() {
                                        let egui_ctx = ctx.clone();
                                        std::thread::spawn(move || {
                                            if let Ok(mut cb) = arboard::Clipboard::new() {
                                                if let Ok(img) = cb.get_image() {
                                                    let w = img.width as u32;
                                                    let h = img.height as u32;
                                                    let mut png_data = Vec::new();
                                                    let encoder = PngEncoder::new(&mut png_data);
                                                    let color_type = ExtendedColorType::Rgba8;
                                                    if encoder
                                                        .write_image(&img.bytes, w, h, color_type)
                                                        .is_ok()
                                                    {
                                                        let _ = tx.send(UiMsg::AttachmentAdded(
                                                            png_data,
                                                            "image/png".to_string(),
                                                        ));
                                                        egui_ctx.request_repaint();
                                                    }
                                                }
                                            }
                                        });
                                    }
                                }

                                let scroll_height = ui.available_height();
                                egui::ScrollArea::vertical().max_height(scroll_height).show(
                                    ui,
                                    |ui| {
                                        if !tab.pending_attachments.is_empty() {
                                            ui.spacing_mut().item_spacing.x = 4.0;
                                            let mut remove_idx = None;
                                            for (idx, _att) in
                                                tab.pending_attachments.iter().enumerate()
                                            {
                                                ui.group(|ui| {
                                                    ui.horizontal(|ui| {
                                                        ui.label("📎 Image");
                                                        if ui.small_button("✖").clicked() {
                                                            remove_idx = Some(idx);
                                                        }
                                                    });
                                                });
                                            }
                                            if let Some(idx) = remove_idx {
                                                tab.pending_attachments.remove(idx);
                                            }
                                        }
                                    },
                                );
                            });
                        });

                        // Remaining area: center (prompt) + right (actions)
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            let panel_height = ui.available_height();
                            let side_width = 200.0;
                            let side_size = egui::vec2(side_width, panel_height);

                            // Right column: actions panel
                            ui.allocate_ui(side_size, |ui| {
                                ui.vertical(|ui| {
                                    if streaming {
                                        if ui
                                            .add_enabled(has_session, egui::Button::new("Stop"))
                                            .clicked()
                                        {
                                            let sid_clone = tab.session_id.clone();

                                            if let (Some(client), Some(sid)) =
                                                (&self.client, sid_clone)
                                            {
                                                Self::cancel_active_response(tab);
                                                let c = client.clone();
                                                let sid_for_abort = sid.clone();
                                                if let Some(rt) = &self.runtime {
                                                    rt.spawn(async move {
                                                        let _ =
                                                            c.abort_session(&sid_for_abort).await;
                                                        tokio::time::sleep(
                                                            std::time::Duration::from_millis(200),
                                                        )
                                                        .await;
                                                        let _ =
                                                            c.abort_session(&sid_for_abort).await;
                                                    });
                                                }
                                            }
                                        }
                                    }

                                    let send_enabled = has_session
                                        && !blocked
                                        && !streaming
                                        && (!tab.input.trim().is_empty()
                                            || !tab.pending_attachments.is_empty());
                                    if ui
                                        .add_enabled(send_enabled, egui::Button::new("Send"))
                                        .clicked()
                                    {
                                        if let (Some(client), Some(sid)) =
                                            (&self.client, &tab.session_id)
                                        {
                                            tab.suppress_incoming = false;
                                            tab.last_send_at = match SystemTime::now()
                                                .duration_since(UNIX_EPOCH)
                                            {
                                                Ok(dur) => dur.as_millis() as i64,
                                                Err(_) => 0,
                                            };

                                            let text = tab.input.clone();
                                            let model = tab.selected_model.clone();
                                            let agent = tab
                                                .selected_agent
                                                .clone()
                                                .unwrap_or_else(|| self.default_agent.clone());
                                            tab.input.clear();
                                            let mut parts = Vec::new();
                                            if !text.is_empty() {
                                                parts.push(
                                                    crate::types::models::MessagePart::Text {
                                                        text,
                                                    },
                                                );
                                            }
                                            for att in &tab.pending_attachments {
                                                let b64 = base64::engine::general_purpose::STANDARD
                                                    .encode(&att.data);
                                                parts.push(
                                                    crate::types::models::MessagePart::File {
                                                        mime: att.mime.clone(),
                                                        filename: None,
                                                        url: format!(
                                                            "data:{};base64,{}",
                                                            att.mime, b64
                                                        ),
                                                    },
                                                );
                                            }
                                            tab.pending_attachments.clear();
                                            let c = client.clone();
                                            let sid = sid.clone();
                                            if let Some(rt) = &self.runtime {
                                                rt.spawn(async move {
                                                    let _ = c
                                                        .send_message(
                                                            &sid,
                                                            parts,
                                                            model,
                                                            Some(agent),
                                                        )
                                                        .await;
                                                });
                                            }
                                        }
                                    }

                                    if !has_session {
                                        ui.small("(Wait...)");
                                    }
                                    if has_session && streaming {
                                        ui.small("Stop to cancel response");
                                    }
                                    if has_session && !blocked && !streaming {
                                        if self.audio_tx.is_some() {
                                            ui.small("⌘+Enter\nAltRight: Record");
                                        } else {
                                            ui.small("⌘+Enter");
                                        }
                                    }
                                });
                            });

                            // Center column: prompt input
                            let center_height = ui.available_height();
                            let center_width = ui.available_width();
                            let text_height = center_height;
                            let row_height = ui.text_style_height(&egui::TextStyle::Body);
                            let rows = (text_height / row_height).floor().max(3.0) as usize;

                            egui::ScrollArea::vertical()
                                .max_height(text_height)
                                .show(ui, |ui| {
                                    let _response = ui.add_enabled(
                                        has_session && !blocked,
                                        egui::TextEdit::multiline(&mut tab.input)
                                            .desired_width(center_width)
                                            .desired_rows(rows),
                                    );

                                    // Send on Cmd+Enter (macOS)
                                    let send_key = ui.input(|i| {
                                        i.modifiers.command && i.key_pressed(egui::Key::Enter)
                                    });

                                    let send_enabled = has_session
                                        && !blocked
                                        && !streaming
                                        && (!tab.input.trim().is_empty()
                                            || !tab.pending_attachments.is_empty());
                                    if send_key && send_enabled {
                                        if let (Some(client), Some(sid)) =
                                            (&self.client, &tab.session_id)
                                        {
                                            tab.suppress_incoming = false;
                                            tab.last_send_at = match SystemTime::now()
                                                .duration_since(UNIX_EPOCH)
                                            {
                                                Ok(dur) => dur.as_millis() as i64,
                                                Err(_) => 0,
                                            };

                                            let text = tab.input.clone();
                                            let model = tab.selected_model.clone();
                                            let agent = tab
                                                .selected_agent
                                                .clone()
                                                .unwrap_or_else(|| self.default_agent.clone());
                                            tab.input.clear();
                                            let mut parts = Vec::new();
                                            if !text.is_empty() {
                                                parts.push(
                                                    crate::types::models::MessagePart::Text {
                                                        text,
                                                    },
                                                );
                                            }
                                            for att in &tab.pending_attachments {
                                                let b64 = base64::engine::general_purpose::STANDARD
                                                    .encode(&att.data);
                                                parts.push(
                                                    crate::types::models::MessagePart::File {
                                                        mime: att.mime.clone(),
                                                        filename: None,
                                                        url: format!(
                                                            "data:{};base64,{}",
                                                            att.mime, b64
                                                        ),
                                                    },
                                                );
                                            }
                                            tab.pending_attachments.clear();
                                            let c = client.clone();
                                            let sid = sid.clone();
                                            if let Some(rt) = &self.runtime {
                                                rt.spawn(async move {
                                                    let _ = c
                                                        .send_message(
                                                            &sid,
                                                            parts,
                                                            model,
                                                            Some(agent),
                                                        )
                                                        .await;
                                                });
                                            }
                                        }
                                    }
                                });
                        });
                    });
                }
            });

        // Center: Chat UI with messages
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical(|ui| {
                // Messages area
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if self.tabs.is_empty() {
                            ui.centered_and_justified(|ui| {
                                ui.label("Click + to create a new session");
                            });
                        } else if let Some(tab) = self.tabs.get(self.active) {
                            let spacing = self.config.ui.chat_density.message_spacing();
                            let (session_id_opt, messages_copy) =
                                (tab.session_id.clone(), tab.messages.clone());
                            let _ = tab;
                            for msg in &messages_copy {
                                self.render_message(ui, msg, session_id_opt.as_deref());
                                ui.add_space(spacing);
                            }
                        }
                    });
            });
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Shutdown audio task first to unblock the recv loop
        if let Some(tx) = &self.audio_tx {
            let _ = tx.send(AudioCmd::Shutdown);
        }

        // Stop server if owned
        if let Some(s) = &self.server {
            if s.owned {
                let _ = stop_pid(s.pid);
            }
        }
    }
}

async fn run_audio_task(
    audio_rx: mpsc::Receiver<AudioCmd>,
    ui_tx: mpsc::Sender<UiMsg>,
    model_path: std::path::PathBuf,
    egui_ctx: egui::Context,
) {
    use crate::audio::AudioManager;

    // Initialize AudioManager
    let mut audio_mgr = match AudioManager::new(&model_path) {
        Ok(mgr) => mgr,
        Err(e) => {
            let _ = ui_tx.send(UiMsg::AudioError(format!(
                "Failed to initialize audio: {}",
                e
            )));
            egui_ctx.request_repaint();
            return;
        }
    };

    // Listen for audio commands
    loop {
        match audio_rx.recv() {
            Ok(AudioCmd::StartRecording) => match audio_mgr.start_recording() {
                Ok(_) => {
                    let _ = ui_tx.send(UiMsg::RecordingStarted);
                    egui_ctx.request_repaint();
                }
                Err(e) => {
                    let _ = ui_tx.send(UiMsg::AudioError(e.to_string()));
                    egui_ctx.request_repaint();
                }
            },
            Ok(AudioCmd::StopRecording) => {
                let _ = ui_tx.send(UiMsg::RecordingStopped);
                egui_ctx.request_repaint();

                // Stop recording, resample, and transcribe
                // This blocks but runs in dedicated audio task, not UI thread
                match audio_mgr.stop_recording() {
                    Ok(text) => {
                        let _ = ui_tx.send(UiMsg::Transcription(text));
                        egui_ctx.request_repaint();
                    }
                    Err(e) => {
                        let _ = ui_tx.send(UiMsg::AudioError(e.to_string()));
                        egui_ctx.request_repaint();
                    }
                }
            }
            Ok(AudioCmd::Shutdown) => {
                // Clean shutdown - exit task loop
                break;
            }
            Err(_) => break, // Channel closed
        }
    }
}
