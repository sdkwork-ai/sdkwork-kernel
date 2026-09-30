use sdkwork_agent_provider_spi::{
    bootstrap_binding, describe_capability, AgentSdkBindingManifest, AgentSdkCapabilityDriver,
    BindingRegistry, DriverRegistry, SdkBackendKind, SdkDriverHealth, SdkRuntimeOperationKind,
    CODEX_BINDING_ID,
};
use std::sync::Arc;

struct FakeDriver {
    id: String,
    capability: String,
    backend: SdkBackendKind,
}

impl AgentSdkCapabilityDriver for FakeDriver {
    fn driver_id(&self) -> &str {
        &self.id
    }

    fn capability_id(&self) -> &str {
        &self.capability
    }

    fn backend_kind(&self) -> SdkBackendKind {
        self.backend
    }

    fn health(&self) -> SdkDriverHealth {
        SdkDriverHealth::healthy()
    }
}

#[test]
fn codex_binding_manifest_parses() {
    let json = include_str!("../../bindings/agent-providers/codex/provider-binding.manifest.json");
    let manifest = AgentSdkBindingManifest::from_json(json).expect("manifest should parse");
    assert_eq!(manifest.agent_id, "agent.codex");
    assert!(manifest.capability_binding("sdk.model.chat").is_some());
}

#[test]
fn session_control_is_a_standard_provider_sdk_capability() {
    let descriptor = describe_capability("sdk.session.control").expect("standard capability");
    assert_eq!(descriptor.kernel_spi_family, "session.control");
}

#[test]
fn opencode_binding_declares_live_session_control_operations() {
    let json =
        include_str!("../../bindings/agent-providers/opencode/provider-binding.manifest.json");
    let manifest = AgentSdkBindingManifest::from_json(json).expect("opencode manifest");
    let typescript = manifest
        .language_packages
        .as_ref()
        .and_then(|packages| packages.typescript.as_ref())
        .expect("OpenCode TypeScript SDK");
    assert_eq!(typescript.version.as_deref(), Some("1.18.12"));

    let control = manifest
        .capability_binding("sdk.session.control")
        .expect("OpenCode session control capability");
    assert!(!control.required);
    assert_eq!(control.backends.len(), 1);
    assert_eq!(
        control.backends[0].runtime_operations,
        vec![
            SdkRuntimeOperationKind::Ping,
            SdkRuntimeOperationKind::SessionInterrupt,
            SdkRuntimeOperationKind::SessionCompact,
            SdkRuntimeOperationKind::SessionFork,
        ]
    );
}

#[test]
fn bootstrap_binding_loads_codex_manifest() {
    let json = include_str!("../../bindings/agent-providers/codex/provider-binding.manifest.json");
    let manifest = AgentSdkBindingManifest::from_json(json).expect("manifest should parse");
    let mut drivers = DriverRegistry::new();
    let mut bindings = BindingRegistry::new();
    let negotiation =
        bootstrap_binding(manifest, &mut drivers, &mut bindings).expect("bootstrap should succeed");

    assert_eq!(negotiation.binding_id, CODEX_BINDING_ID);
    assert!(drivers.len() >= 3);
}

#[test]
fn negotiation_selects_registered_drivers_manually() {
    let json = include_str!("../../bindings/agent-providers/codex/provider-binding.manifest.json");
    let manifest = AgentSdkBindingManifest::from_json(json).expect("manifest should parse");

    let mut bindings = BindingRegistry::new();
    bindings.register(manifest);

    let mut drivers = DriverRegistry::new();
    drivers.register(Arc::new(FakeDriver {
        id: "driver.codex.session.lifecycle.rust".to_string(),
        capability: "sdk.session.lifecycle".to_string(),
        backend: SdkBackendKind::RustNative,
    }));
    drivers.register(Arc::new(FakeDriver {
        id: "driver.codex.session.history.rust".to_string(),
        capability: "sdk.session.history".to_string(),
        backend: SdkBackendKind::RustNative,
    }));
    drivers.register(Arc::new(FakeDriver {
        id: "driver.codex.session.control.rust".to_string(),
        capability: "sdk.session.control".to_string(),
        backend: SdkBackendKind::RustNative,
    }));
    drivers.register(Arc::new(FakeDriver {
        id: "driver.codex.model.chat.rust".to_string(),
        capability: "sdk.model.chat".to_string(),
        backend: SdkBackendKind::RustNative,
    }));

    let negotiation = bindings
        .negotiate("binding.codex", &drivers)
        .expect("negotiation should succeed");

    assert_eq!(negotiation.agent_id, "agent.codex");
    assert!(negotiation.selected_driver("sdk.model.chat").is_some());
}

#[test]
fn negotiation_fails_closed_for_missing_required_driver() {
    let json = include_str!("../../bindings/agent-providers/codex/provider-binding.manifest.json");
    let manifest = AgentSdkBindingManifest::from_json(json).expect("manifest should parse");

    let mut bindings = BindingRegistry::new();
    bindings.register(manifest);

    let drivers = DriverRegistry::new();
    let error = bindings
        .negotiate("binding.codex", &drivers)
        .expect_err("empty registry should fail");

    assert_eq!(error.code, "missing_required_capabilities");
    assert!(!error.missing_capabilities.is_empty());
}

#[test]
fn runtime_router_routes_to_registered_backend() {
    use sdkwork_agent_provider_spi::{
        SdkBackendRuntime, SdkRuntimeRequest, SdkRuntimeResponse, SdkRuntimeRouter,
    };

    struct StubRuntime {
        kind: SdkBackendKind,
    }

    impl SdkBackendRuntime for StubRuntime {
        fn backend_kind(&self) -> SdkBackendKind {
            self.kind
        }

        fn health(&self) -> SdkDriverHealth {
            SdkDriverHealth::healthy()
        }

        fn invoke(
            &self,
            request: &SdkRuntimeRequest,
        ) -> Result<SdkRuntimeResponse, sdkwork_agent_provider_spi::SdkRuntimeError> {
            Ok(SdkRuntimeResponse::success(
                self.kind,
                &request.capability_id,
                serde_json::json!({ "stub": true }),
            ))
        }
    }

    let json = include_str!("../../bindings/agent-providers/codex/provider-binding.manifest.json");
    let manifest = AgentSdkBindingManifest::from_json(json).expect("manifest should parse");

    let mut bindings = BindingRegistry::new();
    bindings.register(manifest);

    let mut drivers = DriverRegistry::new();
    drivers.register(Arc::new(FakeDriver {
        id: "driver.codex.session.lifecycle.rust".to_string(),
        capability: "sdk.session.lifecycle".to_string(),
        backend: SdkBackendKind::RustNative,
    }));
    drivers.register(Arc::new(FakeDriver {
        id: "driver.codex.session.history.rust".to_string(),
        capability: "sdk.session.history".to_string(),
        backend: SdkBackendKind::RustNative,
    }));
    drivers.register(Arc::new(FakeDriver {
        id: "driver.codex.session.control.rust".to_string(),
        capability: "sdk.session.control".to_string(),
        backend: SdkBackendKind::RustNative,
    }));
    drivers.register(Arc::new(FakeDriver {
        id: "driver.codex.model.chat.rust".to_string(),
        capability: "sdk.model.chat".to_string(),
        backend: SdkBackendKind::RustNative,
    }));

    let negotiation = bindings
        .negotiate("binding.codex", &drivers)
        .expect("negotiation should succeed");

    let router = SdkRuntimeRouter::new(negotiation).with_rust_runtime(Arc::new(StubRuntime {
        kind: SdkBackendKind::RustNative,
    }));

    let response = router
        .invoke(&SdkRuntimeRequest::ping("sdk.model.chat"))
        .expect("router invoke should succeed");
    assert!(response.success);
    assert_eq!(response.backend_kind, SdkBackendKind::RustNative);
}

#[test]
fn runtime_router_rejects_operations_not_declared_by_selected_backend() {
    use sdkwork_agent_provider_spi::{
        NegotiatedCapability, SdkBackendRuntime, SdkCapabilityNegotiation, SdkRuntimeRequest,
        SdkRuntimeResponse, SdkRuntimeRouter,
    };

    struct StubRuntime;

    impl SdkBackendRuntime for StubRuntime {
        fn backend_kind(&self) -> SdkBackendKind {
            SdkBackendKind::RustNative
        }

        fn health(&self) -> SdkDriverHealth {
            SdkDriverHealth::healthy()
        }

        fn invoke(
            &self,
            request: &SdkRuntimeRequest,
        ) -> Result<SdkRuntimeResponse, sdkwork_agent_provider_spi::SdkRuntimeError> {
            Ok(SdkRuntimeResponse::success(
                SdkBackendKind::RustNative,
                &request.capability_id,
                serde_json::json!({ "unexpected": true }),
            ))
        }
    }

    let negotiation = SdkCapabilityNegotiation {
        agent_id: "agent.codex".to_string(),
        binding_id: "binding.codex".to_string(),
        binding_version: "0.1.0".to_string(),
        selected: vec![NegotiatedCapability {
            capability_id: "sdk.session.lifecycle".to_string(),
            backend_kind: SdkBackendKind::RustNative,
            driver_id: "driver.codex.session.lifecycle.rust".to_string(),
            runtime_operations: vec![SdkRuntimeOperationKind::Ping],
                execution_scope: sdkwork_agent_provider_spi::CapabilityExecutionScope::TransportRuntime,
        }],
        missing_required: Vec::new(),
        degraded_optional: Vec::new(),
    };
    let router = SdkRuntimeRouter::new(negotiation).with_rust_runtime(Arc::new(StubRuntime));
    let error = router
        .invoke(&SdkRuntimeRequest {
            capability_id: "sdk.session.lifecycle".to_string(),
            operation: sdkwork_agent_provider_spi::SdkRuntimeOperation::SessionCreate {
                agent_id: "agent.1".to_string(),
                user_ref: None,
            },
            payload: None,
        })
        .expect_err("provider-local lifecycle must not execute SessionCreate through runtime");

    assert_eq!(error.code, "operation_not_supported");
    assert!(error.message.contains("session_create"));
}

#[test]
fn hermes_binding_manifest_excludes_unimplemented_skill_invocation() {
    let json = include_str!("../../bindings/agent-providers/hermes/provider-binding.manifest.json");
    let manifest = AgentSdkBindingManifest::from_json(json).expect("hermes manifest");
    let python = manifest
        .language_packages
        .as_ref()
        .and_then(|packages| packages.python.as_ref())
        .expect("python package ref");
    assert_eq!(python.module, "run_agent");
    assert!(manifest.capability_binding("sdk.skill.invoke").is_none());
    assert!(manifest.capability_binding("sdk.tool.invoke").is_none());
}

#[test]
fn openclaw_binding_manifest_declares_official_openai_gateway_client() {
    let json =
        include_str!("../../bindings/agent-providers/openclaw/provider-binding.manifest.json");
    let manifest = AgentSdkBindingManifest::from_json(json).expect("openclaw manifest");
    let typescript = manifest
        .language_packages
        .as_ref()
        .and_then(|packages| packages.typescript.as_ref())
        .expect("typescript package ref");
    assert_eq!(typescript.package, "openai");
}

#[test]
fn binding_manifest_preserves_integration_source_locators() {
    let codex = include_str!("../../bindings/agent-providers/codex/provider-binding.manifest.json");
    let rig = include_str!("../../bindings/agent-providers/rig/provider-binding.manifest.json");

    let codex_manifest = AgentSdkBindingManifest::from_json(codex).expect("codex manifest");
    let codex_sources = codex_manifest
        .integration_sources
        .as_ref()
        .expect("codex integration sources");
    let codex_rust_crate = codex_sources
        .iter()
        .find(|source| {
            source.mode == "rust_crate"
                && source.rust_crate == Some("codex-app-server-client".to_string())
        })
        .expect("codex app-server-client rust crate source");
    assert!(!codex_rust_crate.optional);
    let codex_source_tree = codex_sources
        .iter()
        .find(|source| source.mode == "source_tree")
        .expect("codex source_tree source");
    assert_eq!(
        codex_source_tree.path.as_deref(),
        Some("external/codex/codex-rs/app-server-client")
    );

    let rig_manifest = AgentSdkBindingManifest::from_json(rig).expect("rig manifest");
    let rig_sources = rig_manifest
        .integration_sources
        .as_ref()
        .expect("rig integration sources");
    let rig_source_tree = rig_sources
        .iter()
        .find(|source| source.mode == "source_tree")
        .expect("rig source_tree source");
    assert_eq!(
        rig_source_tree.path.as_deref(),
        Some("external/rig/crates/rig-core")
    );

    let rig_crate = rig_sources
        .iter()
        .find(|source| source.mode == "rust_crate")
        .expect("rig rust_crate source");
    assert_eq!(rig_crate.feature.as_deref(), Some("rig-core-adapter"));
    assert!(rig_crate.optional);
}

#[test]
fn bootstrap_binding_loads_hermes_manifest() {
    let json = include_str!("../../bindings/agent-providers/hermes/provider-binding.manifest.json");
    let manifest = AgentSdkBindingManifest::from_json(json).expect("manifest should parse");
    let mut drivers = DriverRegistry::new();
    let mut bindings = BindingRegistry::new();
    let negotiation =
        bootstrap_binding(manifest, &mut drivers, &mut bindings).expect("bootstrap should succeed");

    assert_eq!(
        negotiation.binding_id,
        sdkwork_agent_provider_spi::HERMES_BINDING_ID
    );
    assert!(drivers.len() >= 4);
}

#[test]
fn bootstrap_binding_loads_openclaw_manifest() {
    let json =
        include_str!("../../bindings/agent-providers/openclaw/provider-binding.manifest.json");
    let manifest = AgentSdkBindingManifest::from_json(json).expect("manifest should parse");
    let mut drivers = DriverRegistry::new();
    let mut bindings = BindingRegistry::new();
    let negotiation =
        bootstrap_binding(manifest, &mut drivers, &mut bindings).expect("bootstrap should succeed");

    assert_eq!(
        negotiation.binding_id,
        sdkwork_agent_provider_spi::OPENCLAW_BINDING_ID
    );
    assert!(drivers.len() >= 3);
}

#[test]
fn hermes_and_openclaw_manifests_parse() {
    let hermes =
        include_str!("../../bindings/agent-providers/hermes/provider-binding.manifest.json");
    let openclaw =
        include_str!("../../bindings/agent-providers/openclaw/provider-binding.manifest.json");

    let hermes_manifest = AgentSdkBindingManifest::from_json(hermes).expect("hermes manifest");
    let openclaw_manifest =
        AgentSdkBindingManifest::from_json(openclaw).expect("openclaw manifest");

    assert_eq!(hermes_manifest.agent_id, "agent.hermes");
    assert_eq!(openclaw_manifest.agent_id, "agent.openclaw");
}
