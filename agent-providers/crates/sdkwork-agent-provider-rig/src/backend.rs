use sdkwork_agent_kernel::{
    AgentConfigValue, AgentConfiguration, KernelError, KernelResult, ModelRequest, ModelResponse,
    PolicyCategory,
};

use crate::ids;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

pub trait RigBackendExecutor: Send + Sync {
    fn invoke_model(&self, request: ModelRequest) -> KernelResult<ModelResponse>;

    /// Abort the in-flight call for `model_request_id` when one exists and
    /// report whether it was found. Executors that cannot interrupt calls
    /// report `false` so the backend can fall back to idempotent-ack
    /// semantics.
    fn cancel_model(&self, model_request_id: &str) -> KernelResult<bool> {
        let _ = model_request_id;
        Ok(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RigBackendMode {
    FailClosed,
    Live,
}

impl RigBackendMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FailClosed => "fail_closed",
            Self::Live => "live",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RigBackendExecutionState {
    FailClosed,
    LivePending,
    Live,
}

impl RigBackendExecutionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FailClosed => "fail_closed",
            Self::LivePending => "live_pending",
            Self::Live => "live",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigBackendExecutionStatus {
    pub mode: RigBackendMode,
    pub state: RigBackendExecutionState,
    pub fail_closed: bool,
    pub safe_reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RigBackendBootstrapState {
    FailClosed,
    LivePending,
    Live,
}

impl RigBackendBootstrapState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FailClosed => "fail_closed",
            Self::LivePending => "live_pending",
            Self::Live => "live",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigBackendBootstrapPlan {
    pub backend_mode: RigBackendMode,
    pub state: RigBackendBootstrapState,
    pub provider_id: Option<String>,
    pub required_secret_refs: Vec<String>,
    pub secret_refs: Vec<(String, String)>,
    pub policy_categories: Vec<String>,
    pub fail_closed: bool,
    pub safe_summary: String,
}

impl RigBackendBootstrapPlan {
    pub fn execution_status(&self) -> RigBackendExecutionStatus {
        execution_status_for_mode(self.backend_mode, false)
    }

    pub fn secret_ref_value(&self, field_key: &str) -> Option<&str> {
        self.secret_refs
            .iter()
            .find(|(key, _)| key == field_key)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Clone)]
pub struct RigBackend {
    pub mode: RigBackendMode,
    config: RigBackendConfig,
    executor: Option<Arc<dyn RigBackendExecutor>>,
}

impl Debug for RigBackend {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RigBackend")
            .field("mode", &self.mode)
            .field("config", &self.config)
            .field("executor_attached", &self.executor.is_some())
            .finish()
    }
}

impl PartialEq for RigBackend {
    fn eq(&self, other: &Self) -> bool {
        self.mode == other.mode
            && self.config == other.config
            && self.executor.is_some() == other.executor.is_some()
    }
}

impl Eq for RigBackend {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigBackendConfig {
    pub mode: RigBackendMode,
    pub provider_id: Option<String>,
    pub api_key_secret_ref: Option<String>,
    /// Custom OpenAI-compatible endpoint for direct (rig-core) provider
    /// calls; `None` uses the vendor's default endpoint.
    pub base_url: Option<String>,
}

impl RigBackendConfig {
    pub fn fail_closed() -> Self {
        Self {
            mode: RigBackendMode::FailClosed,
            provider_id: None,
            api_key_secret_ref: None,
            base_url: None,
        }
    }

    pub fn from_configuration(configuration: &AgentConfiguration) -> KernelResult<Self> {
        let mode = match configuration.value("runtime.rig.backend_mode") {
            Some(AgentConfigValue::String(value)) if value == "fail_closed" => {
                RigBackendMode::FailClosed
            }
            Some(AgentConfigValue::String(value)) if value == "live" => RigBackendMode::Live,
            Some(AgentConfigValue::String(value)) => {
                return Err(KernelError::validation(format!(
                    "unsupported Rig backend mode: {value}"
                )));
            }
            Some(_) => {
                return Err(KernelError::validation(
                    "runtime.rig.backend_mode must be a string",
                ));
            }
            None => RigBackendMode::FailClosed,
        };

        let provider_id = match configuration.value("llm.rig.provider_id") {
            Some(AgentConfigValue::String(value)) if !value.trim().is_empty() => {
                Some(value.clone())
            }
            Some(AgentConfigValue::String(_)) | None => None,
            Some(_) => {
                return Err(KernelError::validation(
                    "llm.rig.provider_id must be a string",
                ));
            }
        };

        let api_key_secret_ref = match configuration.value("llm.rig.api_key") {
            Some(AgentConfigValue::SecretRef(value)) => {
                let trimmed = value.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            Some(_) => {
                return Err(KernelError::validation(
                    "llm.rig.api_key must be a secret reference",
                ));
            }
            None => None,
        };

        let base_url = match configuration.value("llm.rig.base_url") {
            Some(AgentConfigValue::String(value)) if !value.trim().is_empty() => {
                Some(value.trim().to_string())
            }
            Some(AgentConfigValue::String(_)) | None => None,
            Some(_) => {
                return Err(KernelError::validation(
                    "llm.rig.base_url must be a string",
                ));
            }
        };

        // Live mode does not require `llm.rig.api_key`: the default cloud
        // router executor authenticates with the caller's dual tokens, and
        // each executor validates its own credential requirements at
        // construction time (e.g. the rig-core adapter requires an API key).
        Ok(Self {
            mode,
            provider_id,
            api_key_secret_ref,
            base_url,
        })
    }

    pub fn execution_status(&self) -> RigBackendExecutionStatus {
        execution_status_for_mode(self.mode, false)
    }

    pub fn bootstrap_plan(&self) -> RigBackendBootstrapPlan {
        let execution_status = self.execution_status();
        let mut required_secret_refs = Vec::new();
        let mut secret_refs = Vec::new();
        let mut policy_categories = vec![PolicyCategory::ModelInvoke.as_str().to_string()];

        if self.mode == RigBackendMode::Live {
            if let Some(api_key_secret_ref) = self.api_key_secret_ref.clone() {
                required_secret_refs.push("llm.rig.api_key".to_string());
                policy_categories.push(PolicyCategory::HostSecretsRead.as_str().to_string());
                secret_refs.push(("llm.rig.api_key".to_string(), api_key_secret_ref));
            }
        }

        RigBackendBootstrapPlan {
            backend_mode: self.mode,
            state: bootstrap_state_for_mode(self.mode),
            provider_id: self.provider_id.clone(),
            required_secret_refs,
            secret_refs,
            policy_categories,
            fail_closed: execution_status.fail_closed,
            safe_summary: safe_bootstrap_summary(self.mode),
        }
    }
}

impl RigBackend {
    pub fn fail_closed() -> Self {
        let config = RigBackendConfig::fail_closed();
        Self {
            mode: config.mode,
            config,
            executor: None,
        }
    }

    pub fn from_config(config: RigBackendConfig) -> Self {
        Self {
            mode: config.mode,
            config,
            executor: None,
        }
    }

    pub fn with_executor(config: RigBackendConfig, executor: Arc<dyn RigBackendExecutor>) -> Self {
        Self {
            mode: config.mode,
            config,
            executor: Some(executor),
        }
    }

    pub fn execution_status(&self) -> RigBackendExecutionStatus {
        execution_status_for_mode(self.mode, self.executor.is_some())
    }

    pub fn bootstrap_plan(&self) -> RigBackendBootstrapPlan {
        let mut plan = self.config.bootstrap_plan();
        if self.mode == RigBackendMode::Live && self.executor.is_some() {
            plan.state = RigBackendBootstrapState::Live;
            plan.fail_closed = false;
            plan.safe_summary = "Rig official execution adapter is connected".to_string();
        }
        plan
    }

    pub fn invoke_model(&self, _request: ModelRequest) -> KernelResult<ModelResponse> {
        match (&self.mode, &self.executor) {
            (RigBackendMode::Live, Some(executor)) => executor.invoke_model(_request),
            _ => Err(KernelError::ProviderUnavailable {
                provider_id: ids::MODEL_PROVIDER_ID.to_string(),
            }),
        }
    }

    /// Cancel an in-flight rig model call.
    ///
    /// With a live executor the abort reaches the spawned provider task: the
    /// blocked invoke observes a disconnected channel and returns a
    /// cancelled response, and the in-flight HTTP round trip is dropped. A
    /// cancelled-again or already-finished request id is an idempotent
    /// cancelled ack (the caller-visible outcome is terminal either way).
    pub fn cancel_model(&self, model_request_id: &str) -> KernelResult<ModelResponse> {
        let aborted = match &self.executor {
            Some(executor) => executor.cancel_model(model_request_id)?,
            None => false,
        };
        let _ = aborted;
        Ok(ModelResponse::cancelled(
            model_request_id.to_string(),
            ids::MODEL_PROVIDER_ID.to_string(),
        ))
    }
}

fn execution_status_for_mode(
    mode: RigBackendMode,
    executor_attached: bool,
) -> RigBackendExecutionStatus {
    match (mode, executor_attached) {
        (RigBackendMode::Live, true) => RigBackendExecutionStatus {
            mode,
            state: RigBackendExecutionState::Live,
            fail_closed: false,
            safe_reason: "Rig official execution adapter is connected".to_string(),
        },
        (RigBackendMode::FailClosed, _) => RigBackendExecutionStatus {
            mode,
            state: RigBackendExecutionState::FailClosed,
            fail_closed: true,
            safe_reason: "Rig backend is fail-closed; no live model execution adapter is connected"
                .to_string(),
        },
        (RigBackendMode::Live, false) => RigBackendExecutionStatus {
            mode,
            state: RigBackendExecutionState::LivePending,
            fail_closed: true,
            safe_reason: "Rig backend live mode is configured but the upstream execution adapter is not connected"
                .to_string(),
        },
    }
}

fn bootstrap_state_for_mode(mode: RigBackendMode) -> RigBackendBootstrapState {
    match mode {
        RigBackendMode::FailClosed => RigBackendBootstrapState::FailClosed,
        RigBackendMode::Live => RigBackendBootstrapState::LivePending,
    }
}

fn safe_bootstrap_summary(mode: RigBackendMode) -> String {
    match mode {
        RigBackendMode::FailClosed => {
            "Rig backend bootstrap plan stays fail-closed without live adapter configuration"
                .to_string()
        }
        RigBackendMode::Live => {
            "Rig backend bootstrap plan is live-pending and remains fail-closed until the upstream adapter is connected"
                .to_string()
        }
    }
}
