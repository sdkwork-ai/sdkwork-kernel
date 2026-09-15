use sdkwork_agent_kernel::{
    AgentMessage, AgentMessageRole, AgentPart, AgentSession, KernelError, KernelResult,
    ModelDescriptor, ModelProvider, ModelRequest, ModelResponse, ModelResponseFormat,
    ModelStreamChunk, ProviderHealth, ProviderManifest, SessionKind, SessionSource,
    SideEffectLevel, ToolCall, ToolDescriptor, ToolProvider, ToolResult, ToolSchema,
};
use sdkwork_agent_provider_core::{
    create_session_from_config, finalize_provider_session_snapshot, uuid_simple, MessageAdapter,
    SessionAdapter, SessionConfig,
};

// ============================================================================
// MiMo Code Message Types
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MiMoCodeMessage {
    pub role: String,
    pub content: String,
    pub context_chain: Option<Vec<String>>,
    pub tool_calls: Option<Vec<MiMoCodeToolCall>>,
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MiMoCodeToolCall {
    pub id: String,
    pub function_name: String,
    pub arguments: String,
}

// ============================================================================
// MiMo Code Session Adapter (existing, preserved)
// ============================================================================

#[derive(Debug, Clone)]
pub struct MiMoCodeSessionTime {
    pub created: Option<String>,
    pub updated: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MiMoCodeSession {
    pub id: String,
    pub parent_id: Option<String>,
    pub slug: Option<String>,
    pub title: Option<String>,
    pub directory: Option<String>,
    pub time: MiMoCodeSessionTime,
    pub context_from: Option<String>,
    pub context_watermark: Option<String>,
    pub summary: Option<String>,
    pub goal: Option<String>,
    pub origin: Option<String>,
    pub model: Option<String>,
}

pub struct MiMoCodeAdapter;

impl MiMoCodeAdapter {
    pub fn new() -> Self {
        Self
    }

    fn map_source(origin: Option<&str>) -> SessionSource {
        match origin {
            Some(o) => match o.to_lowercase().as_str() {
                "web" => SessionSource::Web,
                "cli" => SessionSource::Cli,
                _ => SessionSource::Cli,
            },
            None => SessionSource::Cli,
        }
    }
}

impl Default for MiMoCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionAdapter for MiMoCodeAdapter {
    type ExternalSession = MiMoCodeSession;

    fn to_agent_session(&self, external: &Self::ExternalSession) -> KernelResult<AgentSession> {
        let source = Self::map_source(external.origin.as_deref());
        let kind = if external.parent_id.is_some() {
            SessionKind::Subagent
        } else {
            SessionKind::Main
        };

        let mut config = SessionConfig::new().with_source(source).with_kind(kind);

        if let Some(ref model) = external.model {
            config = config.with_model(model);
        }
        if let Some(ref title) = external.title {
            config = config.with_title(title);
        }
        if let Some(ref directory) = external.directory {
            config = config.with_cwd(directory);
        }

        let mut session = create_session_from_config(
            &external.id,
            None,
            None,
            None,
            config,
            external.time.created.as_deref().unwrap_or(""),
        );

        session.parent_session_id = external.parent_id.clone();
        session.slug = external.slug.clone();
        session.updated_at = external.time.updated.clone();
        session.context_from = external.context_from.clone();
        session.context_watermark = external.context_watermark.clone();
        session.summary = external.summary.clone();
        session.goal = external.goal.clone();

        finalize_provider_session_snapshot("mimo-code", session)
    }
}

// ============================================================================
// MiMo Code Message Adapter
// ============================================================================

pub struct MiMoCodeMessageAdapter;

impl MiMoCodeMessageAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MiMoCodeMessageAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageAdapter for MiMoCodeMessageAdapter {
    type ExternalMessage = MiMoCodeMessage;

    fn to_agent_message(&self, external: &Self::ExternalMessage) -> KernelResult<AgentMessage> {
        let role = match external.role.as_str() {
            "user" => AgentMessageRole::User,
            "assistant" => AgentMessageRole::Agent,
            "system" => AgentMessageRole::System,
            "tool" => AgentMessageRole::Tool,
            _ => AgentMessageRole::Adapter,
        };

        let mut parts = Vec::new();

        if let Some(chain) = &external.context_chain {
            for (i, ctx) in chain.iter().enumerate() {
                parts.push(
                    AgentPart::text(format!("mimo.context.{}", i), ctx)
                        .with_metadata("mimo-code.content_type", "context"),
                );
            }
        }

        if !external.content.is_empty() {
            parts.push(AgentPart::text("mimo.content", &external.content));
        }

        if let Some(tool_calls) = &external.tool_calls {
            for tc in tool_calls {
                let mut part =
                    AgentPart::tool_call_ref(format!("mimo.tool_call.{}", tc.id), &tc.id);
                part.name = Some(tc.function_name.clone());
                part.json = Some(tc.arguments.clone());
                parts.push(part);
            }
        }

        if parts.is_empty() {
            parts.push(AgentPart::text("mimo.empty", ""));
        }

        let mut message = AgentMessage::new(format!("mimo.msg.{}", uuid_simple()), role, parts);

        if external.context_chain.is_some() {
            message = message.with_metadata("mimo-code.has_context_chain", "true");
        }
        if let Some(tool_call_id) = &external.tool_call_id {
            message = message.with_metadata("mimo-code.tool_call_id", tool_call_id);
        }

        Ok(message)
    }
}

// ============================================================================
// MiMo Code Model Provider
// ============================================================================

pub struct MiMoCodeModelProvider {
    default_model: String,
}

impl MiMoCodeModelProvider {
    pub fn new() -> Self {
        Self {
            default_model: "mimo-v2.5-pro".to_string(),
        }
    }

    pub fn with_default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        self
    }
}

impl Default for MiMoCodeModelProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelProvider for MiMoCodeModelProvider {
    fn provider_manifest(&self) -> ProviderManifest {
        ProviderManifest::new(
            "provider.mimo",
            "model",
            "MiMo Model Provider",
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
        vec![
            ModelDescriptor::new("mimo-v2.5-pro", "provider.mimo", "MiMo v2.5 Pro", "mimo")
                .with_version("2.5")
                .with_capability("chat")
                .with_capability("tool_call")
                .with_context_window_tokens(200000)
                .with_max_output_tokens(32000)
                .with_input_mode("text")
                .with_output_mode("text")
                .with_response_format(ModelResponseFormat::Text)
                .with_response_format(ModelResponseFormat::Json)
                .with_tool_capability("function_calling"),
            ModelDescriptor::new(
                "mimo-v2.5-flash",
                "provider.mimo",
                "MiMo v2.5 Flash",
                "mimo",
            )
            .with_version("2.5")
            .with_capability("chat")
            .with_capability("tool_call")
            .with_context_window_tokens(128000)
            .with_max_output_tokens(16000)
            .with_input_mode("text")
            .with_output_mode("text")
            .with_response_format(ModelResponseFormat::Text)
            .with_tool_capability("function_calling"),
        ]
    }

    fn invoke(&self, _request: ModelRequest) -> KernelResult<ModelResponse> {
        sdkwork_agent_provider_core::reject_in_process_model_invoke("provider.mimo")
    }

    fn stream(&self, _request: ModelRequest) -> KernelResult<Vec<ModelStreamChunk>> {
        sdkwork_agent_provider_core::reject_in_process_model_stream("provider.mimo")
    }
}

// ============================================================================
// MiMo Code Tool Provider
// ============================================================================

pub struct MiMoCodeToolProvider;

impl MiMoCodeToolProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MiMoCodeToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolProvider for MiMoCodeToolProvider {
    fn provider_manifest(&self) -> ProviderManifest {
        ProviderManifest::new(
            "provider.tool.mimo",
            "tool",
            "MiMo Tool Provider",
            "0.1.0",
            vec!["tool.invoke".to_string()],
        )
    }

    fn health(&self) -> ProviderHealth {
        ProviderHealth::available()
    }

    fn list_tools(&self) -> Vec<ToolDescriptor> {
        vec![
            ToolDescriptor::new(
                "mimo.code_edit",
                "provider.tool.mimo",
                "Code Edit",
                SideEffectLevel::SideEffectful,
            )
            .with_name("code_edit")
            .with_description("Edit code files")
            .with_input_schema(ToolSchema::json_schema("mimo.code_edit.input"))
            .with_output_schema(ToolSchema::json_schema("mimo.code_edit.output")),
            ToolDescriptor::new(
                "mimo.terminal",
                "provider.tool.mimo",
                "Terminal",
                SideEffectLevel::SideEffectful,
            )
            .with_name("terminal")
            .with_description("Execute terminal commands")
            .with_input_schema(ToolSchema::json_schema("mimo.terminal.input"))
            .with_output_schema(ToolSchema::json_schema("mimo.terminal.output")),
            ToolDescriptor::new(
                "mimo.search",
                "provider.tool.mimo",
                "Search",
                SideEffectLevel::ReadOnly,
            )
            .with_name("search")
            .with_description("Search code")
            .with_input_schema(ToolSchema::json_schema("mimo.search.input"))
            .with_output_schema(ToolSchema::json_schema("mimo.search.output")),
            ToolDescriptor::new(
                "mimo.analyze",
                "provider.tool.mimo",
                "Analyze",
                SideEffectLevel::ReadOnly,
            )
            .with_name("analyze")
            .with_description("Analyze code quality")
            .with_input_schema(ToolSchema::json_schema("mimo.analyze.input"))
            .with_output_schema(ToolSchema::json_schema("mimo.analyze.output")),
        ]
    }

    fn invoke_tool(&self, call: ToolCall) -> KernelResult<ToolResult> {
        match call.tool_id.as_str() {
            "mimo.code_edit" | "mimo.terminal" | "mimo.search" | "mimo.analyze" => {
                sdkwork_agent_provider_core::reject_in_process_tool_invoke("provider.tool.mimo")
            }
            _ => Err(KernelError::CapabilityMissing {
                capability_id: call.tool_id.clone(),
            }),
        }
    }
}

// ============================================================================
// MiMo Code Lifecycle Provider (existing, preserved)
// ============================================================================

sdkwork_agent_provider_core::define_provider_lifecycle_provider!(
    MiMoCodeLifecycleProvider,
    "mimo-code"
);

mod agent_definition;
mod conformance;
pub mod ids;
mod installer;
mod manifest;
mod package;
mod sdk_integration;

pub use agent_definition::{mimo_code_agent_definition, mimo_code_agent_manifest};
pub use configuration::MiMoCodeConfigurationProvider;
pub use conformance::mimo_code_conformance_profile;
pub use installer::{mimo_code_agent_installer, MIMO_CODE_SDK_PACKAGE, MIMO_CODE_SDK_VERSION};
pub use manifest::{
    mimo_code_kernel_plugin_manifest, mimo_code_provider_manifests, MiMoCodeKernelPlugin,
};
pub use package::mimo_code_package_manifest;
pub use sdk_integration::{mimo_code_binding_manifest, MiMoCodeSdkIntegration};

// ============================================================================`r`n// Tests
// ============================================================================

#[cfg(test)] // WORKSPACE-PATH:allow-fixture-block: this module is the file's #[cfg(test)] unit-test fixture data
mod tests {
    use super::*;
    use sdkwork_agent_kernel::SessionState;
    use sdkwork_agent_provider_core::{
        ConversationManager, InMemoryConversationManager, SessionLifecycleProvider,
    };

    fn sample_mimo_session() -> MiMoCodeSession {
        MiMoCodeSession {
            id: "mimo.session.1".to_string(),
            parent_id: None,
            slug: Some("fix-auth-bug".to_string()),
            title: Some("Fix Authentication Bug".to_string()),
            directory: Some("/home/user/project".to_string()),
            time: MiMoCodeSessionTime {
                created: Some("2026-01-01T00:00:00Z".to_string()),
                updated: Some("2026-01-01T05:00:00Z".to_string()),
            },
            context_from: Some("context.1".to_string()),
            context_watermark: Some("wm.1".to_string()),
            summary: Some("Fixed login flow".to_string()),
            goal: Some("Fix auth bug".to_string()),
            origin: None,
            model: Some("mimo-v2".to_string()),
        }
    }

    // --- Session Adapter Tests ---

    #[test]
    fn maps_basic_fields() {
        let adapter = MiMoCodeAdapter::new();
        let ext = sample_mimo_session();
        let session = adapter.to_agent_session(&ext).unwrap();

        assert_eq!(session.session_id, "mimo.session.1");
        assert_eq!(session.source, SessionSource::Cli);
        assert_eq!(session.kind, SessionKind::Main);
        assert_eq!(session.slug, Some("fix-auth-bug".to_string()));
        assert_eq!(session.title, Some("Fix Authentication Bug".to_string()));
        assert_eq!(session.cwd, Some("/home/user/project".to_string()));
        assert_eq!(session.created_at, Some("2026-01-01T00:00:00Z".to_string()));
        assert_eq!(session.updated_at, Some("2026-01-01T05:00:00Z".to_string()));
        assert_eq!(session.context_from, Some("context.1".to_string()));
        assert_eq!(session.context_watermark, Some("wm.1".to_string()));
        assert_eq!(session.summary, Some("Fixed login flow".to_string()));
        assert_eq!(session.goal, Some("Fix auth bug".to_string()));
        assert_eq!(session.model, Some("mimo-v2".to_string()));
    }

    #[test]
    fn maps_subagent_when_parent_exists() {
        let adapter = MiMoCodeAdapter::new();
        let mut ext = sample_mimo_session();
        ext.parent_id = Some("parent.1".to_string());
        let session = adapter.to_agent_session(&ext).unwrap();

        assert_eq!(session.kind, SessionKind::Subagent);
        assert_eq!(session.parent_session_id, Some("parent.1".to_string()));
    }

    #[test]
    fn maps_web_origin_to_web_source() {
        let adapter = MiMoCodeAdapter::new();
        let mut ext = sample_mimo_session();
        ext.origin = Some("web".to_string());
        let session = adapter.to_agent_session(&ext).unwrap();
        assert_eq!(session.source, SessionSource::Web);
    }

    #[test]
    fn maps_cli_origin_to_cli_source() {
        let adapter = MiMoCodeAdapter::new();
        let mut ext = sample_mimo_session();
        ext.origin = Some("cli".to_string());
        let session = adapter.to_agent_session(&ext).unwrap();
        assert_eq!(session.source, SessionSource::Cli);
    }

    #[test]
    fn defaults_to_cli_when_no_origin() {
        let adapter = MiMoCodeAdapter::new();
        let ext = sample_mimo_session();
        let session = adapter.to_agent_session(&ext).unwrap();
        assert_eq!(session.source, SessionSource::Cli);
    }

    #[test]
    fn lifecycle_provider_full_cycle() {
        let provider = MiMoCodeLifecycleProvider::new();
        let config = SessionConfig::new()
            .with_source(SessionSource::Cli)
            .with_kind(SessionKind::Main);
        let created = provider
            .create_session("agent.1", Some("user.1"), config)
            .unwrap();

        let resumed = provider.resume_session(&created.session_id).unwrap();
        assert_eq!(resumed.state, SessionState::Active);

        let active = provider.list_active_sessions().unwrap();
        assert_eq!(active.len(), 1);

        let closed = provider.close_session(&created.session_id).unwrap();
        assert_eq!(closed.state, SessionState::Closed);
    }

    // --- Message Adapter Tests ---

    #[test]
    fn converts_user_message() {
        let adapter = MiMoCodeMessageAdapter::new();
        let msg = MiMoCodeMessage {
            role: "user".to_string(),
            content: "Fix this bug".to_string(),
            context_chain: None,
            tool_calls: None,
            tool_call_id: None,
        };
        let result = adapter.to_agent_message(&msg).unwrap();
        assert_eq!(result.role, AgentMessageRole::User);
        assert_eq!(result.parts.len(), 1);
        assert_eq!(result.parts[0].text, Some("Fix this bug".to_string()));
    }

    #[test]
    fn converts_assistant_with_context_chain() {
        let adapter = MiMoCodeMessageAdapter::new();
        let msg = MiMoCodeMessage {
            role: "assistant".to_string(),
            content: "Here is the fix".to_string(),
            context_chain: Some(vec![
                "Previous context 1".to_string(),
                "Previous context 2".to_string(),
            ]),
            tool_calls: None,
            tool_call_id: None,
        };
        let result = adapter.to_agent_message(&msg).unwrap();
        assert_eq!(result.role, AgentMessageRole::Agent);
        assert_eq!(result.parts.len(), 3);
        assert_eq!(
            result.parts[0].metadata_value("mimo-code.content_type"),
            Some("context")
        );
        assert_eq!(
            result.parts[1].metadata_value("mimo-code.content_type"),
            Some("context")
        );
        assert_eq!(result.parts[2].text, Some("Here is the fix".to_string()));
        assert_eq!(
            result.metadata_value("mimo-code.has_context_chain"),
            Some("true")
        );
    }

    #[test]
    fn converts_tool_calls() {
        let adapter = MiMoCodeMessageAdapter::new();
        let msg = MiMoCodeMessage {
            role: "assistant".to_string(),
            content: String::new(),
            context_chain: None,
            tool_calls: Some(vec![MiMoCodeToolCall {
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
        let adapter = MiMoCodeMessageAdapter::new();
        let messages = vec![
            MiMoCodeMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
                context_chain: None,
                tool_calls: None,
                tool_call_id: None,
            },
            MiMoCodeMessage {
                role: "assistant".to_string(),
                content: "Hi".to_string(),
                context_chain: None,
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        let result = adapter.to_agent_messages(&messages).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, AgentMessageRole::User);
        assert_eq!(result[1].role, AgentMessageRole::Agent);
    }

    #[test]
    fn converts_system_message() {
        let adapter = MiMoCodeMessageAdapter::new();
        let msg = MiMoCodeMessage {
            role: "system".to_string(),
            content: "You are a coding assistant".to_string(),
            context_chain: None,
            tool_calls: None,
            tool_call_id: None,
        };
        let result = adapter.to_agent_message(&msg).unwrap();
        assert_eq!(result.role, AgentMessageRole::System);
    }

    // --- Model Provider Tests ---

    #[test]
    fn model_provider_manifest() {
        let provider = MiMoCodeModelProvider::new();
        let manifest = provider.provider_manifest();
        assert_eq!(manifest.provider_id, "provider.mimo");
        assert_eq!(manifest.provider_family, "model");
    }

    #[test]
    fn model_provider_health() {
        let provider = MiMoCodeModelProvider::new();
        assert_eq!(provider.health().status, "available");
    }

    #[test]
    fn model_provider_list_models() {
        let provider = MiMoCodeModelProvider::new();
        let models = provider.list_models();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model_id, "mimo-v2.5-pro");
        assert_eq!(models[1].model_id, "mimo-v2.5-flash");
    }

    #[test]
    fn model_provider_describe_model() {
        let provider = MiMoCodeModelProvider::new();
        let model = provider.describe_model("mimo-v2.5-pro").unwrap();
        assert_eq!(model.display_name, "MiMo v2.5 Pro");
        assert_eq!(model.family, "mimo");
        assert!(model.supports_capability("chat"));
        assert!(model.supports_capability("tool_call"));
    }

    #[test]
    fn model_provider_invoke_requires_transport_worker() {
        let provider = MiMoCodeModelProvider::new();
        let request = ModelRequest::new("req.1", vec!["Explain lifetimes".to_string()])
            .with_model_id("mimo-v2.5-pro");
        let error = provider
            .invoke(request)
            .expect_err("in-process model invocation is forbidden");
        assert!(matches!(error, KernelError::ProviderUnavailable { .. }));
    }

    #[test]
    fn model_provider_stream_requires_transport_worker() {
        let provider = MiMoCodeModelProvider::new();
        let request = ModelRequest::new("req.2", vec!["Hello".to_string()]);
        let error = provider
            .stream(request)
            .expect_err("in-process model streaming is forbidden");
        assert!(matches!(error, KernelError::ProviderUnavailable { .. }));
    }

    // --- Tool Provider Tests ---

    #[test]
    fn tool_provider_manifest() {
        let provider = MiMoCodeToolProvider::new();
        let manifest = provider.provider_manifest();
        assert_eq!(manifest.provider_id, "provider.tool.mimo");
        assert_eq!(manifest.provider_family, "tool");
    }

    #[test]
    fn tool_provider_list_tools() {
        let provider = MiMoCodeToolProvider::new();
        let tools = provider.list_tools();
        assert_eq!(tools.len(), 4);
        assert!(tools.iter().any(|t| t.tool_id == "mimo.code_edit"));
        assert!(tools.iter().any(|t| t.tool_id == "mimo.terminal"));
        assert!(tools.iter().any(|t| t.tool_id == "mimo.search"));
        assert!(tools.iter().any(|t| t.tool_id == "mimo.analyze"));

        let analyze = tools.iter().find(|t| t.tool_id == "mimo.analyze").unwrap();
        assert_eq!(analyze.side_effect_level, SideEffectLevel::ReadOnly);
    }

    #[test]
    fn tool_provider_invoke_analyze_requires_transport_worker() {
        let provider = MiMoCodeToolProvider::new();
        let call = ToolCall::new("c.1", "mimo.analyze", r#"{"file":"src/main.rs"}"#);
        let error = provider
            .invoke_tool(call)
            .expect_err("in-process tool invocation is forbidden");
        assert!(matches!(error, KernelError::ProviderUnavailable { .. }));
    }

    #[test]
    fn tool_provider_invoke_unknown() {
        let provider = MiMoCodeToolProvider::new();
        let call = ToolCall::new("c.2", "mimo.nonexistent", "{}");
        assert!(provider.invoke_tool(call).is_err());
    }

    #[test]
    fn tool_provider_describe_tool() {
        let provider = MiMoCodeToolProvider::new();
        let desc = provider.describe_tool("mimo.code_edit").unwrap();
        assert_eq!(desc.display_name, "Code Edit");
        assert_eq!(desc.side_effect_level, SideEffectLevel::SideEffectful);
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
        assert_eq!(manager.current_turn("s1").unwrap(), 0);
        manager.begin_turn("s1").unwrap();
        assert_eq!(manager.current_turn("s1").unwrap(), 1);
        manager.begin_turn("s1").unwrap();
        assert_eq!(manager.current_turn("s1").unwrap(), 2);
    }

    #[test]
    fn conversation_manager_clear_history() {
        let mut manager = InMemoryConversationManager::new();
        manager
            .append_message(
                "s1",
                AgentMessage::new(
                    "m1",
                    AgentMessageRole::User,
                    vec![AgentPart::text("p1", "Hello")],
                ),
            )
            .unwrap();
        manager.begin_turn("s1").unwrap();
        manager.clear_history("s1").unwrap();
        assert!(manager.get_history("s1").unwrap().is_empty());
        assert_eq!(manager.current_turn("s1").unwrap(), 0);
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
mod configuration;
mod materializer;
pub use materializer::mimo_code_settings_path;
