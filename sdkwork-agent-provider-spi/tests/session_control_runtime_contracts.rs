use sdkwork_agent_kernel::{
    ProviderSessionControlAction, ProviderSessionControlOutput, ProviderSessionControlProvider,
    ProviderSessionControlRequest,
};
use sdkwork_agent_provider_spi::{
    CapabilityExecutionScope, NegotiatedCapability, SdkBackendKind, SdkBackendRuntime,
    SdkCapabilityNegotiation, SdkDriverHealth, SdkRuntimeBackedSessionControlProvider,
    SdkRuntimeOperation, SdkRuntimeOperationKind, SdkRuntimeRequest, SdkRuntimeResponse,
    SdkRuntimeRouter, SDK_CAPABILITY_SESSION_CONTROL,
};
use std::sync::{Arc, Mutex};

struct CapturingRuntime {
    requests: Arc<Mutex<Vec<SdkRuntimeRequest>>>,
}

impl SdkBackendRuntime for CapturingRuntime {
    fn backend_kind(&self) -> SdkBackendKind {
        SdkBackendKind::TypeScriptNode
    }

    fn health(&self) -> SdkDriverHealth {
        SdkDriverHealth::healthy()
    }

    fn invoke(
        &self,
        request: &SdkRuntimeRequest,
    ) -> Result<SdkRuntimeResponse, sdkwork_agent_provider_spi::SdkRuntimeError> {
        self.requests.lock().unwrap().push(request.clone());
        let payload = match &request.operation {
            SdkRuntimeOperation::SessionFork { .. } => serde_json::json!({
                "status": "applied",
                "provider_session_id": "provider-session-source",
                "forked_provider_session_id": "provider-session-forked"
            }),
            _ => serde_json::json!({
                "status": "applied",
                "provider_session_id": "provider-session-source"
            }),
        };
        Ok(SdkRuntimeResponse::success(
            SdkBackendKind::TypeScriptNode,
            &request.capability_id,
            payload,
        ))
    }
}

fn runtime(
    operations: Vec<SdkRuntimeOperationKind>,
    requests: Arc<Mutex<Vec<SdkRuntimeRequest>>>,
) -> Arc<SdkRuntimeRouter> {
    let negotiation = SdkCapabilityNegotiation {
        agent_id: "agent.opencode".to_string(),
        binding_id: "binding.opencode".to_string(),
        binding_version: "0.1.0".to_string(),
        selected: vec![NegotiatedCapability {
            capability_id: SDK_CAPABILITY_SESSION_CONTROL.to_string(),
            backend_kind: SdkBackendKind::TypeScriptNode,
            driver_id: "driver.opencode.session.control.ts".to_string(),
            runtime_operations: operations,
            execution_scope: CapabilityExecutionScope::TransportRuntime,
        }],
        missing_required: Vec::new(),
        degraded_optional: Vec::new(),
    };
    Arc::new(
        SdkRuntimeRouter::new(negotiation)
            .with_typescript_runtime(Arc::new(CapturingRuntime { requests })),
    )
}

#[test]
fn runtime_backed_session_control_routes_fork_with_exact_correlation() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = SdkRuntimeBackedSessionControlProvider::new(
        runtime(vec![SdkRuntimeOperationKind::SessionFork], requests.clone()),
        "provider.opencode.session-control",
    );
    let request = ProviderSessionControlRequest::new(
        "control-1",
        "session-kernel",
        "provider-session-source",
        "policy-decision-1",
        ProviderSessionControlAction::Fork {
            before_message_id: Some("message-7".to_string()),
        },
    );

    let result = provider.control(request).expect("runtime control succeeds");
    assert_eq!(
        result.output,
        ProviderSessionControlOutput::Forked {
            provider_session_id: "provider-session-forked".to_string(),
        }
    );

    match &requests.lock().unwrap()[0].operation {
        SdkRuntimeOperation::SessionFork {
            control_request_id,
            session_id,
            provider_session_id,
            policy_decision_id,
            before_message_id,
            ..
        } => {
            assert_eq!(control_request_id, "control-1");
            assert_eq!(session_id, "session-kernel");
            assert_eq!(provider_session_id, "provider-session-source");
            assert_eq!(policy_decision_id, "policy-decision-1");
            assert_eq!(before_message_id.as_deref(), Some("message-7"));
        }
        other => panic!("expected session_fork, got {other:?}"),
    };
}

#[test]
fn runtime_router_fails_closed_when_session_action_is_not_declared() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = SdkRuntimeBackedSessionControlProvider::new(
        runtime(vec![SdkRuntimeOperationKind::SessionInterrupt], requests),
        "provider.opencode.session-control",
    );
    let request = ProviderSessionControlRequest::new(
        "control-2",
        "session-kernel",
        "provider-session-source",
        "policy-decision-2",
        ProviderSessionControlAction::Compact { focus: None },
    );

    let error = provider
        .control(request)
        .expect_err("undeclared compact operation must fail closed");
    assert!(error
        .to_string()
        .contains("provider.opencode.session-control"));
}

#[test]
fn session_control_runtime_operations_have_stable_wire_names() {
    assert_eq!(
        SdkRuntimeOperationKind::SessionInterrupt.as_str(),
        "session_interrupt"
    );
    assert_eq!(
        SdkRuntimeOperationKind::SessionCompact.as_str(),
        "session_compact"
    );
    assert_eq!(
        SdkRuntimeOperationKind::SessionFork.as_str(),
        "session_fork"
    );
}
