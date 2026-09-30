use std::mem::size_of;
use std::sync::Arc;

pub(crate) const PROVIDER_SESSION_ID_METADATA: &str = "providerSessionId";
const SDK_RUNTIME_PROVIDER_SESSION_DIAGNOSTIC_PREFIX: &str = "sdk_runtime_provider_session_id=";

use crate::types::{generate_id, BridgeEvent, BridgeEventSeverity, BridgeModelResult};
use sdkwork_agent_kernel::{
    agent_messages_to_text_lines, AgentInputContract, AgentMessage, AgentRuntime, AgentSession,
    ContextFrame, KernelError, KernelErrorKind, KernelEvent, KernelResult,
    ModelCancellationRequest, ModelDescriptor, ModelExecutionRequest, ModelExecutionService,
    ModelRequest, ModelResponse, ModelStatus, ModelStreamChunk, ModelStreamSink, ModelUsage,
    TraceContext,
};

/// Maximum UTF-8 bytes accepted from one model streaming chunk.
pub const MAX_MODEL_STREAM_CHUNK_BYTES: usize = 256 * 1024;
/// Maximum number of chunks retained for one model response.
pub const MAX_MODEL_STREAM_CHUNKS: usize = 4096;
/// Maximum aggregate UTF-8 bytes retained for one model response.
pub const MAX_MODEL_OUTPUT_BYTES: usize = 3 * 1024 * 1024;
const MAX_MODEL_TOOL_CALLS: usize = 128;
const MAX_MODEL_DIAGNOSTICS: usize = 128;
const MAX_MODEL_AUXILIARY_FIELD_BYTES: usize = 256;

/// Validate a complete model stream before it is concatenated or persisted.
pub fn validate_model_stream_chunks(chunks: &[ModelStreamChunk]) -> KernelResult<usize> {
    if chunks.len() > MAX_MODEL_STREAM_CHUNKS {
        return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
            "model stream chunk limit exceeded",
        ));
    }

    let mut total_bytes = 0usize;
    for chunk in chunks {
        let chunk_bytes = chunk.content.len();
        if chunk_bytes > MAX_MODEL_STREAM_CHUNK_BYTES {
            return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model stream chunk byte limit exceeded",
            ));
        }
        total_bytes = total_bytes.checked_add(chunk_bytes).ok_or_else(|| {
            sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model output byte count overflow",
            )
        })?;
        if total_bytes > MAX_MODEL_OUTPUT_BYTES {
            return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model output byte limit exceeded",
            ));
        }
    }
    Ok(total_bytes)
}

/// Collect validated model chunks into the bounded assistant text payload.
pub fn collect_model_stream_output(chunks: &[ModelStreamChunk]) -> KernelResult<String> {
    let total_bytes = validate_model_stream_chunks(chunks)?;
    let mut output = String::with_capacity(total_bytes);
    for chunk in chunks {
        output.push_str(&chunk.content);
    }
    Ok(output)
}

pub(crate) fn collect_model_stream_output_for_request(
    chunks: &[ModelStreamChunk],
    expected_request_id: &str,
) -> KernelResult<String> {
    let total_bytes = validate_model_stream_chunks(chunks)?;
    let mut last_sequence = None;
    for chunk in chunks {
        validate_chunk_identity(expected_request_id, last_sequence, chunk)?;
        model_stream_chunk_bytes(chunk)?;
        last_sequence = Some(chunk.sequence);
    }
    let mut output = String::with_capacity(total_bytes);
    for chunk in chunks {
        output.push_str(&chunk.content);
    }
    Ok(output)
}

/// Handles model invocations and response processing
#[derive(Clone)]
pub struct ModelBridge {
    default_model: String,
    agent_runtime: Option<Arc<AgentRuntime>>,
    allow_mock_fallback: bool,
}

impl ModelBridge {
    pub fn new() -> Self {
        Self {
            default_model: "gpt-4".to_string(),
            agent_runtime: None,
            allow_mock_fallback: false,
        }
    }

    pub fn with_agent_runtime(agent_runtime: Arc<AgentRuntime>, allow_mock_fallback: bool) -> Self {
        let default_model = resolve_default_model_id(&agent_runtime);
        Self {
            default_model,
            agent_runtime: Some(agent_runtime),
            allow_mock_fallback,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_mock_fallback_enabled() -> Self {
        Self {
            default_model: "gpt-4".to_string(),
            agent_runtime: None,
            allow_mock_fallback: true,
        }
    }

    /// Build a model request from session context
    pub fn build_request(
        &self,
        session_id: &str,
        session: &AgentSession,
        history: &[AgentMessage],
        context: &[ContextFrame],
        input_contract_override: Option<AgentInputContract>,
    ) -> ModelRequest {
        let model_id = session
            .model
            .clone()
            .unwrap_or_else(|| self.default_model.clone());

        let mut input_messages = Vec::with_capacity(
            history
                .len()
                .saturating_add(context.len())
                .saturating_add(usize::from(session.instructions.is_some())),
        );
        if let Some(instructions) = session.instructions.as_deref() {
            input_messages.push(
                AgentMessage::new(
                    format!("message.instructions.{session_id}"),
                    sdkwork_agent_kernel::AgentMessageRole::System,
                    vec![sdkwork_agent_kernel::AgentPart::text(
                        format!("part.instructions.{session_id}"),
                        instructions,
                    )],
                )
                .for_session(session_id),
            );
        }
        input_messages.extend(history.iter().cloned());
        for frame in context {
            input_messages.push(AgentMessage::new(
                format!("message.context.{}", frame.context_frame_id),
                sdkwork_agent_kernel::AgentMessageRole::System,
                vec![sdkwork_agent_kernel::AgentPart::text(
                    format!("part.context.{}", frame.context_frame_id),
                    format!("[context:{}] {}", frame.source, frame.content),
                )],
            ));
        }

        let messages = agent_messages_to_text_lines(&input_messages);
        let input_contract =
            input_contract_override.unwrap_or_else(|| session.resolved_input_contract());
        let mut request = ModelRequest::new(format!("req.{}", generate_id()), messages)
            .with_model_id(model_id)
            .for_session(session_id)
            .with_input_messages(input_messages)
            .with_input_contract(input_contract);
        if let Some(provider_session_id) = session
            .metadata_value(PROVIDER_SESSION_ID_METADATA)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            request = request.for_provider_session(provider_session_id.to_string());
        }
        request
    }

    /// Invoke the typed provider when registered, otherwise use the mock bridge path.
    pub fn invoke(
        &self,
        request: &ModelRequest,
        model_provider_id: Option<&str>,
    ) -> KernelResult<BridgeModelResult> {
        let result = if let Some(runtime) = &self.agent_runtime {
            match self.invoke_typed(runtime, request, model_provider_id) {
                Ok(result) => result,
                Err(error) if self.mock_fallback_eligible(&error) => self.invoke_mock(request)?,
                Err(error) => return Err(error),
            }
        } else {
            if !self.allow_mock_fallback {
                return Err(sdkwork_agent_kernel::KernelError::ProviderUnavailable {
                    provider_id: model_provider_id.unwrap_or("provider.model").to_string(),
                });
            }
            self.invoke_mock(request)?
        };

        self.validate_model_response_for_request(&request.model_request_id, &result.response)?;
        Ok(result)
    }

    /// Stream model response (typed provider when registered, otherwise mock bridge path).
    pub fn stream(
        &self,
        request: &ModelRequest,
        model_provider_id: Option<&str>,
    ) -> KernelResult<Vec<ModelStreamChunk>> {
        let mut collector = BoundedModelStreamCollector::new(&request.model_request_id);
        if let Some(runtime) = &self.agent_runtime {
            match self.stream_typed_into(runtime, request, model_provider_id, &mut collector) {
                Ok(()) => {
                    if !collector.is_empty() || !self.allow_mock_fallback {
                        return Ok(collector.into_chunks());
                    }
                }
                Err(error) if self.mock_fallback_eligible(&error) => {}
                Err(error) => return Err(error),
            }
            collector.clear();
        } else if !self.allow_mock_fallback {
            return Err(sdkwork_agent_kernel::KernelError::ProviderUnavailable {
                provider_id: model_provider_id.unwrap_or("provider.model").to_string(),
            });
        }

        if !self.allow_mock_fallback {
            return Err(sdkwork_agent_kernel::KernelError::ProviderUnavailable {
                provider_id: model_provider_id.unwrap_or("provider.model").to_string(),
            });
        }

        for chunk in self.stream_mock(request)? {
            collector.push_chunk(chunk)?;
        }
        Ok(collector.into_chunks())
    }

    /// Stream model output incrementally through `sink`.
    pub fn stream_into(
        &self,
        request: &ModelRequest,
        model_provider_id: Option<&str>,
        sink: &mut dyn ModelStreamSink,
    ) -> KernelResult<()> {
        let mut bounded_sink = BoundedModelStreamSink::new(sink, &request.model_request_id);
        if let Some(runtime) = &self.agent_runtime {
            match self.stream_typed_into(runtime, request, model_provider_id, &mut bounded_sink) {
                Ok(()) => return Ok(()),
                Err(error) if bounded_sink.is_empty() && self.mock_fallback_eligible(&error) => {}
                Err(error) => return Err(error),
            }
        } else if !self.allow_mock_fallback {
            return Err(sdkwork_agent_kernel::KernelError::ProviderUnavailable {
                provider_id: model_provider_id.unwrap_or("provider.model").to_string(),
            });
        }

        if !self.allow_mock_fallback {
            return Err(sdkwork_agent_kernel::KernelError::ProviderUnavailable {
                provider_id: model_provider_id.unwrap_or("provider.model").to_string(),
            });
        }

        for chunk in self.stream_mock(request)? {
            bounded_sink.push_chunk(chunk)?;
        }
        Ok(())
    }

    /// Get available model descriptors. Falls back to the mock catalog only
    /// when mock fallback is explicitly enabled (test construction); a missing
    /// or empty production provider reports no models instead of phantom ids
    /// that later fail model resolution.
    pub fn list_models(&self) -> Vec<ModelDescriptor> {
        if let Some(runtime) = &self.agent_runtime {
            if let Ok(provider) = runtime.model_provider() {
                let models = provider.list_models();
                if !models.is_empty() {
                    return models;
                }
            }
        }

        if self.allow_mock_fallback {
            self.list_models_mock()
        } else {
            Vec::new()
        }
    }

    fn stream_typed_into(
        &self,
        runtime: &AgentRuntime,
        request: &ModelRequest,
        model_provider_id: Option<&str>,
        sink: &mut dyn ModelStreamSink,
    ) -> KernelResult<()> {
        let mut execution_request =
            ModelExecutionRequest::new(request.model_request_id.clone(), request.clone());
        if let Some(provider_id) = model_provider_id.filter(|value| !value.is_empty()) {
            execution_request = execution_request.with_provider_id(provider_id.to_string());
        }
        ModelExecutionService::new().stream_into(runtime, execution_request, sink)
    }

    /// Validate an invoke response before it can enter bridge-owned history.
    pub fn validate_model_response(&self, response: &ModelResponse) -> KernelResult<()> {
        self.validate_model_response_for_request(&response.model_request_id, response)
    }

    pub(crate) fn validate_model_response_for_request(
        &self,
        expected_request_id: &str,
        response: &ModelResponse,
    ) -> KernelResult<()> {
        if response.model_request_id != expected_request_id {
            return Err(sdkwork_agent_kernel::KernelError::conflict(
                "model response request id does not match the active request",
            ));
        }
        if response.status != ModelStatus::Succeeded {
            return Err(sdkwork_agent_kernel::KernelError::conflict(
                "model provider did not return a successful response",
            ));
        }
        if response.messages.len() > MAX_MODEL_STREAM_CHUNKS {
            return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model response message limit exceeded",
            ));
        }
        if response.messages.is_empty() {
            return Err(sdkwork_agent_kernel::KernelError::validation(
                "successful model response must contain at least one message",
            ));
        }
        if response.tool_calls.len() > MAX_MODEL_TOOL_CALLS {
            return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model tool call limit exceeded",
            ));
        }
        if response.diagnostics.len() > MAX_MODEL_DIAGNOSTICS {
            return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model diagnostics limit exceeded",
            ));
        }
        if model_response_bytes(response)? > MAX_MODEL_OUTPUT_BYTES {
            return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model response retained byte limit exceeded",
            ));
        }

        let mut total_bytes = 0usize;
        for message in &response.messages {
            let message_bytes = message.len();
            if message_bytes > MAX_MODEL_OUTPUT_BYTES {
                return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
                    "model response message byte limit exceeded",
                ));
            }
            total_bytes = total_bytes.checked_add(message_bytes).ok_or_else(|| {
                sdkwork_agent_kernel::KernelError::resource_exhausted(
                    "model response byte count overflow",
                )
            })?;
            if total_bytes > MAX_MODEL_OUTPUT_BYTES {
                return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
                    "model response byte limit exceeded",
                ));
            }
        }
        Ok(())
    }

    /// Cancel an in-flight model invocation (typed provider when registered,
    /// otherwise mock bridge path).
    pub fn cancel(
        &self,
        model_request_id: &str,
        model_provider_id: Option<&str>,
    ) -> KernelResult<ModelResponse> {
        if let Some(runtime) = &self.agent_runtime {
            match self.cancel_typed(runtime, model_request_id, model_provider_id) {
                Ok(response) => return Ok(response),
                Err(_) if self.allow_mock_fallback => {}
                Err(error) => return Err(error),
            }
        }

        if self.allow_mock_fallback {
            return Ok(self.cancel_mock(model_request_id));
        }

        Err(KernelError::ProviderUnavailable {
            provider_id: model_provider_id.unwrap_or("provider.model").to_string(),
        })
    }

    fn cancel_typed(
        &self,
        runtime: &AgentRuntime,
        model_request_id: &str,
        model_provider_id: Option<&str>,
    ) -> KernelResult<ModelResponse> {
        let mut cancellation_request =
            ModelCancellationRequest::new("cancel.bridge", model_request_id.to_string());
        if let Some(provider_id) = model_provider_id.filter(|value| !value.is_empty()) {
            cancellation_request = cancellation_request.with_provider_id(provider_id.to_string());
        }
        let response = ModelExecutionService::new().cancel(runtime, cancellation_request)?;
        Ok(response.model_response)
    }

    fn cancel_mock(&self, model_request_id: &str) -> ModelResponse {
        let mut response = ModelResponse::text(model_request_id, "provider.mock", "");
        response.status = ModelStatus::Cancelled;
        response.finish_reason = Some("cancelled".to_string());
        response
    }

    fn mock_fallback_eligible(&self, error: &KernelError) -> bool {
        self.allow_mock_fallback
            && (error.retryable() || error.kind() == KernelErrorKind::CapabilityMissing)
    }

    fn stream_mock(&self, request: &ModelRequest) -> KernelResult<Vec<ModelStreamChunk>> {
        let text = format!(
            "This is a mock streamed response. Model: {}, Messages: {}",
            request.model_id.as_deref().unwrap_or("unknown"),
            request.messages.len()
        );
        Ok(text
            .split_whitespace()
            .enumerate()
            .map(|(index, word)| {
                ModelStreamChunk::output(
                    &request.model_request_id,
                    index as u64,
                    format!("{word} "),
                )
            })
            .collect())
    }

    fn invoke_typed(
        &self,
        runtime: &AgentRuntime,
        request: &ModelRequest,
        model_provider_id: Option<&str>,
    ) -> KernelResult<BridgeModelResult> {
        let mut execution_request =
            ModelExecutionRequest::new(request.model_request_id.clone(), request.clone());
        if let Some(provider_id) = model_provider_id.filter(|value| !value.is_empty()) {
            execution_request = execution_request.with_provider_id(provider_id.to_string());
        }
        let response = ModelExecutionService::new().invoke(runtime, execution_request)?;
        let events = vec![BridgeEvent {
            event_type: "agent.model.invoked".to_string(),
            session_id: request.session_id.clone(),
            task_id: None,
            payload: format!(
                "provider={};model={}",
                response.provider_id,
                request.model_id.as_deref().unwrap_or("default")
            ),
            severity: BridgeEventSeverity::Info,
        }];

        let provider_session_id = provider_session_id_from_response(&response.model_response)?;
        Ok(BridgeModelResult {
            response: response.model_response,
            provider_session_id,
            tool_calls: Vec::new(),
            events,
        })
    }

    fn invoke_mock(&self, request: &ModelRequest) -> KernelResult<BridgeModelResult> {
        let response = ModelResponse::text(
            &request.model_request_id,
            "provider.mock",
            format!(
                "This is a mock response to your message. Model: {}, Messages: {}",
                request.model_id.as_deref().unwrap_or("unknown"),
                request.messages.len()
            ),
        )
        .with_usage(ModelUsage::new(100, 50));

        let events = vec![BridgeEvent {
            event_type: "agent.model.invoked".to_string(),
            session_id: request.session_id.clone(),
            task_id: None,
            payload: format!(
                "model={};input_tokens=100;output_tokens=50",
                request.model_id.as_deref().unwrap_or("unknown")
            ),
            severity: BridgeEventSeverity::Info,
        }];

        Ok(BridgeModelResult {
            response,
            provider_session_id: None,
            tool_calls: Vec::new(),
            events,
        })
    }

    fn list_models_mock(&self) -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor::new("gpt-4", "provider.openai", "GPT-4", "gpt")
                .with_context_window_tokens(128000)
                .with_max_output_tokens(4096),
            ModelDescriptor::new("gpt-3.5-turbo", "provider.openai", "GPT-3.5 Turbo", "gpt")
                .with_context_window_tokens(16385)
                .with_max_output_tokens(4096),
            ModelDescriptor::new(
                "claude-3-opus",
                "provider.anthropic",
                "Claude 3 Opus",
                "claude",
            )
            .with_context_window_tokens(200000)
            .with_max_output_tokens(4096),
        ]
    }
}

struct BoundedModelStreamCollector {
    chunks: Vec<ModelStreamChunk>,
    bytes: usize,
    expected_request_id: String,
    last_sequence: Option<u64>,
}

impl BoundedModelStreamCollector {
    fn new(expected_request_id: &str) -> Self {
        Self {
            chunks: Vec::new(),
            bytes: 0,
            expected_request_id: expected_request_id.to_string(),
            last_sequence: None,
        }
    }

    fn clear(&mut self) {
        self.chunks = Vec::new();
        self.bytes = 0;
        self.last_sequence = None;
    }

    fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    fn into_chunks(self) -> Vec<ModelStreamChunk> {
        self.chunks
    }
}

impl ModelStreamSink for BoundedModelStreamCollector {
    fn push_chunk(&mut self, chunk: ModelStreamChunk) -> KernelResult<()> {
        if self.chunks.len() >= MAX_MODEL_STREAM_CHUNKS {
            return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model stream chunk limit exceeded",
            ));
        }
        validate_chunk_identity(&self.expected_request_id, self.last_sequence, &chunk)?;
        let chunk_bytes = model_stream_chunk_bytes(&chunk)?;
        let new_total = self.bytes.checked_add(chunk_bytes).ok_or_else(|| {
            sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model output byte count overflow",
            )
        })?;
        if new_total > MAX_MODEL_OUTPUT_BYTES {
            return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model output byte limit exceeded",
            ));
        }
        self.bytes = new_total;
        self.last_sequence = Some(chunk.sequence);
        self.chunks.push(chunk);
        Ok(())
    }
}

struct BoundedModelStreamSink<'a> {
    inner: &'a mut dyn ModelStreamSink,
    chunks: usize,
    bytes: usize,
    event_forwarded: bool,
    expected_request_id: String,
    last_sequence: Option<u64>,
}

impl<'a> BoundedModelStreamSink<'a> {
    fn new(inner: &'a mut dyn ModelStreamSink, expected_request_id: &str) -> Self {
        Self {
            inner,
            chunks: 0,
            bytes: 0,
            event_forwarded: false,
            expected_request_id: expected_request_id.to_string(),
            last_sequence: None,
        }
    }

    fn is_empty(&self) -> bool {
        self.chunks == 0 && !self.event_forwarded
    }
}

impl ModelStreamSink for BoundedModelStreamSink<'_> {
    fn push_chunk(&mut self, chunk: ModelStreamChunk) -> KernelResult<()> {
        if self.chunks >= MAX_MODEL_STREAM_CHUNKS {
            return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model stream chunk limit exceeded",
            ));
        }
        validate_chunk_identity(&self.expected_request_id, self.last_sequence, &chunk)?;
        let chunk_bytes = model_stream_chunk_bytes(&chunk)?;
        let new_total = self.bytes.checked_add(chunk_bytes).ok_or_else(|| {
            sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model output byte count overflow",
            )
        })?;
        if new_total > MAX_MODEL_OUTPUT_BYTES {
            return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model output byte limit exceeded",
            ));
        }
        let sequence = chunk.sequence;
        self.inner.push_chunk(chunk)?;
        self.chunks += 1;
        self.bytes = new_total;
        self.last_sequence = Some(sequence);
        Ok(())
    }

    fn push_event(&mut self, event: KernelEvent) -> KernelResult<()> {
        self.inner.push_event(event)?;
        self.event_forwarded = true;
        Ok(())
    }
}

impl Default for ModelBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the runtime's first advertised model as the default. When no typed
/// model provider is registered (test-only construction), fall back to the
/// mock catalog id only under mock fallback; production construction wired
/// through [`ModelBridge::with_agent_runtime`] always has a provider whose
/// catalog supplies the default, so the phantom placeholder never ships.
fn resolve_default_model_id(runtime: &AgentRuntime) -> String {
    if let Ok(provider) = runtime.model_provider() {
        let models = provider.list_models();
        if let Some(model) = models.first() {
            return model.model_id.clone();
        }
    }
    String::new()
}

fn provider_session_id_from_response(response: &ModelResponse) -> KernelResult<Option<String>> {
    let mut provider_session_id = None;
    for diagnostic in &response.diagnostics {
        let Some(value) = diagnostic
            .strip_prefix(SDK_RUNTIME_PROVIDER_SESSION_DIAGNOSTIC_PREFIX)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if provider_session_id
            .as_deref()
            .is_some_and(|current| current != value)
        {
            return Err(KernelError::conflict(
                "model response contains conflicting provider session identities",
            ));
        }
        provider_session_id = Some(value.to_string());
    }
    Ok(provider_session_id)
}

fn validate_chunk_identity(
    expected_request_id: &str,
    last_sequence: Option<u64>,
    chunk: &ModelStreamChunk,
) -> KernelResult<()> {
    if chunk.model_request_id != expected_request_id {
        return Err(sdkwork_agent_kernel::KernelError::conflict(
            "model stream chunk request id does not match the active request",
        ));
    }
    if last_sequence.is_some_and(|last| chunk.sequence <= last) {
        return Err(sdkwork_agent_kernel::KernelError::conflict(
            "model stream chunk sequence is not strictly increasing",
        ));
    }
    Ok(())
}

fn model_stream_chunk_bytes(chunk: &ModelStreamChunk) -> KernelResult<usize> {
    let mut bytes = size_of::<ModelStreamChunk>()
        .checked_add(chunk.model_request_id.capacity())
        .and_then(|value| value.checked_add(chunk.content.capacity()))
        .ok_or_else(|| {
            sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model stream chunk byte count overflow",
            )
        })?;
    if chunk.model_request_id.len() > MAX_MODEL_AUXILIARY_FIELD_BYTES {
        return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
            "model stream request id is too long",
        ));
    }
    if let Some(trace) = &chunk.trace_context {
        bytes = bytes
            .checked_add(trace_context_bytes(trace)?)
            .ok_or_else(|| {
                sdkwork_agent_kernel::KernelError::resource_exhausted(
                    "model stream trace byte count overflow",
                )
            })?;
    }
    if bytes > MAX_MODEL_STREAM_CHUNK_BYTES {
        return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
            "model stream chunk byte limit exceeded",
        ));
    }
    Ok(bytes)
}

fn trace_context_bytes(trace: &TraceContext) -> KernelResult<usize> {
    if trace.trace_id.len() > MAX_MODEL_AUXILIARY_FIELD_BYTES
        || trace.span_id.len() > MAX_MODEL_AUXILIARY_FIELD_BYTES
        || trace
            .parent_span_id
            .as_deref()
            .is_some_and(|value| value.len() > MAX_MODEL_AUXILIARY_FIELD_BYTES)
    {
        return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
            "trace context field is too long",
        ));
    }
    let mut bytes = size_of::<TraceContext>()
        .checked_add(trace.trace_id.capacity())
        .and_then(|value| value.checked_add(trace.span_id.capacity()))
        .ok_or_else(|| {
            sdkwork_agent_kernel::KernelError::resource_exhausted(
                "trace context byte count overflow",
            )
        })?;
    if let Some(parent) = &trace.parent_span_id {
        bytes = bytes.checked_add(parent.capacity()).ok_or_else(|| {
            sdkwork_agent_kernel::KernelError::resource_exhausted(
                "trace context byte count overflow",
            )
        })?;
    }
    Ok(bytes)
}

fn model_response_bytes(response: &ModelResponse) -> KernelResult<usize> {
    if response.model_request_id.len() > MAX_MODEL_AUXILIARY_FIELD_BYTES
        || response.provider_id.len() > MAX_MODEL_AUXILIARY_FIELD_BYTES
    {
        return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
            "model response identity field is too long",
        ));
    }
    let mut bytes = size_of::<ModelResponse>()
        .checked_add(response.model_request_id.capacity())
        .and_then(|value| value.checked_add(response.provider_id.capacity()))
        .ok_or_else(|| {
            sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model response byte count overflow",
            )
        })?;
    for message in &response.messages {
        bytes = bytes.checked_add(message.capacity()).ok_or_else(|| {
            sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model response byte count overflow",
            )
        })?;
    }
    for call in &response.tool_calls {
        if call.arguments.len() > MAX_MODEL_AUXILIARY_FIELD_BYTES * 1024 {
            return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model tool call arguments are too long",
            ));
        }
        bytes = bytes
            .checked_add(size_of::<sdkwork_agent_kernel::ToolCall>())
            .and_then(|value| value.checked_add(call.tool_call_id.capacity()))
            .and_then(|value| value.checked_add(call.tool_id.capacity()))
            .and_then(|value| value.checked_add(call.arguments.capacity()))
            .ok_or_else(|| {
                sdkwork_agent_kernel::KernelError::resource_exhausted(
                    "model tool call byte count overflow",
                )
            })?;
        if let Some(trace) = &call.trace_context {
            bytes = bytes
                .checked_add(trace_context_bytes(trace)?)
                .ok_or_else(|| sdkwork_kernel_error("model tool trace byte count overflow"))?;
        }
    }
    for diagnostic in &response.diagnostics {
        if diagnostic.len() > MAX_MODEL_AUXILIARY_FIELD_BYTES * 16 {
            return Err(sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model diagnostic is too long",
            ));
        }
        bytes = bytes.checked_add(diagnostic.capacity()).ok_or_else(|| {
            sdkwork_agent_kernel::KernelError::resource_exhausted(
                "model diagnostic byte count overflow",
            )
        })?;
    }
    if let Some(trace) = &response.trace_context {
        bytes = bytes
            .checked_add(trace_context_bytes(trace)?)
            .ok_or_else(|| {
                sdkwork_agent_kernel::KernelError::resource_exhausted(
                    "model response trace byte count overflow",
                )
            })?;
    }
    Ok(bytes)
}

fn sdkwork_kernel_error(message: &str) -> sdkwork_agent_kernel::KernelError {
    sdkwork_agent_kernel::KernelError::resource_exhausted(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_resource_exhausted(error: sdkwork_agent_kernel::KernelError) {
        assert_eq!(
            error.kind(),
            sdkwork_agent_kernel::KernelErrorKind::ResourceExhausted
        );
    }

    fn assert_conflict(error: sdkwork_agent_kernel::KernelError) {
        assert_eq!(
            error.kind(),
            sdkwork_agent_kernel::KernelErrorKind::Conflict
        );
    }

    #[test]
    fn build_request_with_history() {
        let bridge = ModelBridge::new();
        let session = AgentSession::new("session.1").with_model("gpt-4");

        let history = vec![AgentMessage::new(
            "msg.1",
            sdkwork_agent_kernel::AgentMessageRole::User,
            vec![],
        )];

        let request = bridge.build_request("session.1", &session, &history, &[], None);
        assert_eq!(request.model_id, Some("gpt-4".to_string()));
    }

    #[test]
    fn build_request_preserves_independent_provider_session_identity() {
        let bridge = ModelBridge::new();
        let session = AgentSession::new("session.canonical")
            .with_metadata(PROVIDER_SESSION_ID_METADATA, "codex.thread.provider");

        let request = bridge.build_request("session.canonical", &session, &[], &[], None);

        assert_eq!(request.session_id.as_deref(), Some("session.canonical"));
        assert_eq!(
            request.provider_session_id.as_deref(),
            Some("codex.thread.provider")
        );
        assert_ne!(request.session_id, request.provider_session_id);
    }

    #[test]
    fn provider_session_diagnostics_reject_conflicting_identities() {
        let mut response = ModelResponse::text("req.1", "provider.codex", "done");
        response.diagnostics = vec![
            "sdk_runtime_provider_session_id=thread.1".to_string(),
            "sdk_runtime_provider_session_id=thread.2".to_string(),
        ];

        let error = provider_session_id_from_response(&response)
            .expect_err("conflicting provider session ids must fail closed");
        assert_eq!(error.kind(), KernelErrorKind::Conflict);
    }

    #[test]
    fn invoke_returns_mock_response() {
        let bridge = ModelBridge::with_mock_fallback_enabled();
        let request = ModelRequest::new("req.1", vec!["Hello".to_string()]);

        let result = bridge.invoke(&request, None).expect("invoked");
        assert!(!result.response.messages.is_empty());
    }

    #[test]
    fn stream_returns_chunks() {
        let bridge = ModelBridge::with_mock_fallback_enabled();
        let request = ModelRequest::new("req.1", vec!["Hello".to_string()]);

        let chunks = bridge.stream(&request, None).expect("streamed");
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|chunk| !chunk.content.is_empty()));
    }

    #[test]
    fn cancel_returns_standard_mock_response_only_when_fallback_is_enabled() {
        let enabled = ModelBridge::with_mock_fallback_enabled();
        let response = enabled
            .cancel("req.cancel", None)
            .expect("development fallback cancels");
        assert_eq!(response.model_request_id, "req.cancel");
        assert_eq!(response.status, ModelStatus::Cancelled);
        assert_eq!(response.finish_reason.as_deref(), Some("cancelled"));

        let disabled = ModelBridge::new();
        assert!(disabled.cancel("req.cancel", None).is_err());
    }

    #[test]
    fn stream_validation_rejects_chunk_count_single_chunk_and_aggregate_limits() {
        let too_many = (0..=MAX_MODEL_STREAM_CHUNKS)
            .map(|sequence| ModelStreamChunk::output("req.count", sequence as u64, ""))
            .collect::<Vec<_>>();
        assert_resource_exhausted(
            validate_model_stream_chunks(&too_many).expect_err("chunk count must be bounded"),
        );

        let oversized_chunk = vec![ModelStreamChunk::output(
            "req.chunk",
            0,
            "x".repeat(MAX_MODEL_STREAM_CHUNK_BYTES + 1),
        )];
        assert_resource_exhausted(
            validate_model_stream_chunks(&oversized_chunk)
                .expect_err("single chunk bytes must be bounded"),
        );

        let aggregate = (0..=(MAX_MODEL_OUTPUT_BYTES / MAX_MODEL_STREAM_CHUNK_BYTES))
            .map(|sequence| {
                ModelStreamChunk::output(
                    "req.aggregate",
                    sequence as u64,
                    "x".repeat(MAX_MODEL_STREAM_CHUNK_BYTES),
                )
            })
            .collect::<Vec<_>>();
        assert_resource_exhausted(
            collect_model_stream_output(&aggregate)
                .expect_err("aggregate model output bytes must be bounded"),
        );
    }

    #[test]
    fn invoke_response_validation_rejects_aggregate_bytes() {
        let bridge = ModelBridge::new();
        let response = ModelResponse::text(
            "req.large",
            "provider.test",
            "x".repeat(MAX_MODEL_OUTPUT_BYTES + 1),
        );
        assert_resource_exhausted(
            bridge
                .validate_model_response(&response)
                .expect_err("invoke output bytes must be bounded"),
        );
    }

    #[test]
    fn invoke_response_validation_binds_the_response_to_the_active_request() {
        let bridge = ModelBridge::new();
        let response = ModelResponse::text("req.other", "provider.test", "response");

        assert_conflict(
            bridge
                .validate_model_response_for_request("req.expected", &response)
                .expect_err("a response from another request must be rejected"),
        );
    }

    #[test]
    fn invoke_response_validation_rejects_non_success_status() {
        let bridge = ModelBridge::new();
        let response = ModelResponse::text("req.status", "provider.test", "not committed")
            .with_status(ModelStatus::Failed);

        assert_conflict(
            bridge
                .validate_model_response_for_request("req.status", &response)
                .expect_err("failed model responses must not be committed"),
        );
    }

    #[test]
    fn stream_collector_rejects_wrong_request_and_non_monotonic_sequences() {
        let mut collector = BoundedModelStreamCollector::new("req.expected");
        collector
            .push_chunk(ModelStreamChunk::output("req.expected", 7, "first"))
            .expect("first chunk accepted");

        assert_conflict(
            collector
                .push_chunk(ModelStreamChunk::output("req.other", 8, "wrong request"))
                .expect_err("chunks from another request must be rejected"),
        );
        assert_conflict(
            collector
                .push_chunk(ModelStreamChunk::output("req.expected", 7, "duplicate"))
                .expect_err("duplicate sequence must be rejected"),
        );
        assert_conflict(
            collector
                .push_chunk(ModelStreamChunk::output("req.expected", 6, "decreasing"))
                .expect_err("decreasing sequence must be rejected"),
        );
        assert_eq!(collector.chunks.len(), 1);
        assert_eq!(collector.chunks[0].content, "first");
    }

    #[test]
    fn stream_collector_clear_resets_sequence_for_retryable_fallback() {
        let mut collector = BoundedModelStreamCollector::new("req.retry");
        collector
            .push_chunk(ModelStreamChunk::output("req.retry", 0, "typed"))
            .expect("typed provider chunk accepted");

        collector.clear();
        collector
            .push_chunk(ModelStreamChunk::output("req.retry", 0, "fallback"))
            .expect("fallback stream starts a fresh sequence");
        assert_eq!(collector.chunks.len(), 1);
        assert_eq!(collector.chunks[0].content, "fallback");
    }

    #[test]
    fn bounded_forwarding_sink_preserves_events_and_rejects_invalid_chunks() {
        #[derive(Default)]
        struct RecordingSink {
            chunks: usize,
            events: usize,
        }

        impl ModelStreamSink for RecordingSink {
            fn push_chunk(&mut self, _chunk: ModelStreamChunk) -> KernelResult<()> {
                self.chunks += 1;
                Ok(())
            }

            fn push_event(&mut self, _event: KernelEvent) -> KernelResult<()> {
                self.events += 1;
                Ok(())
            }
        }

        let mut recording = RecordingSink::default();
        let mut bounded = BoundedModelStreamSink::new(&mut recording, "req.forward");
        bounded
            .push_event(KernelEvent::new(
                "event.req.forward.0",
                "agent.message.updated",
                sdkwork_agent_kernel::KernelEventSeverity::Info,
                "{}",
            ))
            .expect("kernel event should be forwarded");
        assert!(!bounded.is_empty());
        let error = bounded
            .push_chunk(ModelStreamChunk::output(
                "req.forward",
                0,
                "x".repeat(MAX_MODEL_STREAM_CHUNK_BYTES + 1),
            ))
            .expect_err("oversized chunk must not be forwarded");
        assert_resource_exhausted(error);
        assert_eq!(recording.chunks, 0);
        assert_eq!(recording.events, 1);
    }

    #[test]
    fn list_models_is_fail_closed_without_mock_fallback() {
        // Production construction (no runtime, no mock fallback) must not
        // surface the phantom mock catalog; it reports no models instead.
        let bridge = ModelBridge::new();
        assert!(bridge.list_models().is_empty());
    }

    #[test]
    fn list_models_returns_mock_catalog_only_under_mock_fallback() {
        let bridge = ModelBridge::with_mock_fallback_enabled();
        let models = bridge.list_models();
        assert_eq!(models.len(), 3);
    }
}
