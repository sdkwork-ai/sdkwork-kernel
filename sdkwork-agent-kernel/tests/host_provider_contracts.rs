// WORKSPACE-PATH:allow-fixture: fixtures name a foreign checkout root, drive, or home directory to exercise path handling, so the literal is the value under assertion rather than a binding this build resolves
use sdkwork_agent_kernel::{
    EnvironmentRequest, EnvironmentResult, ExecutorRequest, ExecutorResult, ExecutorStatus,
    FilesystemOperation, FilesystemRequest, FilesystemResult, HostEnvPolicy, HostPathPolicy,
    HostProvider, KernelError, KernelResult, NetworkRequest, NetworkResult, ProcessRequest,
    ProcessResult, ProviderHealth, ProviderManifest, ProviderSecretValue, SecretRef,
    SideEffectLevel, StorageRequest, StorageResult, TimeRequest, TimeResult,
};

#[test]
fn host_path_policy_allows_roots_and_denies_traversal() {
    let policy = HostPathPolicy::new(vec![
        "workspace".to_string(),
        "D:/sdkwork/project".to_string(),
    ]);

    assert!(policy.is_path_allowed("workspace/src/lib.rs"));
    assert!(policy.is_path_allowed("workspace/src/../Cargo.toml"));
    assert!(policy.is_path_allowed("D:\\sdkwork\\project\\README.md"));
    assert!(!policy.is_path_allowed("workspace/../secret.txt"));
    assert!(!policy.is_path_allowed("../secret.txt"));
    assert!(!policy.is_path_allowed("D:/sdkwork/project2/README.md"));
}

#[test]
fn filesystem_requests_classify_side_effects_and_policy_requirements() {
    let read = FilesystemRequest::read("fs.1", "workspace/README.md");
    let write = FilesystemRequest::write("fs.2", "workspace/out.txt", "hello")
        .with_policy_categories(vec!["host.filesystem.write".to_string()]);
    let delete = FilesystemRequest::delete("fs.3", "workspace/out.txt")
        .with_policy_categories(vec!["host.filesystem.delete".to_string()]);

    assert_eq!(read.operation, FilesystemOperation::Read);
    assert_eq!(read.side_effect_level(), SideEffectLevel::ReadOnly);
    assert!(!read.requires_policy());

    assert_eq!(write.side_effect_level(), SideEffectLevel::SideEffectful);
    assert!(write.requires_policy());
    assert_eq!(delete.side_effect_level(), SideEffectLevel::Destructive);
    assert!(delete.requires_policy());
}

#[test]
fn process_requests_declare_working_directory_timeout_env_policy_and_policy_categories() {
    let request =
        ProcessRequest::spawn("process.1", "cargo", vec!["test".to_string()], "workspace")
            .with_timeout_ms(30_000)
            .with_env_policy(HostEnvPolicy::AllowList(vec!["PATH".to_string()]))
            .with_policy_categories(vec!["host.process.spawn".to_string()]);

    assert_eq!(request.command, "cargo");
    assert_eq!(request.args, ["test"]);
    assert_eq!(request.working_directory, "workspace");
    assert_eq!(request.timeout_ms, Some(30_000));
    assert_eq!(
        request.env_policy,
        HostEnvPolicy::AllowList(vec!["PATH".to_string()])
    );
    assert!(request.requires_policy());
}

#[test]
fn network_secret_storage_time_environment_and_executor_contracts_are_explicit() {
    let network = NetworkRequest::get("network.1", "https://example.com")
        .with_policy_categories(vec!["host.network.request".to_string()]);
    let secret_ref = SecretRef::new("secret.openai", "OpenAI API key");
    let secret_value =
        ProviderSecretValue::new(secret_ref.secret_ref_id.clone(), "super-secret-value");
    let storage = StorageRequest::put("storage.1", "session", "task.summary", "result")
        .with_retention_days(7);
    let time = TimeRequest::now("time.1");
    let environment = EnvironmentRequest::get("env.1", "PATH");
    let executor = ExecutorRequest::run("executor.1", "action.plan.1").with_timeout_ms(5_000);

    assert!(network.requires_policy());
    assert_eq!(secret_ref.secret_ref_id, "secret.openai");
    assert_eq!(secret_value.redacted(), "[REDACTED]");
    assert!(!format!("{secret_value:?}").contains("super-secret-value"));
    assert_eq!(storage.scope, "session");
    assert_eq!(storage.retention_days, Some(7));
    assert_eq!(time.operation_id, "time.1");
    assert_eq!(environment.variable_name, "PATH");
    assert_eq!(executor.timeout_ms, Some(5_000));
    assert!(executor.requires_policy());
}

#[test]
fn host_provider_trait_supports_deterministic_fake_host() {
    let provider = FakeHostProvider {
        path_policy: HostPathPolicy::new(vec!["workspace".to_string()]),
    };

    assert_eq!(provider.health().status.as_str(), "available");
    assert_eq!(provider.provider_manifest().provider_family, "host");

    let read = provider
        .filesystem(FilesystemRequest::read("fs.1", "workspace/README.md"))
        .expect("fake filesystem read succeeds");
    assert_eq!(read.content, Some("fake file content".to_string()));

    let denied = provider.filesystem(FilesystemRequest::read("fs.2", "../secret.txt"));
    assert_eq!(
        denied,
        Err(KernelError::PolicyDenied {
            reason_code: "host.path.denied".to_string()
        })
    );

    let process = provider
        .process(ProcessRequest::spawn(
            "process.1",
            "echo",
            vec!["hello".to_string()],
            "workspace",
        ))
        .expect("fake process succeeds");
    assert!(process.is_success());

    let network = provider
        .network(NetworkRequest::get("network.1", "https://example.com"))
        .expect("fake network succeeds");
    assert_eq!(network.status_code, 200);

    let secret = provider
        .resolve_secret(SecretRef::new("secret.fake", "Fake Secret"))
        .expect("fake secret resolves");
    assert_eq!(secret.redacted(), "[REDACTED]");
    assert!(!format!("{secret:?}").contains("fake-secret-value"));

    let storage = provider
        .storage(StorageRequest::put("storage.1", "session", "key", "value"))
        .expect("fake storage succeeds");
    assert!(storage.stored);
    assert_eq!(storage.version, Some(1));

    let time = provider
        .time(TimeRequest::now("time.1"))
        .expect("fake time succeeds");
    assert_eq!(time.timestamp, "2026-01-01T00:00:00Z");
    assert_eq!(time.timezone, Some("UTC".to_string()));

    let env = provider
        .environment(EnvironmentRequest::get("env.1", "PATH"))
        .expect("fake environment succeeds");
    assert_eq!(env.variable_name, "PATH");
    assert!(env.value.is_some());

    let env_missing = provider
        .environment(EnvironmentRequest::get("env.2", "MISSING_VAR"))
        .expect("fake environment missing succeeds");
    assert!(env_missing.value.is_none());

    let executor = provider
        .executor(ExecutorRequest::run("executor.1", "action.1"))
        .expect("fake executor succeeds");
    assert_eq!(executor.status, ExecutorStatus::Completed);
    assert_eq!(executor.output, Some("fake executor output".to_string()));
}

struct FakeHostProvider {
    path_policy: HostPathPolicy,
}

impl HostProvider for FakeHostProvider {
    fn provider_manifest(&self) -> ProviderManifest {
        ProviderManifest::new(
            "provider.host.fake",
            "host",
            "sdkwork-fake-host",
            "0.1.0",
            vec![
                "host.filesystem".to_string(),
                "host.process".to_string(),
                "host.network".to_string(),
                "host.secrets".to_string(),
            ],
        )
    }

    fn health(&self) -> ProviderHealth {
        ProviderHealth::available()
    }

    fn filesystem(&self, request: FilesystemRequest) -> KernelResult<FilesystemResult> {
        if !self.path_policy.is_path_allowed(&request.path) {
            return Err(KernelError::PolicyDenied {
                reason_code: "host.path.denied".to_string(),
            });
        }

        Ok(FilesystemResult::read(
            request.operation_id,
            "fake file content",
        ))
    }

    fn process(&self, request: ProcessRequest) -> KernelResult<ProcessResult> {
        Ok(ProcessResult::exited(request.operation_id, 0, "hello", ""))
    }

    fn network(&self, request: NetworkRequest) -> KernelResult<NetworkResult> {
        Ok(NetworkResult::response(
            request.operation_id,
            200,
            "fake response",
        ))
    }

    fn resolve_secret(&self, secret_ref: SecretRef) -> KernelResult<ProviderSecretValue> {
        Ok(ProviderSecretValue::new(
            secret_ref.secret_ref_id,
            "fake-secret-value",
        ))
    }

    fn storage(&self, request: StorageRequest) -> KernelResult<StorageResult> {
        Ok(StorageResult::stored(request.operation_id).with_version(1))
    }

    fn time(&self, request: TimeRequest) -> KernelResult<TimeResult> {
        Ok(TimeResult::now(request.operation_id, "2026-01-01T00:00:00Z").with_timezone("UTC"))
    }

    fn environment(&self, request: EnvironmentRequest) -> KernelResult<EnvironmentResult> {
        if request.variable_name == "PATH" {
            Ok(EnvironmentResult::resolved(
                request.operation_id,
                request.variable_name,
                "/usr/bin:/usr/local/bin",
            ))
        } else {
            Ok(EnvironmentResult::not_found(
                request.operation_id,
                request.variable_name,
            ))
        }
    }

    fn executor(&self, request: ExecutorRequest) -> KernelResult<ExecutorResult> {
        Ok(ExecutorResult::completed(
            request.operation_id,
            request.action_id,
            "fake executor output",
        ))
    }
}
