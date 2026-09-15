use sdkwork_agent_kernel::{
    AgentMessage, AgentMessageRole, AgentPart, AgentSession, KernelResult, ModelDescriptor,
    ModelProvider, ModelRequest, ModelResponse, ModelResponseFormat, ModelStreamChunk,
    ProviderHealth, ProviderManifest, SessionActivityEvidenceKind, SessionActivitySnapshot,
    SessionActivityState, SessionKind, SessionSource,
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
    OpenCodeConfigurationProvider, OPENCODE_ALLOW_ALL_ACCESS_MODE_ID,
    OPENCODE_ALLOW_EDITS_ACCESS_MODE_ID, OPENCODE_ASK_ACCESS_MODE_ID,
};
pub use local_plugins::OpenCodeLocalPluginProvider;

#[cfg(test)]
use sdkwork_agent_kernel::KernelError;
#[cfg(test)]
use sdkwork_agent_provider_core::{
    ConversationManager, InMemoryConversationManager, SessionLifecycleProvider,
};

// ============================================================================
// OpenCode Message Types
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OpenCodeMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Option<Vec<OpenCodeToolCall>>,
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OpenCodeToolCall {
    pub id: String,
    pub function_name: String,
    pub arguments: String,
}

// ============================================================================
// OpenCode Session Adapter (existing, preserved)
// ============================================================================

#[derive(Debug, Clone)]
pub struct OpenCodeSession {
    pub id: String,
    pub parent_session_id: Option<String>,
    pub title: Option<String>,
    pub message_count: u32,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cost_cents: Option<u64>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeSessionStatus {
    Idle,
    Busy,
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeActivityObservation {
    pub provider_session_id: String,
    pub status: OpenCodeSessionStatus,
    pub observed_at: String,
}

pub struct OpenCodeAdapter;

impl OpenCodeAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OpenCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionAdapter for OpenCodeAdapter {
    type ExternalSession = OpenCodeSession;

    fn to_agent_session(&self, external: &Self::ExternalSession) -> KernelResult<AgentSession> {
        let kind = if external.parent_session_id.is_some() {
            SessionKind::Subagent
        } else {
            SessionKind::Main
        };

        let mut config = SessionConfig::new()
            .with_source(SessionSource::Cli)
            .with_kind(kind);

        if let Some(ref model) = external.model {
            config = config.with_model(model);
        }
        if let Some(ref title) = external.title {
            config = config.with_title(title);
        }
        if let Some(ref cwd) = external.cwd {
            config = config.with_cwd(cwd);
        }

        let mut session = create_session_from_config(
            &external.id,
            None,
            None,
            None,
            config,
            external.created_at.as_deref().unwrap_or(""),
        );

        session.parent_session_id = external.parent_session_id.clone();
        session.updated_at = external.updated_at.clone();
        session.message_count = external.message_count;
        session.token_usage.input_tokens = external.prompt_tokens;
        session.token_usage.output_tokens = external.completion_tokens;
        session.token_usage.total_tokens = external
            .prompt_tokens
            .saturating_add(external.completion_tokens);
        session.cost_cents = external.cost_cents;

        finalize_provider_session_snapshot("opencode", session)
    }
}

impl ProviderSessionActivityAdapter for OpenCodeAdapter {
    type ExternalActivity = OpenCodeActivityObservation;

    fn to_session_activity(
        &self,
        external: &Self::ExternalActivity,
    ) -> KernelResult<SessionActivitySnapshot> {
        let state = match external.status {
            OpenCodeSessionStatus::Idle => SessionActivityState::Idle,
            OpenCodeSessionStatus::Busy | OpenCodeSessionStatus::Retry => {
                SessionActivityState::Working
            }
        };
        session_activity_from_provider_observation(
            &external.provider_session_id,
            state,
            SessionActivityEvidenceKind::ProviderEvent,
            None,
            &external.observed_at,
            DEFAULT_PROVIDER_SESSION_ACTIVITY_TTL,
        )
    }
}

// ============================================================================
// OpenCode Message Adapter
// ============================================================================

pub struct OpenCodeMessageAdapter;

impl OpenCodeMessageAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OpenCodeMessageAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageAdapter for OpenCodeMessageAdapter {
    type ExternalMessage = OpenCodeMessage;

    fn to_agent_message(&self, external: &Self::ExternalMessage) -> KernelResult<AgentMessage> {
        let role = match external.role.as_str() {
            "user" => AgentMessageRole::User,
            "assistant" => AgentMessageRole::Agent,
            "system" => AgentMessageRole::System,
            "tool" => AgentMessageRole::Tool,
            _ => AgentMessageRole::Adapter,
        };

        let mut parts = Vec::new();

        if !external.content.is_empty() {
            parts.push(AgentPart::text("opencode.content", &external.content));
        }

        if let Some(tool_calls) = &external.tool_calls {
            for tc in tool_calls {
                let mut part =
                    AgentPart::tool_call_ref(format!("opencode.tool_call.{}", tc.id), &tc.id);
                part.name = Some(tc.function_name.clone());
                part.json = Some(tc.arguments.clone());
                parts.push(part);
            }
        }

        if parts.is_empty() {
            parts.push(AgentPart::text("opencode.empty", ""));
        }

        let mut message = AgentMessage::new(format!("opencode.msg.{}", uuid_simple()), role, parts);

        if let Some(tool_call_id) = &external.tool_call_id {
            message = message.with_metadata("opencode.tool_call_id", tool_call_id);
        }

        Ok(message)
    }
}

// ============================================================================
// OpenCode Model Provider
// ============================================================================

/// Reads the model id configured for the local opencode installation.
///
/// Precedence: the `OPENCODE_MODEL` environment override, then the top-level
/// `model` key in the opencode config file (`OPENCODE_CONFIG`, else
/// `~/.config/opencode/opencode.json`). Returns `None` when the configuration
/// cannot be read so the model catalog can fall back to the built-in model.
pub fn configured_opencode_model_id() -> Option<String> {
    if let Some(model) = std::env::var("OPENCODE_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Some(model);
    }
    let config_path = opencode_config_path()?;
    let content = std::fs::read_to_string(config_path).ok()?;
    let document: serde_json::Value = content.parse().ok()?;
    document
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}


pub struct OpenCodeModelProvider {
    default_model: String,
}

impl OpenCodeModelProvider {
    pub fn new() -> Self {
        Self {
            default_model: configured_opencode_model_id()
                .unwrap_or_else(|| "opencode-default".to_string()),
        }
    }

    pub fn with_default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        self
    }
}

impl Default for OpenCodeModelProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelProvider for OpenCodeModelProvider {
    fn provider_manifest(&self) -> ProviderManifest {
        ProviderManifest::new(
            "provider.opencode",
            "model",
            "OpenCode Model Provider",
            "0.1.0",
            vec![
                "model.chat".to_string(),
                "model.stream".to_string(),
                "model.tool_call".to_string(),
            ],
        )
    }

    fn health(&self) -> ProviderHealth {
        ProviderHealth::available()
    }

    fn list_models(&self) -> Vec<ModelDescriptor> {
        let mut models = vec![ModelDescriptor::new(
            "opencode-default",
            "provider.opencode",
            "OpenCode Default",
            "opencode",
        )
        .with_version("1.0")
        .with_capability("chat")
        .with_capability("tool_call")
        .with_context_window_tokens(128000)
        .with_max_output_tokens(8192)
        .with_input_mode("text")
        .with_output_mode("text")
        .with_response_format(ModelResponseFormat::Text)
        .with_tool_capability("function_calling")];
        // Surface the model configured for the local opencode installation
        // first so default model selection matches what the opencode CLI would
        // use. The configured model uses the `provider/model` form and may not
        // be part of the built-in catalog.
        if self.default_model != "opencode-default"
            && !models
                .iter()
                .any(|model| model.model_id == self.default_model)
        {
            models.insert(
                0,
                ModelDescriptor::new(
                    &self.default_model,
                    "provider.opencode",
                    &self.default_model,
                    "opencode",
                )
                .with_capability("chat")
                .with_capability("tool_call")
                .with_context_window_tokens(128000)
                .with_max_output_tokens(8192)
                .with_input_mode("text")
                .with_output_mode("text")
                .with_response_format(ModelResponseFormat::Text)
                .with_tool_capability("function_calling"),
            );
        }
        models
    }

    fn invoke(&self, _request: ModelRequest) -> KernelResult<ModelResponse> {
        sdkwork_agent_provider_core::reject_in_process_model_invoke("provider.opencode")
    }

    fn stream(&self, _request: ModelRequest) -> KernelResult<Vec<ModelStreamChunk>> {
        sdkwork_agent_provider_core::reject_in_process_model_stream("provider.opencode")
    }
}

// ============================================================================
// OpenCode Lifecycle Provider (existing, preserved)
// ============================================================================

sdkwork_agent_provider_core::define_provider_lifecycle_provider!(
    OpenCodeLifecycleProvider,
    "opencode"
);

mod agent_definition;
mod conformance;
pub mod ids;
mod installer;
mod manifest;
mod package;
pub mod sdk_integration;

pub use agent_definition::{opencode_agent_definition, opencode_agent_manifest};
pub use conformance::opencode_conformance_profile;
pub use installer::{opencode_agent_installer, OPENCODE_SDK_PACKAGE, OPENCODE_SDK_VERSION};
pub use manifest::{
    opencode_kernel_plugin_manifest, opencode_provider_manifests, OpenCodeKernelPlugin,
};
pub use package::opencode_package_manifest;
pub use sdk_integration::{opencode_binding_manifest, OpenCodeSdkIntegration};

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)] // WORKSPACE-PATH:allow-fixture-block: this module is the file's #[cfg(test)] unit-test fixture data
mod tests {
    use super::*;

    fn sample_opencode_session() -> OpenCodeSession {
        OpenCodeSession {
            id: "oc.session.1".to_string(),
            parent_session_id: None,
            title: Some("Debug Session".to_string()),
            message_count: 15,
            prompt_tokens: 3000,
            completion_tokens: 1200,
            cost_cents: Some(42),
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
            updated_at: Some("2026-01-01T03:00:00Z".to_string()),
            model: Some("deepseek-v3".to_string()),
            cwd: Some("/home/user/code".to_string()),
        }
    }

    #[test]
    fn session_status_events_distinguish_busy_retry_and_idle() {
        let adapter = OpenCodeAdapter::new();
        let observed_at = sdkwork_agent_provider_core::now_iso();
        for status in [OpenCodeSessionStatus::Busy, OpenCodeSessionStatus::Retry] {
            let activity = adapter
                .to_session_activity(&OpenCodeActivityObservation {
                    provider_session_id: "oc.session.1".to_string(),
                    status,
                    observed_at: observed_at.clone(),
                })
                .expect("working activity");
            assert_eq!(activity.state, Some(SessionActivityState::Working));
            assert!(activity.is_authoritative());
        }
        let idle = adapter
            .to_session_activity(&OpenCodeActivityObservation {
                provider_session_id: "oc.session.1".to_string(),
                status: OpenCodeSessionStatus::Idle,
                observed_at,
            })
            .expect("idle activity");
        assert_eq!(idle.state, Some(SessionActivityState::Idle));
    }

    // --- Session Adapter Tests ---

    #[test]
    fn maps_basic_fields() {
        let adapter = OpenCodeAdapter::new();
        let ext = sample_opencode_session();
        let session = adapter.to_agent_session(&ext).unwrap();

        assert_eq!(session.session_id, "oc.session.1");
        assert_eq!(session.source, SessionSource::Cli);
        assert_eq!(session.kind, SessionKind::Main);
        assert_eq!(session.title, Some("Debug Session".to_string()));
        assert_eq!(session.message_count, 15);
        assert_eq!(session.token_usage.input_tokens, 3000);
        assert_eq!(session.token_usage.output_tokens, 1200);
        assert_eq!(session.token_usage.total_tokens, 4200);
        assert_eq!(session.cost_cents, Some(42));
        assert_eq!(session.created_at, Some("2026-01-01T00:00:00Z".to_string()));
        assert_eq!(session.updated_at, Some("2026-01-01T03:00:00Z".to_string()));
        assert_eq!(session.model, Some("deepseek-v3".to_string()));
        assert_eq!(session.cwd, Some("/home/user/code".to_string()));
    }

    #[test]
    fn maps_subagent_when_parent_exists() {
        let adapter = OpenCodeAdapter::new();
        let mut ext = sample_opencode_session();
        ext.parent_session_id = Some("parent.1".to_string());
        let session = adapter.to_agent_session(&ext).unwrap();

        assert_eq!(session.kind, SessionKind::Subagent);
        assert_eq!(session.parent_session_id, Some("parent.1".to_string()));
    }

    #[test]
    fn lifecycle_provider_create_and_list() {
        let provider = OpenCodeLifecycleProvider::new();
        let config = SessionConfig::new()
            .with_source(SessionSource::Cli)
            .with_kind(SessionKind::Main);
        let session = provider
            .create_session("agent.1", Some("user.1"), config)
            .unwrap();
        assert_eq!(session.agent_id, Some("agent.1".to_string()));

        provider.resume_session(&session.session_id).unwrap();
        let active = provider.list_active_sessions().unwrap();
        assert_eq!(active.len(), 1);
    }

    // --- Message Adapter Tests ---

    #[test]
    fn converts_user_message() {
        let adapter = OpenCodeMessageAdapter::new();
        let msg = OpenCodeMessage {
            role: "user".to_string(),
            content: "Debug this function".to_string(),
            tool_calls: None,
            tool_call_id: None,
        };
        let result = adapter.to_agent_message(&msg).unwrap();
        assert_eq!(result.role, AgentMessageRole::User);
        assert_eq!(result.parts.len(), 1);
        assert_eq!(
            result.parts[0].text,
            Some("Debug this function".to_string())
        );
    }

    #[test]
    fn converts_assistant_message() {
        let adapter = OpenCodeMessageAdapter::new();
        let msg = OpenCodeMessage {
            role: "assistant".to_string(),
            content: "Found the issue".to_string(),
            tool_calls: None,
            tool_call_id: None,
        };
        let result = adapter.to_agent_message(&msg).unwrap();
        assert_eq!(result.role, AgentMessageRole::Agent);
    }

    #[test]
    fn converts_tool_calls() {
        let adapter = OpenCodeMessageAdapter::new();
        let msg = OpenCodeMessage {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![OpenCodeToolCall {
                id: "tc.1".to_string(),
                function_name: "code_edit".to_string(),
                arguments: r#"{"file":"src/main.rs"}"#.to_string(),
            }]),
            tool_call_id: None,
        };
        let result = adapter.to_agent_message(&msg).unwrap();
        assert_eq!(result.parts.len(), 1);
        assert_eq!(
            result.parts[0].kind,
            sdkwork_agent_kernel::AgentPartKind::ToolCallRef
        );
        assert_eq!(result.parts[0].name, Some("code_edit".to_string()));
    }

    #[test]
    fn converts_batch_messages() {
        let adapter = OpenCodeMessageAdapter::new();
        let messages = vec![
            OpenCodeMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
            OpenCodeMessage {
                role: "assistant".to_string(),
                content: "Hi".to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        let result = adapter.to_agent_messages(&messages).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn converts_system_message() {
        let adapter = OpenCodeMessageAdapter::new();
        let msg = OpenCodeMessage {
            role: "system".to_string(),
            content: "You are a coding assistant".to_string(),
            tool_calls: None,
            tool_call_id: None,
        };
        let result = adapter.to_agent_message(&msg).unwrap();
        assert_eq!(result.role, AgentMessageRole::System);
    }

    // --- Model Provider Tests ---

    #[test]
    fn model_provider_manifest() {
        let provider = OpenCodeModelProvider::new();
        let manifest = provider.provider_manifest();
        assert_eq!(manifest.provider_id, "provider.opencode");
        assert_eq!(manifest.provider_family, "model");
    }

    #[test]
    fn model_provider_health() {
        let provider = OpenCodeModelProvider::new();
        assert_eq!(provider.health().status, "available");
    }

    #[test]
    fn model_provider_list_models() {
        let provider = OpenCodeModelProvider::new();
        let models = provider.list_models();
        assert!(models.len() >= 1);
        if let Some(configured) = configured_opencode_model_id() {
            assert_eq!(models[0].model_id, configured);
        } else {
            assert_eq!(models[0].model_id, "opencode-default");
        }
        assert!(models
            .iter()
            .any(|model| model.model_id == "opencode-default"));
    }

    #[test]
    fn model_provider_invoke_requires_transport_worker() {
        let provider = OpenCodeModelProvider::new();
        let request = ModelRequest::new("req.1", vec!["Explain async".to_string()])
            .with_model_id("opencode-default");
        let error = provider
            .invoke(request)
            .expect_err("in-process model invocation is forbidden");
        assert!(matches!(error, KernelError::ProviderUnavailable { .. }));
    }

    #[test]
    fn model_provider_stream_requires_transport_worker() {
        let provider = OpenCodeModelProvider::new();
        let request = ModelRequest::new("req.2", vec!["Hello".to_string()]);
        let error = provider
            .stream(request)
            .expect_err("in-process model streaming is forbidden");
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
    }

    #[test]
    fn conversation_manager_turn_tracking() {
        let mut manager = InMemoryConversationManager::new();
        assert_eq!(manager.current_turn("s1").unwrap(), 0);
        manager.begin_turn("s1").unwrap();
        assert_eq!(manager.current_turn("s1").unwrap(), 1);
        manager.begin_turn("s1").unwrap();
        assert_eq!(manager.current_turn("s1").unwrap(), 2);
    }

    #[test]
    fn conversation_manager_compress() {
        let mut manager = InMemoryConversationManager::new();
        for i in 0..10 {
            manager
                .append_message(
                    "s1",
                    AgentMessage::new(
                        format!("m.{}", i),
                        AgentMessageRole::User,
                        vec![AgentPart::text(
                            format!("p.{}", i),
                            format!("Message {}", i),
                        )],
                    ),
                )
                .unwrap();
        }
        let compressed = manager.compress_history("s1", 100).unwrap();
        assert_eq!(compressed.role, AgentMessageRole::System);
        assert!(compressed.parts[0]
            .text
            .as_ref()
            .unwrap()
            .contains("Compressed"));
        let remaining = manager.get_history("s1").unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].role, AgentMessageRole::System);
    }
}
pub use materializer::opencode_config_path;
