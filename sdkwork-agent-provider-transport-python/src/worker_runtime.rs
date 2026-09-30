use sdkwork_agent_provider_core::mock_provider_invocation_allowed;
use sdkwork_agent_provider_spi::{
    SdkBackendKind, SdkBackendRuntime, SdkDriverHealth, SdkRuntimeError,
    SdkRuntimeInteractionResolution, SdkRuntimeOperation, SdkRuntimeRequest, SdkRuntimeResponse,
};
use sdkwork_agent_provider_transport_ipc::{
    provider_worker_concurrency_limit, FailClosedJsonRpcTransport, JsonRpcTransport,
    PackageStubJsonRpcTransport, SpawnedWorker, SpawnedWorkerLease, SpawnedWorkerPool,
    TransportError, SDKWORK_CAPABILITY_INVOKE_METHOD, SDKWORK_PING_METHOD,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

const WORKER_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_WORKER_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_WORKER_OPERATION_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_WORKER_OPERATION_TIMEOUT: Duration = Duration::from_secs(3600);
const PYTHON_BINARY_ENV: &str = "SDKWORK_AGENT_PYTHON_BINARY";
const WORKER_SCRIPT_ENV: &str = "SDKWORK_AGENT_PYTHON_WORKER_SCRIPT";
const PROVIDER_HOST_ROOT_ENV: &str = "SDKWORK_AGENT_PROVIDER_HOST_ROOT";
const LEGACY_PROVIDER_RUNTIME_ROOT_ENV: &str = "SDKWORK_AGENT_PROVIDER_RUNTIME_ROOT";
const PROVIDER_HOST_DIR_NAME: &str = "provider-host";
const LEGACY_PROVIDER_RUNTIME_DIR_NAME: &str = "provider-runtime";
const PYTHON_WORKER_RELATIVE_PATH: &str = "workers/generic_python_sdk_worker.py";

#[derive(Debug, Clone)]
pub struct PythonWorkerLaunchOptions {
    pub python_binary: String,
    pub worker_script: PathBuf,
    pub package_name: String,
}

impl PythonWorkerLaunchOptions {
    pub fn for_package(package_name: impl Into<String>) -> Self {
        Self {
            python_binary: default_python_binary(),
            worker_script: default_python_worker_script(),
            package_name: package_name.into(),
        }
    }
}

pub fn default_python_binary() -> String {
    if let Some(configured) = std::env::var_os(PYTHON_BINARY_ENV).filter(|value| !value.is_empty())
    {
        return configured.to_string_lossy().into_owned();
    }
    if cfg!(windows) {
        "python".to_string()
    } else {
        "python3".to_string()
    }
}

pub fn default_python_worker_script() -> PathBuf {
    if let Some(configured) = std::env::var_os(WORKER_SCRIPT_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return configured;
    }

    if let Some(root) = provider_host_root() {
        return root.join(PYTHON_WORKER_RELATIVE_PATH);
    }

    #[cfg(debug_assertions)]
    {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/provider-transport-workers/generic_python_sdk_worker.py")
    }

    #[cfg(not(debug_assertions))]
    {
        PathBuf::from(PROVIDER_HOST_DIR_NAME).join(PYTHON_WORKER_RELATIVE_PATH)
    }
}

fn provider_host_root() -> Option<PathBuf> {
    for environment_key in [PROVIDER_HOST_ROOT_ENV, LEGACY_PROVIDER_RUNTIME_ROOT_ENV] {
        if let Some(configured) = std::env::var_os(environment_key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
        {
            return Some(configured);
        }
    }

    let executable = std::env::current_exe().ok()?;
    find_packaged_provider_host_root(executable.parent()?, PYTHON_WORKER_RELATIVE_PATH)
}

fn find_packaged_provider_host_root(
    start_directory: &Path,
    worker_relative_path: &str,
) -> Option<PathBuf> {
    for directory_name in [PROVIDER_HOST_DIR_NAME, LEGACY_PROVIDER_RUNTIME_DIR_NAME] {
        let mut ancestors = Some(start_directory);
        while let Some(directory) = ancestors {
            let candidates = [
                directory.join(directory_name),
                directory.join("resources").join(directory_name),
                directory.join("Resources").join(directory_name),
                directory
                    .join("share")
                    .join("sdkwork-birdcoder")
                    .join(directory_name),
            ];
            if let Some(candidate) = candidates
                .into_iter()
                .find(|path| path.join(worker_relative_path).is_file())
            {
                return Some(candidate);
            }
            ancestors = directory.parent();
        }
    }

    None
}

enum PythonRuntimeBackend {
    Stub(Arc<dyn JsonRpcTransport + Send + Sync>),
    FailClosed(Arc<dyn JsonRpcTransport + Send + Sync>),
    Managed { pool: Arc<SpawnedWorkerPool> },
}

pub struct PythonSdkBackendRuntime {
    package_name: String,
    backend: PythonRuntimeBackend,
}

impl PythonSdkBackendRuntime {
    pub fn bootstrap(package_name: impl Into<String>) -> Self {
        let options = PythonWorkerLaunchOptions::for_package(package_name);
        match Self::spawn(&options) {
            Ok(runtime) => runtime,
            Err(error) => {
                if mock_provider_invocation_allowed() {
                    Self::in_memory_stub(options.package_name, true)
                } else {
                    Self::fail_closed(options.package_name, error.to_string())
                }
            }
        }
    }

    pub fn from_transport(
        transport: Arc<dyn JsonRpcTransport + Send + Sync>,
        package_name: impl Into<String>,
    ) -> Self {
        let package_name = package_name.into();
        if !mock_provider_invocation_allowed() {
            return Self::fail_closed(
                package_name,
                "mock provider transport injection is disabled for this runtime profile",
            );
        }

        Self {
            package_name,
            backend: PythonRuntimeBackend::Stub(transport),
        }
    }

    pub fn spawn(options: &PythonWorkerLaunchOptions) -> Result<Self, SdkRuntimeError> {
        if !options.worker_script.exists() {
            return Err(SdkRuntimeError::new(
                "worker_script_missing",
                format!(
                    "python worker script not found: {}",
                    options.worker_script.display()
                ),
            ));
        }

        let launch_options = options.clone();
        let pool = Arc::new(
            SpawnedWorkerPool::new(provider_worker_concurrency_limit(), move || {
                spawn_worker(&launch_options)
            })
            .map_err(map_transport_error)?,
        );
        pool.warm_up(WORKER_ACQUIRE_TIMEOUT)
            .map_err(map_transport_error)?;
        Ok(Self {
            package_name: options.package_name.clone(),
            backend: PythonRuntimeBackend::Managed { pool },
        })
    }

    pub fn in_memory_stub(package_name: impl Into<String>, _ping_ok: bool) -> Self {
        let package_name = package_name.into();
        if !mock_provider_invocation_allowed() {
            return Self::fail_closed(
                package_name,
                "mock provider fallback is disabled for this runtime profile",
            );
        }

        Self {
            package_name: package_name.clone(),
            backend: PythonRuntimeBackend::Stub(Arc::new(PackageStubJsonRpcTransport::new(
                package_name,
                "python_process",
            ))),
        }
    }

    pub fn fail_closed(package_name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            package_name: package_name.into(),
            backend: PythonRuntimeBackend::FailClosed(Arc::new(FailClosedJsonRpcTransport::new(
                reason.into(),
            ))),
        }
    }

    fn shared_transport(&self) -> Result<Arc<dyn JsonRpcTransport + Send + Sync>, SdkRuntimeError> {
        match &self.backend {
            PythonRuntimeBackend::Stub(transport) | PythonRuntimeBackend::FailClosed(transport) => {
                Ok(transport.clone())
            }
            PythonRuntimeBackend::Managed { .. } => Err(SdkRuntimeError::new(
                "transport_error",
                "managed workers require a request-scoped lease",
            )),
        }
    }

    fn acquire_worker(
        pool: &SpawnedWorkerPool,
        request: &SdkRuntimeRequest,
    ) -> Result<SpawnedWorkerLease, SdkRuntimeError> {
        match request.operation.request_id() {
            Some(request_id) => pool
                .acquire(request_id, WORKER_ACQUIRE_TIMEOUT)
                .map_err(map_transport_error),
            None => pool
                .acquire_internal("invoke", WORKER_ACQUIRE_TIMEOUT)
                .map_err(map_transport_error),
        }
    }

    fn ping_worker(&self) -> Result<Value, SdkRuntimeError> {
        match &self.backend {
            PythonRuntimeBackend::Managed { pool } => {
                let lease = pool
                    .acquire_internal("health", HEALTH_WORKER_ACQUIRE_TIMEOUT)
                    .map_err(map_transport_error)?;
                lease
                    .call_with_timeout(SDKWORK_PING_METHOD, None, HEALTH_WORKER_ACQUIRE_TIMEOUT)
                    .map_err(map_transport_error)
            }
            PythonRuntimeBackend::Stub(transport) | PythonRuntimeBackend::FailClosed(transport) => {
                transport
                    .call(SDKWORK_PING_METHOD, None)
                    .map_err(map_transport_error)
            }
        }
    }

    fn invoke_worker(&self, request: &SdkRuntimeRequest) -> Result<Value, SdkRuntimeError> {
        let params = json!({
            "capability_id": request.capability_id,
            "operation": request.operation,
            "payload": request.payload,
            "package": self.package_name,
        });
        match &self.backend {
            PythonRuntimeBackend::Managed { pool } => {
                let lease = Self::acquire_worker(pool, request)?;
                lease
                    .call_with_timeout(
                        SDKWORK_CAPABILITY_INVOKE_METHOD,
                        Some(params),
                        worker_operation_timeout(request),
                    )
                    .map_err(map_transport_error)
            }
            PythonRuntimeBackend::Stub(_) | PythonRuntimeBackend::FailClosed(_) => self
                .shared_transport()?
                .call(SDKWORK_CAPABILITY_INVOKE_METHOD, Some(params))
                .map_err(map_transport_error),
        }
    }

    fn invoke_worker_streaming(
        &self,
        request: &SdkRuntimeRequest,
        sink: &mut dyn FnMut(Value) -> Result<bool, SdkRuntimeError>,
    ) -> Result<(), SdkRuntimeError> {
        let params = json!({
            "capability_id": request.capability_id,
            "operation": request.operation,
            "payload": request.payload,
            "package": self.package_name,
        });
        match &self.backend {
            PythonRuntimeBackend::Managed { pool } => {
                let lease = Self::acquire_worker(pool, request)?;
                lease
                    .call_streaming_with_timeout(
                        SDKWORK_CAPABILITY_INVOKE_METHOD,
                        Some(params),
                        worker_operation_timeout(request),
                        &mut |frame| {
                            sink(frame).map_err(|error| TransportError::new(error.message))
                        },
                    )
                    .map_err(map_transport_error)
            }
            PythonRuntimeBackend::Stub(_) | PythonRuntimeBackend::FailClosed(_) => self
                .shared_transport()?
                .call_streaming(
                    SDKWORK_CAPABILITY_INVOKE_METHOD,
                    Some(params),
                    &mut |frame| sink(frame).map_err(|error| TransportError::new(error.message)),
                )
                .map_err(map_transport_error),
        }
    }
}

impl SdkBackendRuntime for PythonSdkBackendRuntime {
    fn backend_kind(&self) -> SdkBackendKind {
        SdkBackendKind::PythonProcess
    }

    fn health(&self) -> SdkDriverHealth {
        match self.ping_worker() {
            Ok(result) => match result.get("ok").and_then(Value::as_bool) {
                Some(true) => package_probe_health(&self.package_name, &result),
                _ => SdkDriverHealth::degraded("worker ping returned unexpected payload"),
            },
            Err(error) => SdkDriverHealth::unhealthy(error.message),
        }
    }

    fn invoke(&self, request: &SdkRuntimeRequest) -> Result<SdkRuntimeResponse, SdkRuntimeError> {
        if matches!(request.operation, SdkRuntimeOperation::Ping) {
            let payload = self.ping_worker()?;
            return Ok(SdkRuntimeResponse::success(
                SdkBackendKind::PythonProcess,
                &request.capability_id,
                payload,
            ));
        }

        let payload = self.invoke_worker(request)?;
        if payload.get("ok").and_then(Value::as_bool) == Some(false) {
            let message = payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("python worker invoke failed")
                .to_string();
            return Ok(SdkRuntimeResponse::failure(
                SdkBackendKind::PythonProcess,
                message,
            ));
        }
        Ok(SdkRuntimeResponse::success(
            SdkBackendKind::PythonProcess,
            &request.capability_id,
            payload,
        ))
    }

    fn invoke_streaming(
        &self,
        request: &SdkRuntimeRequest,
        sink: &mut dyn FnMut(Value) -> Result<bool, SdkRuntimeError>,
    ) -> Result<(), SdkRuntimeError> {
        if matches!(request.operation, SdkRuntimeOperation::Ping) {
            let payload = self.ping_worker()?;
            sink(payload)?;
            return Ok(());
        }

        if matches!(
            request.operation,
            SdkRuntimeOperation::ModelChatStream { .. }
        ) {
            return self.invoke_worker_streaming(request, sink);
        }

        SdkBackendRuntime::invoke_streaming(self, request, sink)
    }

    fn cancel_inflight(&self, request_id: &str) -> Result<bool, SdkRuntimeError> {
        match &self.backend {
            PythonRuntimeBackend::Managed { pool } => {
                pool.cancel(request_id).map_err(map_transport_error)
            }
            PythonRuntimeBackend::Stub(_) | PythonRuntimeBackend::FailClosed(_) => Ok(false),
        }
    }

    fn resolve_interaction(
        &self,
        resolution: &SdkRuntimeInteractionResolution,
    ) -> Result<Value, SdkRuntimeError> {
        resolution.validate()?;
        match &self.backend {
            PythonRuntimeBackend::Managed { pool } => pool
                .control(
                    &resolution.model_request_id,
                    "sdkwork/serverRequest.respond",
                    Some(json!(resolution)),
                    INTERACTION_CONTROL_TIMEOUT,
                )
                .map_err(map_transport_error),
            PythonRuntimeBackend::Stub(_) | PythonRuntimeBackend::FailClosed(_) => {
                Err(SdkRuntimeError::new(
                    "interaction_resolution_unavailable",
                    "active provider interaction control requires a managed Python worker",
                ))
            }
        }
    }
}

const INTERACTION_CONTROL_TIMEOUT: Duration = Duration::from_secs(30);

fn worker_operation_timeout(request: &SdkRuntimeRequest) -> Duration {
    // Same deadline contract as the Node runtime: the request's timeout_ms is
    // the hard process deadline for unary and session-control calls alike.
    let timeout_ms = match &request.operation {
        SdkRuntimeOperation::ModelChat { timeout_ms, .. }
        | SdkRuntimeOperation::ModelChatStream { timeout_ms, .. }
        | SdkRuntimeOperation::SessionInterrupt { timeout_ms, .. }
        | SdkRuntimeOperation::SessionCompact { timeout_ms, .. }
        | SdkRuntimeOperation::SessionFork { timeout_ms, .. } => *timeout_ms,
        _ => None,
    };
    timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_WORKER_OPERATION_TIMEOUT)
        .clamp(Duration::from_millis(1), MAX_WORKER_OPERATION_TIMEOUT)
}

fn spawn_worker(options: &PythonWorkerLaunchOptions) -> Result<SpawnedWorker, TransportError> {
    let mut command = Command::new(&options.python_binary);
    command
        .arg(&options.worker_script)
        .arg("--package")
        .arg(&options.package_name);
    SpawnedWorker::spawn(command)
}

fn map_transport_error(error: TransportError) -> SdkRuntimeError {
    SdkRuntimeError::new("transport_error", error.message)
}

fn package_probe_health(package_name: &str, payload: &Value) -> SdkDriverHealth {
    match payload.get("package_resolved").and_then(Value::as_bool) {
        Some(true) | None => SdkDriverHealth::healthy(),
        Some(false) if mock_provider_invocation_allowed() => SdkDriverHealth::degraded(format!(
            "official sdk package is not resolved; development mock fallback is enabled: {package_name}"
        )),
        Some(false) => SdkDriverHealth::unhealthy(format!(
            "official sdk package is not resolved and mock fallback is disabled: {package_name}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdkwork_agent_provider_core::mock_provider_invocation_allowed;
    use sdkwork_agent_provider_spi::SdkDriverStatus;
    use std::sync::{Mutex, OnceLock};

    const KERNEL_PROFILE_ID_ENV: &str = "SDKWORK_KERNEL_PROFILE_ID";
    const KERNEL_ENVIRONMENT_ENV: &str = "SDKWORK_KERNEL_ENVIRONMENT";
    const ALLOW_MOCK_PROVIDERS_ENV: &str = "SDKWORK_KERNEL_ALLOW_MOCK_PROVIDERS";

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let previous = std::env::var(key).ok();
            match value {
                Some(next) => std::env::set_var(key, next),
                None => std::env::remove_var(key),
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn default_worker_script_points_to_repository_script() {
        let _lock = env_lock();
        let _host_root = EnvVarGuard::set(PROVIDER_HOST_ROOT_ENV, None);
        let _legacy_root = EnvVarGuard::set(LEGACY_PROVIDER_RUNTIME_ROOT_ENV, None);
        let _script = EnvVarGuard::set(WORKER_SCRIPT_ENV, None);
        let script = default_python_worker_script();
        assert!(
            script.exists(),
            "default Python worker script must exist: {}",
            script.display()
        );
    }

    #[test]
    fn packaged_host_root_resolves_python_worker_without_repository_paths() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "sdkwork-python-provider-host-test-{}",
            std::process::id()
        ));
        let worker = root.join(PYTHON_WORKER_RELATIVE_PATH);
        std::fs::create_dir_all(worker.parent().expect("worker parent")).expect("worker dir");
        std::fs::write(&worker, "#!/usr/bin/env python3\n").expect("worker file");

        let _host_root = EnvVarGuard::set(
            PROVIDER_HOST_ROOT_ENV,
            Some(root.to_string_lossy().as_ref()),
        );
        let _legacy_root = EnvVarGuard::set(LEGACY_PROVIDER_RUNTIME_ROOT_ENV, None);
        let _script = EnvVarGuard::set(WORKER_SCRIPT_ENV, None);

        assert_eq!(default_python_worker_script(), worker);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn canonical_host_root_precedes_the_legacy_runtime_root() {
        let _lock = env_lock();
        let base = std::env::temp_dir().join(format!(
            "sdkwork-python-provider-host-precedence-test-{}",
            std::process::id()
        ));
        let host_root = base.join("provider-host");
        let legacy_root = base.join("provider-runtime");
        let host_worker = host_root.join(PYTHON_WORKER_RELATIVE_PATH);
        let legacy_worker = legacy_root.join(PYTHON_WORKER_RELATIVE_PATH);
        std::fs::create_dir_all(host_worker.parent().expect("host worker parent"))
            .expect("host worker dir");
        std::fs::create_dir_all(legacy_worker.parent().expect("legacy worker parent"))
            .expect("legacy worker dir");
        std::fs::write(&host_worker, "host worker\n").expect("host worker file");
        std::fs::write(&legacy_worker, "legacy worker\n").expect("legacy worker file");

        let _host_root = EnvVarGuard::set(
            PROVIDER_HOST_ROOT_ENV,
            Some(host_root.to_string_lossy().as_ref()),
        );
        let _legacy_root = EnvVarGuard::set(
            LEGACY_PROVIDER_RUNTIME_ROOT_ENV,
            Some(legacy_root.to_string_lossy().as_ref()),
        );
        let _script = EnvVarGuard::set(WORKER_SCRIPT_ENV, None);

        assert_eq!(default_python_worker_script(), host_worker);

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn legacy_runtime_root_remains_a_compatibility_input() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "sdkwork-python-provider-runtime-compatibility-test-{}",
            std::process::id()
        ));
        let worker = root.join(PYTHON_WORKER_RELATIVE_PATH);
        std::fs::create_dir_all(worker.parent().expect("worker parent")).expect("worker dir");
        std::fs::write(&worker, "legacy worker\n").expect("worker file");

        let _host_root = EnvVarGuard::set(PROVIDER_HOST_ROOT_ENV, None);
        let _legacy_root = EnvVarGuard::set(
            LEGACY_PROVIDER_RUNTIME_ROOT_ENV,
            Some(root.to_string_lossy().as_ref()),
        );
        let _script = EnvVarGuard::set(WORKER_SCRIPT_ENV, None);

        assert_eq!(default_python_worker_script(), worker);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn packaged_host_directory_precedes_a_nearer_legacy_runtime_directory() {
        let base = std::env::temp_dir().join(format!(
            "sdkwork-python-provider-host-directory-precedence-test-{}",
            std::process::id()
        ));
        let start_directory = base.join("application").join("bin");
        let host_root = base.join(PROVIDER_HOST_DIR_NAME);
        let legacy_root = start_directory.join(LEGACY_PROVIDER_RUNTIME_DIR_NAME);
        let host_worker = host_root.join(PYTHON_WORKER_RELATIVE_PATH);
        let legacy_worker = legacy_root.join(PYTHON_WORKER_RELATIVE_PATH);
        std::fs::create_dir_all(&start_directory).expect("start directory");
        std::fs::create_dir_all(host_worker.parent().expect("host worker parent"))
            .expect("host worker directory");
        std::fs::create_dir_all(legacy_worker.parent().expect("legacy worker parent"))
            .expect("legacy worker directory");
        std::fs::write(&host_worker, "host worker\n").expect("host worker file");
        std::fs::write(&legacy_worker, "legacy worker\n").expect("legacy worker file");

        assert_eq!(
            find_packaged_provider_host_root(&start_directory, PYTHON_WORKER_RELATIVE_PATH),
            Some(host_root)
        );

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn explicit_python_worker_path_takes_precedence_over_packaged_host() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "sdkwork-python-provider-host-explicit-test-{}",
            std::process::id()
        ));
        let explicit_worker = root.join("explicit-worker.py");
        std::fs::create_dir_all(&root).expect("runtime test dir");
        std::fs::write(&explicit_worker, "worker\n").expect("worker file");

        let _host_root = EnvVarGuard::set(PROVIDER_HOST_ROOT_ENV, None);
        let _legacy_root = EnvVarGuard::set(LEGACY_PROVIDER_RUNTIME_ROOT_ENV, None);
        let _script = EnvVarGuard::set(
            WORKER_SCRIPT_ENV,
            Some(explicit_worker.to_string_lossy().as_ref()),
        );

        assert_eq!(default_python_worker_script(), explicit_worker);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_python_binary_is_shared_with_provider_installers() {
        let _lock = env_lock();
        let _python = EnvVarGuard::set(PYTHON_BINARY_ENV, Some("managed-python"));

        assert_eq!(default_python_binary(), "managed-python");
        assert_eq!(
            PythonWorkerLaunchOptions::for_package("run_agent").python_binary,
            "managed-python"
        );
    }

    #[test]
    fn in_memory_stub_invokes_ping() {
        let _lock = env_lock();
        let _profile = EnvVarGuard::set(KERNEL_PROFILE_ID_ENV, None);
        let _environment = EnvVarGuard::set(KERNEL_ENVIRONMENT_ENV, Some("development"));
        let _allow = EnvVarGuard::set(ALLOW_MOCK_PROVIDERS_ENV, Some("1"));

        let runtime = PythonSdkBackendRuntime::in_memory_stub("run_agent", true);
        let response = runtime
            .invoke(&SdkRuntimeRequest::ping("sdk.model.chat"))
            .expect("ping should succeed");
        assert!(response.success);
        assert_eq!(response.backend_kind, SdkBackendKind::PythonProcess);
    }

    #[test]
    fn in_memory_stub_fails_closed_in_production_profile() {
        let _lock = env_lock();
        let _profile = EnvVarGuard::set(
            KERNEL_PROFILE_ID_ENV,
            Some("cloud.production"),
        );
        let _environment = EnvVarGuard::set(KERNEL_ENVIRONMENT_ENV, Some("production"));
        let _allow = EnvVarGuard::set(ALLOW_MOCK_PROVIDERS_ENV, None);
        assert!(!mock_provider_invocation_allowed());

        let runtime = PythonSdkBackendRuntime::in_memory_stub("run_agent", true);
        let response = runtime
            .invoke(&SdkRuntimeRequest::model_chat(
                "sdk.model.chat",
                "req.production.stub",
                vec!["hello".to_string()],
            ))
            .expect("runtime should map fail-closed payloads");

        assert!(!response.success);
        assert!(response
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("production profile"));
    }

    #[test]
    fn from_transport_fails_closed_in_production_profile() {
        let _lock = env_lock();
        let _profile = EnvVarGuard::set(
            KERNEL_PROFILE_ID_ENV,
            Some("cloud.production"),
        );
        let _environment = EnvVarGuard::set(KERNEL_ENVIRONMENT_ENV, Some("production"));
        let _allow = EnvVarGuard::set(ALLOW_MOCK_PROVIDERS_ENV, None);
        assert!(!mock_provider_invocation_allowed());

        let transport = Arc::new(PackageStubJsonRpcTransport::new(
            "run_agent",
            "python_process",
        ));
        let runtime = PythonSdkBackendRuntime::from_transport(transport, "run_agent");
        let response = runtime
            .invoke(&SdkRuntimeRequest::model_chat(
                "sdk.model.chat",
                "req.production.transport",
                vec!["hello".to_string()],
            ))
            .expect("runtime should map fail-closed payloads");

        assert!(!response.success);
        assert!(response
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("production profile"));
    }

    #[test]
    fn health_is_unhealthy_when_official_sdk_is_missing_in_production_profile() {
        let _lock = env_lock();
        let _profile = EnvVarGuard::set(
            KERNEL_PROFILE_ID_ENV,
            Some("cloud.production"),
        );
        let _environment = EnvVarGuard::set(KERNEL_ENVIRONMENT_ENV, Some("production"));
        let _allow = EnvVarGuard::set(ALLOW_MOCK_PROVIDERS_ENV, None);
        assert!(!mock_provider_invocation_allowed());

        let runtime = PythonSdkBackendRuntime::bootstrap("sdkwork_missing_python_sdk");
        let health = runtime.health();

        assert_eq!(health.status, SdkDriverStatus::Unhealthy);
        assert!(health
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("official sdk package is not resolved"));
    }
}
