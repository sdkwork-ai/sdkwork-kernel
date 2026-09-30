//! Contract tests for the kernel hook SPI.
//!
//! Hooks intercept model invocation, tool invocation, user prompts, and
//! session lifecycle at phase boundaries. Before-hooks can terminate work;
//! tool hooks can skip execution with a denied result.

use sdkwork_agent_kernel::{
    AgentChatRequest, AgentChatService, AgentExecutionRequest, AgentExecutionService,
    AgentExecutionStatus, AgentManifest, AgentStreamEvent, AgentStreamSink, HookAction,
    InMemoryAgentStreamSink, KernelErrorKind, KernelHook, KernelHookRegistry, KernelResult,
    ModelProvider, ModelRequest, ModelResponse, ModelUsage, ProviderHealth, ProviderManifest,
    RuntimeBuilder, ToolCall, ToolCallStatus, ToolDescriptor, ToolHookAction, ToolProvider,
    ToolResult,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const HOOK_AGENT_MANIFEST_JSON: &str = r#"
{
  "schema_version": "0.1.0",
  "manifest_type": "agent",
  "agent_id": "agent.hooks",
  "name": "sdkwork-hooks-agent",
  "display_name": "SDKWork Hooks Agent",
  "description": "Agent used to prove kernel hook contracts.",
  "version": "0.1.0",
  "domain": "intelligence",
  "required_capabilities": [
    {
      "capability_id": "model.chat",
      "min_version": "0.1.0"
    },
    {
      "capability_id": "model.tool_call",
      "min_version": "0.1.0"
    },
    {
      "capability_id": "policy.evaluate",
      "min_version": "0.1.0"
    }
  ],
  "optional_capabilities": [],
  "event_families": ["agent.runtime.*", "agent.stream.*"],
  "owner": {
    "name": "sdkwork-platform"
  },
  "status": "candidate"
}
"#;

/// Recording hook that counts every interception and can terminate or skip
/// on demand.
#[derive(Clone)]
struct RecordingHook {
    before_model: Arc<AtomicUsize>,
    after_model: Arc<AtomicUsize>,
    before_tool: Arc<AtomicUsize>,
    after_tool: Arc<AtomicUsize>,
    prompts: Arc<AtomicUsize>,
    sessions_started: Arc<AtomicUsize>,
    sessions_ended: Arc<AtomicUsize>,
    stops: Arc<AtomicUsize>,
    pre_compacts: Arc<AtomicUsize>,
    subagent_stops: Arc<AtomicUsize>,
    displays: Arc<AtomicUsize>,
    mode: HookMode,
}

#[derive(Clone, Copy, PartialEq)]
enum HookMode {
    Record,
    TerminateModel,
    SkipTool,
    ApprovePermission,
    DenyPermission,
}

impl RecordingHook {
    fn recording() -> Self {
        Self {
            before_model: Arc::new(AtomicUsize::new(0)),
            after_model: Arc::new(AtomicUsize::new(0)),
            before_tool: Arc::new(AtomicUsize::new(0)),
            after_tool: Arc::new(AtomicUsize::new(0)),
            prompts: Arc::new(AtomicUsize::new(0)),
            sessions_started: Arc::new(AtomicUsize::new(0)),
            sessions_ended: Arc::new(AtomicUsize::new(0)),
            stops: Arc::new(AtomicUsize::new(0)),
            pre_compacts: Arc::new(AtomicUsize::new(0)),
            subagent_stops: Arc::new(AtomicUsize::new(0)),
            displays: Arc::new(AtomicUsize::new(0)),
            mode: HookMode::Record,
        }
    }

    fn with_mode(mode: HookMode) -> Self {
        Self {
            mode,
            ..Self::recording()
        }
    }
}

impl KernelHook for RecordingHook {
    fn on_before_model_invoke(&self, _request: &ModelRequest) -> KernelResult<HookAction> {
        self.before_model.fetch_add(1, Ordering::Relaxed);
        if self.mode == HookMode::TerminateModel {
            Ok(HookAction::Terminate {
                reason: "contract terminated".to_string(),
            })
        } else {
            Ok(HookAction::Continue)
        }
    }

    fn on_after_model_invoke(
        &self,
        _request: &ModelRequest,
        _response: &ModelResponse,
    ) -> KernelResult<HookAction> {
        self.after_model.fetch_add(1, Ordering::Relaxed);
        Ok(HookAction::Continue)
    }

    fn on_before_tool_invoke(&self, _tool_call: &ToolCall) -> KernelResult<ToolHookAction> {
        self.before_tool.fetch_add(1, Ordering::Relaxed);
        if self.mode == HookMode::SkipTool {
            Ok(ToolHookAction::Skip {
                reason: "contract skipped".to_string(),
            })
        } else {
            Ok(ToolHookAction::Continue)
        }
    }

    fn on_after_tool_invoke(
        &self,
        _tool_call: &ToolCall,
        _result: &ToolResult,
    ) -> KernelResult<HookAction> {
        self.after_tool.fetch_add(1, Ordering::Relaxed);
        Ok(HookAction::Continue)
    }

    fn on_user_prompt(&self, _prompt: &str) -> KernelResult<HookAction> {
        self.prompts.fetch_add(1, Ordering::Relaxed);
        Ok(HookAction::Continue)
    }

    fn on_pre_compact(
        &self,
        _context: &sdkwork_agent_kernel::CompactBoundaryContext,
    ) -> KernelResult<HookAction> {
        self.pre_compacts.fetch_add(1, Ordering::Relaxed);
        Ok(HookAction::Continue)
    }

    fn on_subagent_stop(
        &self,
        _context: &sdkwork_agent_kernel::SubagentStopContext,
    ) -> KernelResult<()> {
        self.subagent_stops.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn on_message_display(
        &self,
        _context: &sdkwork_agent_kernel::MessageDisplayContext,
    ) -> KernelResult<HookAction> {
        self.displays.fetch_add(1, Ordering::Relaxed);
        Ok(HookAction::Continue)
    }

    fn on_permission_request(
        &self,
        _context: &sdkwork_agent_kernel::PermissionRequestContext,
    ) -> KernelResult<sdkwork_agent_kernel::PermissionHookAction> {
        match self.mode {
            HookMode::ApprovePermission => {
                Ok(sdkwork_agent_kernel::PermissionHookAction::Approve {
                    reason: "contract approved".to_string(),
                })
            }
            HookMode::DenyPermission => Ok(sdkwork_agent_kernel::PermissionHookAction::Deny {
                reason: "contract denied".to_string(),
            }),
            _ => Ok(sdkwork_agent_kernel::PermissionHookAction::Continue),
        }
    }

    fn on_session_start(&self, _session_id: &str) -> KernelResult<()> {
        self.sessions_started.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn on_session_end(&self, _session_id: &str, _reason: &str) -> KernelResult<()> {
        self.sessions_ended.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn on_stop(&self, _reason: &str) -> KernelResult<()> {
        self.stops.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Clone)]
struct StaticModelProvider {
    provider_id: String,
}

impl ModelProvider for StaticModelProvider {
    fn provider_manifest(&self) -> ProviderManifest {
        ProviderManifest::new(
            self.provider_id.clone(),
            "model",
            "static-model",
            "0.1.0",
            vec![
                "model.chat".to_string(),
                "model.tool_call".to_string(),
                "model.streaming".to_string(),
            ],
        )
    }

    fn health(&self) -> ProviderHealth {
        ProviderHealth::available()
    }

    fn invoke(&self, request: ModelRequest) -> KernelResult<ModelResponse> {
        Ok(ModelResponse::text(
            request.model_request_id,
            self.provider_id.clone(),
            "hook response",
        )
        .with_tool_call(
            ToolCall::new("tool-call.hook.1", "tool.hook.search", r#"{"query":"x"}"#)
                .with_provider("provider.tool.hook"),
        )
        .with_usage(ModelUsage::new(3, 1)))
    }

    fn stream(
        &self,
        request: ModelRequest,
    ) -> KernelResult<Vec<sdkwork_agent_kernel::ModelStreamChunk>> {
        Ok(vec![sdkwork_agent_kernel::ModelStreamChunk::output(
            request.model_request_id,
            1,
            "hook streamed",
        )])
    }
}

#[derive(Clone)]
struct HookToolProvider {
    provider_id: String,
}

impl ToolProvider for HookToolProvider {
    fn provider_manifest(&self) -> ProviderManifest {
        ProviderManifest::new(
            self.provider_id.clone(),
            "tool",
            "hook-tool",
            "0.1.0",
            vec!["tool.invoke".to_string()],
        )
    }

    fn health(&self) -> ProviderHealth {
        ProviderHealth::available()
    }

    fn list_tools(&self) -> Vec<ToolDescriptor> {
        vec![ToolDescriptor::new(
            "tool.hook.search",
            "provider.tool.hook",
            "search",
            sdkwork_agent_kernel::SideEffectLevel::ReadOnly,
        )]
    }

    fn invoke_tool(&self, tool_call: ToolCall) -> KernelResult<ToolResult> {
        Ok(ToolResult::succeeded(
            tool_call.tool_call_id,
            "hook tool output",
        ))
    }
}

#[derive(Clone)]
struct AllowPolicyProvider;

impl sdkwork_agent_kernel::PolicyProvider for AllowPolicyProvider {
    fn provider_manifest(&self) -> ProviderManifest {
        ProviderManifest::new(
            "provider.policy.hook",
            "policy",
            "allow-policy",
            "0.1.0",
            vec!["policy.evaluate".to_string()],
        )
    }

    fn health(&self) -> ProviderHealth {
        ProviderHealth::available()
    }

    fn evaluate(
        &self,
        request: sdkwork_agent_kernel::PolicyRequest,
    ) -> KernelResult<sdkwork_agent_kernel::PolicyDecision> {
        Ok(sdkwork_agent_kernel::PolicyDecision::allow(
            format!("decision.{}", request.policy_request_id),
            request.policy_request_id,
            "provider.policy.hook",
        ))
    }
}

fn hook_runtime(hook: Arc<dyn KernelHook>) -> sdkwork_agent_kernel::AgentRuntime {
    RuntimeBuilder::new(
        "runtime.hooks",
        AgentManifest::from_json(HOOK_AGENT_MANIFEST_JSON).expect("hook manifest parses"),
    )
    .with_generated_at("2026-08-01T00:00:00Z")
    .register_kernel_hook(hook)
    .register_model_provider(
        "provider.hook",
        "0.1.0",
        StaticModelProvider {
            provider_id: "provider.hook".to_string(),
        },
    )
    .register_tool_provider(
        "provider.tool.hook",
        "0.1.0",
        HookToolProvider {
            provider_id: "provider.tool.hook".to_string(),
        },
    )
    .register_policy_provider("provider.policy.hook", "0.1.0", AllowPolicyProvider)
    .bootstrap()
    .expect("hook runtime bootstraps")
    .runtime
}

#[test]
fn hook_action_vocabulary_is_stable() {
    assert_eq!(HookAction::Continue, HookAction::Continue);
    assert_eq!(
        HookAction::Terminate {
            reason: "x".to_string()
        },
        HookAction::Terminate {
            reason: "x".to_string()
        }
    );
    assert!(matches!(
        ToolHookAction::Skip {
            reason: "r".to_string()
        },
        ToolHookAction::Skip { .. }
    ));
    assert!(matches!(
        ToolHookAction::Terminate {
            reason: "r".to_string()
        },
        ToolHookAction::Terminate { .. }
    ));
}

#[test]
fn hook_registry_runs_all_hook_points() {
    let hook = Arc::new(RecordingHook::recording());
    let runtime = hook_runtime(hook.clone());

    // Session lifecycle hooks.
    runtime.hooks().run_session_start("session.hook").unwrap();
    runtime
        .hooks()
        .run_session_end("session.hook", "completed")
        .unwrap();
    runtime.hooks().run_stop("stop").unwrap();

    // Model + tool + prompt hooks through a full execution.
    AgentExecutionService::new()
        .execute(
            &runtime,
            AgentExecutionRequest::new("exec.hook.1", vec!["hello".to_string()])
                .for_session("session.hook"),
        )
        .expect("execution with hooks succeeds");

    assert!(hook.before_model.load(Ordering::Relaxed) >= 1);
    assert!(hook.after_model.load(Ordering::Relaxed) >= 1);
    assert!(hook.before_tool.load(Ordering::Relaxed) >= 1);
    assert!(hook.after_tool.load(Ordering::Relaxed) >= 1);
    assert!(hook.prompts.load(Ordering::Relaxed) >= 1);
    assert_eq!(hook.sessions_started.load(Ordering::Relaxed), 1);
    assert_eq!(hook.sessions_ended.load(Ordering::Relaxed), 1);
    assert_eq!(hook.stops.load(Ordering::Relaxed), 1);
}

#[test]
fn before_model_termination_aborts_execution() {
    let hook = Arc::new(RecordingHook::with_mode(HookMode::TerminateModel));
    let runtime = hook_runtime(hook);

    let report = AgentExecutionService::new()
        .execute(
            &runtime,
            AgentExecutionRequest::new("exec.hook.terminate", vec!["hello".to_string()]),
        )
        .expect("terminated execution returns a report");

    assert_eq!(report.status, AgentExecutionStatus::Cancelled);
    let error = report.error.as_ref().expect("report carries error");
    assert_eq!(error.kind(), KernelErrorKind::Cancelled);
    assert!(error.to_string().contains("kernel hook"));
}

#[test]
fn tool_skip_returns_denied_result_without_provider_invocation() {
    let hook = Arc::new(RecordingHook::with_mode(HookMode::SkipTool));
    let runtime = hook_runtime(hook);

    let report = AgentExecutionService::new()
        .execute(
            &runtime,
            AgentExecutionRequest::new("exec.hook.skip", vec!["hello".to_string()]),
        )
        .expect("skipped execution returns a report");

    let tool_execution = &report.tool_executions[0];
    assert_eq!(
        tool_execution.result.normalized_status,
        ToolCallStatus::Denied
    );
    assert!(tool_execution
        .result
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("contract skipped"));
}

#[test]
fn chat_stream_events_honor_model_hooks() {
    let hook = Arc::new(RecordingHook::recording());
    let runtime = hook_runtime(hook.clone());
    let mut sink = InMemoryAgentStreamSink::new();

    AgentChatService::new()
        .stream_events(
            &runtime,
            AgentChatRequest::new("chat.hook.1", vec!["hello".to_string()]),
            &mut sink,
        )
        .expect("chat stream with hooks succeeds");

    let types: Vec<&str> = sink.events().iter().map(|e| e.event_type()).collect();
    assert_eq!(types[0], "agent.stream.message.start");
    assert_eq!(types.last().unwrap(), &"agent.stream.ended");
    assert!(hook.before_model.load(Ordering::Relaxed) >= 1);
}

#[test]
fn permission_hook_approve_still_passes_policy_flow() {
    let hook = Arc::new(RecordingHook::with_mode(HookMode::ApprovePermission));
    let runtime = hook_runtime(hook);

    let report = AgentExecutionService::new()
        .execute(
            &runtime,
            AgentExecutionRequest::new("exec.hook.approve", vec!["hello".to_string()]),
        )
        .expect("approved execution succeeds");

    assert_eq!(report.status, AgentExecutionStatus::Completed);
    let tool_execution = &report.tool_executions[0];
    assert_eq!(
        tool_execution.result.normalized_status,
        ToolCallStatus::Succeeded
    );
    // The hook approval substitutes for interactive human approval only:
    // the policy provider still evaluated the call (fail-closed), so the
    // recorded decision names the policy provider, not the hook.
    assert_ne!(
        tool_execution.policy_decision.policy_provider_id,
        "kernel.hook"
    );
}

#[test]
fn permission_hook_deny_returns_denied_result() {
    let hook = Arc::new(RecordingHook::with_mode(HookMode::DenyPermission));
    let runtime = hook_runtime(hook);

    let report = AgentExecutionService::new()
        .execute(
            &runtime,
            AgentExecutionRequest::new("exec.hook.deny", vec!["hello".to_string()]),
        )
        .expect("denied execution returns a report");

    let tool_execution = &report.tool_executions[0];
    assert_eq!(
        tool_execution.result.normalized_status,
        ToolCallStatus::Denied
    );
    assert!(tool_execution
        .result
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("contract denied"));
}

#[test]
fn extended_hook_points_fire_on_lifecycle_and_display() {
    let hook = Arc::new(RecordingHook::recording());
    let runtime = hook_runtime(hook.clone());

    // Pre-compact interception.
    let action = runtime
        .hooks()
        .run_pre_compact(&sdkwork_agent_kernel::CompactBoundaryContext::new(
            "session.hook",
            120,
        ))
        .expect("pre compact hook runs");
    assert_eq!(action, HookAction::Continue);
    assert_eq!(hook.pre_compacts.load(Ordering::Relaxed), 1);

    // Message display interception.
    let action = runtime
        .hooks()
        .run_message_display(&sdkwork_agent_kernel::MessageDisplayContext::new(
            "msg.1", "agent",
        ))
        .expect("display hook runs");
    assert_eq!(action, HookAction::Continue);
    assert_eq!(hook.displays.load(Ordering::Relaxed), 1);

    // Sub-agent stop interception.
    runtime
        .hooks()
        .run_subagent_stop(&sdkwork_agent_kernel::SubagentStopContext::new(
            "delegation.1",
            "session.subagent.1",
            "completed",
            3,
        ))
        .expect("subagent stop hook runs");
    assert_eq!(hook.subagent_stops.load(Ordering::Relaxed), 1);
}

#[test]
fn delegation_stream_fires_subagent_stop_hook() {
    use sdkwork_agent_kernel::{
        AgentDelegationService, AgentDelegationStreamRequest, AgentExecutionRequest,
    };

    // Reuse the delegation runtime shape from this test file's model
    // provider: full runtime with model/tool/policy providers.
    let hook = Arc::new(RecordingHook::recording());
    let runtime = RuntimeBuilder::new(
        "runtime.hooks-delegation",
        AgentManifest::from_json(HOOK_AGENT_MANIFEST_JSON).expect("hook manifest parses"),
    )
    .with_generated_at("2026-08-01T00:00:00Z")
    .register_kernel_hook(hook.clone())
    .register_model_provider(
        "provider.hook",
        "0.1.0",
        StaticModelProvider {
            provider_id: "provider.hook".to_string(),
        },
    )
    .register_tool_provider(
        "provider.tool.hook",
        "0.1.0",
        HookToolProvider {
            provider_id: "provider.tool.hook".to_string(),
        },
    )
    .register_policy_provider("provider.policy.hook", "0.1.0", AllowPolicyProvider)
    .bootstrap()
    .expect("delegation hook runtime bootstraps")
    .runtime;

    let mut sink = InMemoryAgentStreamSink::new();
    AgentDelegationService::new()
        .delegate_streaming(
            &runtime,
            AgentDelegationStreamRequest::from_tool_call(
                "delegation.hook.1",
                "session.parent",
                "tool-call.delegate.9",
                "child task",
            ),
            &mut sink,
        )
        .expect("delegation stream succeeds");

    assert!(
        hook.subagent_stops.load(Ordering::Relaxed) >= 1,
        "sub-agent stop hook must fire after delegation"
    );
}

#[test]
fn kernel_hook_registry_is_composable() {
    let mut registry = KernelHookRegistry::new();
    let first = Arc::new(RecordingHook::recording());
    let second = Arc::new(RecordingHook::recording());
    registry.register(first);
    registry.register(second);
    assert_eq!(registry.len(), 2);
    assert!(!registry.is_empty());

    let request = ModelRequest::new("model.1", vec!["hi".to_string()]);
    assert_eq!(
        registry
            .run_before_model_invoke(&request)
            .expect("hooks run"),
        HookAction::Continue
    );
}
