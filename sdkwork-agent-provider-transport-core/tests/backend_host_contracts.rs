use sdkwork_agent_provider_spi::{
    CapabilityExecutionScope, NegotiatedCapability, SdkBackendKind, SdkBackendRuntime,
    SdkCapabilityNegotiation, SdkDriverHealth, SdkRuntimeError, SdkRuntimeOperationKind,
    SdkRuntimeRequest, SdkRuntimeResponse,
};
use sdkwork_agent_provider_transport_core::{
    BackendHostRegistry, HttpOpenApiBackendHost, ProviderTransportBootstrap, RustNativeBackendHost,
    TypeScriptNodeBackendHost,
};
use std::sync::Arc;

#[test]
fn registry_prepares_registered_backends() {
    let mut registry = BackendHostRegistry::new();
    registry.register(Arc::new(RustNativeBackendHost::new("codex-core")));
    registry.register(Arc::new(TypeScriptNodeBackendHost::new("openclaw")));
    registry.register(Arc::new(HttpOpenApiBackendHost::new(
        "openclaw-gateway-open-api",
    )));

    registry
        .prepare_all()
        .expect("backend hosts should prepare");
    assert_eq!(
        registry
            .health(SdkBackendKind::RustNative)
            .expect("rust host")
            .status,
        sdkwork_agent_provider_transport_core::SdkBackendStatus::Ready
    );
}

struct UnhealthyRuntime;

impl SdkBackendRuntime for UnhealthyRuntime {
    fn backend_kind(&self) -> SdkBackendKind {
        SdkBackendKind::TypeScriptNode
    }

    fn health(&self) -> SdkDriverHealth {
        SdkDriverHealth::unhealthy("official sdk package is not resolved")
    }

    fn invoke(&self, _request: &SdkRuntimeRequest) -> Result<SdkRuntimeResponse, SdkRuntimeError> {
        Ok(SdkRuntimeResponse::failure(
            SdkBackendKind::TypeScriptNode,
            "should not be invoked",
        ))
    }
}

fn negotiated_typescript_model_chat() -> SdkCapabilityNegotiation {
    SdkCapabilityNegotiation {
        agent_id: "agent.test".to_string(),
        binding_id: "binding.test".to_string(),
        binding_version: "0.1.0".to_string(),
        selected: vec![NegotiatedCapability {
            capability_id: "sdk.model.chat".to_string(),
            backend_kind: SdkBackendKind::TypeScriptNode,
            driver_id: "driver.test.model.chat.ts".to_string(),
            runtime_operations: vec![SdkRuntimeOperationKind::ModelChat],
            execution_scope: CapabilityExecutionScope::TransportRuntime,
        }],
        missing_required: Vec::new(),
        degraded_optional: Vec::new(),
    }
}

#[test]
fn bootstrap_fails_closed_when_selected_runtime_is_unhealthy() {
    let mut bootstrap = ProviderTransportBootstrap::new();
    bootstrap.register_host(Arc::new(TypeScriptNodeBackendHost::new(
        "@sdkwork/missing-sdk",
    )));
    bootstrap.with_typescript_runtime(Arc::new(UnhealthyRuntime));

    let error = match bootstrap.finalize_pair(negotiated_typescript_model_chat()) {
        Ok(_) => panic!("unhealthy selected runtime should fail before router construction"),
        Err(error) => error,
    };

    assert_eq!(error.code, "missing_required_capabilities");
    assert!(error
        .missing_capabilities
        .iter()
        .any(|capability| capability.contains("sdk.model.chat")));
    assert!(error
        .message
        .contains("official sdk package is not resolved"));
}
