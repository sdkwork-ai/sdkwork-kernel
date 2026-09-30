use crate::backend::SdkBackendKind;
use crate::binding::CapabilityExecutionScope;
use crate::runtime::SdkRuntimeOperationKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedCapability {
    pub capability_id: String,
    pub backend_kind: SdkBackendKind,
    pub driver_id: String,
    pub runtime_operations: Vec<SdkRuntimeOperationKind>,
    /// Declared execution scope carried from the capability binding so
    /// dispatch can enforce it (provider-local capabilities are not routable
    /// through transport workers beyond health probes).
    #[serde(default)]
    pub execution_scope: CapabilityExecutionScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkCapabilityNegotiation {
    pub agent_id: String,
    pub binding_id: String,
    pub binding_version: String,
    pub selected: Vec<NegotiatedCapability>,
    pub missing_required: Vec<String>,
    pub degraded_optional: Vec<String>,
}

impl SdkCapabilityNegotiation {
    pub fn is_fully_satisfied(&self) -> bool {
        self.missing_required.is_empty() && self.degraded_optional.is_empty()
    }

    pub fn selected_driver(&self, capability_id: &str) -> Option<&NegotiatedCapability> {
        self.selected
            .iter()
            .find(|entry| entry.capability_id == capability_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkNegotiationError {
    pub code: String,
    pub message: String,
    pub agent_id: Option<String>,
    pub missing_capabilities: Vec<String>,
}

impl SdkNegotiationError {
    pub fn binding_not_found(binding_id: &str) -> Self {
        Self {
            code: "binding_not_found".to_string(),
            message: format!("agent sdk binding not found: {binding_id}"),
            agent_id: None,
            missing_capabilities: Vec::new(),
        }
    }

    pub fn missing_required_capabilities(agent_id: String, missing: Vec<String>) -> Self {
        Self {
            code: "missing_required_capabilities".to_string(),
            message: format!(
                "agent {agent_id} is missing required sdk capabilities: {}",
                missing.join(", ")
            ),
            agent_id: Some(agent_id),
            missing_capabilities: missing,
        }
    }
}

impl std::fmt::Display for SdkNegotiationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SdkNegotiationError {}
