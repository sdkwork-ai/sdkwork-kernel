use crate::backend::SdkBackendKind;
use crate::runtime::SdkRuntimeOperationKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestStatus {
    Experimental,
    Standardizing,
    Stable,
    Deprecated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustPackageRef {
    #[serde(rename = "crate")]
    pub crate_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NpmPackageRef {
    pub package: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonPackageRef {
    pub module: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguagePackages {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust: Option<RustPackageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typescript: Option<NpmPackageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<PythonPackageRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendCandidate {
    pub kind: SdkBackendKind,
    pub driver_id: String,
    #[serde(default)]
    pub runtime_operations: Vec<SdkRuntimeOperationKind>,
    #[serde(default, rename = "crate", skip_serializing_if = "Option::is_none")]
    pub rust_crate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_module: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openapi_authority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityBinding {
    pub capability_id: String,
    pub required: bool,
    pub execution_scope: CapabilityExecutionScope,
    pub backends: Vec<BackendCandidate>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityExecutionScope {
    #[default]
    TransportRuntime,
    ProviderLocal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_backend_priority: Option<Vec<SdkBackendKind>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationSource {
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, rename = "crate", skip_serializing_if = "Option::is_none")]
    pub rust_crate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSdkBindingManifest {
    pub schema_version: String,
    pub manifest_type: String,
    pub binding_id: String,
    pub agent_id: String,
    pub display_name: String,
    pub description: String,
    pub version: String,
    pub sdk_owner: String,
    pub capabilities: Vec<CapabilityBinding>,
    pub status: ManifestStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_compatibility: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_policy: Option<SelectionPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_packages: Option<LanguagePackages>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration_sources: Option<Vec<IntegrationSource>>,
}

impl AgentSdkBindingManifest {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn from_json_file(path: &str) -> Result<Self, std::io::Error> {
        let json = std::fs::read_to_string(path)?;
        Self::from_json(&json).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })
    }

    pub fn capability_binding(&self, capability_id: &str) -> Option<&CapabilityBinding> {
        self.capabilities
            .iter()
            .find(|entry| entry.capability_id == capability_id)
    }

    pub fn backend_priority(&self) -> Vec<SdkBackendKind> {
        self.selection_policy
            .as_ref()
            .and_then(|policy| policy.default_backend_priority.clone())
            .unwrap_or_else(|| crate::backend::default_backend_priority().to_vec())
    }
}
