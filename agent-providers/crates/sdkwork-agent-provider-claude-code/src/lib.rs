use sdkwork_agent_kernel::{
    AgentMessage, AgentMessageRole, AgentPart, AgentSession, KernelResult, ModelDescriptor,
    ModelProvider, ModelRequest, ModelResponse, ModelResponseFormat, ModelStreamChunk,
    ProviderHealth, ProviderManifest, SessionActivityEvidenceKind, SessionActivityInteractionHint,
    SessionActivitySnapshot, SessionActivityState, SessionKind, SessionSource,
};
use sdkwork_agent_provider_core::{
    create_session_from_config, finalize_provider_session_snapshot,
    session_activity_from_provider_observation, uuid_simple, MessageAdapter,
    ProviderSessionActivityAdapter, SessionAdapter, SessionConfig,
    DEFAULT_PROVIDER_SESSION_ACTIVITY_TTL,
};

mod configuration;
mod local_plugins;
mod materializer;

pub use configuration::{
    ClaudeCodeConfigurationProvider, CLAUDE_ACCEPT_EDITS_ACCESS_MODE_ID,
    CLAUDE_BYPASS_PERMISSIONS_ACCESS_MODE_ID, CLAUDE_DEFAULT_ACCESS_MODE_ID,
};
pub use local_plugins::ClaudeCodeLocalPluginProvider;

#[cfg(test)]
use sdkwork_agent_kernel::KernelError;
#[cfg(test)]
use sdkwork_agent_kernel::SessionState;
#[cfg(test)]
use sdkwork_agent_provider_core::{
    ConversationManager, InMemoryConversationManager, SessionLifecycleProvider,
};

// ============================================================================
// Claude Message Types
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClaudeMessage {
    pub role: String,
    pub content: Vec<ClaudeContent>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum ClaudeContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
}

// ============================================================================
// Claude Code Session Adapter (existing, preserved)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeCodeAgentType {
    Main,
    Subagent,
    Task,
}

#[derive(Debug, Clone)]
pub struct ClaudeCodeProcessState {
    pub session_id: String,
    pub agent_type: String,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeCodeHookEventKind {
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    PermissionRequest,
    Stop,
    SessionEnd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCodeActivityObservation {
    pub provider_session_id: String,
    pub event: ClaudeCodeHookEventKind,
    pub observed_at: String,
}

pub struct ClaudeCodeAdapter;

impl ClaudeCodeAdapter {
    pub fn new() -> Self {
        Self
    }

    fn map_agent_type(agent_type: &str) -> SessionKind {
        match agent_type.to_lowercase().as_str() {
            "main" => SessionKind::Main,
            "subagent" => SessionKind::Subagent,
            "task" => SessionKind::Task,
            _ => SessionKind::Main,
        }
    }
}

impl Default for ClaudeCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionAdapter for ClaudeCodeAdapter {
    type ExternalSession = ClaudeCodeProcessState;

    fn to_agent_session(&self, external: &Self::ExternalSession) -> KernelResult<AgentSession> {
        let kind = Self::map_agent_type(&external.agent_type);

        let mut config = SessionConfig::new()
            .with_source(SessionSource::Cli)
            .with_kind(kind);

        if let Some(ref model) = external.model {
            config = config.with_model(model);
        }
        if let Some(ref cwd) = external.cwd {
            config = config.with_cwd(cwd);
        }
        if let Some(ref title) = external.title {
            config = config.with_title(title);
        }

        let session = create_session_from_config(
            &external.session_id,
            None,
            None,
            None,
            config,
            external.created_at.as_deref().unwrap_or(""),
        );

        let mut session = session;
        session.updated_at = external.updated_at.clone();

        finalize_provider_session_snapshot("claude-code", session)
    }
}

impl ProviderSessionActivityAdapter for ClaudeCodeAdapter {
    type ExternalActivity = ClaudeCodeActivityObservation;

    fn to_session_activity(
        &self,
        external: &Self::ExternalActivity,
    ) -> KernelResult<SessionActivitySnapshot> {
        let (state, interaction_hint) = match external.event {
            ClaudeCodeHookEventKind::UserPromptSubmit
            | ClaudeCodeHookEventKind::PreToolUse
            | ClaudeCodeHookEventKind::PostToolUse => (SessionActivityState::Working, None),
            ClaudeCodeHookEventKind::PermissionRequest => (
                SessionActivityState::Waiting,
                Some(SessionActivityInteractionHint::ApprovalRequired),
            ),
            ClaudeCodeHookEventKind::Stop | ClaudeCodeHookEventKind::SessionEnd => {
                (SessionActivityState::Idle, None)
            }
        };
        session_activity_from_provider_observation(
            &external.provider_session_id,
            state,
            SessionActivityEvidenceKind::ProviderEvent,
            interaction_hint,
            &external.observed_at,
            DEFAULT_PROVIDER_SESSION_ACTIVITY_TTL,
        )
    }
}

// ============================================================================
// Claude Message Adapter
// ============================================================================

pub struct ClaudeMessageAdapter;

impl ClaudeMessageAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ClaudeMessageAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageAdapter for ClaudeMessageAdapter {
    type ExternalMessage = ClaudeMessage;

    fn to_agent_message(&self, external: &Self::ExternalMessage) -> KernelResult<AgentMessage> {
        let role = match external.role.as_str() {
            "user" => AgentMessageRole::User,
            "assistant" => AgentMessageRole::Agent,
            "system" => AgentMessageRole::System,
            "tool" => AgentMessageRole::Tool,
            _ => AgentMessageRole::Adapter,
        };

        let mut parts = Vec::new();
        let mut has_thinking = false;

        for (i, content) in external.content.iter().enumerate() {
            match content {
                ClaudeContent::Text { text } => {
                    if !text.is_empty() {
                        parts.push(AgentPart::text(format!("claude.text.{}", i), text));
                    }
                }
                ClaudeContent::ToolUse { id, name, input } => {
                    let mut part = AgentPart::tool_call_ref(format!("claude.tool_use.{}", i), id);
                    part.name = Some(name.clone());
                    part.json = Some(input.to_string());
                    parts.push(part);
                }
                ClaudeContent::ToolResult {
                    tool_use_id,
                    content,
                } => {
                    let mut part = AgentPart::text(format!("claude.tool_result.{}", i), content);
                    part.tool_call_id = Some(tool_use_id.clone());
                    part.kind = sdkwork_agent_kernel::AgentPartKind::ToolCallRef;
                    parts.push(part);
                }
                ClaudeContent::Thinking { thinking } => {
                    has_thinking = true;
                    parts.push(
                        AgentPart::text(format!("claude.thinking.{}", i), thinking)
                            .with_metadata("claude.content_type", "thinking"),
                    );
                }
            }
        }

        if parts.is_empty() {
            parts.push(AgentPart::text("claude.empty", ""));
        }

        let mut message = AgentMessage::new(format!("claude.msg.{}", uuid_simple()), role, parts);

        if has_thinking {
            message = message.with_metadata("claude.has_thinking", "true");
        }

        Ok(message)
    }
}

// ============================================================================
// Claude Model Provider
// ============================================================================

/// Reads the model id configured for the local Claude Code installation.
///
/// Precedence: the `ANTHROPIC_MODEL` environment override, then the `env`
/// section of the Claude Code settings file (`~/.claude/settings.json`).
/// Returns `None` when the configuration cannot be read so the model catalog
/// can fall back to the built-in Claude model list.
pub fn configured_claude_code_model_id() -> Option<String> {
    if let Some(model) = std::env::var("ANTHROPIC_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Some(model);
    }
    let home = sdkwork_agent_provider_core::provider_user_home()?;
    let settings_path = home.join(".claude").join("settings.json");
    let content = std::fs::read_to_string(settings_path).ok()?;
    let document: serde_json::Value = content.parse().ok()?;
    document
        .get("env")
        .and_then(serde_json::Value::as_object)
        .and_then(|env| env.get("ANTHROPIC_MODEL"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub struct ClaudeModelProvider {
    default_model: String,
}

impl ClaudeModelProvider {
    pub fn new() -> Self {
        Self {
            default_model: configured_claude_code_model_id()
                .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string()),
        }
    }

    pub fn with_default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        self
    }
}

impl Default for ClaudeModelProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelProvider for ClaudeModelProvider {
    fn provider_manifest(&self) -> ProviderManifest {
        ProviderManifest::new(
            "provider.claude-code",
            "model",
            "Claude Model Provider",
            "0.1.0",
            vec![
                "model.chat".to_string(),
                "model.stream".to_string(),
                "model.tool_call".to_string(),
                "model.thinking".to_string(),
            ],
        )
    }

    fn health(&self) -> ProviderHealth {
        ProviderHealth::available()
    }

    fn list_models(&self) -> Vec<ModelDescriptor> {
        let mut models = vec![
            ModelDescriptor::new(
                "claude-opus-4-20250514",
                "provider.claude-code",
                "Claude Opus 4",
                "claude",
            )
            .with_version("4.0")
            .with_capability("chat")
            .with_capability("tool_call")
            .with_capability("thinking")
            .with_context_window_tokens(200000)
            .with_max_output_tokens(32000)
            .with_input_mode("text")
            .with_output_mode("text")
            .with_response_format(ModelResponseFormat::Text)
            .with_response_format(ModelResponseFormat::Json)
            .with_tool_capability("function_calling")
            .with_tool_capability("computer_use"),
            ModelDescriptor::new(
                "claude-sonnet-4-20250514",
                "provider.claude-code",
                "Claude Sonnet 4",
                "claude",
            )
            .with_version("4.0")
            .with_capability("chat")
            .with_capability("tool_call")
            .with_capability("thinking")
            .with_context_window_tokens(200000)
            .with_max_output_tokens(16000)
            .with_input_mode("text")
            .with_output_mode("text")
            .with_response_format(ModelResponseFormat::Text)
            .with_response_format(ModelResponseFormat::Json)
            .with_tool_capability("function_calling"),
            ModelDescriptor::new(
                "claude-haiku-3-5-20241022",
                "provider.claude-code",
                "Claude 3.5 Haiku",
                "claude",
            )
            .with_version("3.5")
            .with_capability("chat")
            .with_capability("tool_call")
            .with_context_window_tokens(200000)
            .with_max_output_tokens(8192)
            .with_input_mode("text")
            .with_output_mode("text")
            .with_response_format(ModelResponseFormat::Text)
            .with_tool_capability("function_calling"),
        ];
        // Surface the model configured for the local Claude Code installation
        // (settings.json env or ANTHROPIC_MODEL) first so default model
        // selection matches what the Claude Code CLI would use. The configured
        // model may be provider-specific and not part of the built-in catalog.
        if !models
            .iter()
            .any(|model| model.model_id == self.default_model)
        {
            models.insert(
                0,
                ModelDescriptor::new(
                    &self.default_model,
                    "provider.claude-code",
                    &self.default_model,
                    "claude",
                )
                .with_capability("chat")
                .with_capability("tool_call")
                .with_capability("thinking")
                .with_context_window_tokens(200000)
                .with_max_output_tokens(32000)
                .with_input_mode("text")
                .with_output_mode("text")
                .with_response_format(ModelResponseFormat::Text)
                .with_response_format(ModelResponseFormat::Json)
                .with_tool_capability("function_calling"),
            );
        }
        models
    }

    fn invoke(&self, _request: ModelRequest) -> KernelResult<ModelResponse> {
        sdkwork_agent_provider_core::reject_in_process_model_invoke("provider.claude-code")
    }

    fn stream(&self, _request: ModelRequest) -> KernelResult<Vec<ModelStreamChunk>> {
        sdkwork_agent_provider_core::reject_in_process_model_stream("provider.claude-code")
    }
}

// ============================================================================
// Claude Code Lifecycle Provider (existing, preserved)
// ============================================================================

sdkwork_agent_provider_core::define_provider_lifecycle_provider!(
    ClaudeCodeLifecycleProvider,
    "claude-code"
);

mod agent_definition;
mod conformance;
pub mod ids;
mod installer;
mod manifest;
mod package;
pub mod sdk_integration;

pub use agent_definition::{claude_code_agent_definition, claude_code_agent_manifest};
pub use conformance::claude_code_conformance_profile;
pub use installer::{
    claude_code_agent_installer, ANTHROPIC_SDK_PACKAGE, ANTHROPIC_SDK_VERSION,
    CLAUDE_AGENT_SDK_PACKAGE, CLAUDE_AGENT_SDK_VERSION, MCP_SDK_PACKAGE, MCP_SDK_VERSION,
    ZOD_PACKAGE, ZOD_VERSION,
};
pub use manifest::{
    claude_code_kernel_plugin_manifest, claude_code_provider_manifests, ClaudeCodeKernelPlugin,
};
pub use package::claude_code_package_manifest;
pub use sdk_integration::{claude_code_binding_manifest, ClaudeCodeSdkIntegration};

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)] // WORKSPACE-PATH:allow-fixture-block: this module is the file's #[cfg(test)] unit-test fixture data
mod tests {
    use super::*;

    fn sample_claude_code_state() -> ClaudeCodeProcessState {
        ClaudeCodeProcessState {
            session_id: "cc.session.1".to_string(),
            agent_type: "main".to_string(),
            model: Some("claude-sonnet-4-20250514".to_string()),
            cwd: Some("/home/user/project".to_string()),
            title: Some("Coding Session".to_string()),
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
            updated_at: Some("2026-01-01T01:00:00Z".to_string()),
        }
    }

    #[test]
    fn hook_events_project_fresh_activity_without_history_heuristics() {
        let adapter = ClaudeCodeAdapter::new();
        let waiting = adapter
            .to_session_activity(&ClaudeCodeActivityObservation {
                provider_session_id: "cc.session.1".to_string(),
                event: ClaudeCodeHookEventKind::PermissionRequest,
                observed_at: sdkwork_agent_provider_core::now_iso(),
            })
            .expect("waiting activity");
        let stale = adapter
            .to_session_activity(&ClaudeCodeActivityObservation {
                provider_session_id: "cc.session.1".to_string(),
                event: ClaudeCodeHookEventKind::PreToolUse,
                observed_at: "2000-01-01T00:00:00Z".to_string(),
            })
            .expect("stale activity");

        assert_eq!(waiting.state, Some(SessionActivityState::Waiting));
        assert_eq!(
            waiting.interaction_hint,
            Some(SessionActivityInteractionHint::ApprovalRequired)
        );
        assert!(waiting.is_authoritative());
        assert_eq!(
            stale.freshness,
            sdkwork_agent_kernel::SessionActivityFreshness::Stale
        );
        assert_eq!(
            stale.project_lifecycle_state(SessionState::Active),
            SessionState::Created
        );
    }

    // --- Session Adapter Tests ---

    #[test]
    fn maps_basic_fields() {
        let adapter = ClaudeCodeAdapter::new();
        let ext = sample_claude_code_state();
        let session = adapter.to_agent_session(&ext).unwrap();

        assert_eq!(session.session_id, "cc.session.1");
        assert_eq!(session.source, SessionSource::Cli);
        assert_eq!(session.kind, SessionKind::Main);
        assert_eq!(session.model, Some("claude-sonnet-4-20250514".to_string()));
        assert_eq!(session.cwd, Some("/home/user/project".to_string()));
        assert_eq!(session.title, Some("Coding Session".to_string()));
    }

    #[test]
    fn maps_subagent_type() {
        let adapter = ClaudeCodeAdapter::new();
        let mut ext = sample_claude_code_state();
        ext.agent_type = "subagent".to_string();
        let session = adapter.to_agent_session(&ext).unwrap();
        assert_eq!(session.kind, SessionKind::Subagent);
    }

    #[test]
    fn maps_task_type() {
        let adapter = ClaudeCodeAdapter::new();
        let mut ext = sample_claude_code_state();
        ext.agent_type = "task".to_string();
        let session = adapter.to_agent_session(&ext).unwrap();
        assert_eq!(session.kind, SessionKind::Task);
    }

    #[test]
    fn defaults_to_main_for_unknown_type() {
        let adapter = ClaudeCodeAdapter::new();
        let mut ext = sample_claude_code_state();
        ext.agent_type = "something_else".to_string();
        let session = adapter.to_agent_session(&ext).unwrap();
        assert_eq!(session.kind, SessionKind::Main);
    }

    // --- Message Adapter Tests ---

    #[test]
    fn converts_text_message() {
        let adapter = ClaudeMessageAdapter::new();
        let msg = ClaudeMessage {
            role: "user".to_string(),
            content: vec![ClaudeContent::Text {
                text: "Hello Claude".to_string(),
            }],
        };

        let result = adapter.to_agent_message(&msg).unwrap();
        assert_eq!(result.role, AgentMessageRole::User);
        assert_eq!(result.parts.len(), 1);
        assert_eq!(result.parts[0].text, Some("Hello Claude".to_string()));
    }

    #[test]
    fn converts_assistant_message() {
        let adapter = ClaudeMessageAdapter::new();
        let msg = ClaudeMessage {
            role: "assistant".to_string(),
            content: vec![ClaudeContent::Text {
                text: "I can help with that".to_string(),
            }],
        };

        let result = adapter.to_agent_message(&msg).unwrap();
        assert_eq!(result.role, AgentMessageRole::Agent);
    }

    #[test]
    fn converts_tool_use_content() {
        let adapter = ClaudeMessageAdapter::new();
        let msg = ClaudeMessage {
            role: "assistant".to_string(),
            content: vec![ClaudeContent::ToolUse {
                id: "toolu_123".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({"path": "/tmp/test.txt"}),
            }],
        };

        let result = adapter.to_agent_message(&msg).unwrap();
        assert_eq!(result.parts.len(), 1);
        assert_eq!(
            result.parts[0].kind,
            sdkwork_agent_kernel::AgentPartKind::ToolCallRef
        );
        assert_eq!(result.parts[0].tool_call_id, Some("toolu_123".to_string()));
        assert_eq!(result.parts[0].name, Some("Read".to_string()));
        assert!(result.parts[0].json.as_ref().unwrap().contains("test.txt"));
    }

    #[test]
    fn converts_tool_result_content() {
        let adapter = ClaudeMessageAdapter::new();
        let msg = ClaudeMessage {
            role: "user".to_string(),
            content: vec![ClaudeContent::ToolResult {
                tool_use_id: "toolu_123".to_string(),
                content: "file contents here".to_string(),
            }],
        };

        let result = adapter.to_agent_message(&msg).unwrap();
        assert_eq!(result.parts.len(), 1);
        assert_eq!(result.parts[0].tool_call_id, Some("toolu_123".to_string()));
        assert_eq!(result.parts[0].text, Some("file contents here".to_string()));
    }

    #[test]
    fn converts_thinking_content() {
        let adapter = ClaudeMessageAdapter::new();
        let msg = ClaudeMessage {
            role: "assistant".to_string(),
            content: vec![ClaudeContent::Thinking {
                thinking: "Let me think about this...".to_string(),
            }],
        };

        let result = adapter.to_agent_message(&msg).unwrap();
        assert_eq!(result.parts.len(), 1);
        assert_eq!(
            result.parts[0].text,
            Some("Let me think about this...".to_string())
        );
        assert_eq!(
            result.parts[0].metadata_value("claude.content_type"),
            Some("thinking")
        );
        assert_eq!(result.metadata_value("claude.has_thinking"), Some("true"));
    }

    #[test]
    fn converts_mixed_content() {
        let adapter = ClaudeMessageAdapter::new();
        let msg = ClaudeMessage {
            role: "assistant".to_string(),
            content: vec![
                ClaudeContent::Thinking {
                    thinking: "Analyzing the code...".to_string(),
                },
                ClaudeContent::Text {
                    text: "Here is my analysis".to_string(),
                },
                ClaudeContent::ToolUse {
                    id: "toolu_456".to_string(),
                    name: "Bash".to_string(),
                    input: serde_json::json!({"command": "cargo test"}),
                },
            ],
        };

        let result = adapter.to_agent_message(&msg).unwrap();
        assert_eq!(result.role, AgentMessageRole::Agent);
        assert_eq!(result.parts.len(), 3);
        assert_eq!(
            result.parts[0].metadata_value("claude.content_type"),
            Some("thinking")
        );
        assert_eq!(
            result.parts[1].text,
            Some("Here is my analysis".to_string())
        );
        assert_eq!(result.parts[2].name, Some("Bash".to_string()));
        assert_eq!(result.metadata_value("claude.has_thinking"), Some("true"));
    }

    #[test]
    fn converts_batch_messages() {
        let adapter = ClaudeMessageAdapter::new();
        let messages = vec![
            ClaudeMessage {
                role: "user".to_string(),
                content: vec![ClaudeContent::Text {
                    text: "Hello".to_string(),
                }],
            },
            ClaudeMessage {
                role: "assistant".to_string(),
                content: vec![ClaudeContent::Text {
                    text: "Hi there".to_string(),
                }],
            },
        ];

        let result = adapter.to_agent_messages(&messages).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, AgentMessageRole::User);
        assert_eq!(result[1].role, AgentMessageRole::Agent);
    }

    // --- Model Provider Tests ---

    #[test]
    fn model_provider_manifest() {
        let provider = ClaudeModelProvider::new();
        let manifest = provider.provider_manifest();
        assert_eq!(manifest.provider_id, "provider.claude-code");
        assert_eq!(manifest.provider_family, "model");
        assert!(manifest.capabilities.contains(&"model.chat".to_string()));
        assert!(manifest
            .capabilities
            .contains(&"model.thinking".to_string()));
    }

    #[test]
    fn model_provider_health() {
        let provider = ClaudeModelProvider::new();
        assert_eq!(provider.health().status, "available");
    }

    #[test]
    fn model_provider_list_models() {
        let provider = ClaudeModelProvider::new();
        let models = provider.list_models();
        assert!(models.len() >= 3);
        if let Some(configured) = configured_claude_code_model_id() {
            assert_eq!(models[0].model_id, configured);
        } else {
            assert_eq!(models[0].model_id, "claude-opus-4-20250514");
        }
        assert!(models
            .iter()
            .any(|model| model.model_id == "claude-sonnet-4-20250514"));
        assert!(models
            .iter()
            .any(|model| model.model_id == "claude-haiku-3-5-20241022"));
    }

    #[test]
    fn model_provider_describe_model() {
        let provider = ClaudeModelProvider::new();
        let model = provider.describe_model("claude-opus-4-20250514").unwrap();
        assert_eq!(model.display_name, "Claude Opus 4");
        assert_eq!(model.family, "claude");
        assert!(model.supports_capability("chat"));
        assert!(model.supports_capability("tool_call"));
        assert!(model.supports_capability("thinking"));
        assert!(model.supports_response_format(&ModelResponseFormat::Json));
    }

    #[test]
    fn model_provider_invoke_requires_transport_worker() {
        let provider = ClaudeModelProvider::new();
        let request = ModelRequest::new("req.1", vec!["Explain Rust lifetimes".to_string()])
            .with_model_id("claude-sonnet-4-20250514");
        let error = provider
            .invoke(request)
            .expect_err("in-process invoke is forbidden");
        assert!(matches!(error, KernelError::ProviderUnavailable { .. }));
    }

    #[test]
    fn model_provider_stream_requires_transport_worker() {
        let provider = ClaudeModelProvider::new();
        let request = ModelRequest::new("req.2", vec!["Hello".to_string()]);
        let error = provider
            .stream(request)
            .expect_err("in-process stream is forbidden");
        assert!(matches!(error, KernelError::ProviderUnavailable { .. }));
    }

    // --- Conversation Manager Tests ---

    #[test]
    fn conversation_manager_append_and_get() {
        let mut manager = InMemoryConversationManager::new();
        let msg = AgentMessage::new(
            "msg.1",
            AgentMessageRole::User,
            vec![AgentPart::text("p1", "Hello")],
        );

        manager.append_message("session.1", msg).unwrap();
        let history = manager.get_history("session.1").unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].message_id, "msg.1");
    }

    #[test]
    fn conversation_manager_turn_tracking() {
        let mut manager = InMemoryConversationManager::new();

        assert_eq!(manager.current_turn("session.1").unwrap(), 0);

        manager.begin_turn("session.1").unwrap();
        assert_eq!(manager.current_turn("session.1").unwrap(), 1);

        manager.begin_turn("session.1").unwrap();
        assert_eq!(manager.current_turn("session.1").unwrap(), 2);
    }

    #[test]
    fn conversation_manager_clear_history() {
        let mut manager = InMemoryConversationManager::new();
        let msg = AgentMessage::new(
            "msg.1",
            AgentMessageRole::User,
            vec![AgentPart::text("p1", "Hello")],
        );

        manager.append_message("session.1", msg).unwrap();
        manager.begin_turn("session.1").unwrap();

        manager.clear_history("session.1").unwrap();
        assert!(manager.get_history("session.1").unwrap().is_empty());
        assert_eq!(manager.current_turn("session.1").unwrap(), 0);
    }

    #[test]
    fn conversation_manager_compress_history() {
        let mut manager = InMemoryConversationManager::new();

        for i in 0..10 {
            let msg = AgentMessage::new(
                format!("msg.{}", i),
                AgentMessageRole::User,
                vec![AgentPart::text(
                    format!("p.{}", i),
                    format!("Message {}", i),
                )],
            );
            manager.append_message("session.1", msg).unwrap();
        }

        let compressed = manager.compress_history("session.1", 100).unwrap();
        assert_eq!(compressed.role, AgentMessageRole::System);
        assert!(compressed.parts[0]
            .text
            .as_ref()
            .unwrap()
            .contains("Compressed"));

        let remaining = manager.get_history("session.1").unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].role, AgentMessageRole::System);
    }

    // --- Lifecycle Provider Tests ---

    #[test]
    fn lifecycle_provider_create_and_close() {
        let provider = ClaudeCodeLifecycleProvider::new();
        let config = SessionConfig::new()
            .with_source(SessionSource::Cli)
            .with_kind(SessionKind::Main);
        let created = provider.create_session("agent.1", None, config).unwrap();

        let closed = provider.close_session(&created.session_id).unwrap();
        assert_eq!(closed.state, SessionState::Closed);
    }
}
pub use materializer::claude_code_settings_path;
