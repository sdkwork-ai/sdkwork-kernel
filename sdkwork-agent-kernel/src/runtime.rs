use crate::{
    AgentClassificationProvider, AgentCollaborationProvider, AgentConfigSectionKind,
    AgentConfigurationProvider, AgentInstaller, AgentManifest, AgentPackageManifest,
    AgentRuntimeConformanceProfile, AgentSkillProvider, Capability, CapabilityManifest,
    CapabilityRequirement, ContextProvider, EnvFileSecretFallbackHostProvider, HostProvider,
    KernelConformanceCase, KernelConformanceReport, KernelError, KernelEvent, KernelEventSeverity,
    KernelHook, KernelHookRegistry, KernelResult, KnowledgeProvider, McpProvider, MemoryProvider,
    MessageQueryProvider, ModelCostCalculator, ModelPrice, ModelProvider, PlanningProvider,
    PlatformSandboxProvider, PolicyCategory, PolicyProvider, ProtocolAdapter, ProviderHealth,
    ProviderManifest, ProviderSessionActivityProvider, ProviderSessionControlProvider,
    SandboxProvider, SandboxingHostProvider, SideEffectLevel, TaskSchedulingProvider,
    TelemetryProvider, ToolProvider, AGENT_KERNEL_SPEC_VERSION,
};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    Ready,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRuntime {
    state: RuntimeState,
    capability_manifest: CapabilityManifest,
    provider_registry: RuntimeProviderRegistry,
    hooks: KernelHookRegistry,
    cost_calculator: ModelCostCalculator,
}

impl AgentRuntime {
    pub fn from_capability_manifest(capability_manifest: CapabilityManifest) -> Self {
        Self::from_capability_manifest_with_provider_registry(
            capability_manifest,
            RuntimeProviderRegistry::default(),
            KernelHookRegistry::new(),
            ModelCostCalculator::default(),
        )
    }

    fn from_capability_manifest_with_provider_registry(
        capability_manifest: CapabilityManifest,
        provider_registry: RuntimeProviderRegistry,
        hooks: KernelHookRegistry,
        cost_calculator: ModelCostCalculator,
    ) -> Self {
        let state = if !capability_manifest.missing_required_capabilities.is_empty() {
            RuntimeState::Failed
        } else if !capability_manifest.degraded_capabilities.is_empty()
            || provider_registry.has_degraded_provider_health(&capability_manifest.providers)
        {
            RuntimeState::Degraded
        } else {
            RuntimeState::Ready
        };

        Self {
            state,
            capability_manifest,
            provider_registry,
            hooks,
            cost_calculator,
        }
    }

    /// Kernel hooks attached to this runtime.
    pub fn hooks(&self) -> &KernelHookRegistry {
        &self.hooks
    }

    /// Model price table used for cost estimation.
    pub fn cost_calculator(&self) -> &ModelCostCalculator {
        &self.cost_calculator
    }

    pub fn state(&self) -> RuntimeState {
        self.state
    }

    pub fn capability_manifest(&self) -> &CapabilityManifest {
        &self.capability_manifest
    }

    pub fn diagnostics(&self) -> AgentRuntimeDiagnostics {
        let provider_diagnostics: Vec<AgentProviderDiagnostic> = self
            .capability_manifest
            .providers
            .iter()
            .map(|provider| {
                let typed_registered = self.provider_registry.has_typed_provider(provider);

                AgentProviderDiagnostic {
                    provider_id: provider.provider_id.clone(),
                    provider_family: provider.provider_family.clone(),
                    provider_version: provider.version.clone(),
                    typed_registered,
                    health: self.provider_registry.health_for_provider(provider),
                    capabilities: provider.capabilities.clone(),
                }
            })
            .collect();

        let typed_provider_count = provider_diagnostics
            .iter()
            .filter(|provider| provider.typed_registered)
            .count();

        AgentRuntimeDiagnostics {
            runtime_id: self.capability_manifest.runtime_id.clone(),
            agent_id: self.capability_manifest.agent_id.clone(),
            state: self.state.as_str().to_string(),
            provider_count: self.capability_manifest.providers.len(),
            capability_count: self.capability_manifest.capabilities.len(),
            typed_provider_count,
            manifest_only_provider_count: self
                .capability_manifest
                .providers
                .len()
                .saturating_sub(typed_provider_count),
            missing_required_capabilities: self
                .capability_manifest
                .missing_required_capabilities
                .clone(),
            degraded_capabilities: self.capability_manifest.degraded_capabilities.clone(),
            provider_diagnostics,
        }
    }

    pub fn conformance_report(
        &self,
        profile: AgentRuntimeConformanceProfile,
    ) -> KernelConformanceReport {
        let diagnostics = self.diagnostics();
        let mut report = KernelConformanceReport::new(
            format!(
                "report.{}.{}",
                self.capability_manifest.runtime_id,
                profile.as_str()
            ),
            profile.as_str(),
            self.capability_manifest.runtime_id.clone(),
            self.capability_manifest.kernel_version.clone(),
        )
        .with_spec_version(AGENT_KERNEL_SPEC_VERSION)
        .with_test_suite_version("agent-runtime-conformance.0.1.0")
        .with_security_profile(self.capability_manifest.security_profile.clone());

        for capability_id in self.required_capability_ids() {
            report = report.with_required_capability(capability_id);
        }

        report
            .add_case(self.required_capabilities_case())
            .add_case(self.optional_capabilities_case(profile))
            .add_case(self.capability_namespace_case())
            .add_case(self.protocol_adapter_exposure_subset_case())
            .add_case(self.provider_manifest_case())
            .add_case(self.local_provider_typed_case(profile, &diagnostics))
            .add_case(self.local_provider_health_case(profile, &diagnostics))
    }

    pub fn agent_installer(&self) -> KernelResult<&(dyn AgentInstaller + Send + Sync)> {
        if let Some(installer) = self.provider_registry.agent_installer.as_deref() {
            return Ok(installer);
        }

        match self.provider_id_for_capability("agent.install") {
            Some(provider_id) => Err(KernelError::ProviderUnavailable {
                provider_id: provider_id.to_string(),
            }),
            None => Err(KernelError::CapabilityMissing {
                capability_id: "agent.install".to_string(),
            }),
        }
    }

    pub fn agent_configuration_provider(
        &self,
    ) -> KernelResult<&(dyn AgentConfigurationProvider + Send + Sync)> {
        if let Some(provider) = self.provider_registry.agent_configuration.as_deref() {
            return Ok(provider);
        }

        match self.provider_id_for_capability("agent.configure") {
            Some(provider_id) => Err(KernelError::ProviderUnavailable {
                provider_id: provider_id.to_string(),
            }),
            None => Err(KernelError::CapabilityMissing {
                capability_id: "agent.configure".to_string(),
            }),
        }
    }

    pub fn model_provider(&self) -> KernelResult<&(dyn ModelProvider + Send + Sync)> {
        self.provider_registry
            .model_provider
            .as_deref()
            .ok_or_else(|| self.provider_error_for_family("model", "model.chat"))
    }

    pub fn model_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<&(dyn ModelProvider + Send + Sync)> {
        self.provider_registry
            .model_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "model.chat"))
    }

    pub fn model_provider_ids(&self) -> Vec<String> {
        self.provider_registry.model_provider_ids()
    }

    pub fn provider_session_activity_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<&(dyn ProviderSessionActivityProvider + Send + Sync)> {
        self.provider_registry
            .provider_session_activity_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "session.activity"))
    }

    pub fn provider_session_activity_provider_ids(&self) -> Vec<String> {
        self.provider_registry
            .provider_session_activity_provider_ids()
    }

    pub fn provider_session_control_provider(
        &self,
    ) -> KernelResult<&(dyn ProviderSessionControlProvider + Send + Sync)> {
        self.provider_registry
            .provider_session_control_provider
            .as_deref()
            .ok_or_else(|| {
                self.provider_error_for_family("session_control", "session.control.interrupt")
            })
    }

    pub fn provider_session_control_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<&(dyn ProviderSessionControlProvider + Send + Sync)> {
        self.provider_registry
            .provider_session_control_provider_by_id(provider_id)
            .ok_or_else(|| {
                self.provider_error_for_provider_id(provider_id, "session.control.interrupt")
            })
    }

    pub fn provider_session_control_provider_ids(&self) -> Vec<String> {
        self.provider_registry
            .provider_session_control_provider_ids()
    }

    pub fn tool_provider(&self) -> KernelResult<&(dyn ToolProvider + Send + Sync)> {
        self.provider_registry
            .tool_provider
            .as_deref()
            .ok_or_else(|| self.provider_error_for_family("tool", "tool.invoke"))
    }

    pub fn tool_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<&(dyn ToolProvider + Send + Sync)> {
        self.provider_registry
            .tool_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "tool.invoke"))
    }

    pub fn tool_provider_ids(&self) -> Vec<String> {
        self.provider_registry.tool_provider_ids()
    }

    pub fn policy_provider(&self) -> KernelResult<&(dyn PolicyProvider + Send + Sync)> {
        self.provider_registry
            .policy_provider
            .as_deref()
            .ok_or_else(|| self.provider_error_for_family("policy", "policy.evaluate"))
    }

    pub fn policy_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<&(dyn PolicyProvider + Send + Sync)> {
        self.provider_registry
            .policy_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "policy.evaluate"))
    }

    pub fn policy_provider_ids(&self) -> Vec<String> {
        self.provider_registry.policy_provider_ids()
    }

    pub fn context_provider(&self) -> KernelResult<&(dyn ContextProvider + Send + Sync)> {
        self.provider_registry
            .context_provider
            .as_deref()
            .ok_or_else(|| self.provider_error_for_family("context", "context.collect"))
    }

    pub fn context_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<&(dyn ContextProvider + Send + Sync)> {
        self.provider_registry
            .context_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "context.collect"))
    }

    pub fn context_provider_ids(&self) -> Vec<String> {
        self.provider_registry.context_provider_ids()
    }

    pub fn memory_provider(&self) -> KernelResult<Arc<Mutex<dyn MemoryProvider + Send>>> {
        self.provider_registry
            .memory_provider
            .as_ref()
            .cloned()
            .ok_or_else(|| self.provider_error_for_family("memory", "memory.query"))
    }

    pub fn memory_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<Arc<Mutex<dyn MemoryProvider + Send>>> {
        self.provider_registry
            .memory_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "memory.query"))
    }

    pub fn memory_provider_ids(&self) -> Vec<String> {
        self.provider_registry.memory_provider_ids()
    }

    pub fn knowledge_provider(&self) -> KernelResult<&(dyn KnowledgeProvider + Send + Sync)> {
        self.provider_registry
            .knowledge_provider
            .as_deref()
            .ok_or_else(|| self.provider_error_for_family("knowledge", "knowledge.search"))
    }

    pub fn knowledge_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<&(dyn KnowledgeProvider + Send + Sync)> {
        self.provider_registry
            .knowledge_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "knowledge.search"))
    }

    pub fn knowledge_provider_ids(&self) -> Vec<String> {
        self.provider_registry.knowledge_provider_ids()
    }

    pub fn planning_provider(&self) -> KernelResult<&(dyn PlanningProvider + Send + Sync)> {
        self.provider_registry
            .planning_provider
            .as_deref()
            .ok_or_else(|| self.provider_error_for_family("planning", "planning.create"))
    }

    pub fn planning_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<&(dyn PlanningProvider + Send + Sync)> {
        self.provider_registry
            .planning_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "planning.create"))
    }

    pub fn planning_provider_ids(&self) -> Vec<String> {
        self.provider_registry.planning_provider_ids()
    }

    pub fn host_provider(&self) -> KernelResult<&(dyn HostProvider + Send + Sync)> {
        self.provider_registry
            .host_provider
            .as_deref()
            .ok_or_else(|| self.provider_error_for_family("host", "host.filesystem"))
    }

    pub fn host_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<&(dyn HostProvider + Send + Sync)> {
        self.provider_registry
            .host_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "host.filesystem"))
    }

    pub fn host_provider_ids(&self) -> Vec<String> {
        self.provider_registry.host_provider_ids()
    }

    pub fn protocol_adapter(&self) -> KernelResult<&(dyn ProtocolAdapter + Send + Sync)> {
        self.provider_registry
            .protocol_adapter
            .as_deref()
            .ok_or_else(|| self.provider_error_for_family("protocol_adapter", "protocol.map"))
    }

    pub fn protocol_adapter_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<&(dyn ProtocolAdapter + Send + Sync)> {
        self.provider_registry
            .protocol_adapter_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "protocol.map"))
    }

    pub fn protocol_adapter_ids(&self) -> Vec<String> {
        self.provider_registry.protocol_adapter_ids()
    }

    pub fn mcp_provider(&self) -> KernelResult<&(dyn McpProvider + Send + Sync)> {
        self.provider_registry
            .mcp_provider
            .as_deref()
            .ok_or_else(|| self.provider_error_for_family("mcp", "mcp.tools"))
    }

    pub fn mcp_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<&(dyn McpProvider + Send + Sync)> {
        self.provider_registry
            .mcp_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "mcp.tools"))
    }

    pub fn mcp_provider_ids(&self) -> Vec<String> {
        self.provider_registry.mcp_provider_ids()
    }

    pub fn agent_skill_provider(&self) -> KernelResult<&(dyn AgentSkillProvider + Send + Sync)> {
        self.provider_registry
            .agent_skill_provider
            .as_deref()
            .ok_or_else(|| self.provider_error_for_family("skill", "skill.invoke"))
    }

    pub fn agent_skill_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<&(dyn AgentSkillProvider + Send + Sync)> {
        self.provider_registry
            .agent_skill_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "skill.invoke"))
    }

    pub fn agent_skill_provider_ids(&self) -> Vec<String> {
        self.provider_registry.agent_skill_provider_ids()
    }

    pub fn collaboration_provider(
        &self,
    ) -> KernelResult<&(dyn AgentCollaborationProvider + Send + Sync)> {
        self.provider_registry
            .collaboration_provider
            .as_deref()
            .ok_or_else(|| self.provider_error_for_family("collaboration", "agent.handoff"))
    }

    pub fn collaboration_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<&(dyn AgentCollaborationProvider + Send + Sync)> {
        self.provider_registry
            .collaboration_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "agent.handoff"))
    }

    pub fn collaboration_provider_ids(&self) -> Vec<String> {
        self.provider_registry.collaboration_provider_ids()
    }

    pub fn telemetry_provider(&self) -> KernelResult<Arc<Mutex<dyn TelemetryProvider + Send>>> {
        self.provider_registry
            .telemetry_provider
            .as_ref()
            .cloned()
            .ok_or_else(|| self.provider_error_for_family("telemetry", "telemetry.record"))
    }

    pub fn telemetry_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<Arc<Mutex<dyn TelemetryProvider + Send>>> {
        self.provider_registry
            .telemetry_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "telemetry.record"))
    }

    pub fn telemetry_provider_ids(&self) -> Vec<String> {
        self.provider_registry.telemetry_provider_ids()
    }

    pub fn task_scheduling_provider(
        &self,
    ) -> KernelResult<Arc<Mutex<dyn TaskSchedulingProvider + Send>>> {
        self.provider_registry
            .task_scheduling_provider
            .as_ref()
            .cloned()
            .ok_or_else(|| self.provider_error_for_family("task_scheduling", "task.schedule"))
    }

    pub fn task_scheduling_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<Arc<Mutex<dyn TaskSchedulingProvider + Send>>> {
        self.provider_registry
            .task_scheduling_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "task.schedule"))
    }

    pub fn task_scheduling_provider_ids(&self) -> Vec<String> {
        self.provider_registry.task_scheduling_provider_ids()
    }

    pub fn classification_provider(
        &self,
    ) -> KernelResult<Arc<Mutex<dyn AgentClassificationProvider + Send>>> {
        self.provider_registry
            .classification_provider
            .as_ref()
            .cloned()
            .ok_or_else(|| self.provider_error_for_family("agent_classification", "agent.classify"))
    }

    pub fn classification_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<Arc<Mutex<dyn AgentClassificationProvider + Send>>> {
        self.provider_registry
            .classification_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "agent.classify"))
    }

    pub fn classification_provider_ids(&self) -> Vec<String> {
        self.provider_registry.classification_provider_ids()
    }

    pub fn message_query_provider(
        &self,
    ) -> KernelResult<&(dyn MessageQueryProvider + Send + Sync)> {
        self.provider_registry
            .message_query_provider
            .as_deref()
            .ok_or_else(|| self.provider_error_for_family("message_query", "message.query"))
    }

    pub fn message_query_provider_by_id(
        &self,
        provider_id: &str,
    ) -> KernelResult<&(dyn MessageQueryProvider + Send + Sync)> {
        self.provider_registry
            .message_query_provider_by_id(provider_id)
            .ok_or_else(|| self.provider_error_for_provider_id(provider_id, "message.query"))
    }

    pub fn message_query_provider_ids(&self) -> Vec<String> {
        self.provider_registry.message_query_provider_ids()
    }

    fn provider_id_for_capability(&self, capability_id: &str) -> Option<&str> {
        self.capability_manifest
            .capabilities
            .iter()
            .find(|capability| capability.capability_id == capability_id)
            .map(|capability| capability.provider_id.as_str())
    }

    fn provider_id_for_family(&self, provider_family: &str) -> Option<&str> {
        self.capability_manifest
            .providers
            .iter()
            .find(|provider| provider.provider_family == provider_family)
            .map(|provider| provider.provider_id.as_str())
    }

    fn provider_error_for_family(&self, provider_family: &str, capability_id: &str) -> KernelError {
        match self.provider_id_for_family(provider_family) {
            Some(provider_id) => KernelError::ProviderUnavailable {
                provider_id: provider_id.to_string(),
            },
            None => KernelError::CapabilityMissing {
                capability_id: capability_id.to_string(),
            },
        }
    }

    fn provider_error_for_provider_id(
        &self,
        provider_id: &str,
        capability_id: &str,
    ) -> KernelError {
        if self
            .capability_manifest
            .providers
            .iter()
            .any(|provider| provider.provider_id == provider_id)
        {
            KernelError::ProviderUnavailable {
                provider_id: provider_id.to_string(),
            }
        } else {
            KernelError::CapabilityMissing {
                capability_id: capability_id.to_string(),
            }
        }
    }

    fn required_capability_ids(&self) -> Vec<String> {
        let mut capability_ids: Vec<String> = self
            .capability_manifest
            .capabilities
            .iter()
            .filter(|capability| capability.required)
            .map(|capability| capability.capability_id.clone())
            .collect();

        for capability_id in &self.capability_manifest.missing_required_capabilities {
            if !capability_ids.contains(capability_id) {
                capability_ids.push(capability_id.clone());
            }
        }

        capability_ids
    }

    fn required_capabilities_case(&self) -> KernelConformanceCase {
        if self
            .capability_manifest
            .missing_required_capabilities
            .is_empty()
        {
            KernelConformanceCase::passed(
                "agent.conformance.runtime.required_capabilities.available",
                "required capabilities are available",
            )
            .required()
        } else {
            KernelConformanceCase::failed(
                "agent.conformance.runtime.required_capabilities.available",
                format!(
                    "missing required capabilities: {}",
                    self.capability_manifest
                        .missing_required_capabilities
                        .join(", ")
                ),
            )
            .required()
        }
    }

    fn optional_capabilities_case(
        &self,
        profile: AgentRuntimeConformanceProfile,
    ) -> KernelConformanceCase {
        if self.capability_manifest.degraded_capabilities.is_empty() {
            return KernelConformanceCase::passed(
                "agent.conformance.runtime.optional_capabilities.available",
                "optional capabilities are available",
            );
        }

        let message = format!(
            "degraded capabilities: {}",
            self.capability_manifest.degraded_capabilities.join(", ")
        );

        KernelConformanceCase::skipped(
            "agent.conformance.runtime.optional_capabilities.available",
            message,
        )
        .with_skip_reason(format!(
            "{} profile allows optional capability degradation",
            profile.as_str()
        ))
    }

    fn capability_namespace_case(&self) -> KernelConformanceCase {
        let invalid_capabilities: Vec<String> = self
            .capability_manifest
            .capabilities
            .iter()
            .map(|capability| capability.capability_id.as_str())
            .chain(
                self.capability_manifest
                    .missing_required_capabilities
                    .iter()
                    .map(String::as_str),
            )
            .chain(
                self.capability_manifest
                    .degraded_capabilities
                    .iter()
                    .map(String::as_str),
            )
            .filter(|capability_id| !is_valid_capability_id(capability_id))
            .map(std::string::ToString::to_string)
            .collect();

        if invalid_capabilities.is_empty() {
            KernelConformanceCase::passed(
                "agent.conformance.runtime.capabilities.namespaced",
                "capability ids are namespaced",
            )
        } else {
            KernelConformanceCase::failed(
                "agent.conformance.runtime.capabilities.namespaced",
                format!(
                    "unnamespaced capabilities: {}",
                    invalid_capabilities.join(", ")
                ),
            )
        }
    }

    fn provider_manifest_case(&self) -> KernelConformanceCase {
        if self.capability_manifest.providers.is_empty() {
            KernelConformanceCase::failed(
                "agent.conformance.runtime.providers.declared",
                "no providers are declared",
            )
            .required()
        } else {
            KernelConformanceCase::passed(
                "agent.conformance.runtime.providers.declared",
                "providers are declared",
            )
            .required()
        }
    }

    fn protocol_adapter_exposure_subset_case(&self) -> KernelConformanceCase {
        let effective_capability_ids: Vec<&str> = self
            .capability_manifest
            .capabilities
            .iter()
            .map(|capability| capability.capability_id.as_str())
            .collect();
        let unsupported_exposures: Vec<String> = self
            .capability_manifest
            .providers
            .iter()
            .filter(|provider| provider.provider_family == "protocol_adapter")
            .flat_map(|provider| {
                provider
                    .capabilities
                    .iter()
                    .filter(|capability| {
                        !effective_capability_ids
                            .iter()
                            .any(|effective| effective == &capability.as_str())
                    })
                    .map(|capability| format!("{}:{capability}", provider.provider_id))
            })
            .collect();

        if unsupported_exposures.is_empty() {
            KernelConformanceCase::passed(
                "agent.conformance.runtime.protocol_adapters.exposure_subset",
                "protocol adapter exposed capabilities are effective runtime capabilities",
            )
        } else {
            KernelConformanceCase::failed(
                "agent.conformance.runtime.protocol_adapters.exposure_subset",
                format!(
                    "protocol adapter exposures are not effective runtime capabilities: {}",
                    unsupported_exposures.join(", ")
                ),
            )
        }
    }

    fn local_provider_typed_case(
        &self,
        profile: AgentRuntimeConformanceProfile,
        diagnostics: &AgentRuntimeDiagnostics,
    ) -> KernelConformanceCase {
        if profile == AgentRuntimeConformanceProfile::Manifest {
            return KernelConformanceCase::skipped(
                "agent.conformance.runtime.local_providers.typed",
                "local runtime profile not requested",
            )
            .with_skip_reason("manifest profile validates negotiation only");
        }

        let manifest_only_provider_ids = diagnostics.manifest_only_provider_ids();
        if manifest_only_provider_ids.is_empty() {
            KernelConformanceCase::passed(
                "agent.conformance.runtime.local_providers.typed",
                "all declared providers have typed local SPI instances",
            )
            .required()
        } else {
            KernelConformanceCase::failed(
                "agent.conformance.runtime.local_providers.typed",
                format!(
                    "manifest-only providers: {}",
                    manifest_only_provider_ids.join(", ")
                ),
            )
            .required()
        }
    }

    fn local_provider_health_case(
        &self,
        profile: AgentRuntimeConformanceProfile,
        diagnostics: &AgentRuntimeDiagnostics,
    ) -> KernelConformanceCase {
        if profile == AgentRuntimeConformanceProfile::Manifest {
            return KernelConformanceCase::skipped(
                "agent.conformance.runtime.local_providers.health_available",
                "local runtime profile not requested",
            )
            .with_skip_reason("manifest profile validates negotiation only");
        }

        let unhealthy_provider_ids: Vec<String> = diagnostics
            .provider_diagnostics
            .iter()
            .filter(|provider| provider.health_is_degraded())
            .map(|provider| provider.provider_id.clone())
            .collect();

        if unhealthy_provider_ids.is_empty() {
            KernelConformanceCase::passed(
                "agent.conformance.runtime.local_providers.health_available",
                "typed provider health is available",
            )
            .required()
        } else {
            KernelConformanceCase::failed(
                "agent.conformance.runtime.local_providers.health_available",
                format!("unhealthy providers: {}", unhealthy_provider_ids.join(", ")),
            )
            .required()
        }
    }
}

impl RuntimeState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRuntimeDiagnostics {
    pub runtime_id: String,
    pub agent_id: String,
    pub state: String,
    pub provider_count: usize,
    pub capability_count: usize,
    pub typed_provider_count: usize,
    pub manifest_only_provider_count: usize,
    pub missing_required_capabilities: Vec<String>,
    pub degraded_capabilities: Vec<String>,
    pub provider_diagnostics: Vec<AgentProviderDiagnostic>,
}

impl AgentRuntimeDiagnostics {
    pub fn provider(&self, provider_id: &str) -> Option<&AgentProviderDiagnostic> {
        self.provider_diagnostics
            .iter()
            .find(|provider| provider.provider_id == provider_id)
    }

    pub fn manifest_only_provider_ids(&self) -> Vec<String> {
        self.provider_diagnostics
            .iter()
            .filter(|provider| !provider.typed_registered)
            .map(|provider| provider.provider_id.clone())
            .collect()
    }

    pub fn missing_standard_provider_families(&self) -> Vec<String> {
        standard_agent_provider_families()
            .iter()
            .filter(|provider_family| {
                !self
                    .provider_diagnostics
                    .iter()
                    .any(|provider| provider.provider_family == **provider_family)
            })
            .map(|provider_family| (*provider_family).to_string())
            .collect()
    }

    pub fn is_degraded(&self) -> bool {
        !self.missing_required_capabilities.is_empty()
            || !self.degraded_capabilities.is_empty()
            || self.manifest_only_provider_count > 0
            || self
                .provider_diagnostics
                .iter()
                .any(|provider| provider.health_is_degraded())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProviderDiagnostic {
    pub provider_id: String,
    pub provider_family: String,
    pub provider_version: String,
    pub typed_registered: bool,
    pub health: Option<ProviderHealth>,
    pub capabilities: Vec<String>,
}

impl AgentProviderDiagnostic {
    pub fn health_is_degraded(&self) -> bool {
        self.health
            .as_ref()
            .is_some_and(|health| health.status != "available")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBootstrapReport {
    pub runtime: AgentRuntime,
    pub events: Vec<KernelEvent>,
}

#[derive(Clone)]
pub struct RuntimeBuilder {
    runtime_id: String,
    agent_manifest: AgentManifest,
    agent_package_manifest: Option<AgentPackageManifest>,
    providers: Vec<ProviderManifest>,
    provider_registry: RuntimeProviderRegistry,
    security_profile: String,
    required_security_profile: Option<String>,
    generated_at: String,
    host_sandbox_provider: Option<Arc<dyn SandboxProvider>>,
    host_sandbox_enabled: bool,
    kernel_hooks: KernelHookRegistry,
    model_prices: Vec<ModelPrice>,
}

impl std::fmt::Debug for RuntimeBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeBuilder")
            .field("runtime_id", &self.runtime_id)
            .field("agent_id", &self.agent_manifest.agent_id)
            .field("provider_count", &self.providers.len())
            .field("security_profile", &self.security_profile)
            .field("host_sandbox_enabled", &self.host_sandbox_enabled)
            .field(
                "has_host_sandbox_provider",
                &self.host_sandbox_provider.is_some(),
            )
            .finish()
    }
}

impl RuntimeBuilder {
    pub fn new(runtime_id: impl Into<String>, agent_manifest: AgentManifest) -> Self {
        Self {
            runtime_id: runtime_id.into(),
            agent_manifest,
            agent_package_manifest: None,
            providers: Vec::new(),
            provider_registry: RuntimeProviderRegistry::default(),
            security_profile: "fail_closed=true".to_string(),
            required_security_profile: None,
            generated_at: "1970-01-01T00:00:00Z".to_string(),
            host_sandbox_provider: None,
            host_sandbox_enabled: false,
            kernel_hooks: KernelHookRegistry::new(),
            model_prices: Vec::new(),
        }
    }

    /// Register model prices for cost estimation; prices are keyed by
    /// model id and the first registration wins.
    pub fn with_model_prices(mut self, prices: Vec<ModelPrice>) -> Self {
        self.model_prices = prices;
        self
    }

    /// Register a kernel hook; hooks run in registration order at phase
    /// boundaries (model, tool, user prompt, session lifecycle).
    pub fn register_kernel_hook(mut self, hook: Arc<dyn KernelHook>) -> Self {
        self.kernel_hooks.register(hook);
        self
    }

    pub fn register_provider(mut self, provider: ProviderManifest) -> Self {
        self.providers.push(provider);
        self
    }

    pub fn register_model_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "model",
            version,
            vec!["model.chat"],
        ))
    }

    pub fn register_model_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: ModelProvider + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "model",
            version,
            vec!["model.chat"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_model_provider(provider_id, Arc::new(provider));
        self
    }

    /// Registers the live provider-session activity query boundary owned by a
    /// model provider. This companion registry does not create a new provider
    /// family or capability manifest entry.
    pub fn register_provider_session_activity_provider(
        mut self,
        provider_id: impl Into<String>,
        provider: Arc<dyn ProviderSessionActivityProvider + Send + Sync>,
    ) -> Self {
        self.provider_registry
            .add_provider_session_activity_provider(provider_id.into(), provider);
        self
    }

    pub fn register_provider_session_control_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "session_control",
            version,
            vec![
                "session.control.interrupt",
                "session.control.compact",
                "session.control.fork",
            ],
        ))
    }

    pub fn register_provider_session_control_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: ProviderSessionControlProvider + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "session_control",
            version,
            vec![
                "session.control.interrupt",
                "session.control.compact",
                "session.control.fork",
            ],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_provider_session_control_provider(provider_id, Arc::new(provider));
        self
    }

    pub fn register_tool_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "tool",
            version,
            vec!["tool.invoke"],
        ))
    }

    pub fn register_tool_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: ToolProvider + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "tool",
            version,
            vec!["tool.invoke"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_tool_provider(provider_id, Arc::new(provider));
        self
    }

    pub fn register_policy_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "policy",
            version,
            vec!["policy.evaluate"],
        ))
    }

    pub fn register_policy_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: PolicyProvider + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "policy",
            version,
            vec!["policy.evaluate"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_policy_provider(provider_id, Arc::new(provider));
        self
    }

    pub fn register_context_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "context",
            version,
            vec!["context.collect"],
        ))
    }

    pub fn register_context_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: ContextProvider + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "context",
            version,
            vec!["context.collect"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_context_provider(provider_id, Arc::new(provider));
        self
    }

    pub fn register_memory_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "memory",
            version,
            vec!["memory.query"],
        ))
    }

    pub fn register_memory_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: MemoryProvider + Send + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "memory",
            version,
            vec!["memory.query"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_memory_provider(provider_id, Arc::new(Mutex::new(provider)));
        self
    }

    pub fn register_knowledge_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "knowledge",
            version,
            vec!["knowledge.search", "knowledge.read", "knowledge.list"],
        ))
    }

    pub fn register_knowledge_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: KnowledgeProvider + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "knowledge",
            version,
            vec!["knowledge.search"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_knowledge_provider(provider_id, Arc::new(provider));
        self
    }

    pub fn register_planning_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "planning",
            version,
            vec!["planning.create"],
        ))
    }

    pub fn register_planning_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: PlanningProvider + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "planning",
            version,
            vec!["planning.create"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_planning_provider(provider_id, Arc::new(provider));
        self
    }

    pub fn register_host_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "host",
            version,
            vec![
                "host.filesystem",
                "host.process",
                "host.network",
                "host.secrets",
            ],
        ))
    }

    pub fn register_host_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: HostProvider + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "host",
            version,
            vec![
                "host.filesystem",
                "host.process",
                "host.network",
                "host.secrets",
            ],
        );
        self.providers.push(provider_manifest);
        let provider: Arc<dyn HostProvider + Send + Sync> = Arc::new(provider);
        self.provider_registry
            .add_host_provider(provider_id, self.wrap_host_provider(provider));
        self
    }

    pub fn register_protocol_adapter_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "protocol_adapter",
            version,
            vec!["protocol.map"],
        ))
    }

    pub fn register_protocol_adapter<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: ProtocolAdapter + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest =
            protocol_adapter_provider_manifest(provider.manifest(), provider_id.clone(), version);
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_protocol_adapter(provider_id, Arc::new(provider));
        self
    }

    pub fn register_mcp_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "mcp",
            version,
            vec!["mcp.tools", "mcp.resources", "mcp.prompts"],
        ))
    }

    pub fn register_mcp_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: McpProvider + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "mcp",
            version,
            vec!["mcp.tools", "mcp.resources", "mcp.prompts"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_mcp_provider(provider_id, Arc::new(provider));
        self
    }

    pub fn register_agent_skill_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "skill",
            version,
            vec!["skill.discover", "skill.invoke"],
        ))
    }

    pub fn register_agent_skill_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: AgentSkillProvider + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "skill",
            version,
            vec!["skill.discover", "skill.invoke"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_agent_skill_provider(provider_id, Arc::new(provider));
        self
    }

    pub fn register_collaboration_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "collaboration",
            version,
            vec!["agent.discover", "agent.handoff", "agent.delegate"],
        ))
    }

    pub fn register_collaboration_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: AgentCollaborationProvider + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "collaboration",
            version,
            vec!["agent.discover", "agent.handoff", "agent.delegate"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_collaboration_provider(provider_id, Arc::new(provider));
        self
    }

    pub fn register_telemetry_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "telemetry",
            version,
            vec!["telemetry.record"],
        ))
    }

    pub fn register_telemetry_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: TelemetryProvider + Send + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "telemetry",
            version,
            vec!["telemetry.record"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_telemetry_provider(provider_id, Arc::new(Mutex::new(provider)));
        self
    }

    pub fn register_task_scheduling_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "task_scheduling",
            version,
            vec!["task.schedule", "task.cancel", "task.list"],
        ))
    }

    pub fn register_task_scheduling_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: TaskSchedulingProvider + Send + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "task_scheduling",
            version,
            vec!["task.schedule", "task.cancel", "task.list"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_task_scheduling_provider(provider_id, Arc::new(Mutex::new(provider)));
        self
    }

    pub fn register_classification_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "agent_classification",
            version,
            vec!["agent.classify", "agent.classification.list"],
        ))
    }

    pub fn register_classification_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: AgentClassificationProvider + Send + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "agent_classification",
            version,
            vec!["agent.classify", "agent.classification.list"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_classification_provider(provider_id, Arc::new(Mutex::new(provider)));
        self
    }

    pub fn register_message_query_provider_manifest(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(core_provider_manifest(
            provider_id,
            "message_query",
            version,
            vec!["message.query", "message.list_sessions"],
        ))
    }

    pub fn register_message_query_provider<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: MessageQueryProvider + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "message_query",
            version,
            vec!["message.query", "message.list_sessions"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry
            .add_message_query_provider(provider_id, Arc::new(provider));
        self
    }

    pub fn register_agent_installer_provider(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(agent_installer_provider(provider_id, version))
    }

    pub fn register_agent_installer<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: AgentInstaller + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "agent_installer",
            version,
            vec!["agent.install", "agent.uninstall", "agent.upgrade"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry.agent_installer_provider_id = Some(provider_id);
        self.provider_registry.agent_installer = Some(Arc::new(provider));
        self
    }

    pub fn register_agent_configuration_provider(
        self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.register_provider(agent_configuration_provider(provider_id, version))
    }

    pub fn register_agent_configuration<T>(
        mut self,
        provider_id: impl Into<String>,
        version: impl Into<String>,
        provider: T,
    ) -> Self
    where
        T: AgentConfigurationProvider + Send + Sync + 'static,
    {
        let provider_id = provider_id.into();
        let version = version.into();
        let provider_manifest = typed_provider_manifest(
            provider.provider_manifest(),
            provider_id.clone(),
            "agent_configuration",
            version,
            vec!["agent.configure"],
        );
        self.providers.push(provider_manifest);
        self.provider_registry.agent_configuration_provider_id = Some(provider_id);
        self.provider_registry.agent_configuration = Some(Arc::new(provider));
        self
    }

    pub fn with_agent_package_manifest(mut self, package: AgentPackageManifest) -> Self {
        let installer_provider_id = package.provider_binding.installer_provider_id.clone();
        let configuration_provider_id = package.provider_binding.configuration_provider_id.clone();
        let version = package.version.clone();

        self.providers.push(agent_installer_provider(
            installer_provider_id,
            version.clone(),
        ));
        self.providers.push(agent_configuration_provider(
            configuration_provider_id,
            version,
        ));
        self.agent_package_manifest = Some(package);
        self
    }

    pub fn with_security_profile(mut self, security_profile: impl Into<String>) -> Self {
        self.security_profile = security_profile.into();
        self
    }

    pub fn with_required_security_profile(
        mut self,
        required_security_profile: impl Into<String>,
    ) -> Self {
        self.required_security_profile = Some(required_security_profile.into());
        self
    }

    pub fn with_generated_at(mut self, generated_at: impl Into<String>) -> Self {
        self.generated_at = generated_at.into();
        self
    }

    /// Register a sandbox provider used to wrap host `process` execution.
    pub fn with_host_sandbox_provider(mut self, sandbox: Arc<dyn SandboxProvider>) -> Self {
        self.host_sandbox_provider = Some(sandbox);
        self.host_sandbox_enabled = true;
        self
    }

    /// Enable platform-native host sandboxing when the current OS supports it.
    pub fn enable_platform_host_sandbox(mut self) -> Self {
        if let Some(provider) = PlatformSandboxProvider::detect() {
            self.host_sandbox_provider = Some(Arc::new(provider));
            self.host_sandbox_enabled = true;
        }
        self
    }

    fn wrap_host_provider(
        &self,
        provider: Arc<dyn HostProvider + Send + Sync>,
    ) -> Arc<dyn HostProvider + Send + Sync> {
        let provider = Arc::new(EnvFileSecretFallbackHostProvider::new(provider));
        if !self.host_sandbox_enabled {
            return provider;
        }

        let Some(sandbox) = self.host_sandbox_provider.clone() else {
            return provider;
        };

        Arc::new(SandboxingHostProvider::new(provider, sandbox))
    }

    pub fn bootstrap(self) -> KernelResult<RuntimeBootstrapReport> {
        if let Some(required_security_profile) = &self.required_security_profile {
            if required_security_profile != &self.security_profile {
                return Err(KernelError::validation(
                    "runtime security profile does not match required security profile",
                ));
            }
        }
        self.validate_agent_package_manifest()?;
        self.validate_agent_package_configuration_sections()?;

        let mut events = vec![self.runtime_event("agent.runtime.bootstrap.started")];
        if !self.providers.is_empty() {
            events.push(self.runtime_event("agent.runtime.providers.registered"));
        }
        if self.provider_for_capability("agent.install").is_some() {
            events.push(self.runtime_event("agent.install.provider.registered"));
        }
        if self.provider_for_capability("agent.configure").is_some() {
            events.push(self.runtime_event("agent.configure.provider.registered"));
        }

        let capability_manifest = self.build_capability_manifest();
        let runtime = AgentRuntime::from_capability_manifest_with_provider_registry(
            capability_manifest,
            self.provider_registry.clone(),
            self.kernel_hooks.clone(),
            ModelCostCalculator::new(self.model_prices.clone()),
        );
        events.push(match runtime.state() {
            RuntimeState::Ready => self.runtime_event("agent.runtime.ready"),
            RuntimeState::Degraded => self.runtime_event("agent.runtime.degraded"),
            RuntimeState::Failed => self.runtime_event("agent.runtime.failed"),
        });

        Ok(RuntimeBootstrapReport { runtime, events })
    }

    fn build_capability_manifest(&self) -> CapabilityManifest {
        let mut capabilities = Vec::new();
        let mut missing_required_capabilities = Vec::new();
        let mut degraded_capabilities = Vec::new();

        for requirement in &self.agent_manifest.required_capability_requirements {
            match self.provider_for_requirement(requirement) {
                Some(provider) => capabilities.push(capability_from_provider(
                    &requirement.capability_id,
                    provider,
                    true,
                )),
                None => missing_required_capabilities.push(requirement.capability_id.clone()),
            }
        }

        for requirement in &self.agent_manifest.optional_capability_requirements {
            match self.provider_for_requirement(requirement) {
                Some(provider) => capabilities.push(capability_from_provider(
                    &requirement.capability_id,
                    provider,
                    false,
                )),
                None => degraded_capabilities.push(requirement.capability_id.clone()),
            }
        }

        for provider in &self.providers {
            for capability_id in &provider.capabilities {
                let already_negotiated = capabilities
                    .iter()
                    .any(|capability| capability.capability_id == *capability_id);
                let incompatible_required = missing_required_capabilities
                    .iter()
                    .any(|missing| missing == capability_id);
                let incompatible_optional = degraded_capabilities
                    .iter()
                    .any(|degraded| degraded == capability_id);

                if !already_negotiated && !incompatible_required && !incompatible_optional {
                    capabilities.push(capability_from_provider(capability_id, provider, false));
                }
            }
        }

        CapabilityManifest {
            schema_version: self.agent_manifest.schema_version.clone(),
            manifest_type: "capability".to_string(),
            runtime_id: self.runtime_id.clone(),
            agent_id: self.agent_manifest.agent_id.clone(),
            kernel_version: AGENT_KERNEL_SPEC_VERSION.to_string(),
            providers: self.providers.clone(),
            capabilities,
            missing_required_capabilities,
            degraded_capabilities,
            protocol_adapters: self.protocol_adapter_ids(),
            security_profile: self.security_profile.clone(),
            generated_at: self.generated_at.clone(),
        }
    }

    fn protocol_adapter_ids(&self) -> Vec<String> {
        self.providers
            .iter()
            .filter(|provider| provider.provider_family == "protocol_adapter")
            .map(|provider| provider.provider_id.clone())
            .collect()
    }

    fn provider_for_capability(&self, capability_id: &str) -> Option<&ProviderManifest> {
        self.providers.iter().find(|provider| {
            provider
                .capabilities
                .iter()
                .any(|capability| capability == capability_id)
        })
    }

    fn provider_for_requirement(
        &self,
        requirement: &CapabilityRequirement,
    ) -> Option<&ProviderManifest> {
        self.providers
            .iter()
            .filter(|provider| {
                provider
                    .capabilities
                    .iter()
                    .any(|capability| capability == &requirement.capability_id)
            })
            .find(|provider| {
                requirement
                    .min_version
                    .as_deref()
                    .is_none_or(|min_version| {
                        provider_version_satisfies(&provider.version, min_version)
                    })
            })
    }

    fn validate_agent_package_manifest(&self) -> KernelResult<()> {
        let Some(package) = &self.agent_package_manifest else {
            return Ok(());
        };

        package.lifecycle.validate()?;
        package.provider_binding.validate()?;

        if package.agent_id != self.agent_manifest.agent_id {
            return Err(KernelError::validation(
                "agent id mismatch between agent manifest and package manifest",
            ));
        }

        if !package.is_compatible_with_agent_kernel(AGENT_KERNEL_SPEC_VERSION) {
            return Err(KernelError::validation(
                "agent package is incompatible with current kernel version",
            ));
        }

        if package.required_configuration_sections.is_empty() {
            return Err(KernelError::validation(
                "at least one required configuration section must be declared",
            ));
        }

        Ok(())
    }

    fn validate_agent_package_configuration_sections(&self) -> KernelResult<()> {
        let (Some(package), Some(provider)) = (
            &self.agent_package_manifest,
            self.provider_registry.agent_configuration.as_deref(),
        ) else {
            return Ok(());
        };

        let spec = provider.configuration_spec(&package.agent_id)?;
        for required_section in &package.required_configuration_sections {
            if !configuration_spec_has_section_kind(&spec.sections, required_section) {
                return Err(KernelError::validation(format!(
                    "required configuration section is missing: {required_section:?}"
                )));
            }
        }

        Ok(())
    }

    fn runtime_event(&self, event_type: &str) -> KernelEvent {
        KernelEvent::new(
            format!("event.{}.{}", self.runtime_id, event_type),
            event_type,
            KernelEventSeverity::Info,
            format!(
                "runtime_id={};agent_id={}",
                self.runtime_id, self.agent_manifest.agent_id
            ),
        )
    }
}

#[derive(Clone, Default)]
pub struct RuntimeProviderRegistry {
    agent_installer_provider_id: Option<String>,
    agent_installer: Option<Arc<dyn AgentInstaller + Send + Sync>>,
    agent_configuration_provider_id: Option<String>,
    agent_configuration: Option<Arc<dyn AgentConfigurationProvider + Send + Sync>>,
    model_provider_id: Option<String>,
    model_provider: Option<Arc<dyn ModelProvider + Send + Sync>>,
    model_providers: Vec<(String, Arc<dyn ModelProvider + Send + Sync>)>,
    provider_session_activity_providers: Vec<(
        String,
        Arc<dyn ProviderSessionActivityProvider + Send + Sync>,
    )>,
    provider_session_control_provider_id: Option<String>,
    provider_session_control_provider:
        Option<Arc<dyn ProviderSessionControlProvider + Send + Sync>>,
    provider_session_control_providers: Vec<(
        String,
        Arc<dyn ProviderSessionControlProvider + Send + Sync>,
    )>,
    tool_provider_id: Option<String>,
    tool_provider: Option<Arc<dyn ToolProvider + Send + Sync>>,
    tool_providers: Vec<(String, Arc<dyn ToolProvider + Send + Sync>)>,
    policy_provider_id: Option<String>,
    policy_provider: Option<Arc<dyn PolicyProvider + Send + Sync>>,
    policy_providers: Vec<(String, Arc<dyn PolicyProvider + Send + Sync>)>,
    context_provider_id: Option<String>,
    context_provider: Option<Arc<dyn ContextProvider + Send + Sync>>,
    context_providers: Vec<(String, Arc<dyn ContextProvider + Send + Sync>)>,
    memory_provider_id: Option<String>,
    memory_provider: Option<Arc<Mutex<dyn MemoryProvider + Send>>>,
    memory_providers: Vec<(String, Arc<Mutex<dyn MemoryProvider + Send>>)>,
    knowledge_provider_id: Option<String>,
    knowledge_provider: Option<Arc<dyn KnowledgeProvider + Send + Sync>>,
    knowledge_providers: Vec<(String, Arc<dyn KnowledgeProvider + Send + Sync>)>,
    planning_provider_id: Option<String>,
    planning_provider: Option<Arc<dyn PlanningProvider + Send + Sync>>,
    planning_providers: Vec<(String, Arc<dyn PlanningProvider + Send + Sync>)>,
    host_provider_id: Option<String>,
    host_provider: Option<Arc<dyn HostProvider + Send + Sync>>,
    host_providers: Vec<(String, Arc<dyn HostProvider + Send + Sync>)>,
    protocol_adapter_id: Option<String>,
    protocol_adapter: Option<Arc<dyn ProtocolAdapter + Send + Sync>>,
    protocol_adapters: Vec<(String, Arc<dyn ProtocolAdapter + Send + Sync>)>,
    mcp_provider_id: Option<String>,
    mcp_provider: Option<Arc<dyn McpProvider + Send + Sync>>,
    mcp_providers: Vec<(String, Arc<dyn McpProvider + Send + Sync>)>,
    agent_skill_provider_id: Option<String>,
    agent_skill_provider: Option<Arc<dyn AgentSkillProvider + Send + Sync>>,
    agent_skill_providers: Vec<(String, Arc<dyn AgentSkillProvider + Send + Sync>)>,
    collaboration_provider_id: Option<String>,
    collaboration_provider: Option<Arc<dyn AgentCollaborationProvider + Send + Sync>>,
    collaboration_providers: Vec<(String, Arc<dyn AgentCollaborationProvider + Send + Sync>)>,
    telemetry_provider_id: Option<String>,
    telemetry_provider: Option<Arc<Mutex<dyn TelemetryProvider + Send>>>,
    telemetry_providers: Vec<(String, Arc<Mutex<dyn TelemetryProvider + Send>>)>,
    task_scheduling_provider_id: Option<String>,
    task_scheduling_provider: Option<Arc<Mutex<dyn TaskSchedulingProvider + Send>>>,
    task_scheduling_providers: Vec<(String, Arc<Mutex<dyn TaskSchedulingProvider + Send>>)>,
    classification_provider_id: Option<String>,
    classification_provider: Option<Arc<Mutex<dyn AgentClassificationProvider + Send>>>,
    classification_providers: Vec<(String, Arc<Mutex<dyn AgentClassificationProvider + Send>>)>,
    message_query_provider_id: Option<String>,
    message_query_provider: Option<Arc<dyn MessageQueryProvider + Send + Sync>>,
    message_query_providers: Vec<(String, Arc<dyn MessageQueryProvider + Send + Sync>)>,
}

impl RuntimeProviderRegistry {
    fn add_model_provider(
        &mut self,
        provider_id: String,
        provider: Arc<dyn ModelProvider + Send + Sync>,
    ) {
        if self.model_provider.is_none() {
            self.model_provider_id = Some(provider_id.clone());
            self.model_provider = Some(provider.clone());
        }
        self.model_providers.push((provider_id, provider));
    }

    fn model_provider_by_id(
        &self,
        provider_id: &str,
    ) -> Option<&(dyn ModelProvider + Send + Sync)> {
        self.model_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.as_ref())
    }

    pub fn model_provider_ids(&self) -> Vec<String> {
        self.model_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_provider_session_activity_provider(
        &mut self,
        provider_id: String,
        provider: Arc<dyn ProviderSessionActivityProvider + Send + Sync>,
    ) {
        self.provider_session_activity_providers
            .push((provider_id, provider));
    }

    fn provider_session_activity_provider_by_id(
        &self,
        provider_id: &str,
    ) -> Option<&(dyn ProviderSessionActivityProvider + Send + Sync)> {
        self.provider_session_activity_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.as_ref())
    }

    pub fn provider_session_activity_provider_ids(&self) -> Vec<String> {
        self.provider_session_activity_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_provider_session_control_provider(
        &mut self,
        provider_id: String,
        provider: Arc<dyn ProviderSessionControlProvider + Send + Sync>,
    ) {
        if self.provider_session_control_provider.is_none() {
            self.provider_session_control_provider_id = Some(provider_id.clone());
            self.provider_session_control_provider = Some(provider.clone());
        }
        self.provider_session_control_providers
            .push((provider_id, provider));
    }

    fn provider_session_control_provider_by_id(
        &self,
        provider_id: &str,
    ) -> Option<&(dyn ProviderSessionControlProvider + Send + Sync)> {
        self.provider_session_control_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.as_ref())
    }

    pub fn provider_session_control_provider_ids(&self) -> Vec<String> {
        self.provider_session_control_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_tool_provider(
        &mut self,
        provider_id: String,
        provider: Arc<dyn ToolProvider + Send + Sync>,
    ) {
        if self.tool_provider.is_none() {
            self.tool_provider_id = Some(provider_id.clone());
            self.tool_provider = Some(provider.clone());
        }
        self.tool_providers.push((provider_id, provider));
    }

    fn tool_provider_by_id(&self, provider_id: &str) -> Option<&(dyn ToolProvider + Send + Sync)> {
        self.tool_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.as_ref())
    }

    pub fn tool_provider_ids(&self) -> Vec<String> {
        self.tool_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_policy_provider(
        &mut self,
        provider_id: String,
        provider: Arc<dyn PolicyProvider + Send + Sync>,
    ) {
        if self.policy_provider.is_none() {
            self.policy_provider_id = Some(provider_id.clone());
            self.policy_provider = Some(provider.clone());
        }
        self.policy_providers.push((provider_id, provider));
    }

    fn policy_provider_by_id(
        &self,
        provider_id: &str,
    ) -> Option<&(dyn PolicyProvider + Send + Sync)> {
        self.policy_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.as_ref())
    }

    pub fn policy_provider_ids(&self) -> Vec<String> {
        self.policy_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_context_provider(
        &mut self,
        provider_id: String,
        provider: Arc<dyn ContextProvider + Send + Sync>,
    ) {
        if self.context_provider.is_none() {
            self.context_provider_id = Some(provider_id.clone());
            self.context_provider = Some(provider.clone());
        }
        self.context_providers.push((provider_id, provider));
    }

    fn context_provider_by_id(
        &self,
        provider_id: &str,
    ) -> Option<&(dyn ContextProvider + Send + Sync)> {
        self.context_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.as_ref())
    }

    pub fn context_provider_ids(&self) -> Vec<String> {
        self.context_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_memory_provider(
        &mut self,
        provider_id: String,
        provider: Arc<Mutex<dyn MemoryProvider + Send>>,
    ) {
        if self.memory_provider.is_none() {
            self.memory_provider_id = Some(provider_id.clone());
            self.memory_provider = Some(provider.clone());
        }
        self.memory_providers.push((provider_id, provider));
    }

    fn memory_provider_by_id(
        &self,
        provider_id: &str,
    ) -> Option<Arc<Mutex<dyn MemoryProvider + Send>>> {
        self.memory_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.clone())
    }

    pub fn memory_provider_ids(&self) -> Vec<String> {
        self.memory_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_knowledge_provider(
        &mut self,
        provider_id: String,
        provider: Arc<dyn KnowledgeProvider + Send + Sync>,
    ) {
        if self.knowledge_provider.is_none() {
            self.knowledge_provider_id = Some(provider_id.clone());
            self.knowledge_provider = Some(provider.clone());
        }
        self.knowledge_providers.push((provider_id, provider));
    }

    fn knowledge_provider_by_id(
        &self,
        provider_id: &str,
    ) -> Option<&(dyn KnowledgeProvider + Send + Sync)> {
        self.knowledge_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.as_ref())
    }

    pub fn knowledge_provider_ids(&self) -> Vec<String> {
        self.knowledge_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_planning_provider(
        &mut self,
        provider_id: String,
        provider: Arc<dyn PlanningProvider + Send + Sync>,
    ) {
        if self.planning_provider.is_none() {
            self.planning_provider_id = Some(provider_id.clone());
            self.planning_provider = Some(provider.clone());
        }
        self.planning_providers.push((provider_id, provider));
    }

    fn planning_provider_by_id(
        &self,
        provider_id: &str,
    ) -> Option<&(dyn PlanningProvider + Send + Sync)> {
        self.planning_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.as_ref())
    }

    pub fn planning_provider_ids(&self) -> Vec<String> {
        self.planning_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_host_provider(
        &mut self,
        provider_id: String,
        provider: Arc<dyn HostProvider + Send + Sync>,
    ) {
        if self.host_provider.is_none() {
            self.host_provider_id = Some(provider_id.clone());
            self.host_provider = Some(provider.clone());
        }
        self.host_providers.push((provider_id, provider));
    }

    fn host_provider_by_id(&self, provider_id: &str) -> Option<&(dyn HostProvider + Send + Sync)> {
        self.host_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.as_ref())
    }

    pub fn host_provider_ids(&self) -> Vec<String> {
        self.host_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_mcp_provider(
        &mut self,
        provider_id: String,
        provider: Arc<dyn McpProvider + Send + Sync>,
    ) {
        if self.mcp_provider.is_none() {
            self.mcp_provider_id = Some(provider_id.clone());
            self.mcp_provider = Some(provider.clone());
        }
        self.mcp_providers.push((provider_id, provider));
    }

    fn mcp_provider_by_id(&self, provider_id: &str) -> Option<&(dyn McpProvider + Send + Sync)> {
        self.mcp_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.as_ref())
    }

    pub fn mcp_provider_ids(&self) -> Vec<String> {
        self.mcp_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_agent_skill_provider(
        &mut self,
        provider_id: String,
        provider: Arc<dyn AgentSkillProvider + Send + Sync>,
    ) {
        if self.agent_skill_provider.is_none() {
            self.agent_skill_provider_id = Some(provider_id.clone());
            self.agent_skill_provider = Some(provider.clone());
        }
        self.agent_skill_providers.push((provider_id, provider));
    }

    fn agent_skill_provider_by_id(
        &self,
        provider_id: &str,
    ) -> Option<&(dyn AgentSkillProvider + Send + Sync)> {
        self.agent_skill_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.as_ref())
    }

    pub fn agent_skill_provider_ids(&self) -> Vec<String> {
        self.agent_skill_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_collaboration_provider(
        &mut self,
        provider_id: String,
        provider: Arc<dyn AgentCollaborationProvider + Send + Sync>,
    ) {
        if self.collaboration_provider.is_none() {
            self.collaboration_provider_id = Some(provider_id.clone());
            self.collaboration_provider = Some(provider.clone());
        }
        self.collaboration_providers.push((provider_id, provider));
    }

    fn collaboration_provider_by_id(
        &self,
        provider_id: &str,
    ) -> Option<&(dyn AgentCollaborationProvider + Send + Sync)> {
        self.collaboration_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.as_ref())
    }

    pub fn collaboration_provider_ids(&self) -> Vec<String> {
        self.collaboration_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    pub fn has_agent_installer(&self) -> bool {
        self.agent_installer.is_some()
    }

    pub fn has_agent_configuration_provider(&self) -> bool {
        self.agent_configuration.is_some()
    }

    pub fn has_model_provider(&self) -> bool {
        !self.model_providers.is_empty()
    }

    pub fn has_provider_session_activity_provider(&self) -> bool {
        !self.provider_session_activity_providers.is_empty()
    }

    pub fn has_provider_session_control_provider(&self) -> bool {
        !self.provider_session_control_providers.is_empty()
    }

    pub fn has_tool_provider(&self) -> bool {
        !self.tool_providers.is_empty()
    }

    pub fn has_policy_provider(&self) -> bool {
        !self.policy_providers.is_empty()
    }

    pub fn has_context_provider(&self) -> bool {
        !self.context_providers.is_empty()
    }

    pub fn has_memory_provider(&self) -> bool {
        !self.memory_providers.is_empty()
    }

    pub fn has_knowledge_provider(&self) -> bool {
        !self.knowledge_providers.is_empty()
    }

    pub fn has_planning_provider(&self) -> bool {
        !self.planning_providers.is_empty()
    }

    pub fn has_host_provider(&self) -> bool {
        !self.host_providers.is_empty()
    }

    pub fn has_protocol_adapter(&self) -> bool {
        !self.protocol_adapters.is_empty()
    }

    fn add_protocol_adapter(
        &mut self,
        provider_id: String,
        provider: Arc<dyn ProtocolAdapter + Send + Sync>,
    ) {
        if self.protocol_adapter.is_none() {
            self.protocol_adapter_id = Some(provider_id.clone());
            self.protocol_adapter = Some(provider.clone());
        }
        self.protocol_adapters.push((provider_id, provider));
    }

    fn protocol_adapter_by_id(
        &self,
        provider_id: &str,
    ) -> Option<&(dyn ProtocolAdapter + Send + Sync)> {
        self.protocol_adapters
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.as_ref())
    }

    pub fn protocol_adapter_ids(&self) -> Vec<String> {
        self.protocol_adapters
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    pub fn has_mcp_provider(&self) -> bool {
        !self.mcp_providers.is_empty()
    }

    pub fn has_agent_skill_provider(&self) -> bool {
        !self.agent_skill_providers.is_empty()
    }

    pub fn has_collaboration_provider(&self) -> bool {
        !self.collaboration_providers.is_empty()
    }

    pub fn has_telemetry_provider(&self) -> bool {
        !self.telemetry_providers.is_empty()
    }

    pub fn has_task_scheduling_provider(&self) -> bool {
        !self.task_scheduling_providers.is_empty()
    }

    pub fn has_classification_provider(&self) -> bool {
        !self.classification_providers.is_empty()
    }

    pub fn has_message_query_provider(&self) -> bool {
        !self.message_query_providers.is_empty()
    }

    fn add_task_scheduling_provider(
        &mut self,
        provider_id: String,
        provider: Arc<Mutex<dyn TaskSchedulingProvider + Send>>,
    ) {
        if self.task_scheduling_provider.is_none() {
            self.task_scheduling_provider_id = Some(provider_id.clone());
            self.task_scheduling_provider = Some(provider.clone());
        }
        self.task_scheduling_providers.push((provider_id, provider));
    }

    fn task_scheduling_provider_by_id(
        &self,
        provider_id: &str,
    ) -> Option<Arc<Mutex<dyn TaskSchedulingProvider + Send>>> {
        self.task_scheduling_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.clone())
    }

    pub fn task_scheduling_provider_ids(&self) -> Vec<String> {
        self.task_scheduling_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_classification_provider(
        &mut self,
        provider_id: String,
        provider: Arc<Mutex<dyn AgentClassificationProvider + Send>>,
    ) {
        if self.classification_provider.is_none() {
            self.classification_provider_id = Some(provider_id.clone());
            self.classification_provider = Some(provider.clone());
        }
        self.classification_providers.push((provider_id, provider));
    }

    fn classification_provider_by_id(
        &self,
        provider_id: &str,
    ) -> Option<Arc<Mutex<dyn AgentClassificationProvider + Send>>> {
        self.classification_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.clone())
    }

    pub fn classification_provider_ids(&self) -> Vec<String> {
        self.classification_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_message_query_provider(
        &mut self,
        provider_id: String,
        provider: Arc<dyn MessageQueryProvider + Send + Sync>,
    ) {
        if self.message_query_provider.is_none() {
            self.message_query_provider_id = Some(provider_id.clone());
            self.message_query_provider = Some(provider.clone());
        }
        self.message_query_providers.push((provider_id, provider));
    }

    fn message_query_provider_by_id(
        &self,
        provider_id: &str,
    ) -> Option<&(dyn MessageQueryProvider + Send + Sync)> {
        self.message_query_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.as_ref())
    }

    pub fn message_query_provider_ids(&self) -> Vec<String> {
        self.message_query_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn add_telemetry_provider(
        &mut self,
        provider_id: String,
        provider: Arc<Mutex<dyn TelemetryProvider + Send>>,
    ) {
        if self.telemetry_provider.is_none() {
            self.telemetry_provider_id = Some(provider_id.clone());
            self.telemetry_provider = Some(provider.clone());
        }
        self.telemetry_providers.push((provider_id, provider));
    }

    fn telemetry_provider_by_id(
        &self,
        provider_id: &str,
    ) -> Option<Arc<Mutex<dyn TelemetryProvider + Send>>> {
        self.telemetry_providers
            .iter()
            .find(|(registered_provider_id, _)| registered_provider_id == provider_id)
            .map(|(_, provider)| provider.clone())
    }

    pub fn telemetry_provider_ids(&self) -> Vec<String> {
        self.telemetry_providers
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect()
    }

    fn has_typed_provider(&self, provider: &ProviderManifest) -> bool {
        match provider.provider_family.as_str() {
            "agent_installer" => {
                self.agent_installer_provider_id.as_deref() == Some(provider.provider_id.as_str())
                    && self.has_agent_installer()
            }
            "agent_configuration" => {
                self.agent_configuration_provider_id.as_deref()
                    == Some(provider.provider_id.as_str())
                    && self.has_agent_configuration_provider()
            }
            "model" => self
                .model_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "session_control" => self
                .provider_session_control_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "tool" => self
                .tool_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "policy" => self
                .policy_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "context" => self
                .context_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "memory" => self
                .memory_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "knowledge" => self
                .knowledge_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "planning" => self
                .planning_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "host" => self
                .host_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "protocol_adapter" => self
                .protocol_adapter_by_id(provider.provider_id.as_str())
                .is_some(),
            "mcp" => self
                .mcp_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "skill" => self
                .agent_skill_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "collaboration" => self
                .collaboration_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "telemetry" => self
                .telemetry_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "task_scheduling" => self
                .task_scheduling_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "agent_classification" => self
                .classification_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            "message_query" => self
                .message_query_provider_by_id(provider.provider_id.as_str())
                .is_some(),
            _ => false,
        }
    }

    fn health_for_provider(&self, provider: &ProviderManifest) -> Option<ProviderHealth> {
        match provider.provider_family.as_str() {
            "agent_installer"
                if self.agent_installer_provider_id.as_deref()
                    == Some(provider.provider_id.as_str()) =>
            {
                self.agent_installer
                    .as_ref()
                    .map(|provider| provider.health())
            }
            "agent_configuration"
                if self.agent_configuration_provider_id.as_deref()
                    == Some(provider.provider_id.as_str()) =>
            {
                self.agent_configuration
                    .as_ref()
                    .map(|provider| provider.health())
            }
            "model" => self
                .model_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .map(|(_, provider)| provider.health()),
            "session_control" => self
                .provider_session_control_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .map(|(_, provider)| provider.health()),
            "tool" => self
                .tool_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .map(|(_, provider)| provider.health()),
            "policy" => self
                .policy_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .map(|(_, provider)| provider.health()),
            "context" => self
                .context_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .map(|(_, provider)| provider.health()),
            "memory" => self
                .memory_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .and_then(|(_, provider)| provider.lock().ok().map(|provider| provider.health())),
            "knowledge" => self
                .knowledge_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .map(|(_, provider)| provider.health()),
            "planning" => self
                .planning_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .map(|(_, provider)| provider.health()),
            "host" => self
                .host_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .map(|(_, provider)| provider.health()),
            "protocol_adapter" => self
                .protocol_adapters
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .map(|(_, provider)| provider.health()),
            "mcp" => self
                .mcp_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .map(|(_, provider)| provider.health()),
            "skill" => self
                .agent_skill_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .map(|(_, provider)| provider.health()),
            "collaboration" => self
                .collaboration_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .map(|(_, provider)| provider.health()),
            "telemetry" => self
                .telemetry_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .and_then(|(_, provider)| provider.lock().ok().map(|provider| provider.health())),
            "task_scheduling" => self
                .task_scheduling_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .and_then(|(_, provider)| provider.lock().ok().map(|provider| provider.health())),
            "agent_classification" => self
                .classification_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .and_then(|(_, provider)| provider.lock().ok().map(|provider| provider.health())),
            "message_query" => self
                .message_query_providers
                .iter()
                .find(|(provider_id, _)| provider_id == &provider.provider_id)
                .map(|(_, provider)| provider.health()),
            _ => None,
        }
    }

    fn has_degraded_provider_health(&self, providers: &[ProviderManifest]) -> bool {
        providers.iter().any(|provider| {
            self.health_for_provider(provider)
                .is_some_and(|health| health.status != "available")
        })
    }
}

impl std::fmt::Debug for RuntimeProviderRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeProviderRegistry")
            .field(
                "agent_installer_provider_id",
                &self.agent_installer_provider_id,
            )
            .field("has_agent_installer", &self.has_agent_installer())
            .field(
                "agent_configuration_provider_id",
                &self.agent_configuration_provider_id,
            )
            .field(
                "has_agent_configuration_provider",
                &self.has_agent_configuration_provider(),
            )
            .field("model_provider_id", &self.model_provider_id)
            .field("model_provider_ids", &self.model_provider_ids())
            .field("has_model_provider", &self.has_model_provider())
            .field(
                "provider_session_activity_provider_ids",
                &self.provider_session_activity_provider_ids(),
            )
            .field(
                "has_provider_session_activity_provider",
                &self.has_provider_session_activity_provider(),
            )
            .field(
                "provider_session_control_provider_id",
                &self.provider_session_control_provider_id,
            )
            .field(
                "provider_session_control_provider_ids",
                &self.provider_session_control_provider_ids(),
            )
            .field(
                "has_provider_session_control_provider",
                &self.has_provider_session_control_provider(),
            )
            .field("tool_provider_id", &self.tool_provider_id)
            .field("tool_provider_ids", &self.tool_provider_ids())
            .field("has_tool_provider", &self.has_tool_provider())
            .field("policy_provider_id", &self.policy_provider_id)
            .field("policy_provider_ids", &self.policy_provider_ids())
            .field("has_policy_provider", &self.has_policy_provider())
            .field("context_provider_id", &self.context_provider_id)
            .field("context_provider_ids", &self.context_provider_ids())
            .field("has_context_provider", &self.has_context_provider())
            .field("memory_provider_id", &self.memory_provider_id)
            .field("memory_provider_ids", &self.memory_provider_ids())
            .field("has_memory_provider", &self.has_memory_provider())
            .field("knowledge_provider_id", &self.knowledge_provider_id)
            .field("knowledge_provider_ids", &self.knowledge_provider_ids())
            .field("has_knowledge_provider", &self.has_knowledge_provider())
            .field("planning_provider_id", &self.planning_provider_id)
            .field("planning_provider_ids", &self.planning_provider_ids())
            .field("has_planning_provider", &self.has_planning_provider())
            .field("host_provider_id", &self.host_provider_id)
            .field("host_provider_ids", &self.host_provider_ids())
            .field("has_host_provider", &self.has_host_provider())
            .field("protocol_adapter_id", &self.protocol_adapter_id)
            .field("protocol_adapter_ids", &self.protocol_adapter_ids())
            .field("has_protocol_adapter", &self.has_protocol_adapter())
            .field("mcp_provider_id", &self.mcp_provider_id)
            .field("has_mcp_provider", &self.has_mcp_provider())
            .field("agent_skill_provider_id", &self.agent_skill_provider_id)
            .field("agent_skill_provider_ids", &self.agent_skill_provider_ids())
            .field("has_agent_skill_provider", &self.has_agent_skill_provider())
            .field("collaboration_provider_id", &self.collaboration_provider_id)
            .field(
                "collaboration_provider_ids",
                &self.collaboration_provider_ids(),
            )
            .field(
                "has_collaboration_provider",
                &self.has_collaboration_provider(),
            )
            .field("telemetry_provider_id", &self.telemetry_provider_id)
            .field("telemetry_provider_ids", &self.telemetry_provider_ids())
            .field("has_telemetry_provider", &self.has_telemetry_provider())
            .field(
                "task_scheduling_provider_id",
                &self.task_scheduling_provider_id,
            )
            .field(
                "task_scheduling_provider_ids",
                &self.task_scheduling_provider_ids(),
            )
            .field(
                "has_task_scheduling_provider",
                &self.has_task_scheduling_provider(),
            )
            .field(
                "classification_provider_id",
                &self.classification_provider_id,
            )
            .field(
                "classification_provider_ids",
                &self.classification_provider_ids(),
            )
            .field(
                "has_classification_provider",
                &self.has_classification_provider(),
            )
            .field("message_query_provider_id", &self.message_query_provider_id)
            .field(
                "message_query_provider_ids",
                &self.message_query_provider_ids(),
            )
            .field(
                "has_message_query_provider",
                &self.has_message_query_provider(),
            )
            .finish()
    }
}

impl PartialEq for RuntimeProviderRegistry {
    fn eq(&self, other: &Self) -> bool {
        self.agent_installer_provider_id == other.agent_installer_provider_id
            && self.has_agent_installer() == other.has_agent_installer()
            && self.agent_configuration_provider_id == other.agent_configuration_provider_id
            && self.has_agent_configuration_provider() == other.has_agent_configuration_provider()
            && self.model_provider_id == other.model_provider_id
            && self.model_provider_ids() == other.model_provider_ids()
            && self.has_model_provider() == other.has_model_provider()
            && self.provider_session_activity_provider_ids()
                == other.provider_session_activity_provider_ids()
            && self.has_provider_session_activity_provider()
                == other.has_provider_session_activity_provider()
            && self.provider_session_control_provider_id
                == other.provider_session_control_provider_id
            && self.provider_session_control_provider_ids()
                == other.provider_session_control_provider_ids()
            && self.has_provider_session_control_provider()
                == other.has_provider_session_control_provider()
            && self.tool_provider_id == other.tool_provider_id
            && self.tool_provider_ids() == other.tool_provider_ids()
            && self.has_tool_provider() == other.has_tool_provider()
            && self.policy_provider_id == other.policy_provider_id
            && self.policy_provider_ids() == other.policy_provider_ids()
            && self.has_policy_provider() == other.has_policy_provider()
            && self.context_provider_id == other.context_provider_id
            && self.context_provider_ids() == other.context_provider_ids()
            && self.has_context_provider() == other.has_context_provider()
            && self.memory_provider_id == other.memory_provider_id
            && self.memory_provider_ids() == other.memory_provider_ids()
            && self.has_memory_provider() == other.has_memory_provider()
            && self.knowledge_provider_id == other.knowledge_provider_id
            && self.knowledge_provider_ids() == other.knowledge_provider_ids()
            && self.has_knowledge_provider() == other.has_knowledge_provider()
            && self.planning_provider_id == other.planning_provider_id
            && self.planning_provider_ids() == other.planning_provider_ids()
            && self.has_planning_provider() == other.has_planning_provider()
            && self.host_provider_id == other.host_provider_id
            && self.host_provider_ids() == other.host_provider_ids()
            && self.has_host_provider() == other.has_host_provider()
            && self.protocol_adapter_id == other.protocol_adapter_id
            && self.protocol_adapter_ids() == other.protocol_adapter_ids()
            && self.has_protocol_adapter() == other.has_protocol_adapter()
            && self.mcp_provider_id == other.mcp_provider_id
            && self.has_mcp_provider() == other.has_mcp_provider()
            && self.agent_skill_provider_id == other.agent_skill_provider_id
            && self.agent_skill_provider_ids() == other.agent_skill_provider_ids()
            && self.has_agent_skill_provider() == other.has_agent_skill_provider()
            && self.collaboration_provider_id == other.collaboration_provider_id
            && self.collaboration_provider_ids() == other.collaboration_provider_ids()
            && self.has_collaboration_provider() == other.has_collaboration_provider()
            && self.telemetry_provider_id == other.telemetry_provider_id
            && self.telemetry_provider_ids() == other.telemetry_provider_ids()
            && self.has_telemetry_provider() == other.has_telemetry_provider()
            && self.task_scheduling_provider_id == other.task_scheduling_provider_id
            && self.task_scheduling_provider_ids() == other.task_scheduling_provider_ids()
            && self.has_task_scheduling_provider() == other.has_task_scheduling_provider()
            && self.classification_provider_id == other.classification_provider_id
            && self.classification_provider_ids() == other.classification_provider_ids()
            && self.has_classification_provider() == other.has_classification_provider()
            && self.message_query_provider_id == other.message_query_provider_id
            && self.message_query_provider_ids() == other.message_query_provider_ids()
            && self.has_message_query_provider() == other.has_message_query_provider()
    }
}

impl Eq for RuntimeProviderRegistry {}

fn capability_from_provider(
    capability_id: &str,
    provider: &ProviderManifest,
    required: bool,
) -> Capability {
    let metadata = capability_metadata(capability_id);

    Capability {
        capability_id: capability_id.to_string(),
        version: provider.version.clone(),
        provider_id: provider.provider_id.clone(),
        status: "available".to_string(),
        required,
        operations: metadata.operations,
        side_effect_level: metadata.side_effect_level,
        policy_categories: metadata.policy_categories,
    }
}

fn configuration_spec_has_section_kind(
    sections: &[crate::AgentConfigSection],
    required_section: &AgentConfigSectionKind,
) -> bool {
    sections
        .iter()
        .any(|section| &section.kind == required_section)
}

fn provider_version_satisfies(provider_version: &str, min_version: &str) -> bool {
    parse_semver_like(provider_version) >= parse_semver_like(min_version)
}

fn parse_semver_like(version: &str) -> (u64, u64, u64) {
    let core = version
        .split_once('-')
        .map(|(core, _)| core)
        .unwrap_or(version)
        .split_once('+')
        .map(|(core, _)| core)
        .unwrap_or(version);
    let mut parts = core.split('.').map(|part| part.parse::<u64>().unwrap_or(0));

    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

fn core_provider_manifest(
    provider_id: impl Into<String>,
    provider_family: impl Into<String>,
    version: impl Into<String>,
    capabilities: Vec<&str>,
) -> ProviderManifest {
    let provider_id = provider_id.into();
    ProviderManifest::new(
        provider_id.clone(),
        provider_family,
        provider_id,
        version,
        capabilities
            .into_iter()
            .map(std::string::ToString::to_string)
            .collect(),
    )
}

fn typed_provider_manifest(
    mut provider_manifest: ProviderManifest,
    provider_id: impl Into<String>,
    provider_family: impl Into<String>,
    version: impl Into<String>,
    fallback_capabilities: Vec<&str>,
) -> ProviderManifest {
    provider_manifest.provider_id = provider_id.into();
    provider_manifest.provider_family = provider_family.into();
    provider_manifest.version = version.into();
    if provider_manifest.capabilities.is_empty() {
        provider_manifest.capabilities = fallback_capabilities
            .into_iter()
            .map(std::string::ToString::to_string)
            .collect();
    }
    provider_manifest
}

fn protocol_adapter_provider_manifest(
    adapter_manifest: crate::ProtocolAdapterManifest,
    provider_id: impl Into<String>,
    version: impl Into<String>,
) -> ProviderManifest {
    let streaming_support = adapter_manifest.streaming_support;
    let mut capabilities: Vec<String> = adapter_manifest
        .exposed_capabilities
        .into_iter()
        .filter(|capability| capability.starts_with("protocol."))
        .collect();
    if !capabilities
        .iter()
        .any(|capability| capability == "protocol.map")
    {
        capabilities.push("protocol.map".to_string());
    }
    if streaming_support != crate::ProtocolAdapterStreamingSupport::None
        && !capabilities
            .iter()
            .any(|capability| capability == "protocol.stream")
    {
        capabilities.push("protocol.stream".to_string());
    }

    ProviderManifest::new(
        provider_id,
        "protocol_adapter",
        adapter_manifest.adapter_id,
        version,
        capabilities,
    )
}

fn agent_installer_provider(
    provider_id: impl Into<String>,
    version: impl Into<String>,
) -> ProviderManifest {
    let provider_id = provider_id.into();
    ProviderManifest::new(
        provider_id.clone(),
        "agent_installer",
        provider_id,
        version,
        vec![
            "agent.install".to_string(),
            "agent.uninstall".to_string(),
            "agent.upgrade".to_string(),
        ],
    )
}

fn agent_configuration_provider(
    provider_id: impl Into<String>,
    version: impl Into<String>,
) -> ProviderManifest {
    let provider_id = provider_id.into();
    ProviderManifest::new(
        provider_id.clone(),
        "agent_configuration",
        provider_id,
        version,
        vec!["agent.configure".to_string()],
    )
}

struct CapabilityMetadata {
    operations: Vec<String>,
    side_effect_level: Option<String>,
    policy_categories: Vec<String>,
}

fn capability_metadata(capability_id: &str) -> CapabilityMetadata {
    match capability_id {
        "agent.install" => lifecycle_capability_metadata(
            vec![
                "detect_installation",
                "configuration_spec",
                "plan_install",
                "install",
                "health",
            ],
            SideEffectLevel::SideEffectful,
            PolicyCategory::AgentInstall,
        ),
        "agent.uninstall" => lifecycle_capability_metadata(
            vec![
                "detect_installation",
                "plan_uninstall",
                "uninstall",
                "health",
            ],
            SideEffectLevel::Destructive,
            PolicyCategory::AgentUninstall,
        ),
        "agent.upgrade" => lifecycle_capability_metadata(
            vec!["detect_installation", "plan_upgrade", "upgrade", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::AgentUpgrade,
        ),
        "agent.configure" => lifecycle_capability_metadata(
            vec!["configuration_spec", "validate_configuration", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::AgentConfigure,
        ),
        "agent.discover" => lifecycle_capability_metadata(
            vec!["list_agents", "describe_agent", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("agent.discover".to_string()),
        ),
        "agent.handoff" => lifecycle_capability_metadata(
            vec!["handoff", "health"],
            SideEffectLevel::ExternalSend,
            PolicyCategory::ProductSpecific("agent.handoff".to_string()),
        ),
        "agent.delegate" => lifecycle_capability_metadata(
            vec!["handoff", "describe_agent", "health"],
            SideEffectLevel::ExternalSend,
            PolicyCategory::ProductSpecific("agent.delegate".to_string()),
        ),
        "model.catalog" => lifecycle_capability_metadata(
            vec!["list_models", "describe_model", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ModelInvoke,
        ),
        "model.chat" => lifecycle_capability_metadata(
            vec!["invoke", "health"],
            SideEffectLevel::ExternalSend,
            PolicyCategory::ModelInvoke,
        ),
        "model.reasoning" => lifecycle_capability_metadata(
            vec!["invoke", "stream", "health"],
            SideEffectLevel::ExternalSend,
            PolicyCategory::ModelInvoke,
        ),
        "model.tool_call" => lifecycle_capability_metadata(
            vec!["invoke", "stream", "health"],
            SideEffectLevel::ExternalSend,
            PolicyCategory::ToolInvoke,
        ),
        "model.structured_output" => lifecycle_capability_metadata(
            vec!["invoke", "validate_structured_output", "health"],
            SideEffectLevel::ExternalSend,
            PolicyCategory::ModelInvoke,
        ),
        "model.streaming" => lifecycle_capability_metadata(
            vec!["stream", "health"],
            SideEffectLevel::ExternalSend,
            PolicyCategory::ModelInvoke,
        ),
        "model.embedding" => lifecycle_capability_metadata(
            vec!["embed", "health"],
            SideEffectLevel::ExternalSend,
            PolicyCategory::ModelInvoke,
        ),
        "model.cancellation" => lifecycle_capability_metadata(
            vec!["cancel", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::ModelInvoke,
        ),
        "tool.invoke" => lifecycle_capability_metadata(
            vec!["invoke_tool"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::ToolInvoke,
        ),
        "tool.discovery" => lifecycle_capability_metadata(
            vec!["list_tools", "describe_tool", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ToolInvoke,
        ),
        "tool.streaming" => lifecycle_capability_metadata(
            vec!["stream_tool_call", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::ToolInvoke,
        ),
        "tool.cancellation" => lifecycle_capability_metadata(
            vec!["cancel_tool_call", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::ToolInvoke,
        ),
        "policy.evaluate" => lifecycle_capability_metadata(
            vec!["evaluate", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProviderConfigure,
        ),
        "context.collect" => lifecycle_capability_metadata(
            vec!["collect", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("context.collect".to_string()),
        ),
        "memory.query" => lifecycle_capability_metadata(
            vec!["query", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::MemoryRead,
        ),
        "memory.write" => lifecycle_capability_metadata(
            vec!["write", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::MemoryWrite,
        ),
        "memory.delete" => lifecycle_capability_metadata(
            vec!["delete", "health"],
            SideEffectLevel::Destructive,
            PolicyCategory::MemoryDelete,
        ),
        "memory.export" => lifecycle_capability_metadata(
            vec!["export", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::MemoryRead,
        ),
        "knowledge.search" => lifecycle_capability_metadata(
            vec!["search", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::KnowledgeSearch,
        ),
        "knowledge.read" => lifecycle_capability_metadata(
            vec!["read", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::KnowledgeRead,
        ),
        "knowledge.list" => lifecycle_capability_metadata(
            vec!["list", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::KnowledgeList,
        ),
        "planning.create" => lifecycle_capability_metadata(
            vec!["create_plan", "validate_plan", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("planning.create".to_string()),
        ),
        "session.control.interrupt" => lifecycle_capability_metadata(
            vec!["control", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::ProductSpecific("session.control.interrupt".to_string()),
        ),
        "session.control.compact" => lifecycle_capability_metadata(
            vec!["control", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::ProductSpecific("session.control.compact".to_string()),
        ),
        "session.control.fork" => lifecycle_capability_metadata(
            vec!["control", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::ProductSpecific("session.control.fork".to_string()),
        ),
        "telemetry.record" => lifecycle_capability_metadata(
            vec!["record_event", "record_metric", "record_log", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::ProductSpecific("telemetry.record".to_string()),
        ),
        "protocol.map" => lifecycle_capability_metadata(
            vec!["map_request", "map_response", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProtocolSend,
        ),
        "protocol.stream" => lifecycle_capability_metadata(
            vec!["map_event_to_stream_update", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProtocolSend,
        ),
        "mcp.tools" => lifecycle_capability_metadata(
            vec!["list_servers", "list_tools", "invoke_tool", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::ProductSpecific("mcp.tools".to_string()),
        ),
        "mcp.resources" => lifecycle_capability_metadata(
            vec!["list_servers", "list_resources", "read_resource", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("mcp.resources".to_string()),
        ),
        "mcp.prompts" => lifecycle_capability_metadata(
            vec!["list_servers", "list_prompts", "get_prompt", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("mcp.prompts".to_string()),
        ),
        "skill.discover" => lifecycle_capability_metadata(
            vec!["list_skills", "describe_skill", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("skill.discover".to_string()),
        ),
        "skill.invoke" => lifecycle_capability_metadata(
            vec!["describe_skill", "invoke_skill", "cancel_skill", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::ProductSpecific("skill.invoke".to_string()),
        ),
        "task.schedule" => lifecycle_capability_metadata(
            vec!["schedule", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::ProductSpecific("task.schedule".to_string()),
        ),
        "task.cancel" => lifecycle_capability_metadata(
            vec!["cancel", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::ProductSpecific("task.cancel".to_string()),
        ),
        "task.list" => lifecycle_capability_metadata(
            vec!["list", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("task.list".to_string()),
        ),
        "task.pause" => lifecycle_capability_metadata(
            vec!["pause", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::ProductSpecific("task.pause".to_string()),
        ),
        "task.resume" => lifecycle_capability_metadata(
            vec!["resume", "health"],
            SideEffectLevel::SideEffectful,
            PolicyCategory::ProductSpecific("task.resume".to_string()),
        ),
        "task.get_due" => lifecycle_capability_metadata(
            vec!["get_due", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("task.get_due".to_string()),
        ),
        "agent.classify" => lifecycle_capability_metadata(
            vec!["classify", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("agent.classify".to_string()),
        ),
        "agent.classification.get" => lifecycle_capability_metadata(
            vec!["get_classification", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("agent.classification.get".to_string()),
        ),
        "agent.classification.list" => lifecycle_capability_metadata(
            vec!["list_classifications", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("agent.classification.list".to_string()),
        ),
        "agent.classification.search" => lifecycle_capability_metadata(
            vec!["search_by_capability", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("agent.classification.search".to_string()),
        ),
        "message.query" => lifecycle_capability_metadata(
            vec!["query", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("message.query".to_string()),
        ),
        "message.count" => lifecycle_capability_metadata(
            vec!["count", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("message.count".to_string()),
        ),
        "message.list_sessions" => lifecycle_capability_metadata(
            vec!["list_sessions", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("message.list_sessions".to_string()),
        ),
        "message.search" => lifecycle_capability_metadata(
            vec!["search_content", "health"],
            SideEffectLevel::ReadOnly,
            PolicyCategory::ProductSpecific("message.search".to_string()),
        ),
        _ => CapabilityMetadata {
            operations: Vec::new(),
            side_effect_level: None,
            policy_categories: Vec::new(),
        },
    }
}

fn lifecycle_capability_metadata(
    operations: Vec<&str>,
    side_effect_level: SideEffectLevel,
    policy_category: PolicyCategory,
) -> CapabilityMetadata {
    CapabilityMetadata {
        operations: operations
            .into_iter()
            .map(std::string::ToString::to_string)
            .collect(),
        side_effect_level: Some(side_effect_level.as_str().to_string()),
        policy_categories: vec![policy_category.as_str().to_string()],
    }
}

fn standard_agent_provider_families() -> &'static [&'static str] {
    &[
        "model",
        "tool",
        "policy",
        "context",
        "memory",
        "knowledge",
        "planning",
        "host",
        "protocol_adapter",
        "mcp",
        "skill",
        "collaboration",
        "telemetry",
        "agent_installer",
        "agent_configuration",
        "task_scheduling",
        "agent_classification",
        "message_query",
    ]
}

fn is_valid_capability_id(capability_id: &str) -> bool {
    capability_id.contains('.')
        && capability_id.chars().all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-')
        })
}
