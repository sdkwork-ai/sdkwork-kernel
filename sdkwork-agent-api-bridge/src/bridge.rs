use crate::{
    BridgeEvent, BridgeEventSeverity, BridgeMessageResponse, BridgeModelResult,
    BridgeSessionConfig, BridgeSnapshot, BridgeToolResult, ContextBridge, EventBridge, ModelBridge,
    SessionBridge, ToolBridge, BRIDGE_IDLE_EVICTION,
};
use sdkwork_agent_kernel::{
    AgentMessage, AgentMessageRole, AgentPart, AgentSession, KernelResult, ModelRequest,
    ModelStreamChunk, ToolCall,
};

const PREPARED_MODEL_REQUEST_ID_METADATA: &str = "sdkwork.model_request_id";
const PREPARED_PROVIDER_SESSION_ID_METADATA: &str = "sdkwork.provider_session_id";

/// Main bridge connecting kernel runtime to business API layer
pub struct AgentRuntimeBridge {
    session_bridge: SessionBridge,
    model_bridge: ModelBridge,
    tool_bridge: ToolBridge,
    context_bridge: ContextBridge,
    event_bridge: EventBridge,
}

impl AgentRuntimeBridge {
    /// Create a new bridge instance
    pub fn new() -> Self {
        Self {
            session_bridge: SessionBridge::new(),
            model_bridge: ModelBridge::new(),
            tool_bridge: ToolBridge::new(),
            context_bridge: ContextBridge::new(),
            event_bridge: EventBridge::new(),
        }
    }

    #[cfg(test)]
    pub fn new_with_mock_fallback() -> Self {
        Self {
            session_bridge: SessionBridge::new(),
            model_bridge: ModelBridge::with_mock_fallback_enabled(),
            tool_bridge: ToolBridge::with_mock_fallback_enabled(),
            context_bridge: ContextBridge::new(),
            event_bridge: EventBridge::new(),
        }
    }

    /// Create a bridge wired to a bootstrapped typed agent runtime.
    pub fn with_agent_runtime(
        agent_runtime: std::sync::Arc<sdkwork_agent_kernel::AgentRuntime>,
        allow_mock_fallback: bool,
    ) -> Self {
        Self {
            session_bridge: SessionBridge::new(),
            model_bridge: ModelBridge::with_agent_runtime(
                agent_runtime.clone(),
                allow_mock_fallback,
            ),
            tool_bridge: ToolBridge::with_agent_runtime(agent_runtime, allow_mock_fallback),
            context_bridge: ContextBridge::new(),
            event_bridge: EventBridge::new(),
        }
    }

    // =========================================================================
    // Session Management
    // =========================================================================

    /// Lazily evict transient bridge state for sessions idle beyond
    /// [`BRIDGE_IDLE_EVICTION`]. Registration is the natural sweep point: it
    /// is the only path that grows per-process session capacity, so abandoned
    /// sessions (clients that never close) are reclaimed exactly when that
    /// capacity could otherwise be exhausted into hard failures.
    fn sweep_idle_bridge_state(&mut self) {
        let evicted = self
            .session_bridge
            .sweep_idle_sessions(BRIDGE_IDLE_EVICTION);
        for session_id in evicted {
            self.event_bridge.clear_events(&session_id);
        }
        self.event_bridge.sweep_idle_sessions(BRIDGE_IDLE_EVICTION);
    }

    /// Register a persisted session in the in-memory bridge runtime.
    pub fn register_session(
        &mut self,
        session_id: &str,
        config: BridgeSessionConfig,
    ) -> KernelResult<AgentSession> {
        self.sweep_idle_bridge_state();
        self.session_bridge.register_session(session_id, config)
    }

    /// Register a persisted session and atomically refresh its bounded history.
    pub fn register_session_with_history(
        &mut self,
        session_id: &str,
        config: BridgeSessionConfig,
        history: Vec<AgentMessage>,
    ) -> KernelResult<AgentSession> {
        self.sweep_idle_bridge_state();
        let previous_session = self.session_bridge.get_session(session_id).ok();
        let session = self.session_bridge.register_session(session_id, config)?;
        if let Err(error) = self
            .session_bridge
            .replace_history_if_newer(session_id, history)
        {
            if let Some(previous_session) = previous_session {
                self.session_bridge.restore_session(previous_session)?;
            } else {
                self.session_bridge.remove_session(session_id);
            }
            return Err(error);
        }
        Ok(session)
    }

    /// Register a persisted session and refresh history with an authoritative
    /// monotonic message-count revision.
    pub fn register_session_with_history_revision(
        &mut self,
        session_id: &str,
        config: BridgeSessionConfig,
        history_revision: u64,
        history: Vec<AgentMessage>,
    ) -> KernelResult<AgentSession> {
        self.sweep_idle_bridge_state();
        let previous_session = self.session_bridge.get_session(session_id).ok();
        let session = self.session_bridge.register_session(session_id, config)?;
        if let Err(error) =
            self.session_bridge
                .replace_history_if_revision(session_id, history_revision, history)
        {
            if let Some(previous_session) = previous_session {
                self.session_bridge.restore_session(previous_session)?;
            } else {
                self.session_bridge.remove_session(session_id);
            }
            return Err(error);
        }
        Ok(session)
    }

    /// Create a new agent session
    pub fn create_session(&mut self, config: BridgeSessionConfig) -> KernelResult<AgentSession> {
        self.sweep_idle_bridge_state();
        self.session_bridge.create_session(config)
    }

    /// Get an existing session by ID
    pub fn get_session(&self, session_id: &str) -> KernelResult<AgentSession> {
        self.session_bridge.get_session(session_id)
    }

    /// List all active sessions
    pub fn list_sessions(&self) -> KernelResult<Vec<AgentSession>> {
        self.session_bridge.list_sessions()
    }

    /// Close a session
    pub fn close_session(&mut self, session_id: &str) -> KernelResult<AgentSession> {
        self.session_bridge.close_session(session_id)
    }

    /// Remove a session and its transient bridge-owned state.
    pub fn remove_session(&mut self, session_id: &str) -> bool {
        self.event_bridge.clear_events(session_id);
        self.session_bridge.remove_session(session_id)
    }

    // =========================================================================
    // Message Handling
    // =========================================================================

    /// Send a user message and get the complete response (including model + tool calls)
    pub fn send_message(
        &mut self,
        session_id: &str,
        content: &str,
    ) -> KernelResult<BridgeMessageResponse> {
        let user_message = AgentMessage::new(
            format!("msg.{}", crate::types::generate_id()),
            AgentMessageRole::User,
            vec![AgentPart::text(
                format!("part.{}", crate::types::generate_id()),
                content,
            )],
        )
        .for_session(session_id)
        .mark_untrusted();
        self.send_user_message(session_id, user_message)
    }

    /// Send a structured user message (multimodal-safe) and return the assistant turn.
    pub fn send_user_message(
        &mut self,
        session_id: &str,
        user_message: AgentMessage,
    ) -> KernelResult<BridgeMessageResponse> {
        let (model_bridge, model_request, provider_id, prepared_user_message) =
            self.prepare_user_message_turn(session_id, user_message)?;
        let model_result = model_bridge.invoke(&model_request, provider_id.as_deref())?;
        self.complete_user_message_turn(session_id, prepared_user_message, model_result)
    }

    /// Append a text user message and build the model request for the turn.
    pub fn prepare_send_message_turn(
        &mut self,
        session_id: &str,
        content: &str,
    ) -> KernelResult<(ModelBridge, ModelRequest, Option<String>, AgentMessage)> {
        let user_message = AgentMessage::new(
            format!("msg.{}", crate::types::generate_id()),
            AgentMessageRole::User,
            vec![AgentPart::text(
                format!("part.{}", crate::types::generate_id()),
                content,
            )],
        );
        self.prepare_user_message_turn(session_id, user_message)
    }

    /// Append a structured user message and build the model request for the turn.
    pub fn prepare_user_message_turn(
        &mut self,
        session_id: &str,
        user_message: AgentMessage,
    ) -> KernelResult<(ModelBridge, ModelRequest, Option<String>, AgentMessage)> {
        if user_message.role != AgentMessageRole::User {
            return Err(sdkwork_agent_kernel::KernelError::validation(
                "user message turn requires the user role",
            ));
        }
        let user_message = if user_message.session_id.is_none() {
            user_message.for_session(session_id)
        } else {
            user_message
        }
        .mark_untrusted();
        user_message.validate()?;
        let session = self.session_bridge.get_session(session_id)?;
        self.session_bridge
            .validate_message(session_id, &user_message)?;

        let context = self.context_bridge.collect_context(session_id)?;
        let mut history = self.session_bridge.get_history(session_id)?;
        history.push(user_message.clone());
        let model_request = self
            .model_bridge
            .build_request(session_id, &session, &history, &context, None);
        let mut user_message = user_message.with_metadata(
            PREPARED_MODEL_REQUEST_ID_METADATA,
            model_request.model_request_id.clone(),
        );
        if let Some(provider_session_id) = model_request.provider_session_id.as_deref() {
            user_message = user_message
                .with_metadata(PREPARED_PROVIDER_SESSION_ID_METADATA, provider_session_id);
        }

        let provider_id = session
            .metadata_value("modelProvider")
            .map(std::string::ToString::to_string);
        Ok((
            self.model_bridge.clone(),
            model_request,
            provider_id,
            user_message,
        ))
    }

    /// Append the assistant response and record bridge events for a prepared turn.
    pub fn complete_user_message_turn(
        &mut self,
        session_id: &str,
        user_message: AgentMessage,
        model_result: BridgeModelResult,
    ) -> KernelResult<BridgeMessageResponse> {
        let expected_request_id = user_message
            .metadata_value(PREPARED_MODEL_REQUEST_ID_METADATA)
            .ok_or_else(|| {
                sdkwork_agent_kernel::KernelError::validation(
                    "prepared turn is missing its model request id",
                )
            })?;
        self.model_bridge
            .validate_model_response_for_request(expected_request_id, &model_result.response)?;
        validate_provider_session_completion(
            user_message.metadata_value(PREPARED_PROVIDER_SESSION_ID_METADATA),
            model_result.provider_session_id.as_deref(),
        )?;
        let user_payload_len = sdkwork_agent_kernel::flatten_message_to_text(&user_message).len();
        let assistant_text = model_result.response.messages.join("");
        let assistant_message = AgentMessage::new(
            format!("msg.{}", crate::types::generate_id()),
            AgentMessageRole::Agent,
            vec![AgentPart::text(
                format!("part.{}", crate::types::generate_id()),
                assistant_text,
            )],
        )
        .for_session(session_id);

        self.session_bridge.append_completed_turn(
            session_id,
            vec![user_message, assistant_message.clone()],
            model_result.provider_session_id.as_deref(),
        )?;

        let mut events = model_result.events.clone();
        events.push(BridgeEvent {
            event_type: "agent.message.user".to_string(),
            session_id: Some(session_id.to_string()),
            task_id: None,
            payload: format!("content_length={user_payload_len}"),
            severity: BridgeEventSeverity::Info,
        });
        events.push(BridgeEvent {
            event_type: "agent.message.assistant".to_string(),
            session_id: Some(session_id.to_string()),
            task_id: None,
            payload: format!(
                "content_length={}",
                model_result
                    .response
                    .messages
                    .iter()
                    .map(String::len)
                    .sum::<usize>()
            ),
            severity: BridgeEventSeverity::Info,
        });

        self.event_bridge.record_events(&events);

        Ok(BridgeMessageResponse {
            session_id: session_id.to_string(),
            provider_session_id: model_result.provider_session_id,
            message: assistant_message,
            model_response: Some(model_result.response),
            tool_results: Vec::new(),
            events,
        })
    }

    /// Stream a user message turn and return ordered output chunks plus the assistant message id.
    pub fn stream_message(
        &mut self,
        session_id: &str,
        content: &str,
        model_override: Option<&str>,
    ) -> KernelResult<(String, Vec<sdkwork_agent_kernel::ModelStreamChunk>)> {
        let (model_bridge, model_request, provider_id, prepared_user_message) =
            self.prepare_stream_message_turn(session_id, content, model_override)?;
        let chunks = model_bridge.stream(&model_request, provider_id.as_deref())?;
        self.complete_stream_message_turn(session_id, prepared_user_message, chunks)
    }

    /// Append a user message and build the stream request for the turn.
    pub fn prepare_stream_message_turn(
        &mut self,
        session_id: &str,
        content: &str,
        model_override: Option<&str>,
    ) -> KernelResult<(ModelBridge, ModelRequest, Option<String>, AgentMessage)> {
        let mut session = self.session_bridge.get_session(session_id)?;
        if let Some(model_id) = model_override {
            session.model = Some(model_id.to_string());
        }

        let user_message = AgentMessage::new(
            format!("msg.{}", crate::types::generate_id()),
            AgentMessageRole::User,
            vec![AgentPart::text(
                format!("part.{}", crate::types::generate_id()),
                content,
            )],
        )
        .for_session(session_id)
        .mark_untrusted();

        self.session_bridge
            .validate_message(session_id, &user_message)?;

        let context = self.context_bridge.collect_context(session_id)?;
        let mut history = self.session_bridge.get_history(session_id)?;
        history.push(user_message.clone());
        let model_request = self
            .model_bridge
            .build_request(session_id, &session, &history, &context, None);
        let mut user_message = user_message.with_metadata(
            PREPARED_MODEL_REQUEST_ID_METADATA,
            model_request.model_request_id.clone(),
        );
        if let Some(provider_session_id) = model_request.provider_session_id.as_deref() {
            user_message = user_message
                .with_metadata(PREPARED_PROVIDER_SESSION_ID_METADATA, provider_session_id);
        }

        let provider_id = session
            .metadata_value("modelProvider")
            .map(std::string::ToString::to_string);
        Ok((
            self.model_bridge.clone(),
            model_request,
            provider_id,
            user_message,
        ))
    }

    /// Append the streamed assistant response and record bridge stream events.
    pub fn complete_stream_message_turn(
        &mut self,
        session_id: &str,
        user_message: AgentMessage,
        chunks: Vec<ModelStreamChunk>,
    ) -> KernelResult<(String, Vec<ModelStreamChunk>)> {
        let expected_request_id = user_message
            .metadata_value(PREPARED_MODEL_REQUEST_ID_METADATA)
            .ok_or_else(|| {
                sdkwork_agent_kernel::KernelError::validation(
                    "prepared turn is missing its model request id",
                )
            })?;
        let assistant_text = crate::model_bridge::collect_model_stream_output_for_request(
            &chunks,
            expected_request_id,
        )?;
        let user_payload_len = sdkwork_agent_kernel::flatten_message_to_text(&user_message).len();
        let assistant_message_id = format!("msg.{}", crate::types::generate_id());
        let assistant_message = AgentMessage::new(
            assistant_message_id.clone(),
            AgentMessageRole::Agent,
            vec![AgentPart::text(
                format!("part.{}", crate::types::generate_id()),
                assistant_text.clone(),
            )],
        )
        .for_session(session_id);

        self.session_bridge
            .append_messages(session_id, vec![user_message, assistant_message])?;

        let mut events = vec![
            BridgeEvent {
                event_type: "agent.message.user".to_string(),
                session_id: Some(session_id.to_string()),
                task_id: None,
                payload: format!("content_length={user_payload_len}"),
                severity: BridgeEventSeverity::Info,
            },
            BridgeEvent {
                event_type: "agent.message.assistant".to_string(),
                session_id: Some(session_id.to_string()),
                task_id: None,
                payload: format!("content_length={}", assistant_text.len()),
                severity: BridgeEventSeverity::Info,
            },
        ];
        for chunk in &chunks {
            events.push(BridgeEvent {
                event_type: "agent.model.output.streamed".to_string(),
                session_id: Some(session_id.to_string()),
                task_id: None,
                payload: format!(
                    "model_request_id={};sequence={}",
                    chunk.model_request_id, chunk.sequence
                ),
                severity: BridgeEventSeverity::Info,
            });
        }
        self.event_bridge.record_events(&events);

        Ok((assistant_message_id, chunks))
    }

    /// Get message history for a session
    pub fn get_messages(&self, session_id: &str) -> KernelResult<Vec<AgentMessage>> {
        self.session_bridge.get_history(session_id)
    }

    // =========================================================================
    // Model Invocation
    // =========================================================================

    /// Invoke the model directly (without conversation context)
    pub fn invoke_model(&self, request: ModelRequest) -> KernelResult<BridgeModelResult> {
        self.model_bridge.invoke(&request, None)
    }

    /// Invoke the model using persisted session history and context frames.
    pub fn invoke_model_for_session(
        &self,
        session_id: &str,
        model_id: Option<String>,
    ) -> KernelResult<BridgeModelResult> {
        let (request, provider_id) =
            self.prepare_model_request_for_session(session_id, model_id, None)?;
        self.model_bridge.invoke(&request, provider_id.as_deref())
    }

    /// Stream model response
    pub fn stream_model(
        &self,
        request: ModelRequest,
    ) -> KernelResult<Vec<sdkwork_agent_kernel::ModelStreamChunk>> {
        self.model_bridge.stream(&request, None)
    }

    /// Stream model output using session history, with optional message override.
    pub fn stream_model_for_session(
        &self,
        session_id: &str,
        model_id: Option<String>,
        override_messages: Option<Vec<String>>,
    ) -> KernelResult<Vec<sdkwork_agent_kernel::ModelStreamChunk>> {
        let (request, provider_id) =
            self.prepare_model_request_for_session(session_id, model_id, override_messages)?;
        self.model_bridge.stream(&request, provider_id.as_deref())
    }

    /// Stream model output incrementally using session history.
    pub fn stream_model_for_session_into(
        &self,
        session_id: &str,
        model_id: Option<String>,
        override_messages: Option<Vec<String>>,
        sink: &mut dyn sdkwork_agent_kernel::ModelStreamSink,
    ) -> KernelResult<()> {
        let (request, provider_id) =
            self.prepare_model_request_for_session(session_id, model_id, override_messages)?;
        self.model_bridge
            .stream_into(&request, provider_id.as_deref(), sink)
    }

    /// Build a model request from bridge session state without invoking a provider.
    ///
    /// Server runtimes use this to keep the bridge state lock scoped to local
    /// session/history reads. Provider calls can then run outside the lock.
    pub fn prepare_model_request_for_session(
        &self,
        session_id: &str,
        model_id: Option<String>,
        override_messages: Option<Vec<String>>,
    ) -> KernelResult<(ModelRequest, Option<String>)> {
        let session = self.get_session(session_id)?;
        let provider_id = session
            .metadata_value("modelProvider")
            .map(std::string::ToString::to_string);
        let context = self.context_bridge.collect_context(session_id)?;
        let history = if let Some(messages) = override_messages {
            if messages.is_empty() {
                self.get_messages(session_id)?
            } else {
                sdkwork_agent_kernel::agent_messages_from_text_lines(
                    sdkwork_agent_kernel::AgentMessageRole::User,
                    &messages,
                )
            }
        } else {
            self.get_messages(session_id)?
        };
        let mut request = self
            .model_bridge
            .build_request(session_id, &session, &history, &context, None);
        if let Some(model_id) = model_id {
            request = request.with_model_id(model_id);
        }
        if let Some(timeout_ms) = session.metadata_value("timeoutMs") {
            let timeout_ms = timeout_ms.parse::<u64>().map_err(|_| {
                sdkwork_agent_kernel::KernelError::validation(
                    "session timeoutMs metadata must be an unsigned integer",
                )
            })?;
            request = request.with_timeout_ms(timeout_ms);
        }
        Ok((request, provider_id))
    }

    /// Cancel an in-flight model invocation by its model request id.
    pub fn cancel_model(
        &self,
        model_request_id: &str,
        model_provider_id: Option<&str>,
    ) -> KernelResult<sdkwork_agent_kernel::ModelResponse> {
        self.model_bridge
            .cancel(model_request_id, model_provider_id)
    }

    /// List registered model descriptors
    pub fn list_models(&self) -> KernelResult<Vec<sdkwork_agent_kernel::ModelDescriptor>> {
        Ok(self.model_bridge.list_models())
    }

    // =========================================================================
    // Tool Execution
    // =========================================================================

    /// List available tools
    pub fn list_tools(&self) -> KernelResult<Vec<sdkwork_agent_kernel::ToolDescriptor>> {
        self.tool_bridge.list_tools()
    }

    /// Execute a tool call
    pub fn execute_tool(
        &mut self,
        session_id: &str,
        tool_name: &str,
        arguments: &str,
    ) -> KernelResult<BridgeToolResult> {
        let call = ToolCall::new(
            format!("call.{}", crate::types::generate_id()),
            tool_name,
            arguments,
        )
        .for_session(session_id);

        let result = self.tool_bridge.execute(&call)?;

        Ok(self.commit_tool_execution(session_id, tool_name, call, result))
    }

    /// Commit the bridge event after a tool provider call completed outside
    /// the bridge state lock.
    pub fn commit_tool_execution(
        &mut self,
        session_id: &str,
        tool_name: &str,
        call: ToolCall,
        result: sdkwork_agent_kernel::ToolResult,
    ) -> BridgeToolResult {
        let events = vec![BridgeEvent {
            event_type: "agent.tool.executed".to_string(),
            session_id: Some(session_id.to_string()),
            task_id: None,
            payload: format!("tool={};status={}", tool_name, result.status),
            severity: BridgeEventSeverity::Info,
        }];

        self.event_bridge.record_events(&events);

        BridgeToolResult {
            call_id: call.tool_call_id,
            result,
            events,
        }
    }

    // =========================================================================
    // Snapshot
    // =========================================================================

    /// Get a complete snapshot of the bridge state for UI consumption
    pub fn get_snapshot(&self, session_id: &str) -> KernelResult<BridgeSnapshot> {
        let session = self.session_bridge.get_session(session_id)?;
        let messages = self.session_bridge.get_history(session_id)?;
        let tools = self.tool_bridge.list_tools()?;
        let events = self.event_bridge.get_events(session_id);

        Ok(BridgeSnapshot {
            session_id: session_id.to_string(),
            session,
            messages,
            available_tools: tools,
            pending_tool_calls: Vec::new(),
            events,
        })
    }

    // =========================================================================
    // Accessors
    // =========================================================================

    /// Get reference to session bridge
    pub fn session_bridge(&self) -> &SessionBridge {
        &self.session_bridge
    }

    /// Get mutable reference to session bridge
    pub fn session_bridge_mut(&mut self) -> &mut SessionBridge {
        &mut self.session_bridge
    }

    /// Get reference to model bridge
    pub fn model_bridge(&self) -> &ModelBridge {
        &self.model_bridge
    }

    /// Get reference to tool bridge
    pub fn tool_bridge(&self) -> &ToolBridge {
        &self.tool_bridge
    }

    /// Get reference to context bridge
    pub fn context_bridge(&self) -> &ContextBridge {
        &self.context_bridge
    }

    /// Get reference to event bridge
    pub fn event_bridge(&self) -> &EventBridge {
        &self.event_bridge
    }
}

impl Default for AgentRuntimeBridge {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_provider_session_completion(
    requested_provider_session_id: Option<&str>,
    completed_provider_session_id: Option<&str>,
) -> KernelResult<()> {
    let requested = requested_provider_session_id
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let completed = completed_provider_session_id
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match (requested, completed) {
        (Some(requested), Some(completed)) if requested == completed => Ok(()),
        (Some(_), Some(_)) => Err(sdkwork_agent_kernel::KernelError::conflict(
            "provider session id changed during a resumed turn",
        )),
        (Some(_), None) => Err(sdkwork_agent_kernel::KernelError::conflict(
            "resumed turn completed without its provider session id",
        )),
        (None, _) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> BridgeSessionConfig {
        BridgeSessionConfig {
            agent_id: "agent.test".to_string(),
            tenant_id: "tenant.100001".to_string(),
            user_ref: Some("user.1".to_string()),
            model: Some("gpt-4".to_string()),
            instructions: Some("original instructions".to_string()),
            cwd: None,
            metadata: Vec::new(),
        }
    }

    #[test]
    fn bridge_create_session() {
        let mut bridge = AgentRuntimeBridge::new();
        let config = BridgeSessionConfig {
            agent_id: "agent.test".to_string(),
            tenant_id: "tenant.100001".to_string(),
            user_ref: Some("user.1".to_string()),
            model: Some("gpt-4".to_string()),
            instructions: None,
            cwd: None,
            metadata: Vec::new(),
        };

        let session = bridge.create_session(config).expect("session created");
        assert_eq!(session.agent_id, Some("agent.test".to_string()));
        assert_eq!(session.model, Some("gpt-4".to_string()));
    }

    #[test]
    fn failed_hydration_removes_a_newly_registered_transient_session() {
        let mut bridge = AgentRuntimeBridge::new();
        let invalid_history = vec![AgentMessage::new(
            "msg.invalid",
            AgentMessageRole::User,
            Vec::new(),
        )];

        let error = bridge
            .register_session_with_history_revision(
                "session.hydration-new",
                test_config(),
                1,
                invalid_history,
            )
            .expect_err("invalid persisted history must fail hydration");
        assert_eq!(
            error.kind(),
            sdkwork_agent_kernel::KernelErrorKind::ValidationError
        );
        assert!(bridge.get_session("session.hydration-new").is_err());
        assert!(bridge.get_messages("session.hydration-new").is_err());
    }

    #[test]
    fn failed_hydration_restores_the_existing_session_configuration() {
        let mut bridge = AgentRuntimeBridge::new();
        let original_config = test_config();
        bridge
            .register_session("session.hydration-existing", original_config.clone())
            .expect("original session registered");

        let mut refreshed_config = original_config;
        refreshed_config.model = Some("gpt-4.1".to_string());
        refreshed_config.instructions = Some("new instructions".to_string());
        let error = bridge
            .register_session_with_history_revision(
                "session.hydration-existing",
                refreshed_config,
                1,
                vec![AgentMessage::new(
                    "msg.invalid",
                    AgentMessageRole::User,
                    Vec::new(),
                )],
            )
            .expect_err("invalid persisted history must not partially refresh the session");
        assert_eq!(
            error.kind(),
            sdkwork_agent_kernel::KernelErrorKind::ValidationError
        );

        let restored = bridge
            .get_session("session.hydration-existing")
            .expect("existing session restored");
        assert_eq!(restored.model.as_deref(), Some("gpt-4"));
        assert_eq!(
            restored.instructions.as_deref(),
            Some("original instructions")
        );
    }

    #[test]
    fn bridge_send_message() {
        let mut bridge = AgentRuntimeBridge::new_with_mock_fallback();
        let config = BridgeSessionConfig {
            agent_id: "agent.test".to_string(),
            tenant_id: "tenant.100001".to_string(),
            user_ref: Some("user.1".to_string()),
            model: Some("gpt-4".to_string()),
            instructions: None,
            cwd: None,
            metadata: Vec::new(),
        };

        let session = bridge.create_session(config).expect("session created");
        let response = bridge
            .send_message(&session.session_id, "Hello")
            .expect("message sent");

        assert_eq!(response.session_id, session.session_id);
        assert!(response.model_response.is_some());
    }

    #[test]
    fn remove_session_deletes_recorded_bridge_events() {
        let mut bridge = AgentRuntimeBridge::new_with_mock_fallback();
        let config = BridgeSessionConfig {
            agent_id: "agent.test".to_string(),
            tenant_id: "tenant.100001".to_string(),
            user_ref: Some("user.1".to_string()),
            model: Some("gpt-4".to_string()),
            instructions: None,
            cwd: None,
            metadata: Vec::new(),
        };

        let session = bridge.create_session(config).expect("session created");
        bridge
            .send_message(&session.session_id, "Hello")
            .expect("message sent");
        assert!(
            !bridge
                .event_bridge()
                .get_events(&session.session_id)
                .is_empty(),
            "message turn should record bridge events for the session"
        );

        bridge.remove_session(&session.session_id);

        assert!(
            bridge
                .event_bridge()
                .get_events(&session.session_id)
                .is_empty(),
            "removing a session must clear per-session bridge events"
        );
    }

    #[test]
    fn failed_stream_completion_leaves_no_partial_turn_in_history() {
        let mut bridge = AgentRuntimeBridge::new_with_mock_fallback();
        let session = bridge
            .create_session(BridgeSessionConfig {
                agent_id: "agent.test".to_string(),
                tenant_id: "tenant.100001".to_string(),
                user_ref: Some("user.1".to_string()),
                model: Some("gpt-4".to_string()),
                instructions: None,
                cwd: None,
                metadata: Vec::new(),
            })
            .expect("session created");

        let (_, _, _, user_message) = bridge
            .prepare_stream_message_turn(&session.session_id, "hello", None)
            .expect("turn prepared");
        assert!(
            bridge
                .get_messages(&session.session_id)
                .expect("history loaded")
                .is_empty(),
            "preparing a provider request must not mutate retained history"
        );

        let error = bridge
            .complete_stream_message_turn(
                &session.session_id,
                user_message,
                vec![ModelStreamChunk::output(
                    "req.large",
                    0,
                    "x".repeat(crate::MAX_MODEL_STREAM_CHUNK_BYTES + 1),
                )],
            )
            .expect_err("oversized completion must fail");
        assert_eq!(
            error.kind(),
            sdkwork_agent_kernel::KernelErrorKind::ResourceExhausted
        );
        assert!(
            bridge
                .get_messages(&session.session_id)
                .expect("history loaded")
                .is_empty(),
            "a rejected provider response must not leave a partial user turn"
        );
    }

    #[test]
    fn failed_model_response_cannot_commit_a_partial_user_turn() {
        let mut bridge = AgentRuntimeBridge::new_with_mock_fallback();
        let session = bridge
            .create_session(test_config())
            .expect("session created");
        let (_, request, _, user_message) = bridge
            .prepare_send_message_turn(&session.session_id, "hello")
            .expect("turn prepared");
        let result = BridgeModelResult {
            response: sdkwork_agent_kernel::ModelResponse::text(
                &request.model_request_id,
                "provider.test",
                "not persisted",
            )
            .with_status(sdkwork_agent_kernel::ModelStatus::Failed),
            provider_session_id: None,
            tool_calls: Vec::new(),
            events: Vec::new(),
        };

        let error = bridge
            .complete_user_message_turn(&session.session_id, user_message, result)
            .expect_err("non-success model results must not complete a turn");
        assert_eq!(
            error.kind(),
            sdkwork_agent_kernel::KernelErrorKind::Conflict
        );
        assert!(bridge
            .get_messages(&session.session_id)
            .expect("history")
            .is_empty());
    }

    #[test]
    fn bridge_list_tools() {
        let bridge = AgentRuntimeBridge::new();
        let tools = bridge.list_tools().expect("tools listed");
        assert!(tools.is_empty(), "no provider means no advertised tools");
    }

    #[test]
    fn bridge_get_snapshot() {
        let mut bridge = AgentRuntimeBridge::new();
        let config = BridgeSessionConfig {
            agent_id: "agent.test".to_string(),
            tenant_id: "tenant.100001".to_string(),
            user_ref: None,
            model: None,
            instructions: None,
            cwd: None,
            metadata: Vec::new(),
        };

        let session = bridge.create_session(config).expect("session created");
        let snapshot = bridge
            .get_snapshot(&session.session_id)
            .expect("snapshot created");

        assert_eq!(snapshot.session_id, session.session_id);
        assert!(
            snapshot.available_tools.is_empty(),
            "snapshot must not advertise unavailable tools"
        );
    }
}
