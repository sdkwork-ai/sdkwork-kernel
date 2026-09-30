use crate::{
    AgentRuntime, KernelError, KernelErrorSource, KernelEvent, KernelEventRedaction,
    KernelEventSeverity, KernelEventSource, KernelResult, PolicyCategory, PolicyDecision,
    PolicyDecisionValue, PolicyRequest, ProviderHealth, ProviderManifest, TraceContext,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideEffectLevel {
    ReadOnly,
    SideEffectful,
    Destructive,
    ExternalSend,
    Privileged,
}

impl SideEffectLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::SideEffectful => "side_effectful",
            Self::Destructive => "destructive",
            Self::ExternalSend => "external_send",
            Self::Privileged => "privileged",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDescriptor {
    pub tool_id: String,
    pub provider_id: String,
    pub name: Option<String>,
    pub display_name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub input_schema: Option<ToolSchema>,
    pub output_schema: Option<ToolSchema>,
    pub side_effect_level: SideEffectLevel,
    pub policy_categories: Vec<String>,
    pub timeout_ms: Option<u64>,
    pub cancellation_supported: bool,
    pub audit_required: bool,
}

impl ToolDescriptor {
    pub fn new(
        tool_id: impl Into<String>,
        provider_id: impl Into<String>,
        display_name: impl Into<String>,
        side_effect_level: SideEffectLevel,
    ) -> Self {
        Self {
            tool_id: tool_id.into(),
            provider_id: provider_id.into(),
            name: None,
            display_name: display_name.into(),
            version: None,
            description: None,
            input_schema: None,
            output_schema: None,
            side_effect_level,
            policy_categories: Vec::new(),
            timeout_ms: None,
            cancellation_supported: false,
            audit_required: false,
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_input_schema(mut self, input_schema: ToolSchema) -> Self {
        self.input_schema = Some(input_schema);
        self
    }

    pub fn with_output_schema(mut self, output_schema: ToolSchema) -> Self {
        self.output_schema = Some(output_schema);
        self
    }

    pub fn with_policy_categories(mut self, policy_categories: Vec<String>) -> Self {
        self.policy_categories = policy_categories;
        self
    }

    pub fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn supports_cancellation(mut self, cancellation_supported: bool) -> Self {
        self.cancellation_supported = cancellation_supported;
        self
    }

    pub fn require_audit(mut self) -> Self {
        self.audit_required = true;
        self
    }

    pub fn requires_policy(&self) -> bool {
        self.side_effect_level != SideEffectLevel::ReadOnly || !self.policy_categories.is_empty()
    }

    pub fn policy_request(
        &self,
        policy_request_id: impl Into<String>,
        call: &ToolCall,
    ) -> PolicyRequest {
        let category = self
            .policy_categories
            .first()
            .cloned()
            .unwrap_or_else(|| "tool.invoke".to_string());
        let mut request =
            PolicyRequest::new(policy_request_id, category.clone(), self.tool_id.clone())
                .with_category(PolicyCategory::ProductSpecific(category))
                .with_action("invoke")
                .with_side_effect_level(self.side_effect_level)
                .with_redaction(KernelEventRedaction::Internal);

        if let Some(session_id) = &call.session_id {
            request = request.with_session(session_id.clone());
        }

        if let Some(task_id) = &call.task_id {
            request = request.with_task(task_id.clone());
        }

        if let Some(run_id) = &call.run_id {
            request = request.with_run(run_id.clone());
        }

        if let Some(step_id) = &call.step_id {
            request = request.with_context("step_id", step_id.clone());
        }

        request
    }
}

/// Tool schema contract.
///
/// `schema_id` is the stable registry identity; `document` carries the JSON
/// Schema body (Draft 2020-12 text) so tool/skill contracts are
/// self-describing without a registry lookup. Stored as text to keep the
/// kernel types `Eq`-compatible; use [`ToolSchema::document_json`] for the
/// parsed value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSchema {
    pub schema_id: String,
    /// JSON Schema document text (Draft 2020-12) describing the contract.
    pub document: Option<String>,
    /// JSON Schema dialect identifier, e.g. `https://json-schema.org/draft/2020-12/schema`.
    pub dialect: Option<String>,
}

impl ToolSchema {
    pub fn json_schema(schema_id: impl Into<String>) -> Self {
        Self {
            schema_id: schema_id.into(),
            document: None,
            dialect: None,
        }
    }

    /// Attach a parsed JSON Schema document.
    pub fn with_document(mut self, document: serde_json::Value) -> Self {
        self.document = Some(document.to_string());
        self
    }

    /// Attach a raw JSON Schema document text.
    pub fn with_document_text(mut self, document: impl Into<String>) -> Self {
        self.document = Some(document.into());
        self
    }

    /// Attach the JSON Schema dialect identifier.
    pub fn with_dialect(mut self, dialect: impl Into<String>) -> Self {
        self.dialect = Some(dialect.into());
        self
    }

    /// Parsed JSON Schema document, when present and well-formed.
    pub fn document_json(&self) -> Option<serde_json::Value> {
        self.document
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok())
    }

    /// Whether this schema carries a concrete JSON Schema document.
    pub fn has_document(&self) -> bool {
        self.document.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub tool_call_id: String,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
    pub step_id: Option<String>,
    pub tool_id: String,
    pub provider_id: Option<String>,
    pub arguments: String,
    pub trace_context: Option<TraceContext>,
    pub policy_decision_id: Option<String>,
    pub timeout_ms: Option<u64>,
    pub created_at: Option<String>,
}

impl ToolCall {
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_id: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            session_id: None,
            task_id: None,
            run_id: None,
            step_id: None,
            tool_id: tool_id.into(),
            provider_id: None,
            arguments: arguments.into(),
            trace_context: None,
            policy_decision_id: None,
            timeout_ms: None,
            created_at: None,
        }
    }

    pub fn for_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn for_task(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    pub fn for_run(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    pub fn for_step(mut self, step_id: impl Into<String>) -> Self {
        self.step_id = Some(step_id.into());
        self
    }

    pub fn with_provider(mut self, provider_id: impl Into<String>) -> Self {
        self.provider_id = Some(provider_id.into());
        self
    }

    pub fn with_policy_decision(mut self, policy_decision_id: impl Into<String>) -> Self {
        self.policy_decision_id = Some(policy_decision_id.into());
        self
    }

    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = Some(trace_context);
        self
    }

    pub fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn created_at(mut self, created_at: impl Into<String>) -> Self {
        self.created_at = Some(created_at.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub status: String,
    pub normalized_status: ToolCallStatus,
    pub output: String,
    pub error: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub duration_ms: Option<u64>,
    pub trace_context: Option<TraceContext>,
    pub redaction_classification: KernelEventRedaction,
    pub audit_refs: Vec<String>,
}

impl ToolResult {
    pub fn succeeded(tool_call_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            status: "succeeded".to_string(),
            normalized_status: ToolCallStatus::Succeeded,
            output: output.into(),
            error: None,
            started_at: None,
            completed_at: None,
            duration_ms: None,
            trace_context: None,
            redaction_classification: KernelEventRedaction::Unknown,
            audit_refs: Vec::new(),
        }
    }

    pub fn failed(tool_call_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            status: "failed".to_string(),
            normalized_status: ToolCallStatus::Failed,
            output: String::new(),
            error: Some(error.into()),
            started_at: None,
            completed_at: None,
            duration_ms: None,
            trace_context: None,
            redaction_classification: KernelEventRedaction::Unknown,
            audit_refs: Vec::new(),
        }
    }

    /// Tool result for a hook-skipped or policy-denied invocation: the tool
    /// did not execute and the reason is delivered back to the model.
    pub fn denied(tool_call_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            status: "denied".to_string(),
            normalized_status: ToolCallStatus::Denied,
            output: String::new(),
            error: Some(reason.into()),
            started_at: None,
            completed_at: None,
            duration_ms: None,
            trace_context: None,
            redaction_classification: KernelEventRedaction::Internal,
            audit_refs: Vec::new(),
        }
    }

    pub fn with_status(mut self, status: ToolCallStatus) -> Self {
        self.status = status.as_str().to_string();
        self.normalized_status = status;
        self
    }

    pub fn started_at(mut self, started_at: impl Into<String>) -> Self {
        self.started_at = Some(started_at.into());
        self
    }

    pub fn completed_at(mut self, completed_at: impl Into<String>) -> Self {
        self.completed_at = Some(completed_at.into());
        self
    }

    pub fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = Some(trace_context);
        self
    }

    pub fn with_redaction(mut self, redaction_classification: KernelEventRedaction) -> Self {
        self.redaction_classification = redaction_classification;
        self
    }

    pub fn with_audit_ref(mut self, audit_ref: impl Into<String>) -> Self {
        self.audit_refs.push(audit_ref.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallStatus {
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Denied,
    InvalidInput,
}

impl ToolCallStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Denied => "denied",
            Self::InvalidInput => "invalid_input",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStreamChunk {
    pub tool_call_id: String,
    pub sequence: u64,
    pub content: String,
    pub trace_context: Option<TraceContext>,
    pub redaction_classification: KernelEventRedaction,
}

impl ToolStreamChunk {
    pub fn output(
        tool_call_id: impl Into<String>,
        sequence: u64,
        content: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            sequence,
            content: content.into(),
            trace_context: None,
            redaction_classification: KernelEventRedaction::Unknown,
        }
    }

    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = Some(trace_context);
        self
    }

    pub fn with_redaction(mut self, redaction_classification: KernelEventRedaction) -> Self {
        self.redaction_classification = redaction_classification;
        self
    }

    pub fn to_event(&self, event_id: impl Into<String>) -> KernelEvent {
        let mut event = KernelEvent::new(
            event_id,
            "agent.tool.call.output_streamed",
            KernelEventSeverity::Info,
            format!(
                "tool_call_id={};sequence={};chunk={}",
                self.tool_call_id, self.sequence, self.content
            ),
        )
        .from_source(KernelEventSource::Tool)
        .with_redaction(self.redaction_classification)
        .with_payload_schema("sdkwork.agent.tool.stream_chunk.v1");

        if let Some(trace_context) = &self.trace_context {
            event = event.with_trace_context(trace_context.clone());
        }

        event
    }
}

pub trait ToolProvider {
    fn provider_manifest(&self) -> ProviderManifest {
        let provider_id = self
            .list_tools()
            .first()
            .map(|descriptor| descriptor.provider_id.clone())
            .unwrap_or_else(|| "provider.tool.unspecified".to_string());

        ProviderManifest::new(
            provider_id,
            "tool",
            "tool-provider",
            "0.0.0",
            vec!["tool.invoke".to_string()],
        )
    }

    fn list_tools(&self) -> Vec<ToolDescriptor>;

    fn health(&self) -> ProviderHealth;

    fn invoke_tool(&self, call: ToolCall) -> KernelResult<ToolResult>;

    fn describe_tool(&self, tool_id: &str) -> KernelResult<ToolDescriptor> {
        self.list_tools()
            .into_iter()
            .find(|descriptor| descriptor.tool_id == tool_id)
            .ok_or_else(|| KernelError::CapabilityMissing {
                capability_id: tool_id.to_string(),
            })
    }

    fn authorize_tool_call(
        &self,
        descriptor: &ToolDescriptor,
        call: &ToolCall,
    ) -> KernelResult<PolicyRequest> {
        Ok(descriptor.policy_request(format!("policy-request.{}", call.tool_call_id), call))
    }

    fn stream_tool_call(&self, _call: ToolCall) -> KernelResult<Vec<ToolStreamChunk>> {
        Err(KernelError::CapabilityMissing {
            capability_id: "tool.streaming".to_string(),
        })
    }

    fn cancel_tool_call(&self, _tool_call_id: &str) -> KernelResult<ToolResult> {
        Err(KernelError::CapabilityMissing {
            capability_id: "tool.cancellation".to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionRequest {
    pub tool_execution_id: String,
    pub tool_call: ToolCall,
}

/// One-shot authorization for resuming a previously approved tool call.
///
/// The caller must load this data from the fenced durable permission operation.
/// The execution service still re-evaluates current policy and verifies provider
/// revisions before invoking the tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedToolExecution {
    pub permission_request_id: String,
    pub provider_id: String,
    pub descriptor_revision: String,
    pub policy_revision: String,
}

impl ApprovedToolExecution {
    pub fn new(
        permission_request_id: impl Into<String>,
        provider_id: impl Into<String>,
        descriptor_revision: impl Into<String>,
        policy_revision: impl Into<String>,
    ) -> Self {
        Self {
            permission_request_id: permission_request_id.into(),
            provider_id: provider_id.into(),
            descriptor_revision: descriptor_revision.into(),
            policy_revision: policy_revision.into(),
        }
    }
}

impl ToolExecutionRequest {
    pub fn new(tool_execution_id: impl Into<String>, tool_call: ToolCall) -> Self {
        Self {
            tool_execution_id: tool_execution_id.into(),
            tool_call,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionResponse {
    pub tool_execution_id: String,
    pub provider_id: String,
    pub descriptor: ToolDescriptor,
    pub policy_decision: PolicyDecision,
    pub result: ToolResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStreamExecutionResponse {
    pub tool_execution_id: String,
    pub provider_id: String,
    pub descriptor: ToolDescriptor,
    pub policy_decision: PolicyDecision,
    pub chunks: Vec<ToolStreamChunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCancellationRequest {
    pub tool_cancellation_id: String,
    pub provider_id: Option<String>,
    pub tool_call_id: String,
}

impl ToolCancellationRequest {
    pub fn new(tool_cancellation_id: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Self {
            tool_cancellation_id: tool_cancellation_id.into(),
            provider_id: None,
            tool_call_id: tool_call_id.into(),
        }
    }

    pub fn with_provider_id(mut self, provider_id: impl Into<String>) -> Self {
        self.provider_id = Some(provider_id.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCancellationResponse {
    pub tool_cancellation_id: String,
    pub provider_id: String,
    pub result: ToolResult,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ToolExecutionService;

impl ToolExecutionService {
    pub fn new() -> Self {
        Self
    }

    pub fn invoke(
        &self,
        runtime: &AgentRuntime,
        request: ToolExecutionRequest,
    ) -> KernelResult<ToolExecutionResponse> {
        let provider = self.select_provider(runtime, request.tool_call.provider_id.as_deref())?;

        // Permission hook: hooks may approve or deny before the normal
        // policy flow, mirroring the agent SDK permission hooks.
        let permission_context = crate::PermissionRequestContext::for_tool_call(
            format!("permission.{}", request.tool_call.tool_call_id),
            request.tool_call.tool_call_id.clone(),
            request.tool_call.tool_id.clone(),
        );
        match runtime
            .hooks()
            .run_permission_request(&permission_context)?
        {
            crate::PermissionHookAction::Continue => {}
            crate::PermissionHookAction::Approve { reason } => {
                // A hook approval substitutes for the interactive human
                // approval only; the policy provider still gates the call
                // (fail-closed), and an explicit policy Deny still wins.
                let descriptor = provider.describe_tool(&request.tool_call.tool_id)?;
                let policy_request =
                    provider.authorize_tool_call(&descriptor, &request.tool_call)?;
                let mut policy_decision = runtime.policy_provider()?.evaluate(policy_request)?;
                if policy_decision.decision == PolicyDecisionValue::NeedsApproval {
                    policy_decision =
                        PolicyDecision::allow(
                            format!("hook-approve.{}", request.tool_call.tool_call_id),
                            permission_context.permission_request_id.clone(),
                            "kernel.hook",
                        )
                        .with_safe_reason(reason);
                }
                self.ensure_allowed(&policy_decision, &descriptor, &request.tool_call)?;
                let tool_call =
                    self.with_policy_metadata(request.tool_call, &descriptor, &policy_decision);
                let result = provider.invoke_tool(tool_call.clone())?;
                let _ = runtime.hooks().run_after_tool_invoke(&tool_call, &result)?;
                return Ok(ToolExecutionResponse {
                    tool_execution_id: request.tool_execution_id,
                    provider_id: descriptor.provider_id.clone(),
                    descriptor,
                    policy_decision,
                    result,
                });
            }
            crate::PermissionHookAction::Deny { reason } => {
                let descriptor = provider.describe_tool(&request.tool_call.tool_id)?;
                let policy_decision = PolicyDecision::deny(
                    format!("hook-deny.{}", request.tool_call.tool_call_id),
                    permission_context.permission_request_id.clone(),
                    "kernel.hook",
                    "hook_denied",
                )
                .with_safe_reason(reason.clone());
                let result = ToolResult::denied(request.tool_call.tool_call_id, reason);
                return Ok(ToolExecutionResponse {
                    tool_execution_id: request.tool_execution_id,
                    provider_id: descriptor.provider_id.clone(),
                    descriptor,
                    policy_decision,
                    result,
                });
            }
        }

        let (descriptor, policy_decision) =
            self.authorize_tool_call(runtime, provider, &request.tool_call)?;
        let tool_call = self.with_policy_metadata(request.tool_call, &descriptor, &policy_decision);

        match runtime.hooks().run_before_tool_invoke(&tool_call)? {
            crate::ToolHookAction::Continue => {}
            crate::ToolHookAction::Skip { reason } => {
                let result = ToolResult::denied(tool_call.tool_call_id.clone(), reason);
                let _ = runtime.hooks().run_after_tool_invoke(&tool_call, &result)?;
                return Ok(ToolExecutionResponse {
                    tool_execution_id: request.tool_execution_id,
                    provider_id: descriptor.provider_id.clone(),
                    descriptor,
                    policy_decision,
                    result,
                });
            }
            crate::ToolHookAction::Terminate { reason } => {
                return Err(KernelError::cancelled(format!(
                    "tool invocation terminated by kernel hook: {reason}"
                ))
                .from_source(KernelErrorSource::Runtime));
            }
        }

        let result = provider.invoke_tool(tool_call.clone())?;
        let _ = runtime.hooks().run_after_tool_invoke(&tool_call, &result)?;

        Ok(ToolExecutionResponse {
            tool_execution_id: request.tool_execution_id,
            provider_id: descriptor.provider_id.clone(),
            descriptor,
            policy_decision,
            result,
        })
    }

    pub fn invoke_approved(
        &self,
        runtime: &AgentRuntime,
        request: ToolExecutionRequest,
        approval: &ApprovedToolExecution,
    ) -> KernelResult<ToolExecutionResponse> {
        let provider = self.select_provider(runtime, Some(&approval.provider_id))?;
        let descriptor = provider.describe_tool(&request.tool_call.tool_id)?;
        let provider_manifest = provider.provider_manifest();
        let descriptor_revision = descriptor
            .version
            .as_deref()
            .unwrap_or(provider_manifest.version.as_str());
        if descriptor.provider_id != approval.provider_id
            || descriptor_revision != approval.descriptor_revision
        {
            return Err(KernelError::PolicyDenied {
                reason_code: "tool_descriptor_revision_changed".to_string(),
            });
        }

        let policy_provider = runtime.policy_provider()?;
        if policy_provider.provider_manifest().version != approval.policy_revision {
            return Err(KernelError::PolicyDenied {
                reason_code: "policy_revision_changed".to_string(),
            });
        }

        let policy_request = provider.authorize_tool_call(&descriptor, &request.tool_call)?;
        let current_decision = policy_provider.evaluate(policy_request)?;
        let policy_decision = match current_decision.decision {
            PolicyDecisionValue::Allow => current_decision,
            PolicyDecisionValue::NeedsApproval
                if current_decision.request_id == approval.permission_request_id =>
            {
                let approved = PolicyDecision::allow(
                    format!("permission-approval.{}", approval.permission_request_id),
                    current_decision.request_id,
                    current_decision.policy_provider_id,
                )
                .with_safe_reason("approved one-shot tool execution")
                .require_audit();
                policy_provider.record_decision(&approved)?;
                approved
            }
            PolicyDecisionValue::NeedsApproval => {
                return Err(KernelError::PolicyDenied {
                    reason_code: "permission_request_identity_changed".to_string(),
                });
            }
            PolicyDecisionValue::Deny => {
                return Err(KernelError::PolicyDenied {
                    reason_code: current_decision.reason_code,
                });
            }
            PolicyDecisionValue::Defer => {
                return Err(KernelError::provider_error(
                    "policy.deferred",
                    current_decision.reason_code,
                ));
            }
        };

        let tool_call = self.with_policy_metadata(request.tool_call, &descriptor, &policy_decision);
        let result = provider.invoke_tool(tool_call)?;
        Ok(ToolExecutionResponse {
            tool_execution_id: request.tool_execution_id,
            provider_id: descriptor.provider_id.clone(),
            descriptor,
            policy_decision,
            result,
        })
    }

    pub fn stream(
        &self,
        runtime: &AgentRuntime,
        request: ToolExecutionRequest,
    ) -> KernelResult<ToolStreamExecutionResponse> {
        let provider = self.select_provider(runtime, request.tool_call.provider_id.as_deref())?;
        let (descriptor, policy_decision) =
            self.authorize_tool_call(runtime, provider, &request.tool_call)?;
        let tool_call = self.with_policy_metadata(request.tool_call, &descriptor, &policy_decision);
        let chunks = provider.stream_tool_call(tool_call)?;

        Ok(ToolStreamExecutionResponse {
            tool_execution_id: request.tool_execution_id,
            provider_id: descriptor.provider_id.clone(),
            descriptor,
            policy_decision,
            chunks,
        })
    }

    pub fn cancel(
        &self,
        runtime: &AgentRuntime,
        request: ToolCancellationRequest,
    ) -> KernelResult<ToolCancellationResponse> {
        let provider = self.select_provider(runtime, request.provider_id.as_deref())?;
        let provider_id = provider.provider_manifest().provider_id;
        let result = provider.cancel_tool_call(&request.tool_call_id)?;

        Ok(ToolCancellationResponse {
            tool_cancellation_id: request.tool_cancellation_id,
            provider_id,
            result,
        })
    }

    fn select_provider<'a>(
        &self,
        runtime: &'a AgentRuntime,
        provider_id: Option<&str>,
    ) -> KernelResult<&'a (dyn ToolProvider + Send + Sync)> {
        match provider_id {
            Some(provider_id) => runtime.tool_provider_by_id(provider_id),
            None => runtime.tool_provider(),
        }
    }

    fn authorize_tool_call(
        &self,
        runtime: &AgentRuntime,
        provider: &(dyn ToolProvider + Send + Sync),
        tool_call: &ToolCall,
    ) -> KernelResult<(ToolDescriptor, PolicyDecision)> {
        let descriptor = provider.describe_tool(&tool_call.tool_id)?;
        let policy_request = provider.authorize_tool_call(&descriptor, tool_call)?;
        let policy_decision = runtime.policy_provider()?.evaluate(policy_request)?;
        self.ensure_allowed(&policy_decision, &descriptor, tool_call)
            .map_err(|error| {
                if error.kind() != crate::KernelErrorKind::PermissionRequired {
                    return error;
                }
                let provider_manifest = provider.provider_manifest();
                let descriptor_revision = descriptor
                    .version
                    .as_deref()
                    .unwrap_or(provider_manifest.version.as_str());
                let policy_revision = runtime
                    .policy_provider_by_id(&policy_decision.policy_provider_id)
                    .map(|policy| policy.provider_manifest().version)
                    .unwrap_or_default();
                error
                    .with_detail("tool_call_id", tool_call.tool_call_id.clone())
                    .with_detail("provider_id", descriptor.provider_id.clone())
                    .with_detail("descriptor_revision", descriptor_revision)
                    .with_detail("policy_revision", policy_revision)
            })?;
        Ok((descriptor, policy_decision))
    }

    fn with_policy_metadata(
        &self,
        mut tool_call: ToolCall,
        descriptor: &ToolDescriptor,
        policy_decision: &PolicyDecision,
    ) -> ToolCall {
        tool_call.policy_decision_id = Some(policy_decision.decision_id.clone());
        if tool_call.provider_id.is_none() {
            tool_call.provider_id = Some(descriptor.provider_id.clone());
        }

        tool_call
    }

    fn ensure_allowed(
        &self,
        policy_decision: &PolicyDecision,
        descriptor: &ToolDescriptor,
        tool_call: &ToolCall,
    ) -> KernelResult<()> {
        match policy_decision.decision {
            PolicyDecisionValue::Allow => Ok(()),
            PolicyDecisionValue::Deny => Err(KernelError::PolicyDenied {
                reason_code: policy_decision.reason_code.clone(),
            }),
            PolicyDecisionValue::NeedsApproval => {
                let mut error = KernelError::permission_required(
                    policy_decision
                        .safe_reason
                        .clone()
                        .unwrap_or_else(|| policy_decision.reason_code.clone()),
                )
                .from_source(crate::KernelErrorSource::Policy)
                .with_detail("permission_request_id", policy_decision.request_id.clone())
                .with_detail("policy_decision_id", policy_decision.decision_id.clone())
                .with_detail("policy_category", "tool.invoke")
                .with_detail("resource", descriptor.tool_id.clone())
                .with_detail("side_effect_level", descriptor.side_effect_level.as_str());
                if let Some(session_id) = &tool_call.session_id {
                    error = error.with_detail("session_id", session_id.clone());
                }
                Err(error)
            }
            PolicyDecisionValue::Defer => Err(KernelError::provider_error(
                "policy.deferred",
                policy_decision.reason_code.clone(),
            )),
        }
    }
}
