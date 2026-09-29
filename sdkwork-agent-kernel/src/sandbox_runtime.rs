//! Adapter from Agents-owned identities to the SDKWork Sandbox lifecycle port.

use std::collections::BTreeSet;
use std::sync::Arc;

use sdkwork_intelligence_sandbox_service::{
    CreateSandboxSessionCommand, SandboxLifecycleError, SandboxSession,
    SandboxSessionLifecycleCommand, SandboxSessionLifecyclePort, SandboxSessionRepositoryError,
    SandboxSessionState,
};
use sdkwork_sandbox_provider_spi::{
    IsolationAssurance, OperationId, RuntimeCapability, SandboxSessionId, SandboxWorkspaceId,
    TenantId,
};

use crate::{KernelError, KernelErrorSource, KernelResult};

/// Kernel-owned input for creating a Sandbox runtime projection of an Agent Session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SandboxSessionCreateRequest {
    pub tenant_id: String,
    pub agent_workspace_id: String,
    pub agent_session_id: String,
    pub sandbox_operation_id: String,
    pub sandbox_required_capabilities: BTreeSet<RuntimeCapability>,
    pub sandbox_minimum_assurance: IsolationAssurance,
}

/// Kernel-owned input for an existing Sandbox Session lifecycle command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SandboxSessionCommandRequest {
    pub tenant_id: String,
    pub agent_session_id: String,
    pub sandbox_operation_id: String,
}

/// Safe runtime projection returned to Kernel and Agents integration code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SandboxSessionRuntimeProjection {
    sandbox_workspace_id: String,
    sandbox_session_id: String,
    sandbox_session_state: SandboxSessionState,
    sandbox_id: Option<String>,
    sandbox_runtime_binding_id: Option<String>,
    sandbox_provider_id: Option<String>,
    agent_runtime_location_id: Option<String>,
}

impl SandboxSessionRuntimeProjection {
    /// Construct a projection directly (tooling/tests). Runtime paths
    /// produce projections via `From<SandboxSession>`.
    pub fn new(
        sandbox_workspace_id: impl Into<String>,
        sandbox_session_id: impl Into<String>,
        sandbox_session_state: SandboxSessionState,
        sandbox_id: Option<impl Into<String>>,
        sandbox_runtime_binding_id: Option<impl Into<String>>,
        sandbox_provider_id: Option<impl Into<String>>,
        agent_runtime_location_id: Option<impl Into<String>>,
    ) -> Self {
        Self {
            sandbox_workspace_id: sandbox_workspace_id.into(),
            sandbox_session_id: sandbox_session_id.into(),
            sandbox_session_state,
            sandbox_id: sandbox_id.map(Into::into),
            sandbox_runtime_binding_id: sandbox_runtime_binding_id.map(Into::into),
            sandbox_provider_id: sandbox_provider_id.map(Into::into),
            agent_runtime_location_id: agent_runtime_location_id.map(Into::into),
        }
    }

    pub fn sandbox_workspace_id(&self) -> &str {
        self.sandbox_workspace_id.as_str()
    }

    pub fn sandbox_session_id(&self) -> &str {
        self.sandbox_session_id.as_str()
    }

    pub fn sandbox_session_state(&self) -> SandboxSessionState {
        self.sandbox_session_state
    }

    pub fn sandbox_id(&self) -> Option<&str> {
        self.sandbox_id.as_deref()
    }

    pub fn sandbox_runtime_binding_id(&self) -> Option<&str> {
        self.sandbox_runtime_binding_id.as_deref()
    }

    pub fn sandbox_provider_id(&self) -> Option<&str> {
        self.sandbox_provider_id.as_deref()
    }

    /// Agents persists this opaque value as `runtimeLocationId`.
    pub fn agent_runtime_location_id(&self) -> Option<&str> {
        self.agent_runtime_location_id.as_deref()
    }
}

impl From<SandboxSession> for SandboxSessionRuntimeProjection {
    fn from(sandbox_session: SandboxSession) -> Self {
        let sandbox_runtime_binding = sandbox_session.sandbox_runtime_binding();
        let sandbox_runtime_binding_id = sandbox_runtime_binding.map(|sandbox_runtime_binding| {
            sandbox_runtime_binding
                .sandbox_runtime_binding_id()
                .as_str()
                .to_string()
        });

        Self {
            sandbox_workspace_id: sandbox_session.sandbox_workspace_id().as_str().to_string(),
            sandbox_session_id: sandbox_session.sandbox_session_id().as_str().to_string(),
            sandbox_session_state: sandbox_session.sandbox_session_state(),
            sandbox_id: sandbox_runtime_binding.map(|sandbox_runtime_binding| {
                sandbox_runtime_binding.sandbox_id().as_str().to_string()
            }),
            sandbox_provider_id: sandbox_runtime_binding.map(|sandbox_runtime_binding| {
                sandbox_runtime_binding
                    .sandbox_provider_id()
                    .as_str()
                    .to_string()
            }),
            agent_runtime_location_id: sandbox_runtime_binding_id.clone(),
            sandbox_runtime_binding_id,
        }
    }
}

/// Namespaced Kernel adapter that consumes the Sandbox-owned lifecycle port.
pub struct SandboxSessionLifecycleAdapter {
    sandbox_session_lifecycle_port: Arc<dyn SandboxSessionLifecyclePort>,
}

impl SandboxSessionLifecycleAdapter {
    pub fn new(sandbox_session_lifecycle_port: Arc<dyn SandboxSessionLifecyclePort>) -> Self {
        Self {
            sandbox_session_lifecycle_port,
        }
    }

    pub async fn create_sandbox_session(
        &self,
        sandbox_request: SandboxSessionCreateRequest,
    ) -> KernelResult<SandboxSessionRuntimeProjection> {
        let sandbox_command = CreateSandboxSessionCommand {
            tenant_id: parse_tenant_id(sandbox_request.tenant_id)?,
            sandbox_workspace_id: parse_sandbox_workspace_id(sandbox_request.agent_workspace_id)?,
            sandbox_session_id: parse_sandbox_session_id(sandbox_request.agent_session_id)?,
            sandbox_operation_id: parse_sandbox_operation_id(sandbox_request.sandbox_operation_id)?,
            sandbox_required_capabilities: sandbox_request.sandbox_required_capabilities,
            sandbox_minimum_assurance: sandbox_request.sandbox_minimum_assurance,
        };

        self.sandbox_session_lifecycle_port
            .create_sandbox_session(sandbox_command)
            .await
            .map(SandboxSessionRuntimeProjection::from)
            .map_err(map_sandbox_lifecycle_error)
    }

    pub async fn get_sandbox_session(
        &self,
        tenant_id: impl Into<String>,
        agent_session_id: impl Into<String>,
    ) -> KernelResult<SandboxSessionRuntimeProjection> {
        let tenant_id = parse_tenant_id(tenant_id.into())?;
        let sandbox_session_id = parse_sandbox_session_id(agent_session_id.into())?;
        self.sandbox_session_lifecycle_port
            .get_sandbox_session(&tenant_id, &sandbox_session_id)
            .await
            .map(SandboxSessionRuntimeProjection::from)
            .map_err(map_sandbox_lifecycle_error)
    }

    pub async fn start_sandbox_session(
        &self,
        sandbox_request: SandboxSessionCommandRequest,
    ) -> KernelResult<SandboxSessionRuntimeProjection> {
        self.execute_sandbox_session_command(sandbox_request, SandboxLifecycleAction::Start)
            .await
    }

    pub async fn stop_sandbox_session(
        &self,
        sandbox_request: SandboxSessionCommandRequest,
    ) -> KernelResult<SandboxSessionRuntimeProjection> {
        self.execute_sandbox_session_command(sandbox_request, SandboxLifecycleAction::Stop)
            .await
    }

    pub async fn destroy_sandbox_session(
        &self,
        sandbox_request: SandboxSessionCommandRequest,
    ) -> KernelResult<SandboxSessionRuntimeProjection> {
        self.execute_sandbox_session_command(sandbox_request, SandboxLifecycleAction::Destroy)
            .await
    }

    async fn execute_sandbox_session_command(
        &self,
        sandbox_request: SandboxSessionCommandRequest,
        sandbox_lifecycle_action: SandboxLifecycleAction,
    ) -> KernelResult<SandboxSessionRuntimeProjection> {
        let sandbox_command = SandboxSessionLifecycleCommand {
            tenant_id: parse_tenant_id(sandbox_request.tenant_id)?,
            sandbox_session_id: parse_sandbox_session_id(sandbox_request.agent_session_id)?,
            sandbox_operation_id: parse_sandbox_operation_id(sandbox_request.sandbox_operation_id)?,
        };

        let sandbox_result = match sandbox_lifecycle_action {
            SandboxLifecycleAction::Start => {
                self.sandbox_session_lifecycle_port
                    .start_sandbox_session(sandbox_command)
                    .await
            }
            SandboxLifecycleAction::Stop => {
                self.sandbox_session_lifecycle_port
                    .stop_sandbox_session(sandbox_command)
                    .await
            }
            SandboxLifecycleAction::Destroy => {
                self.sandbox_session_lifecycle_port
                    .destroy_sandbox_session(sandbox_command)
                    .await
            }
        };

        sandbox_result
            .map(SandboxSessionRuntimeProjection::from)
            .map_err(map_sandbox_lifecycle_error)
    }
}

/// Kernel-side sandbox session lifecycle surface consumed by the
/// execution coordinator. Implemented by [`SandboxSessionLifecycleAdapter`]
/// over the Sandbox-owned port; kept as a kernel trait so execution
/// paths and contract tests can substitute a recording double.
#[async_trait::async_trait]
pub trait SandboxedSessionPort: Send + Sync {
    async fn get_sandbox_session(
        &self,
        tenant_id: String,
        agent_session_id: String,
    ) -> KernelResult<SandboxSessionRuntimeProjection>;

    async fn start_sandbox_session(
        &self,
        request: SandboxSessionCommandRequest,
    ) -> KernelResult<SandboxSessionRuntimeProjection>;

    async fn stop_sandbox_session(
        &self,
        request: SandboxSessionCommandRequest,
    ) -> KernelResult<SandboxSessionRuntimeProjection>;
}

#[async_trait::async_trait]
impl SandboxedSessionPort for SandboxSessionLifecycleAdapter {
    async fn get_sandbox_session(
        &self,
        tenant_id: String,
        agent_session_id: String,
    ) -> KernelResult<SandboxSessionRuntimeProjection> {
        self.get_sandbox_session(tenant_id, agent_session_id).await
    }

    async fn start_sandbox_session(
        &self,
        request: SandboxSessionCommandRequest,
    ) -> KernelResult<SandboxSessionRuntimeProjection> {
        self.start_sandbox_session(request).await
    }

    async fn stop_sandbox_session(
        &self,
        request: SandboxSessionCommandRequest,
    ) -> KernelResult<SandboxSessionRuntimeProjection> {
        self.stop_sandbox_session(request).await
    }
}

/// One step of a sandboxed execution lifecycle, recorded in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxedLifecycleStep {
    /// The bound session was found before execution.
    SessionFound,
    /// The coordinator issued a start; the session was not running.
    SessionStarted,
    /// The session was already running; no start was issued.
    SessionAlreadyRunning,
    /// The coordinator issued a stop after execution.
    SessionStopped,
    /// The session was already stopped; no stop was issued.
    SessionAlreadyStopped,
    /// The bound session does not exist; execution was refused.
    SessionMissing,
}

/// Outcome of a sandboxed execution: the action result plus the lifecycle
/// trace observed around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxedExecutionResult<T> {
    pub result: T,
    pub lifecycle: Vec<SandboxedLifecycleStep>,
}

/// Coordinates execution inside a bound sandbox session lifecycle:
/// get -> (start when `auto_start`) -> run action -> (stop when
/// `auto_stop`). Fail-closed: a missing session refuses execution before
/// the action runs, and lifecycle failures propagate as kernel errors.
#[derive(Clone)]
pub struct SandboxedExecutionCoordinator {
    sandboxed_session_port: Arc<dyn SandboxedSessionPort>,
}

impl SandboxedExecutionCoordinator {
    pub fn new(sandboxed_session_port: Arc<dyn SandboxedSessionPort>) -> Self {
        Self {
            sandboxed_session_port,
        }
    }

    pub async fn run_sandboxed<T, F, Fut>(
        &self,
        tenant_id: String,
        binding: &crate::SandboxExecutionBinding,
        operation_id: String,
        action: F,
    ) -> KernelResult<SandboxedExecutionResult<T>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = KernelResult<T>>,
    {
        binding.validate()?;

        let mut lifecycle = Vec::new();

        // Fail-closed lookup: the bound session must exist before any
        // execution work is allowed to proceed.
        let projection = self
            .sandboxed_session_port
            .get_sandbox_session(tenant_id.clone(), binding.sandbox_session_id.clone())
            .await
            .map_err(|error| match error.kind() {
                crate::KernelErrorKind::ValidationError => {
                    lifecycle.push(SandboxedLifecycleStep::SessionMissing);
                    error
                }
                _ => error,
            })?;
        lifecycle.push(SandboxedLifecycleStep::SessionFound);

        // Ensure the session is running: explicit start when requested,
        // otherwise refuse when the session is not already running.
        if projection.sandbox_session_state() != SandboxSessionState::Running {
            if !binding.auto_start {
                return Err(crate::KernelError::validation(format!(
                    "sandbox session '{}' is not running and auto_start is disabled",
                    binding.sandbox_session_id
                )));
            }
            self.sandboxed_session_port
                .start_sandbox_session(SandboxSessionCommandRequest {
                    tenant_id: tenant_id.clone(),
                    agent_session_id: binding.sandbox_session_id.clone(),
                    sandbox_operation_id: operation_id.clone(),
                })
                .await?;
            lifecycle.push(SandboxedLifecycleStep::SessionStarted);
        } else {
            lifecycle.push(SandboxedLifecycleStep::SessionAlreadyRunning);
        }

        // Run the action, then always attempt the stop cleanup when
        // requested, even if the action failed.
        let action_result = action().await;
        let stop_result = if binding.auto_stop {
            Some(
                self.sandboxed_session_port
                    .stop_sandbox_session(SandboxSessionCommandRequest {
                        tenant_id: tenant_id.clone(),
                        agent_session_id: binding.sandbox_session_id.clone(),
                        sandbox_operation_id: operation_id,
                    })
                    .await,
            )
        } else {
            None
        };

        if let Some(stop_result) = stop_result {
            match stop_result {
                Ok(_) => lifecycle.push(SandboxedLifecycleStep::SessionStopped),
                // The action already failed: keep its error as the primary
                // result and record the cleanup as already-stopped.
                Err(_) if action_result.is_err() => {
                    lifecycle.push(SandboxedLifecycleStep::SessionAlreadyStopped)
                }
                // Fail-closed: a failed stop on a successful action
                // surfaces the cleanup failure instead of hiding it.
                Err(error) => return Err(error),
            }
        }

        Ok(SandboxedExecutionResult {
            result: action_result?,
            lifecycle,
        })
    }
}

#[derive(Clone, Copy)]
enum SandboxLifecycleAction {
    Start,
    Stop,
    Destroy,
}

fn parse_tenant_id(tenant_id: String) -> KernelResult<TenantId> {
    TenantId::parse(tenant_id).map_err(map_sandbox_identifier_error)
}

fn parse_sandbox_workspace_id(agent_workspace_id: String) -> KernelResult<SandboxWorkspaceId> {
    SandboxWorkspaceId::parse(agent_workspace_id).map_err(map_sandbox_identifier_error)
}

fn parse_sandbox_session_id(agent_session_id: String) -> KernelResult<SandboxSessionId> {
    SandboxSessionId::parse(agent_session_id).map_err(map_sandbox_identifier_error)
}

fn parse_sandbox_operation_id(sandbox_operation_id: String) -> KernelResult<OperationId> {
    OperationId::parse(sandbox_operation_id).map_err(map_sandbox_identifier_error)
}

fn map_sandbox_identifier_error(
    sandbox_identifier_error: sdkwork_sandbox_provider_spi::SandboxIdentifierError,
) -> KernelError {
    KernelError::validation(format!(
        "sandbox identity mapping failed: {sandbox_identifier_error}"
    ))
}

fn map_sandbox_lifecycle_error(sandbox_lifecycle_error: SandboxLifecycleError) -> KernelError {
    match sandbox_lifecycle_error {
        SandboxLifecycleError::SandboxSessionNotFound { .. }
        | SandboxLifecycleError::Repository(SandboxSessionRepositoryError::NotFound) => {
            KernelError::validation("sandbox session was not found")
        }
        SandboxLifecycleError::InvalidTransition { .. }
        | SandboxLifecycleError::IdempotencyConflict { .. }
        | SandboxLifecycleError::OperationInProgress { .. }
        | SandboxLifecycleError::OperationPreviouslyFailed { .. }
        | SandboxLifecycleError::SandboxSessionIdConflict { .. }
        | SandboxLifecycleError::Repository(SandboxSessionRepositoryError::VersionConflict)
        | SandboxLifecycleError::Repository(SandboxSessionRepositoryError::DuplicateOperation)
        | SandboxLifecycleError::Repository(SandboxSessionRepositoryError::DuplicateSandboxSession)
        | SandboxLifecycleError::Repository(SandboxSessionRepositoryError::RuntimeBindingConflict) => {
            KernelError::conflict(sandbox_lifecycle_error.to_string())
                .from_source(KernelErrorSource::Runtime)
        }
        SandboxLifecycleError::NoEligibleProvider => KernelError::CapabilityMissing {
            capability_id: "sandbox.runtime".to_string(),
        },
        SandboxLifecycleError::NoHealthyProvider => KernelError::ProviderUnavailable {
            provider_id: "sandbox".to_string(),
        },
        SandboxLifecycleError::ProviderReadinessRejected {
            sandbox_provider_id,
        } => KernelError::ProviderUnavailable {
            provider_id: sandbox_provider_id.as_str().to_string(),
        },
        SandboxLifecycleError::Provider(sandbox_provider_error) => KernelError::provider_error(
            "sandbox_provider_error",
            sandbox_provider_error.to_string(),
        ),
        SandboxLifecycleError::LeaseUnavailable
        | SandboxLifecycleError::Repository(SandboxSessionRepositoryError::LeaseConflict) => {
            KernelError::conflict(sandbox_lifecycle_error.to_string())
                .from_source(KernelErrorSource::Runtime)
                .with_detail("sandbox_lifecycle_error", "lease_unavailable")
                .with_retryable(true)
        }
        SandboxLifecycleError::LeaseLost => {
            KernelError::conflict(sandbox_lifecycle_error.to_string())
                .from_source(KernelErrorSource::Runtime)
                .with_detail("sandbox_lifecycle_error", "lease_lost")
                .with_retryable(true)
        }
        SandboxLifecycleError::Repository(SandboxSessionRepositoryError::InvalidPageRequest) => {
            KernelError::validation("sandbox page request is invalid")
                .from_source(KernelErrorSource::Runtime)
        }
        SandboxLifecycleError::Repository(SandboxSessionRepositoryError::Unavailable) => {
            KernelError::Internal {
                message: "sandbox lifecycle service is internally unavailable".to_string(),
            }
            .from_source(KernelErrorSource::Runtime)
            .with_retryable(true)
        }
        SandboxLifecycleError::DuplicateProvider { .. }
        | SandboxLifecycleError::InvariantViolation(_)
        | SandboxLifecycleError::Repository(
            SandboxSessionRepositoryError::InvalidStoredData
            | SandboxSessionRepositoryError::ProtectionFailed
            | SandboxSessionRepositoryError::UnsupportedDatabaseEngine,
        ) => KernelError::Internal {
            message: "sandbox lifecycle service is internally unavailable".to_string(),
        }
        .from_source(KernelErrorSource::Runtime),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use sdkwork_intelligence_sandbox_service::{
        SandboxLifecycleResult, SandboxSessionLifecyclePort,
    };

    use super::*;

    #[derive(Default)]
    struct CapturingSandboxSessionLifecyclePort {
        sandbox_create_command: Mutex<Option<CreateSandboxSessionCommand>>,
    }

    #[async_trait]
    impl SandboxSessionLifecyclePort for CapturingSandboxSessionLifecyclePort {
        async fn create_sandbox_session(
            &self,
            sandbox_command: CreateSandboxSessionCommand,
        ) -> SandboxLifecycleResult<SandboxSession> {
            let mut sandbox_create_command = self
                .sandbox_create_command
                .lock()
                .unwrap_or_else(|poisoned_state| poisoned_state.into_inner());
            *sandbox_create_command = Some(sandbox_command);
            Err(SandboxLifecycleError::NoHealthyProvider)
        }

        async fn get_sandbox_session(
            &self,
            _tenant_id: &TenantId,
            _sandbox_session_id: &SandboxSessionId,
        ) -> SandboxLifecycleResult<SandboxSession> {
            Err(SandboxLifecycleError::NoHealthyProvider)
        }

        async fn start_sandbox_session(
            &self,
            _sandbox_command: SandboxSessionLifecycleCommand,
        ) -> SandboxLifecycleResult<SandboxSession> {
            Err(SandboxLifecycleError::NoHealthyProvider)
        }

        async fn stop_sandbox_session(
            &self,
            _sandbox_command: SandboxSessionLifecycleCommand,
        ) -> SandboxLifecycleResult<SandboxSession> {
            Err(SandboxLifecycleError::NoHealthyProvider)
        }

        async fn destroy_sandbox_session(
            &self,
            _sandbox_command: SandboxSessionLifecycleCommand,
        ) -> SandboxLifecycleResult<SandboxSession> {
            Err(SandboxLifecycleError::NoHealthyProvider)
        }
    }

    #[tokio::test]
    async fn maps_agents_owned_ids_into_sandbox_qualified_command_fields() {
        let sandbox_port = Arc::new(CapturingSandboxSessionLifecyclePort::default());
        let sandbox_port_for_assertion = Arc::clone(&sandbox_port);
        let sandbox_adapter = SandboxSessionLifecycleAdapter::new(sandbox_port);

        let sandbox_result = sandbox_adapter
            .create_sandbox_session(SandboxSessionCreateRequest {
                tenant_id: "tenant-1".to_string(),
                agent_workspace_id: "agent-workspace-1".to_string(),
                agent_session_id: "agent-session-1".to_string(),
                sandbox_operation_id: "sandbox-operation-1".to_string(),
                sandbox_required_capabilities: BTreeSet::from([RuntimeCapability::Filesystem]),
                sandbox_minimum_assurance: IsolationAssurance::HostUser,
            })
            .await;

        assert!(matches!(
            sandbox_result,
            Err(KernelError::ProviderUnavailable { ref provider_id }) if provider_id == "sandbox"
        ));
        let sandbox_create_command = sandbox_port_for_assertion
            .sandbox_create_command
            .lock()
            .unwrap_or_else(|poisoned_state| poisoned_state.into_inner())
            .clone()
            .unwrap_or_else(|| panic!("sandbox create command must be captured"));
        assert_eq!(
            sandbox_create_command.sandbox_workspace_id.as_str(),
            "agent-workspace-1"
        );
        assert_eq!(
            sandbox_create_command.sandbox_session_id.as_str(),
            "agent-session-1"
        );
        assert_eq!(
            sandbox_create_command.sandbox_operation_id.as_str(),
            "sandbox-operation-1"
        );
    }

    #[test]
    fn rejects_path_like_agents_ids_before_calling_sandbox() {
        let sandbox_mapping_result = parse_sandbox_workspace_id("../workspace".to_string());
        assert!(matches!(
            sandbox_mapping_result,
            Err(KernelError::Validation { .. })
        ));
    }

    #[test]
    fn maps_unavailable_sandbox_lease_to_retryable_runtime_conflict() {
        let sandbox_kernel_error =
            map_sandbox_lifecycle_error(SandboxLifecycleError::LeaseUnavailable);

        assert_eq!(
            sandbox_kernel_error.kind(),
            crate::KernelErrorKind::Conflict
        );
        assert_eq!(sandbox_kernel_error.code(), "conflict");
        assert_eq!(sandbox_kernel_error.source(), KernelErrorSource::Runtime);
        assert!(sandbox_kernel_error.retryable());
        assert_eq!(
            sandbox_kernel_error.detail_value("sandbox_lifecycle_error"),
            Some("lease_unavailable")
        );
    }

    #[test]
    fn maps_lost_sandbox_lease_to_retryable_runtime_conflict() {
        let sandbox_kernel_error = map_sandbox_lifecycle_error(SandboxLifecycleError::LeaseLost);

        assert_eq!(
            sandbox_kernel_error.kind(),
            crate::KernelErrorKind::Conflict
        );
        assert_eq!(sandbox_kernel_error.code(), "conflict");
        assert_eq!(sandbox_kernel_error.source(), KernelErrorSource::Runtime);
        assert!(sandbox_kernel_error.retryable());
        assert_eq!(
            sandbox_kernel_error.detail_value("sandbox_lifecycle_error"),
            Some("lease_lost")
        );
    }

    #[test]
    fn maps_sandbox_repository_lease_conflict_to_retryable_runtime_conflict() {
        let sandbox_kernel_error = map_sandbox_lifecycle_error(SandboxLifecycleError::Repository(
            SandboxSessionRepositoryError::LeaseConflict,
        ));

        assert_eq!(
            sandbox_kernel_error.kind(),
            crate::KernelErrorKind::Conflict
        );
        assert_eq!(sandbox_kernel_error.source(), KernelErrorSource::Runtime);
        assert!(sandbox_kernel_error.retryable());
        assert_eq!(
            sandbox_kernel_error.detail_value("sandbox_lifecycle_error"),
            Some("lease_unavailable")
        );
    }

    #[test]
    fn maps_sandbox_repository_unavailability_to_retryable_internal_runtime_error() {
        let sandbox_kernel_error = map_sandbox_lifecycle_error(SandboxLifecycleError::Repository(
            SandboxSessionRepositoryError::Unavailable,
        ));

        assert_eq!(
            sandbox_kernel_error.kind(),
            crate::KernelErrorKind::InternalError
        );
        assert_eq!(sandbox_kernel_error.source(), KernelErrorSource::Runtime);
        assert!(sandbox_kernel_error.retryable());
        assert!(!sandbox_kernel_error.safe_for_user());
    }

    #[test]
    fn maps_invalid_sandbox_page_request_to_non_retryable_runtime_validation_error() {
        let sandbox_kernel_error = map_sandbox_lifecycle_error(SandboxLifecycleError::Repository(
            SandboxSessionRepositoryError::InvalidPageRequest,
        ));

        assert_eq!(
            sandbox_kernel_error.kind(),
            crate::KernelErrorKind::ValidationError
        );
        assert_eq!(sandbox_kernel_error.code(), "validation_error");
        assert_eq!(sandbox_kernel_error.source(), KernelErrorSource::Runtime);
        assert!(!sandbox_kernel_error.retryable());
        assert!(sandbox_kernel_error.safe_for_user());
        assert_eq!(
            sandbox_kernel_error.safe_message(),
            "sandbox page request is invalid"
        );
        for sandbox_internal_term in [
            "repository",
            "database",
            "storage",
            "crypto",
            "encryption",
            "protection",
        ] {
            assert!(!sandbox_kernel_error
                .safe_message()
                .contains(sandbox_internal_term));
        }
    }

    #[test]
    fn maps_sandbox_repository_integrity_errors_to_internal_runtime_error() {
        for sandbox_repository_error in [
            SandboxSessionRepositoryError::InvalidStoredData,
            SandboxSessionRepositoryError::ProtectionFailed,
            SandboxSessionRepositoryError::UnsupportedDatabaseEngine,
        ] {
            let sandbox_kernel_error = map_sandbox_lifecycle_error(
                SandboxLifecycleError::Repository(sandbox_repository_error),
            );

            assert_eq!(
                sandbox_kernel_error.kind(),
                crate::KernelErrorKind::InternalError
            );
            assert_eq!(sandbox_kernel_error.source(), KernelErrorSource::Runtime);
            assert!(!sandbox_kernel_error.retryable());
            assert!(!sandbox_kernel_error.safe_for_user());
            assert_eq!(sandbox_kernel_error.safe_message(), "internal kernel error");
        }
    }
}
