use std::{
    collections::{HashMap, HashSet},
    fmt,
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, OnceLock, RwLock, Weak,
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(windows)]
use std::ffi::OsString;

use pep440_rs::Version as PythonVersion;
use sdkwork_agent_kernel::{
    AgentAvailableUpgrade, AgentConfigField, AgentConfigSection, AgentConfigSectionKind,
    AgentConfigValue, AgentConfigValueKind, AgentConfiguration, AgentConfigurationProvider,
    AgentConfigurationSpec, AgentConfigurationValidation, AgentInstallOptions, AgentInstallPlan,
    AgentInstallRecord, AgentInstallReport, AgentInstallRequest, AgentInstallStep,
    AgentInstallStepKind, AgentInstallation, AgentInstallationDependency, AgentInstaller,
    AgentModelConfigurationApplication, AgentModelConfigurationFieldMapping,
    AgentModelConfigurationRequest, AgentModelSelectionRequest, AgentPackageSource,
    AgentPackageSourceInfo, AgentRollbackReport, AgentRollbackRequest, AgentSecretBinding,
    AgentUninstallPlan, AgentUninstallReport, AgentUninstallRequest, AgentUpgradePlan,
    AgentUpgradeReport, AgentUpgradeRequest, KernelError, KernelEventRedaction, KernelResult,
    PolicyCategory, ProviderHealth, ProviderManifest,
};
use semver::Version;
use serde_json::Value;

use crate::StandardPluginIds;

const PROVIDER_HOST_ROOT_ENV: &str = "SDKWORK_AGENT_PROVIDER_HOST_ROOT";
const LEGACY_PROVIDER_RUNTIME_ROOT_ENV: &str = "SDKWORK_AGENT_PROVIDER_RUNTIME_ROOT";
const PYTHON_BINARY_ENV: &str = "SDKWORK_AGENT_PYTHON_BINARY";
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const DETECTION_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const REGISTRY_QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const PACKAGE_MUTATION_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const OPERATION_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(windows)]
const PROCESS_TREE_TERMINATION_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CAPTURE_BYTES: usize = 64 * 1024;
const MAX_ACTIVE_OUTPUT_READERS: usize = 64;
const MAX_MANAGED_PACKAGES: usize = 32;
const MAX_REQUEST_ID_CHARACTERS: usize = 128;
const OPERATION_LOCK_DIRECTORY: &str = "sdkwork-agent-provider-lifecycle-locks";
const OPERATION_LOCK_SHARD_COUNT: u64 = 4096;
const ROLLBACK_TOKEN_SCHEMA_VERSION: u64 = 1;
const MAX_ROLLBACK_TOKEN_CHARACTERS: usize = 32 * 1024;
const PYTHON_METADATA_PROBE: &str = r#"import importlib.metadata as metadata
import json
import sys

versions = {}
for package in sys.argv[1:]:
    try:
        versions[package] = metadata.version(package)
    except metadata.PackageNotFoundError:
        versions[package] = None
print(json.dumps(versions))
"#;

static OPERATION_LOCKS: OnceLock<Mutex<HashMap<String, Weak<RwLock<()>>>>> = OnceLock::new();
static ACTIVE_OUTPUT_READERS: AtomicUsize = AtomicUsize::new(0);

struct CrossProcessOperationLock {
    file: File,
}

impl CrossProcessOperationLock {
    fn acquire(key: &str, exclusive: bool, started: Instant) -> KernelResult<Self> {
        let lock_path = operation_lock_file_path(key)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|_| {
                KernelError::provider_error(
                    "provider_installer_lock_unavailable",
                    "provider lifecycle cross-process coordination is unavailable",
                )
            })?;
        loop {
            let result = if exclusive {
                file.try_lock()
            } else {
                file.try_lock_shared()
            };
            match result {
                Ok(()) => return Ok(Self { file }),
                Err(fs::TryLockError::WouldBlock) if started.elapsed() < OPERATION_LOCK_TIMEOUT => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(fs::TryLockError::WouldBlock) => {
                    return Err(KernelError::timeout(
                        "provider lifecycle coordination timed out",
                    ));
                }
                Err(_) => {
                    return Err(KernelError::provider_error(
                        "provider_installer_lock_unavailable",
                        "provider lifecycle cross-process coordination is unavailable",
                    ));
                }
            }
        }
    }
}

impl Drop for CrossProcessOperationLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn operation_lock_file_path(key: &str) -> KernelResult<PathBuf> {
    let directory = operation_lock_directory()?;
    let shard = stable_lock_key_hash(key) % OPERATION_LOCK_SHARD_COUNT;
    Ok(directory.join(format!("{shard:03x}.lock")))
}

fn operation_lock_directory() -> KernelResult<PathBuf> {
    let directory = std::env::temp_dir().join(OPERATION_LOCK_DIRECTORY);
    #[cfg(unix)]
    let create_result = {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700).create(&directory)
    };
    #[cfg(not(unix))]
    let create_result = fs::create_dir(&directory);
    if let Err(error) = create_result {
        if error.kind() != ErrorKind::AlreadyExists {
            return Err(KernelError::provider_error(
                "provider_installer_lock_unavailable",
                "provider lifecycle cross-process coordination is unavailable",
            ));
        }
    }
    let metadata = fs::symlink_metadata(&directory).map_err(|_| {
        KernelError::provider_error(
            "provider_installer_lock_unavailable",
            "provider lifecycle cross-process coordination is unavailable",
        )
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(KernelError::provider_error(
            "provider_installer_lock_unavailable",
            "provider lifecycle cross-process coordination is unavailable",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = metadata.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&directory, permissions).map_err(|_| {
            KernelError::provider_error(
                "provider_installer_lock_unavailable",
                "provider lifecycle cross-process coordination is unavailable",
            )
        })?;
    }
    Ok(directory)
}

fn stable_lock_key_hash(key: &str) -> u64 {
    key.as_bytes()
        .iter()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessAdapterCommand {
    pub program: String,
    pub args: Vec<String>,
    pub timeout: Option<Duration>,
}

impl ProcessAdapterCommand {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            timeout: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProcessAdapterCommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl fmt::Debug for ProcessAdapterCommandOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessAdapterCommandOutput")
            .field("exit_code", &self.exit_code)
            .field("stdout_bytes", &self.stdout.len())
            .field("stderr_bytes", &self.stderr.len())
            .field("timed_out", &self.timed_out)
            .finish()
    }
}

impl ProcessAdapterCommandOutput {
    pub fn success(stdout: impl Into<String>) -> Self {
        Self {
            exit_code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    pub fn failure(exit_code: i32, stderr: impl Into<String>) -> Self {
        Self {
            exit_code,
            stdout: String::new(),
            stderr: stderr.into(),
            timed_out: false,
        }
    }

    pub fn is_success(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
    }
}

pub trait ProcessAdapterCommandExecutor: Send + Sync {
    fn execute(&self, command: &ProcessAdapterCommand)
        -> KernelResult<ProcessAdapterCommandOutput>;
}

#[derive(Debug, Clone)]
pub struct SystemProcessAdapterCommandExecutor {
    timeout: Duration,
}

impl Default for SystemProcessAdapterCommandExecutor {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_COMMAND_TIMEOUT,
        }
    }
}

impl SystemProcessAdapterCommandExecutor {
    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl ProcessAdapterCommandExecutor for SystemProcessAdapterCommandExecutor {
    fn execute(
        &self,
        command: &ProcessAdapterCommand,
    ) -> KernelResult<ProcessAdapterCommandOutput> {
        let mut process = system_process_command(command)?;
        process
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            process.process_group(0);
        }
        let mut child = process.spawn().map_err(|_| {
            KernelError::provider_error(
                "provider_installer_command_unavailable",
                "provider installer command could not start",
            )
        })?;
        let process_tree = ProcessTreeGuard::attach(&child);

        let stdout = child
            .stdout
            .take()
            .expect("piped provider installer stdout");
        let stderr = child
            .stderr
            .take()
            .expect("piped provider installer stderr");
        let stdout_reader = match BoundedReader::spawn(stdout) {
            Ok(reader) => reader,
            Err(error) => {
                process_tree.terminate(&mut child);
                let _ = child.wait();
                return Err(error);
            }
        };
        let stderr_reader = match BoundedReader::spawn(stderr) {
            Ok(reader) => reader,
            Err(error) => {
                process_tree.terminate(&mut child);
                let _ = child.wait();
                let _ = stdout_reader.finish();
                return Err(error);
            }
        };
        let timeout = command.timeout.unwrap_or(self.timeout);
        let started = Instant::now();
        let (status, timed_out) = loop {
            if let Some(status) = child.try_wait().map_err(|error| KernelError::Internal {
                message: format!("provider installer command wait failed: {error}"),
            })? {
                break (status, false);
            }
            if started.elapsed() >= timeout {
                process_tree.terminate(&mut child);
                let status = child.wait().map_err(|error| KernelError::Internal {
                    message: format!("provider installer command termination failed: {error}"),
                })?;
                break (status, true);
            }
            thread::sleep(Duration::from_millis(25));
        };

        let drain_started = Instant::now();
        while (!stdout_reader.is_finished() || !stderr_reader.is_finished())
            && drain_started.elapsed() < OUTPUT_DRAIN_TIMEOUT
        {
            thread::sleep(Duration::from_millis(10));
        }
        if !stdout_reader.is_finished() || !stderr_reader.is_finished() {
            process_tree.terminate(&mut child);
        }
        let stdout = stdout_reader.finish()?;
        let stderr = stderr_reader.finish()?;

        Ok(ProcessAdapterCommandOutput {
            exit_code: status.code().unwrap_or(-1),
            stdout,
            stderr,
            timed_out,
        })
    }
}

fn system_process_command(command: &ProcessAdapterCommand) -> KernelResult<Command> {
    #[cfg(windows)]
    if command.program.eq_ignore_ascii_case("npm") {
        let (node_binary, npm_cli) = resolve_windows_npm_runtime()?;
        let mut process = Command::new(node_binary);
        process.arg(npm_cli).args(&command.args);
        return Ok(process);
    }

    let mut process = Command::new(&command.program);
    process.args(&command.args);
    Ok(process)
}

#[cfg(windows)]
fn resolve_windows_npm_runtime() -> KernelResult<(PathBuf, PathBuf)> {
    let path = std::env::var_os("PATH").ok_or_else(|| {
        KernelError::provider_error(
            "provider_installer_command_unavailable",
            "Node.js and the npm CLI could not be resolved",
        )
    })?;
    resolve_windows_npm_runtime_from_path(&path)
}

#[cfg(windows)]
fn resolve_windows_npm_runtime_from_path(
    path: &std::ffi::OsStr,
) -> KernelResult<(PathBuf, PathBuf)> {
    for directory in std::env::split_paths(&path) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        let node_candidate = directory.join("node.exe");
        let npm_candidate = directory
            .join("node_modules")
            .join("npm")
            .join("bin")
            .join("npm-cli.js");
        if node_candidate.is_file() && npm_candidate.is_file() {
            let node_binary = dunce::canonicalize(node_candidate).map_err(|_| {
                KernelError::provider_error(
                    "provider_installer_command_unavailable",
                    "Node.js and the npm CLI could not be resolved",
                )
            })?;
            let npm_cli = dunce::canonicalize(npm_candidate).map_err(|_| {
                KernelError::provider_error(
                    "provider_installer_command_unavailable",
                    "Node.js and the npm CLI could not be resolved",
                )
            })?;
            let node_root = node_binary.parent();
            let npm_root = npm_cli.ancestors().nth(4);
            if node_root == npm_root {
                return Ok((node_binary, npm_cli));
            }
        }
    }
    Err(KernelError::provider_error(
        "provider_installer_command_unavailable",
        "Node.js and the npm CLI could not be resolved",
    ))
}

struct BoundedReader {
    captured: Arc<Mutex<Vec<u8>>>,
    handle: thread::JoinHandle<()>,
}

impl BoundedReader {
    fn spawn(mut reader: impl Read + Send + 'static) -> KernelResult<Self> {
        ACTIVE_OUTPUT_READERS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_ACTIVE_OUTPUT_READERS).then_some(active + 1)
            })
            .map_err(|_| {
                KernelError::provider_error(
                    "provider_installer_output_capacity_exhausted",
                    "provider installer output capacity is exhausted",
                )
            })?;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::clone(&captured);
        let handle = thread::Builder::new()
            .name("provider-output-reader".to_string())
            .spawn(move || {
                let _active_reader = ActiveOutputReader;
                let mut buffer = [0_u8; 4096];
                while let Ok(count) = reader.read(&mut buffer) {
                    if count == 0 {
                        break;
                    }
                    let Ok(mut captured) = writer.lock() else {
                        break;
                    };
                    let remaining = MAX_CAPTURE_BYTES.saturating_sub(captured.len());
                    captured.extend_from_slice(&buffer[..count.min(remaining)]);
                }
            })
            .map_err(|_| {
                ACTIVE_OUTPUT_READERS.fetch_sub(1, Ordering::AcqRel);
                KernelError::provider_error(
                    "provider_installer_output_unavailable",
                    "provider installer output capture could not start",
                )
            })?;
        Ok(Self { captured, handle })
    }

    fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    fn finish(self) -> KernelResult<String> {
        if self.handle.is_finished() {
            self.handle.join().map_err(|_| KernelError::Internal {
                message: "provider installer output reader failed".to_string(),
            })?;
        }
        let captured = self.captured.lock().map_err(|_| KernelError::Internal {
            message: "provider installer output capture failed".to_string(),
        })?;
        Ok(String::from_utf8_lossy(&captured).into_owned())
    }
}

struct ActiveOutputReader;

impl Drop for ActiveOutputReader {
    fn drop(&mut self) {
        ACTIVE_OUTPUT_READERS.fetch_sub(1, Ordering::AcqRel);
    }
}

struct ProcessTreeGuard {
    #[cfg(windows)]
    job: Option<WindowsProcessJob>,
}

impl ProcessTreeGuard {
    fn attach(child: &Child) -> Self {
        Self {
            #[cfg(windows)]
            job: WindowsProcessJob::attach(child),
        }
    }

    fn terminate(&self, child: &mut Child) {
        let process_id = child.id().to_string();
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.terminate();
        } else {
            let taskkill = Command::new("taskkill")
                .args(["/PID", &process_id, "/T", "/F"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(mut taskkill) = taskkill {
                let started = Instant::now();
                loop {
                    match taskkill.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) if started.elapsed() < PROCESS_TREE_TERMINATION_TIMEOUT => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Ok(None) | Err(_) => {
                            let _ = taskkill.kill();
                            let _ = taskkill.wait();
                            break;
                        }
                    }
                }
            }
        }
        #[cfg(unix)]
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{process_id}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
    }
}

#[cfg(windows)]
struct WindowsProcessJob {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl WindowsProcessJob {
    fn attach(child: &Child) -> Option<Self> {
        use std::{mem::size_of, os::windows::io::AsRawHandle, ptr};
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
        };

        // The handle is owned by this guard and closed in Drop. Every failure path closes it here.
        unsafe {
            let handle = CreateJobObjectW(ptr::null(), ptr::null());
            if handle.is_null() {
                return None;
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            let assigned = configured != 0
                && AssignProcessToJobObject(handle, child.as_raw_handle().cast()) != 0;
            if !assigned {
                CloseHandle(handle);
                return None;
            }
            Some(Self { handle })
        }
    }

    fn terminate(&self) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        // The job handle remains valid for the guard lifetime.
        unsafe {
            TerminateJobObject(self.handle, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for WindowsProcessJob {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;

        // Closing a KILL_ON_JOB_CLOSE job also cleans up children left after normal parent exit.
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessAdapterPackageManager {
    Npm,
    PythonPip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessAdapterPackage {
    pub registry_id: String,
    pub package_id: String,
    pub version: String,
    pub package_manager: ProcessAdapterPackageManager,
}

impl ProcessAdapterPackage {
    pub fn npm(package_id: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            registry_id: "npm".to_string(),
            package_id: package_id.into(),
            version: version.into(),
            package_manager: ProcessAdapterPackageManager::Npm,
        }
    }

    pub fn pypi(package_id: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            registry_id: "pypi".to_string(),
            package_id: package_id.into(),
            version: version.into(),
            package_manager: ProcessAdapterPackageManager::PythonPip,
        }
    }

    fn exact_spec(&self) -> String {
        match self.package_manager {
            ProcessAdapterPackageManager::Npm => {
                format!("{}@{}", self.package_id, self.version)
            }
            ProcessAdapterPackageManager::PythonPip => {
                format!("{}=={}", self.package_id, self.version)
            }
        }
    }

    fn validate(&self) -> KernelResult<()> {
        match self.package_manager {
            ProcessAdapterPackageManager::Npm => {
                if !is_valid_npm_package_id(&self.package_id) {
                    return Err(KernelError::validation(
                        "npm package id must be a canonical registry package name",
                    ));
                }
                if Version::parse(&self.version).is_err() {
                    return Err(KernelError::validation(
                        "npm package version must be an exact semantic version",
                    ));
                }
            }
            ProcessAdapterPackageManager::PythonPip => {
                if !is_valid_pypi_package_id(&self.package_id) {
                    return Err(KernelError::validation(
                        "PyPI package id must be a canonical distribution name",
                    ));
                }
                if !is_exact_python_version(&self.version) {
                    return Err(KernelError::validation(
                        "PyPI package version must be an exact PEP 440-style version",
                    ));
                }
            }
        }
        Ok(())
    }
}

fn is_valid_npm_package_id(package_id: &str) -> bool {
    if package_id.is_empty() || package_id.len() > 214 || package_id.trim() != package_id {
        return false;
    }
    if let Some(scoped) = package_id.strip_prefix('@') {
        let Some((scope, name)) = scoped.split_once('/') else {
            return false;
        };
        return !name.contains('/')
            && is_valid_npm_package_segment(scope)
            && is_valid_npm_package_segment(name);
    }
    !package_id.contains('/') && is_valid_npm_package_segment(package_id)
}

fn is_valid_npm_package_segment(segment: &str) -> bool {
    segment
        .chars()
        .next()
        .is_some_and(|first| !matches!(first, '.' | '_'))
        && segment.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_' | '.' | '~')
        })
}

fn is_valid_pypi_package_id(package_id: &str) -> bool {
    if package_id.is_empty() || package_id.len() > 200 || package_id.trim() != package_id {
        return false;
    }
    let mut characters = package_id.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    let Some(last) = package_id.chars().last() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && last.is_ascii_alphanumeric()
        && package_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn is_exact_python_version(version: &str) -> bool {
    if version.is_empty() || version.len() > 128 || version.trim() != version {
        return false;
    }
    version
        .parse::<PythonVersion>()
        .is_ok_and(|parsed| parsed.to_string() == version)
}

fn normalized_package_id(package: &ProcessAdapterPackage) -> String {
    match package.package_manager {
        ProcessAdapterPackageManager::Npm => package.package_id.clone(),
        ProcessAdapterPackageManager::PythonPip => package
            .package_id
            .chars()
            .map(|character| match character {
                '.' | '_' => '-',
                _ => character.to_ascii_lowercase(),
            })
            .collect(),
    }
}

/// Installer for npm and PyPI-backed process-adapter providers.
#[derive(Clone)]
pub struct ProcessAdapterInstaller {
    agent_id: String,
    provider_id: String,
    provider_version: String,
    packages: Vec<ProcessAdapterPackage>,
    install_root: Option<PathBuf>,
    install_scripts_enabled: bool,
    executor: Arc<dyn ProcessAdapterCommandExecutor>,
}

impl fmt::Debug for ProcessAdapterInstaller {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessAdapterInstaller")
            .field("agent_id", &self.agent_id)
            .field("provider_id", &self.provider_id)
            .field("provider_version", &self.provider_version)
            .field("packages", &self.packages)
            .field("install_root_configured", &self.install_root.is_some())
            .field("install_scripts_enabled", &self.install_scripts_enabled)
            .finish_non_exhaustive()
    }
}

impl ProcessAdapterInstaller {
    pub fn new(
        agent_id: impl Into<String>,
        provider_id: impl Into<String>,
        provider_version: impl Into<String>,
        primary_package: ProcessAdapterPackage,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            provider_id: provider_id.into(),
            provider_version: provider_version.into(),
            packages: vec![primary_package],
            install_root: None,
            install_scripts_enabled: false,
            executor: Arc::new(SystemProcessAdapterCommandExecutor::default()),
        }
    }

    pub fn with_dependency(mut self, package: ProcessAdapterPackage) -> Self {
        self.packages.push(package);
        self
    }

    pub fn with_install_root(mut self, install_root: impl Into<PathBuf>) -> Self {
        self.install_root = Some(install_root.into());
        self
    }

    pub fn with_install_scripts(mut self) -> Self {
        self.install_scripts_enabled = true;
        self
    }

    pub fn with_executor(mut self, executor: Arc<dyn ProcessAdapterCommandExecutor>) -> Self {
        self.executor = executor;
        self
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    pub fn provider_version(&self) -> &str {
        &self.provider_version
    }

    pub fn packages(&self) -> &[ProcessAdapterPackage] {
        &self.packages
    }

    pub fn install_scripts_enabled(&self) -> bool {
        self.install_scripts_enabled
    }

    fn validate_agent_id(&self, agent_id: &str) -> KernelResult<()> {
        if agent_id == self.agent_id {
            return Ok(());
        }
        Err(KernelError::validation(format!(
            "{} can only manage {}; received {agent_id}",
            self.provider_id, self.agent_id
        )))
    }

    fn validate_descriptor(&self) -> KernelResult<ProcessAdapterPackageManager> {
        StandardPluginIds::validate_agent_id(&self.agent_id).map_err(KernelError::validation)?;
        StandardPluginIds::validate_provider_id(&self.provider_id)
            .map_err(KernelError::validation)?;
        if !is_exact_semantic_version(&self.provider_version) {
            return Err(KernelError::validation(
                "provider installer version must be an exact semantic version",
            ));
        }
        if self.packages.len() > MAX_MANAGED_PACKAGES {
            return Err(KernelError::validation(format!(
                "provider installer cannot manage more than {MAX_MANAGED_PACKAGES} packages"
            )));
        }
        let manager = self
            .packages
            .first()
            .ok_or_else(|| KernelError::validation("provider installer requires a package"))?
            .package_manager;
        let mut package_ids = HashSet::new();
        for package in &self.packages {
            package.validate()?;
            if package.package_manager != manager {
                return Err(KernelError::validation(
                    "one provider installer cannot mix package managers",
                ));
            }
            let normalized_id = normalized_package_id(package);
            if !package_ids.insert(normalized_id) {
                return Err(KernelError::validation(
                    "provider installer cannot manage a package more than once",
                ));
            }
        }
        Ok(manager)
    }

    fn operation_lock_key(&self, environment: &InstallEnvironment) -> KernelResult<String> {
        match self.validate_descriptor()? {
            ProcessAdapterPackageManager::Npm => {
                let root = self.resolve_npm_install_root(environment)?;
                let mut key = root.to_string_lossy().into_owned();
                if cfg!(windows) {
                    key.make_ascii_lowercase();
                }
                Ok(format!("npm:{key}"))
            }
            ProcessAdapterPackageManager::PythonPip => {
                let mut binary =
                    executable_lock_identity(&self.resolve_python_binary(environment)?);
                if cfg!(windows) {
                    binary.make_ascii_lowercase();
                }
                Ok(format!("pypi:{binary}"))
            }
        }
    }

    fn shared_operation_lock(
        &self,
        environment: &InstallEnvironment,
    ) -> KernelResult<(String, Arc<RwLock<()>>)> {
        let key = self.operation_lock_key(environment)?;
        let registry = OPERATION_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut registry = registry.lock().map_err(|_| {
            KernelError::provider_error(
                "provider_installer_lock_unavailable",
                "provider lifecycle coordination is unavailable",
            )
        })?;
        registry.retain(|_, lock| lock.strong_count() > 0);
        let lock = match registry.get(&key).and_then(Weak::upgrade) {
            Some(lock) => lock,
            None => {
                let lock = Arc::new(RwLock::new(()));
                registry.insert(key.clone(), Arc::downgrade(&lock));
                lock
            }
        };
        Ok((key, lock))
    }

    fn with_detection_lock<T>(
        &self,
        environment: &InstallEnvironment,
        operation: impl FnOnce() -> KernelResult<T>,
    ) -> KernelResult<T> {
        let (key, lock) = self.shared_operation_lock(environment)?;
        let started = Instant::now();
        loop {
            match lock.try_read() {
                Ok(_guard) => {
                    let _cross_process_lock =
                        CrossProcessOperationLock::acquire(&key, false, started)?;
                    return operation();
                }
                Err(std::sync::TryLockError::WouldBlock)
                    if started.elapsed() < OPERATION_LOCK_TIMEOUT =>
                {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    return Err(KernelError::timeout(
                        "provider lifecycle coordination timed out",
                    ));
                }
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(KernelError::provider_error(
                        "provider_installer_lock_poisoned",
                        "provider lifecycle state requires host recovery",
                    ));
                }
            }
        }
    }

    fn with_mutation_lock<T>(
        &self,
        environment: &InstallEnvironment,
        operation: impl FnOnce() -> KernelResult<T>,
    ) -> KernelResult<T> {
        let (key, lock) = self.shared_operation_lock(environment)?;
        let started = Instant::now();
        loop {
            match lock.try_write() {
                Ok(_guard) => {
                    let _cross_process_lock =
                        CrossProcessOperationLock::acquire(&key, true, started)?;
                    return operation();
                }
                Err(std::sync::TryLockError::WouldBlock)
                    if started.elapsed() < OPERATION_LOCK_TIMEOUT =>
                {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    return Err(KernelError::timeout(
                        "provider lifecycle coordination timed out",
                    ));
                }
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(KernelError::provider_error(
                        "provider_installer_lock_poisoned",
                        "provider lifecycle state requires host recovery",
                    ));
                }
            }
        }
    }

    fn validate_install_request(&self, request: &AgentInstallRequest) -> KernelResult<()> {
        validate_lifecycle_request_id(&request.request_id)?;
        self.validate_agent_id(&request.agent_id)?;
        self.validate_descriptor()?;
        if request.target_version != self.provider_version {
            return Err(KernelError::validation(format!(
                "provider package target version must be {}; received {}",
                self.provider_version, request.target_version
            )));
        }
        let primary = &self.packages[0];
        match &request.source {
            AgentPackageSource::Registry {
                registry_id,
                package_id,
                version,
            } if registry_id == &primary.registry_id
                && package_id == &primary.package_id
                && version == &primary.version =>
            {
                self.validate_install_options(request.options.as_ref())?;
                self.validate_request_configuration(request.configuration.as_ref())
            }
            _ => Err(KernelError::validation(
                "provider package source does not match the installer descriptor",
            )),
        }
    }

    fn validate_upgrade_request(&self, request: &AgentUpgradeRequest) -> KernelResult<()> {
        validate_lifecycle_request_id(&request.request_id)?;
        self.validate_agent_id(&request.agent_id)?;
        self.validate_descriptor()?;
        if request.to_version != self.provider_version {
            return Err(KernelError::validation(format!(
                "provider upgrade target version must be {}; received {}",
                self.provider_version, request.to_version
            )));
        }
        if !is_exact_semantic_version(&request.from_version) {
            return Err(KernelError::validation(
                "provider upgrade source version must be an exact semantic version",
            ));
        }
        self.validate_install_options(request.options.as_ref())?;
        self.validate_request_configuration(request.configuration.as_ref())
    }

    fn validate_install_options(&self, options: Option<&AgentInstallOptions>) -> KernelResult<()> {
        let Some(options) = options else {
            return Ok(());
        };
        if let Some(root) = &options.install_root {
            if root.trim().is_empty() || root.trim() != root || root.chars().any(char::is_control) {
                return Err(KernelError::validation(
                    "provider install root option must be a non-empty bounded path",
                ));
            }
        }
        if let Some(binary) = &options.python_binary {
            if binary.trim().is_empty()
                || binary.trim() != binary
                || binary.chars().any(char::is_control)
            {
                return Err(KernelError::validation(
                    "provider python binary option must be a non-empty bounded executable",
                ));
            }
        }
        Ok(())
    }

    fn resolve_npm_install_root(&self, environment: &InstallEnvironment) -> KernelResult<PathBuf> {
        let root = if let Some(root) = &environment.install_root {
            root.clone()
        } else if let Some(root) = configured_provider_host_root()? {
            root
        } else {
            find_packaged_provider_host_root().ok_or_else(|| {
                KernelError::provider_error(
                    "provider_install_root_missing",
                    format!(
                        "{PROVIDER_HOST_ROOT_ENV} is not configured and no packaged provider-host directory was found"
                    ),
                )
            })?
        };
        resolve_install_root(&root)
    }

    fn resolve_python_binary(&self, environment: &InstallEnvironment) -> KernelResult<String> {
        if let Some(binary) = &environment.python_binary {
            return Ok(binary.clone());
        }
        match std::env::var(PYTHON_BINARY_ENV) {
            Ok(binary) if binary.trim().is_empty() => Err(KernelError::validation(format!(
                "{PYTHON_BINARY_ENV} must not be empty"
            ))),
            Ok(binary) => Ok(binary),
            Err(std::env::VarError::NotPresent) => {
                if cfg!(windows) {
                    Ok("python".to_string())
                } else {
                    Ok("python3".to_string())
                }
            }
            Err(std::env::VarError::NotUnicode(_)) => Err(KernelError::validation(format!(
                "{PYTHON_BINARY_ENV} must be valid Unicode"
            ))),
        }
    }

    fn validate_request_configuration(
        &self,
        configuration: Option<&AgentConfiguration>,
    ) -> KernelResult<()> {
        let Some(configuration) = configuration else {
            return Ok(());
        };
        if ProcessAdapterConfigurationProvider::spec_for(&self.agent_id)
            .validate(configuration)
            .is_valid()
        {
            Ok(())
        } else {
            Err(KernelError::validation(
                "provider configuration does not satisfy the installation schema",
            ))
        }
    }

    fn detect_dependencies(
        &self,
        environment: &InstallEnvironment,
    ) -> KernelResult<Vec<AgentInstallationDependency>> {
        match self.validate_descriptor()? {
            ProcessAdapterPackageManager::Npm => self.detect_npm_dependencies(environment),
            ProcessAdapterPackageManager::PythonPip => self.detect_pypi_dependencies(environment),
        }
    }

    fn detect_npm_dependencies(
        &self,
        environment: &InstallEnvironment,
    ) -> KernelResult<Vec<AgentInstallationDependency>> {
        let root = self.resolve_npm_install_root(environment)?;
        let mut args = vec![
            "--prefix".to_string(),
            root.to_string_lossy().into_owned(),
            "list".to_string(),
            "--depth=0".to_string(),
            "--json".to_string(),
        ];
        args.extend(
            self.packages
                .iter()
                .map(|package| package.package_id.clone()),
        );
        let output = self.executor.execute(
            &ProcessAdapterCommand::new("npm", args).with_timeout(DETECTION_COMMAND_TIMEOUT),
        )?;
        if output.timed_out {
            return Err(KernelError::timeout(
                "provider installation detection timed out",
            ));
        }
        let payload: Value = serde_json::from_str(&output.stdout).map_err(|error| {
            KernelError::provider_error(
                "provider_installation_detection_failed",
                format!("npm installation detection returned invalid JSON: {error}"),
            )
        })?;
        let error_code = payload
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str);
        let expected_absence = matches!(error_code, Some("ENOENT" | "ELSPROBLEMS"));
        let empty_absence = payload.as_object().is_some_and(serde_json::Map::is_empty)
            && output.stderr.trim().is_empty();
        if (!output.is_success() && !expected_absence && !empty_absence)
            || error_code.is_some_and(|code| !matches!(code, "ENOENT" | "ELSPROBLEMS"))
        {
            return Err(KernelError::provider_error(
                "provider_installation_detection_failed",
                "npm installation detection failed",
            ));
        }
        let dependencies = payload.get("dependencies").and_then(Value::as_object);
        Ok(self
            .packages
            .iter()
            .map(|package| {
                let installed_version = dependencies
                    .and_then(|items| items.get(&package.package_id))
                    .and_then(|item| item.get("version"))
                    .and_then(Value::as_str);
                dependency_detection(package, installed_version)
            })
            .collect())
    }

    fn detect_pypi_dependencies(
        &self,
        environment: &InstallEnvironment,
    ) -> KernelResult<Vec<AgentInstallationDependency>> {
        let mut args = vec!["-c".to_string(), PYTHON_METADATA_PROBE.to_string()];
        args.extend(
            self.packages
                .iter()
                .map(|package| package.package_id.clone()),
        );
        let output = self.executor.execute(
            &ProcessAdapterCommand::new(self.resolve_python_binary(environment)?, args)
                .with_timeout(DETECTION_COMMAND_TIMEOUT),
        )?;
        if output.timed_out {
            return Err(KernelError::timeout(
                "provider installation detection timed out",
            ));
        }
        if !output.is_success() {
            return Err(KernelError::provider_error(
                "provider_installation_detection_failed",
                format!(
                    "Python installation detection failed with exit code {}",
                    output.exit_code
                ),
            ));
        }
        let payload: Value = serde_json::from_str(&output.stdout).map_err(|error| {
            KernelError::provider_error(
                "provider_installation_detection_failed",
                format!("Python installation detection returned invalid JSON: {error}"),
            )
        })?;
        let versions = payload.as_object().ok_or_else(|| {
            KernelError::provider_error(
                "provider_installation_detection_failed",
                "Python installation detection returned an invalid payload",
            )
        })?;
        self.packages
            .iter()
            .map(|package| match versions.get(&package.package_id) {
                Some(Value::String(version)) if !version.is_empty() => {
                    Ok(dependency_detection(package, Some(version)))
                }
                Some(Value::Null) => Ok(dependency_detection(package, None)),
                _ => Err(KernelError::provider_error(
                    "provider_installation_detection_failed",
                    "Python installation detection omitted package metadata",
                )),
            })
            .collect()
    }

    fn run_package_install(
        &self,
        code: &str,
        environment: &InstallEnvironment,
    ) -> KernelResult<()> {
        let exact_packages: Vec<String> = self
            .packages
            .iter()
            .map(ProcessAdapterPackage::exact_spec)
            .collect();
        self.run_exact_package_install(&exact_packages, code, environment)
    }

    fn run_exact_package_install(
        &self,
        exact_packages: &[String],
        code: &str,
        environment: &InstallEnvironment,
    ) -> KernelResult<()> {
        let manager = self.validate_descriptor()?;
        let command = match manager {
            ProcessAdapterPackageManager::Npm => {
                let root = self.resolve_npm_install_root(environment)?;
                let mut args = vec![
                    "--prefix".to_string(),
                    root.to_string_lossy().into_owned(),
                    "install".to_string(),
                    "--no-audit".to_string(),
                    "--no-fund".to_string(),
                    "--omit=dev".to_string(),
                    "--save-exact".to_string(),
                ];
                if !environment.install_scripts_enabled {
                    args.push("--ignore-scripts".to_string());
                }
                args.extend(exact_packages.iter().cloned());
                ProcessAdapterCommand::new("npm", args)
            }
            ProcessAdapterPackageManager::PythonPip => {
                let mut args = vec![
                    "-m".to_string(),
                    "pip".to_string(),
                    "install".to_string(),
                    "--disable-pip-version-check".to_string(),
                    "--no-input".to_string(),
                    "--only-binary=:all:".to_string(),
                ];
                args.extend(exact_packages.iter().cloned());
                ProcessAdapterCommand::new(self.resolve_python_binary(environment)?, args)
            }
        };
        self.run_checked(&command.with_timeout(PACKAGE_MUTATION_TIMEOUT), code)
    }

    fn run_package_uninstall(
        &self,
        code: &str,
        environment: &InstallEnvironment,
    ) -> KernelResult<()> {
        let package_ids: Vec<String> = self
            .packages
            .iter()
            .rev()
            .map(|package| package.package_id.clone())
            .collect();
        self.run_package_uninstall_ids(&package_ids, code, environment)
    }

    fn run_package_uninstall_ids(
        &self,
        package_ids: &[String],
        code: &str,
        environment: &InstallEnvironment,
    ) -> KernelResult<()> {
        let manager = self.validate_descriptor()?;
        let command = match manager {
            ProcessAdapterPackageManager::Npm => {
                let root = self.resolve_npm_install_root(environment)?;
                let mut args = vec![
                    "--prefix".to_string(),
                    root.to_string_lossy().into_owned(),
                    "uninstall".to_string(),
                    "--no-audit".to_string(),
                    "--no-fund".to_string(),
                ];
                args.extend(package_ids.iter().cloned());
                ProcessAdapterCommand::new("npm", args)
            }
            ProcessAdapterPackageManager::PythonPip => {
                let mut args = vec![
                    "-m".to_string(),
                    "pip".to_string(),
                    "uninstall".to_string(),
                    "--yes".to_string(),
                ];
                args.extend(package_ids.iter().cloned());
                ProcessAdapterCommand::new(self.resolve_python_binary(environment)?, args)
            }
        };
        self.run_checked(&command.with_timeout(PACKAGE_MUTATION_TIMEOUT), code)
    }

    fn run_checked(&self, command: &ProcessAdapterCommand, code: &str) -> KernelResult<()> {
        let output = self.executor.execute(command)?;
        if output.timed_out {
            return Err(KernelError::timeout(
                "provider package manager command timed out",
            ));
        }
        if output.is_success() {
            return Ok(());
        }
        Err(KernelError::provider_error(
            code,
            format!(
                "provider package manager command failed with exit code {}",
                output.exit_code
            ),
        ))
    }

    fn detect_installation_unlocked(
        &self,
        agent_id: &str,
        environment: &InstallEnvironment,
    ) -> KernelResult<AgentInstallation> {
        self.validate_agent_id(agent_id)?;
        let dependencies = self.detect_dependencies(environment)?;
        let primary_present = dependencies
            .first()
            .and_then(|dependency| dependency.installed_version.as_ref())
            .is_some();
        let any_present = dependencies
            .iter()
            .any(|dependency| dependency.installed_version.is_some());
        let complete = dependencies
            .iter()
            .all(AgentInstallationDependency::version_matches);
        let mut installation = if !any_present {
            AgentInstallation::not_installed(agent_id)
        } else if complete {
            AgentInstallation::installed(agent_id, &self.provider_version)
        } else if primary_present {
            AgentInstallation::degraded(agent_id, &self.provider_version)
        } else {
            AgentInstallation::partially_installed(agent_id)
        };
        installation.dependencies = dependencies;
        Ok(installation)
    }

    fn verify_installed_unlocked(
        &self,
        environment: &InstallEnvironment,
    ) -> KernelResult<AgentInstallation> {
        let detection = self.detect_installation_unlocked(&self.agent_id, environment)?;
        if detection.is_installed() {
            Ok(detection)
        } else {
            Err(KernelError::provider_error(
                "provider_installation_verification_failed",
                "provider dependencies did not match the exact installation descriptor",
            ))
        }
    }

    fn verify_uninstalled_unlocked(&self, environment: &InstallEnvironment) -> KernelResult<()> {
        let remaining = self.detect_dependencies(environment)?;
        if remaining
            .iter()
            .any(|dependency| dependency.installed_version.is_some())
        {
            return Err(KernelError::provider_error(
                "provider_uninstall_verification_failed",
                "one or more provider packages remain installed",
            ));
        }
        Ok(())
    }

    fn restore_dependency_snapshot(
        &self,
        snapshot: &[AgentInstallationDependency],
        environment: &InstallEnvironment,
    ) -> KernelResult<()> {
        let manager = self.validate_dependency_snapshot(snapshot)?;
        let mut restore_specs = Vec::new();
        let mut remove_ids = Vec::new();
        for dependency in snapshot {
            match &dependency.installed_version {
                Some(version) => {
                    let separator = match manager {
                        ProcessAdapterPackageManager::Npm => "@",
                        ProcessAdapterPackageManager::PythonPip => "==",
                    };
                    restore_specs
                        .push(format!("{}{}{}", dependency.package_id, separator, version));
                }
                None => remove_ids.push(dependency.package_id.clone()),
            }
        }
        if !restore_specs.is_empty() {
            self.run_exact_package_install(
                &restore_specs,
                "provider_package_rollback_install_failed",
                environment,
            )?;
        }
        if !remove_ids.is_empty() {
            self.run_package_uninstall_ids(
                &remove_ids,
                "provider_package_rollback_uninstall_failed",
                environment,
            )?;
        }
        let restored = self.detect_dependencies(environment)?;
        if dependency_versions_match(snapshot, &restored) {
            Ok(())
        } else {
            Err(KernelError::provider_error(
                "provider_package_rollback_verification_failed",
                "provider package rollback did not restore the previous dependency state",
            ))
        }
    }

    fn validate_dependency_snapshot(
        &self,
        snapshot: &[AgentInstallationDependency],
    ) -> KernelResult<ProcessAdapterPackageManager> {
        let manager = self.validate_descriptor()?;
        for dependency in snapshot {
            if let Some(version) = &dependency.installed_version {
                validate_detected_version(manager, version)?;
            }
        }
        Ok(manager)
    }

    fn compensate_failure(
        &self,
        snapshot: &[AgentInstallationDependency],
        original: KernelError,
        environment: &InstallEnvironment,
    ) -> KernelError {
        match self.restore_dependency_snapshot(snapshot, environment) {
            Ok(()) => original,
            Err(_) => KernelError::provider_error(
                "provider_package_rollback_failed",
                "provider package mutation failed and the previous state could not be restored",
            ),
        }
    }
}

fn resolve_install_root(root: &Path) -> KernelResult<PathBuf> {
    if root.as_os_str().is_empty() {
        return Err(KernelError::validation(
            "provider install root must not be empty",
        ));
    }
    let absolute = std::path::absolute(root).map_err(|_| {
        KernelError::provider_error(
            "provider_install_root_unavailable",
            "provider install root could not be resolved",
        )
    })?;
    let normalized = lexical_normalize(&absolute);
    if normalized.parent().is_none() {
        return Err(KernelError::validation(
            "provider install root must not be a filesystem root",
        ));
    }
    if normalized.exists() && !normalized.is_dir() {
        return Err(KernelError::validation(
            "provider install root must be a directory",
        ));
    }
    let mut existing_ancestor = normalized.as_path();
    loop {
        match existing_ancestor.try_exists() {
            Ok(true) => break,
            Ok(false) => {
                existing_ancestor = existing_ancestor.parent().ok_or_else(|| {
                    KernelError::provider_error(
                        "provider_install_root_unavailable",
                        "provider install root could not be resolved",
                    )
                })?;
            }
            Err(_) => {
                return Err(KernelError::provider_error(
                    "provider_install_root_unavailable",
                    "provider install root could not be resolved",
                ));
            }
        }
    }
    if !existing_ancestor.is_dir() {
        return Err(KernelError::validation(
            "provider install root must have a directory ancestor",
        ));
    }
    let canonical_ancestor = dunce::canonicalize(existing_ancestor).map_err(|_| {
        KernelError::provider_error(
            "provider_install_root_unavailable",
            "provider install root could not be resolved",
        )
    })?;
    let missing_tail = normalized.strip_prefix(existing_ancestor).map_err(|_| {
        KernelError::provider_error(
            "provider_install_root_unavailable",
            "provider install root could not be resolved",
        )
    })?;
    Ok(canonical_ancestor.join(missing_tail))
}

fn configured_provider_host_root() -> KernelResult<Option<PathBuf>> {
    for environment_key in [PROVIDER_HOST_ROOT_ENV, LEGACY_PROVIDER_RUNTIME_ROOT_ENV] {
        match std::env::var_os(environment_key) {
            Some(root) if root.is_empty() => {
                return Err(KernelError::validation(format!(
                    "{environment_key} must not be empty"
                )));
            }
            Some(root) => return Ok(Some(PathBuf::from(root))),
            None => {}
        }
    }
    Ok(None)
}

fn executable_lock_identity(program: &str) -> String {
    let path = resolve_executable_path(program).unwrap_or_else(|| {
        let path = Path::new(program);
        if path.components().count() > 1 {
            std::path::absolute(path)
                .map(|absolute| lexical_normalize(&absolute))
                .unwrap_or_else(|_| path.to_path_buf())
        } else {
            path.to_path_buf()
        }
    });
    path.to_string_lossy().into_owned()
}

fn resolve_executable_path(program: &str) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return canonical_executable(program_path);
    }

    #[cfg(windows)]
    if let Ok(current_directory) = std::env::current_dir() {
        if let Some(executable) = executable_in_directory(&current_directory, program) {
            return Some(executable);
        }
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .find_map(|directory| executable_in_directory(&directory, program))
}

fn executable_in_directory(directory: &Path, program: &str) -> Option<PathBuf> {
    let direct = directory.join(program);
    if let Some(executable) = canonical_executable(&direct) {
        return Some(executable);
    }

    #[cfg(windows)]
    if Path::new(program).extension().is_none() {
        let extensions =
            std::env::var_os("PATHEXT").unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"));
        for extension in extensions.to_string_lossy().split(';') {
            if extension.is_empty()
                || extension.contains(['/', '\\'])
                || !extension.starts_with('.')
            {
                continue;
            }
            let mut name = OsString::from(program);
            name.push(extension);
            if let Some(executable) = canonical_executable(&directory.join(name)) {
                return Some(executable);
            }
        }
    }
    None
}

fn canonical_executable(path: &Path) -> Option<PathBuf> {
    let metadata = path.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    dunce::canonicalize(path).ok()
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

/// Effective install environment for one lifecycle operation.
///
/// Request-level `AgentInstallOptions` override the installer's builder
/// defaults; values not supplied on the request defer to the installer
/// configuration and finally to environment variables and platform defaults.
/// Detection, mutation, post-verification, compensation, and rollback within a
/// single operation all use the same environment so a custom install root is
/// honored consistently across the whole lifecycle.
#[derive(Debug, Clone, Default)]
struct InstallEnvironment {
    install_root: Option<PathBuf>,
    python_binary: Option<String>,
    install_scripts_enabled: bool,
}

impl InstallEnvironment {
    fn from_installer(installer: &ProcessAdapterInstaller) -> Self {
        Self {
            install_root: installer.install_root.clone(),
            python_binary: None,
            install_scripts_enabled: installer.install_scripts_enabled,
        }
    }

    fn with_options(mut self, options: Option<&AgentInstallOptions>) -> Self {
        let Some(options) = options else {
            return self;
        };
        if let Some(install_root) = &options.install_root {
            self.install_root = Some(PathBuf::from(install_root));
        }
        if let Some(python_binary) = &options.python_binary {
            self.python_binary = Some(python_binary.clone());
        }
        if let Some(install_scripts_enabled) = options.install_scripts_enabled {
            self.install_scripts_enabled = install_scripts_enabled;
        }
        self
    }
}

impl AgentInstaller for ProcessAdapterInstaller {
    fn provider_manifest(&self) -> ProviderManifest {
        ProviderManifest::new(
            self.provider_id.clone(),
            "agent_installer",
            format!("{} package lifecycle", self.agent_id),
            self.provider_version.clone(),
            vec![
                "agent.install".to_string(),
                "agent.uninstall".to_string(),
                "agent.upgrade".to_string(),
            ],
        )
    }

    fn detect_installation(&self, agent_id: &str) -> KernelResult<AgentInstallation> {
        self.validate_agent_id(agent_id)?;
        let environment = InstallEnvironment::from_installer(self);
        self.with_detection_lock(&environment, || {
            self.detect_installation_unlocked(agent_id, &environment)
        })
    }

    fn configuration_spec(&self, agent_id: &str) -> KernelResult<AgentConfigurationSpec> {
        ProcessAdapterConfigurationProvider::new(self.agent_id.clone()).configuration_spec(agent_id)
    }

    fn plan_install(&self, request: &AgentInstallRequest) -> KernelResult<AgentInstallPlan> {
        self.validate_install_request(request)?;
        Ok(AgentInstallPlan::new(
            format!("plan.{}.install", self.agent_id),
            self.agent_id.clone(),
            request.target_version.clone(),
        )
        .add_step(AgentInstallStep::new(
            "step.verify_provider_packages",
            AgentInstallStepKind::VerifyPackage,
            "detect exact provider package versions",
        ))
        .add_step(AgentInstallStep::new(
            "step.install_provider_packages",
            AgentInstallStepKind::WriteFiles,
            "install exact provider package versions",
        ))
        .add_step(AgentInstallStep::new(
            "step.verify_installed_provider_packages",
            AgentInstallStepKind::VerifyPackage,
            "verify installed provider package versions",
        ))
        .require_policy(PolicyCategory::AgentInstall))
    }

    fn install(&self, request: AgentInstallRequest) -> KernelResult<AgentInstallReport> {
        self.plan_install(&request)?;
        if request.dry_run {
            return Ok(AgentInstallReport::planned(
                request.request_id,
                request.agent_id,
                request.target_version,
            ));
        }
        let environment =
            InstallEnvironment::from_installer(self).with_options(request.options.as_ref());
        self.with_mutation_lock(&environment, || {
            let snapshot = self.detect_dependencies(&environment)?;
            self.validate_dependency_snapshot(&snapshot)?;
            if snapshot
                .iter()
                .all(AgentInstallationDependency::version_matches)
            {
                return Ok(AgentInstallReport::installed(
                    request.request_id,
                    request.agent_id,
                    request.target_version,
                ));
            }
            if let Err(error) =
                self.run_package_install("provider_package_install_failed", &environment)
            {
                return Err(self.compensate_failure(&snapshot, error, &environment));
            }
            if let Err(error) = self.verify_installed_unlocked(&environment) {
                return Err(self.compensate_failure(&snapshot, error, &environment));
            }
            Ok(AgentInstallReport::installed(
                request.request_id,
                request.agent_id,
                request.target_version,
            ))
        })
    }

    fn plan_upgrade(&self, request: &AgentUpgradeRequest) -> KernelResult<AgentUpgradePlan> {
        self.validate_upgrade_request(request)?;
        let mut plan = AgentUpgradePlan::new(
            format!("plan.{}.upgrade", self.agent_id),
            self.agent_id.clone(),
            request.from_version.clone(),
            request.to_version.clone(),
        )
        .with_rollback_required(request.rollback_required)
        .add_step(AgentInstallStep::new(
            "step.detect_provider_version",
            AgentInstallStepKind::VerifyPackage,
            "detect current provider package versions",
        ));
        if request.rollback_required {
            plan = plan.add_step(AgentInstallStep::new(
                "step.backup_provider_version",
                AgentInstallStepKind::BackupCurrentVersion,
                "record current provider package versions for automatic rollback",
            ));
        }
        Ok(plan
            .add_step(AgentInstallStep::new(
                "step.replace_provider_packages",
                AgentInstallStepKind::ReplaceVersion,
                "replace provider packages with exact target versions",
            ))
            .add_step(AgentInstallStep::new(
                "step.verify_upgraded_provider_packages",
                AgentInstallStepKind::VerifyPackage,
                "verify upgraded provider package versions",
            ))
            .require_policy(PolicyCategory::AgentUpgrade))
    }

    fn upgrade(&self, request: AgentUpgradeRequest) -> KernelResult<AgentUpgradeReport> {
        self.plan_upgrade(&request)?;
        if request.dry_run {
            return Ok(AgentUpgradeReport::planned(
                request.request_id,
                request.agent_id,
                request.from_version,
                request.to_version,
            ));
        }
        let environment =
            InstallEnvironment::from_installer(self).with_options(request.options.as_ref());
        self.with_mutation_lock(&environment, || {
            let snapshot = self.detect_dependencies(&environment)?;
            self.validate_dependency_snapshot(&snapshot)?;
            let rollback_token = if request.rollback_required {
                Some(encode_rollback_token(&snapshot, &request.from_version))
            } else {
                None
            };
            if snapshot
                .iter()
                .all(AgentInstallationDependency::version_matches)
            {
                return Ok(AgentUpgradeReport::upgraded(
                    request.request_id,
                    request.agent_id,
                    request.from_version,
                    request.to_version,
                )
                .with_rollback_token_option(rollback_token));
            }
            if let Err(error) =
                self.run_package_install("provider_package_upgrade_failed", &environment)
            {
                return Err(self.compensate_failure(&snapshot, error, &environment));
            }
            if let Err(error) = self.verify_installed_unlocked(&environment) {
                return Err(self.compensate_failure(&snapshot, error, &environment));
            }
            Ok(AgentUpgradeReport::upgraded(
                request.request_id,
                request.agent_id,
                request.from_version,
                request.to_version,
            )
            .with_rollback_token_option(rollback_token))
        })
    }

    fn plan_uninstall(&self, request: &AgentUninstallRequest) -> KernelResult<AgentUninstallPlan> {
        validate_lifecycle_request_id(&request.request_id)?;
        self.validate_agent_id(&request.agent_id)?;
        self.validate_descriptor()?;
        if !request.preserve_data {
            return Err(KernelError::validation(
                "provider package installers do not own agent data removal",
            ));
        }
        if request.remove_configuration {
            return Err(KernelError::provider_error(
                "provider_configuration_removal_requires_host",
                "provider configuration removal must be completed by the host configuration store before package uninstall",
            ));
        }
        self.validate_install_options(request.options.as_ref())?;
        let plan = AgentUninstallPlan::new(
            format!("plan.{}.uninstall", self.agent_id),
            self.agent_id.clone(),
        )
        .add_step(AgentInstallStep::new(
            "step.remove_provider_packages",
            AgentInstallStepKind::RemoveFiles,
            "remove provider packages from the managed runtime",
        ))
        .add_step(AgentInstallStep::new(
            "step.verify_removed_provider_packages",
            AgentInstallStepKind::VerifyPackage,
            "verify provider packages are absent",
        ))
        .require_policy(PolicyCategory::AgentUninstall);
        Ok(plan)
    }

    fn uninstall(&self, request: AgentUninstallRequest) -> KernelResult<AgentUninstallReport> {
        self.plan_uninstall(&request)?;
        if request.dry_run {
            return Ok(AgentUninstallReport::planned(
                request.request_id,
                request.agent_id,
            ));
        }
        let environment =
            InstallEnvironment::from_installer(self).with_options(request.options.as_ref());
        self.with_mutation_lock(&environment, || {
            let snapshot = self.detect_dependencies(&environment)?;
            self.validate_dependency_snapshot(&snapshot)?;
            if snapshot
                .iter()
                .all(|dependency| dependency.installed_version.is_none())
            {
                return Ok(
                    AgentUninstallReport::uninstalled(request.request_id, request.agent_id)
                        .with_configuration_removed(false),
                );
            }
            if let Err(error) =
                self.run_package_uninstall("provider_package_uninstall_failed", &environment)
            {
                return Err(self.compensate_failure(&snapshot, error, &environment));
            }
            if let Err(error) = self.verify_uninstalled_unlocked(&environment) {
                return Err(self.compensate_failure(&snapshot, error, &environment));
            }
            Ok(
                AgentUninstallReport::uninstalled(request.request_id, request.agent_id)
                    .with_configuration_removed(false),
            )
        })
    }

    fn rollback(&self, request: AgentRollbackRequest) -> KernelResult<AgentRollbackReport> {
        validate_lifecycle_request_id(&request.request_id)?;
        self.validate_agent_id(&request.agent_id)?;
        self.validate_descriptor()?;
        if !request.preserve_data {
            return Err(KernelError::provider_error(
                "provider_data_removal_requires_host",
                "provider package installers do not own agent data removal",
            ));
        }
        self.validate_install_options(request.options.as_ref())?;
        let token = request.rollback_token.as_deref().ok_or_else(|| {
            KernelError::validation(
                "provider rollback requires the rollback token returned by the upgrade report",
            )
        })?;
        let snapshot = decode_rollback_token(token)?;
        self.validate_dependency_snapshot(&snapshot.packages)?;
        self.snapshot_matches_descriptor(&snapshot.packages)?;
        if let Some(target_version) = &request.target_version {
            if target_version != &snapshot.provider_version {
                return Err(KernelError::validation(format!(
                    "provider rollback target {target_version} does not match the snapshotted version {}",
                    snapshot.provider_version
                )));
            }
        }
        let environment =
            InstallEnvironment::from_installer(self).with_options(request.options.as_ref());
        self.with_mutation_lock(&environment, || {
            self.restore_dependency_snapshot(&snapshot.packages, &environment)?;
            Ok(AgentRollbackReport::success(
                request.request_id,
                request.agent_id,
                self.provider_version.clone(),
                snapshot.provider_version.clone(),
            ))
        })
    }

    fn list_installed(&self) -> KernelResult<Vec<AgentInstallRecord>> {
        let environment = InstallEnvironment::from_installer(self);
        self.with_detection_lock(&environment, || {
            let installation = self.detect_installation_unlocked(&self.agent_id, &environment)?;
            let registry_id = match self.validate_descriptor()? {
                ProcessAdapterPackageManager::Npm => "npm",
                ProcessAdapterPackageManager::PythonPip => "pypi",
            };
            let source = AgentPackageSourceInfo::Registry {
                registry_id: registry_id.to_string(),
                package_id: self.packages[0].package_id.clone(),
            };
            Ok(
                <ProcessAdapterInstaller as AgentInstaller>::record_from_detection(
                    &installation,
                    &self.provider_version,
                    source,
                ),
            )
        })
    }

    fn available_upgrade(&self, agent_id: &str) -> KernelResult<AgentAvailableUpgrade> {
        self.validate_agent_id(agent_id)?;
        let environment = InstallEnvironment::from_installer(self);
        // Compare against the actually detected package version (the primary
        // managed dependency), not the provider manifest version: the registry
        // reports package versions while `AgentInstallation::installed_version`
        // carries the provider version. Mixing the two bases would make every
        // registry query look like an upgrade.
        let current_version = self
            .detect_installation_unlocked(agent_id, &environment)?
            .dependencies
            .first()
            .and_then(|dependency| dependency.installed_version.clone());
        let latest_version = match self.validate_descriptor()? {
            ProcessAdapterPackageManager::Npm => self.fetch_npm_latest_version()?,
            ProcessAdapterPackageManager::PythonPip => self.fetch_pypi_latest_version()?,
        };
        // Strict semantic-version ordering: a local pre-release build that
        // outranks `latest` (or an unparseable version) never reports a false
        // upgrade prompt.
        let update_available = match (&current_version, &latest_version) {
            (Some(current), Some(latest)) => strict_semver_greater(latest, current),
            _ => false,
        };
        let mut upgrade =
            AgentAvailableUpgrade::new(agent_id).with_update_available(update_available);
        if let Some(current) = current_version {
            upgrade = upgrade.with_current_version(current);
        }
        if let Some(latest) = latest_version {
            upgrade = upgrade.with_latest_version(latest);
        }
        Ok(upgrade)
    }

    fn health(&self) -> ProviderHealth {
        let manager = match self.validate_descriptor() {
            Ok(manager) => manager,
            Err(_) => {
                return ProviderHealth {
                    status: "unhealthy".to_string(),
                }
            }
        };
        let command = match manager {
            ProcessAdapterPackageManager::Npm => {
                ProcessAdapterCommand::new("npm", vec!["--version".to_string()])
            }
            ProcessAdapterPackageManager::PythonPip => {
                let environment = InstallEnvironment::from_installer(self);
                let binary = match self.resolve_python_binary(&environment) {
                    Ok(binary) => binary,
                    Err(_) => {
                        return ProviderHealth {
                            status: "unhealthy".to_string(),
                        }
                    }
                };
                ProcessAdapterCommand::new(
                    binary,
                    vec!["-m".to_string(), "pip".to_string(), "--version".to_string()],
                )
            }
        };
        match self
            .executor
            .execute(&command.with_timeout(HEALTH_COMMAND_TIMEOUT))
        {
            Ok(output) if output.is_success() => ProviderHealth::available(),
            _ => ProviderHealth {
                status: "degraded".to_string(),
            },
        }
    }
}

impl ProcessAdapterInstaller {
    fn snapshot_matches_descriptor(
        &self,
        snapshot: &[AgentInstallationDependency],
    ) -> KernelResult<()> {
        let expected: HashSet<(&str, &str)> = self
            .packages
            .iter()
            .map(|package| (package.registry_id.as_str(), package.package_id.as_str()))
            .collect();
        let actual: HashSet<(&str, &str)> = snapshot
            .iter()
            .map(|dependency| {
                (
                    dependency.registry_id.as_str(),
                    dependency.package_id.as_str(),
                )
            })
            .collect();
        if expected == actual {
            Ok(())
        } else {
            Err(KernelError::validation(
                "provider rollback snapshot does not match the managed package descriptor",
            ))
        }
    }

    /// Queries the primary package's npm `dist-tags` in a single `npm view`
    /// invocation and returns the `latest` tag. A failed, timed-out, or
    /// unparseable query reports `None` (no upgrade information) rather than
    /// an error so hosts can degrade gracefully when the registry is
    /// unreachable.
    fn fetch_npm_latest_version(&self) -> KernelResult<Option<String>> {
        let primary = &self.packages[0];
        let output = self.executor.execute(
            &ProcessAdapterCommand::new(
                "npm",
                vec![
                    "view".to_string(),
                    primary.package_id.clone(),
                    "dist-tags".to_string(),
                    "--json".to_string(),
                ],
            )
            .with_timeout(REGISTRY_QUERY_TIMEOUT),
        )?;
        if output.timed_out || !output.is_success() {
            return Ok(None);
        }
        let payload: Value = match serde_json::from_str(&output.stdout) {
            Ok(payload) => payload,
            Err(_) => return Ok(None),
        };
        Ok(payload
            .get("latest")
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// Queries PyPI through the managed Python's `pip index versions` and
    /// returns the highest parseable semantic version. A failed, timed-out,
    /// or unparseable query reports `None` (no upgrade information).
    fn fetch_pypi_latest_version(&self) -> KernelResult<Option<String>> {
        let environment = InstallEnvironment::from_installer(self);
        let primary = &self.packages[0];
        let output = self.executor.execute(
            &ProcessAdapterCommand::new(
                self.resolve_python_binary(&environment)?,
                vec![
                    "-m".to_string(),
                    "pip".to_string(),
                    "index".to_string(),
                    "versions".to_string(),
                    primary.package_id.clone(),
                ],
            )
            .with_timeout(REGISTRY_QUERY_TIMEOUT),
        )?;
        if output.timed_out || !output.is_success() {
            return Ok(None);
        }
        Ok(parse_pip_index_versions(&output.stdout))
    }
}

/// Strict semantic-version ordering: `candidate` is newer than `baseline`
/// only when both parse as valid semantic versions and the candidate is
/// strictly greater. Pre-release ordering follows semver (`1.0.0-beta <
/// 1.0.0`), so a local pre-release build that outranks `latest` never
/// reports a false upgrade.
fn strict_semver_greater(candidate: &str, baseline: &str) -> bool {
    match (Version::parse(candidate), Version::parse(baseline)) {
        (Ok(candidate), Ok(baseline)) => candidate > baseline,
        _ => false,
    }
}

/// Extracts the highest parseable semantic version from `pip index versions`
/// output:
///
/// ```text
/// hermes-agent (0.20.0)
/// Available versions: 0.19.0, 0.18.0, ...
/// ```
fn parse_pip_index_versions(stdout: &str) -> Option<String> {
    let line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with("Available versions:"))?;
    let versions = line.split_once(':')?.1;
    versions
        .split(',')
        .filter_map(|candidate| {
            let candidate = candidate.trim();
            Version::parse(candidate)
                .ok()
                .map(|parsed| (parsed, candidate.to_string()))
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, version)| version)
}

fn validate_detected_version(
    manager: ProcessAdapterPackageManager,
    version: &str,
) -> KernelResult<()> {
    let valid = match manager {
        ProcessAdapterPackageManager::Npm => Version::parse(version).is_ok(),
        ProcessAdapterPackageManager::PythonPip => is_exact_python_version(version),
    };
    if valid {
        Ok(())
    } else {
        Err(KernelError::provider_error(
            "provider_package_snapshot_version_invalid",
            "detected provider package version cannot be restored safely",
        ))
    }
}

fn is_exact_semantic_version(version: &str) -> bool {
    if version.is_empty() || version.len() > 128 || version.trim() != version {
        return false;
    }
    Version::parse(version).is_ok_and(|parsed| parsed.to_string() == version)
}

fn validate_lifecycle_request_id(request_id: &str) -> KernelResult<()> {
    if request_id.trim().is_empty()
        || request_id.trim() != request_id
        || request_id.chars().count() > MAX_REQUEST_ID_CHARACTERS
        || request_id.chars().any(char::is_control)
    {
        return Err(KernelError::validation(
            "provider lifecycle request id must be a non-empty bounded identifier",
        ));
    }
    Ok(())
}

fn dependency_versions_match(
    expected: &[AgentInstallationDependency],
    actual: &[AgentInstallationDependency],
) -> bool {
    expected.len() == actual.len()
        && expected.iter().zip(actual).all(|(expected, actual)| {
            expected.registry_id == actual.registry_id
                && expected.package_id == actual.package_id
                && expected.installed_version == actual.installed_version
        })
}

fn dependency_detection(
    package: &ProcessAdapterPackage,
    installed_version: Option<&str>,
) -> AgentInstallationDependency {
    match installed_version {
        Some(version) => AgentInstallationDependency::installed(
            &package.registry_id,
            &package.package_id,
            &package.version,
            version,
        ),
        None => AgentInstallationDependency::missing(
            &package.registry_id,
            &package.package_id,
            &package.version,
        ),
    }
}

/// Opaque upgrade rollback handle payload.
///
/// The handle is a hex-encoded JSON document carrying only the pre-upgrade
/// provider version and the exact managed package snapshot. It is opaque to
/// hosts, never enters lifecycle events, and is re-validated against the
/// installer descriptor before any package-manager mutation.
struct RollbackSnapshot {
    provider_version: String,
    packages: Vec<AgentInstallationDependency>,
}

fn encode_rollback_token(
    snapshot: &[AgentInstallationDependency],
    provider_version: &str,
) -> String {
    let packages: Vec<Value> = snapshot
        .iter()
        .map(|dependency| {
            serde_json::json!({
                "registry_id": dependency.registry_id,
                "package_id": dependency.package_id,
                "expected_version": dependency.expected_version,
                "installed_version": dependency.installed_version,
            })
        })
        .collect();
    let payload = serde_json::json!({
        "version": ROLLBACK_TOKEN_SCHEMA_VERSION,
        "provider_version": provider_version,
        "packages": packages,
    });
    let json = serde_json::to_string(&payload)
        .expect("rollback snapshot serializes into a bounded JSON document");
    hex_encode(json.as_bytes())
}

fn decode_rollback_token(token: &str) -> KernelResult<RollbackSnapshot> {
    if token.len() > MAX_ROLLBACK_TOKEN_CHARACTERS || token.trim() != token {
        return Err(KernelError::validation(
            "provider rollback token is malformed",
        ));
    }
    let bytes = hex_decode(token).ok_or_else(|| {
        KernelError::validation("provider rollback token is not valid hexadecimal")
    })?;
    let payload: Value = serde_json::from_slice(&bytes).map_err(|_| {
        KernelError::validation("provider rollback token does not carry a snapshot payload")
    })?;
    let version = payload
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            KernelError::validation("provider rollback token omits its schema version")
        })?;
    if version != ROLLBACK_TOKEN_SCHEMA_VERSION {
        return Err(KernelError::validation(format!(
            "provider rollback token schema version {version} is not supported"
        )));
    }
    let provider_version = payload
        .get("provider_version")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            KernelError::validation(
                "provider rollback token omits the snapshotted provider version",
            )
        })?;
    let packages = payload
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            KernelError::validation("provider rollback token omits the managed package snapshot")
        })?;
    let packages = packages
        .iter()
        .map(|package| {
            let registry_id = package.get("registry_id").and_then(Value::as_str);
            let package_id = package.get("package_id").and_then(Value::as_str);
            match (registry_id, package_id) {
                (Some(registry_id), Some(package_id)) => Ok(AgentInstallationDependency {
                    registry_id: registry_id.to_string(),
                    package_id: package_id.to_string(),
                    expected_version: package
                        .get("expected_version")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    installed_version: package
                        .get("installed_version")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                }),
                _ => Err(KernelError::validation(
                    "provider rollback token carries a malformed package entry",
                )),
            }
        })
        .collect::<KernelResult<Vec<_>>>()?;
    Ok(RollbackSnapshot {
        provider_version: provider_version.to_string(),
        packages,
    })
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

fn hex_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX_DIGITS[(byte >> 4) as usize] as char);
        encoded.push(HEX_DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn hex_decode(encoded: &str) -> Option<Vec<u8>> {
    if !encoded.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(encoded.len() / 2);
    let mut characters = encoded.chars();
    while let (Some(high), Some(low)) = (characters.next(), characters.next()) {
        bytes.push((hex_nibble(high)? << 4) | hex_nibble(low)?);
    }
    Some(bytes)
}

fn hex_nibble(character: char) -> Option<u8> {
    match character {
        '0'..='9' => Some(character as u8 - b'0'),
        'a'..='f' => Some(character as u8 - b'a' + 10),
        'A'..='F' => Some(character as u8 - b'A' + 10),
        _ => None,
    }
}

fn find_packaged_provider_host_root() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    find_packaged_provider_host_root_from(executable.parent()?)
}

fn find_packaged_provider_host_root_from(start_directory: &Path) -> Option<PathBuf> {
    for directory_name in ["provider-host", "provider-runtime"] {
        let mut ancestors = Some(start_directory);
        while let Some(directory) = ancestors {
            let candidates = [
                directory.join(directory_name),
                directory.join("resources").join(directory_name),
                directory.join("Resources").join(directory_name),
            ];
            if let Some(candidate) = candidates.into_iter().find(|path| {
                path.join("workers")
                    .join("generic-ts-sdk-worker.mjs")
                    .is_file()
            }) {
                return Some(candidate);
            }
            ancestors = directory.parent();
        }
    }
    None
}

/// Minimal configuration surface for external process-adapter agents.
#[derive(Debug, Clone)]
pub struct ProcessAdapterConfigurationProvider {
    agent_id: String,
    model_mapping: AgentModelConfigurationFieldMapping,
}

impl ProcessAdapterConfigurationProvider {
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            model_mapping: AgentModelConfigurationFieldMapping::namespaced("process_adapter"),
        }
    }

    pub fn with_model_configuration_scope(
        agent_id: impl Into<String>,
        provider_scope: impl Into<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            model_mapping: AgentModelConfigurationFieldMapping::namespaced(provider_scope),
        }
    }

    pub fn spec_for(agent_id: &str) -> AgentConfigurationSpec {
        Self::spec_for_mapping(
            agent_id,
            &AgentModelConfigurationFieldMapping::namespaced("process_adapter"),
        )
    }

    fn spec_for_mapping(
        agent_id: &str,
        mapping: &AgentModelConfigurationFieldMapping,
    ) -> AgentConfigurationSpec {
        AgentConfigurationSpec::new(agent_id)
            .add_section(
                AgentConfigSection::base("base", "Base").add_field(
                    AgentConfigField::text("agent.display_name", "Display name").required(),
                ),
            )
            .add_section(
                AgentConfigSection::llm_api_key("model", "Model")
                    .add_field(AgentConfigField::text(&mapping.vendor_key, "Model vendor"))
                    .add_field(AgentConfigField::text(
                        &mapping.base_url_key,
                        "Model API base URL",
                    ))
                    .add_field(AgentConfigField::llm_api_key(
                        &mapping.api_key_key,
                        "Model provider API key",
                    ))
                    .add_field(AgentConfigField::text(
                        &mapping.default_model_key,
                        "Default model",
                    ))
                    .add_field(AgentConfigField::new(
                        &mapping.supported_models_key,
                        "Supported models",
                        AgentConfigValueKind::StringList,
                    ))
                    .add_field(AgentConfigField::new(
                        &mapping.input_context_tokens_key,
                        "Input context tokens",
                        AgentConfigValueKind::Integer,
                    ))
                    .add_field(AgentConfigField::new(
                        &mapping.output_context_tokens_key,
                        "Output context tokens",
                        AgentConfigValueKind::Integer,
                    ))
                    .add_field(AgentConfigField::new(
                        &mapping.tool_call_rounds_key,
                        "Tool call rounds",
                        AgentConfigValueKind::Integer,
                    ))
                    .add_field(
                        AgentConfigField::new(
                            &mapping.supports_multimodal_key,
                            "Supports multimodal input",
                            AgentConfigValueKind::Boolean,
                        )
                        .with_default(AgentConfigValue::boolean(false)),
                    ),
            )
            .add_section(
                AgentConfigSection::new("runtime", "Runtime", AgentConfigSectionKind::Runtime)
                    .add_field(
                        AgentConfigField::text("runtime.external.backend", "External backend")
                            .required()
                            .with_default(AgentConfigValue::string("process_adapter")),
                    ),
            )
            .add_section(
                AgentConfigSection::new("security", "Security", AgentConfigSectionKind::Security)
                    .add_field(
                        AgentConfigField::new(
                            "security.fail_closed",
                            "Fail closed",
                            AgentConfigValueKind::String,
                        )
                        .required()
                        .with_default(AgentConfigValue::string("true"))
                        .with_redaction(KernelEventRedaction::Internal),
                    ),
            )
    }
}

impl AgentConfigurationProvider for ProcessAdapterConfigurationProvider {
    fn configuration_spec(&self, agent_id: &str) -> KernelResult<AgentConfigurationSpec> {
        if agent_id != self.agent_id {
            return Err(KernelError::CapabilityMissing {
                capability_id: agent_id.to_string(),
            });
        }

        Ok(Self::spec_for_mapping(agent_id, &self.model_mapping))
    }

    fn validate_configuration(
        &self,
        configuration: &AgentConfiguration,
    ) -> KernelResult<AgentConfigurationValidation> {
        Ok(Self::spec_for_mapping(&self.agent_id, &self.model_mapping).validate(configuration))
    }

    fn apply_model_configuration(
        &self,
        request: &AgentModelConfigurationRequest,
    ) -> KernelResult<AgentModelConfigurationApplication> {
        if request.agent_id != self.agent_id {
            return Err(KernelError::CapabilityMissing {
                capability_id: request.agent_id.clone(),
            });
        }
        request.validate()?;

        let mapping = &self.model_mapping;
        let mut configuration = AgentConfiguration::new(&request.agent_id, &request.profile_id)
            .set(
                "agent.display_name",
                AgentConfigValue::string(&request.agent_id),
            )
            .set(
                &mapping.vendor_key,
                AgentConfigValue::string(request.vendor_code.trim()),
            )
            .set(
                &mapping.base_url_key,
                AgentConfigValue::string(request.base_url.trim()),
            )
            .set(
                &mapping.api_key_key,
                AgentConfigValue::secret_ref(request.api_key_secret_ref.trim()),
            )
            .set(
                &mapping.default_model_key,
                AgentConfigValue::string(request.default_model_id.trim()),
            )
            .set(
                &mapping.supported_models_key,
                AgentConfigValue::string_list(
                    request
                        .supported_model_ids
                        .iter()
                        .map(|model_id| model_id.trim().to_string())
                        .collect(),
                ),
            )
            .set(
                &mapping.supports_multimodal_key,
                AgentConfigValue::boolean(request.supports_multimodal),
            )
            .set(
                "runtime.external.backend",
                AgentConfigValue::string("process_adapter"),
            )
            .set("security.fail_closed", AgentConfigValue::string("true"));

        for (key, value) in [
            (
                &mapping.input_context_tokens_key,
                request.input_context_tokens,
            ),
            (
                &mapping.output_context_tokens_key,
                request.output_context_tokens,
            ),
            (&mapping.tool_call_rounds_key, request.tool_call_rounds),
        ] {
            if let Some(value) = value {
                configuration = configuration.set(key, AgentConfigValue::integer(value));
            }
        }

        let profile = sdkwork_agent_kernel::AgentConfigurationProfile::new(
            &request.profile_id,
            &request.agent_id,
            "0.2.0",
            configuration,
        )
        .add_secret_binding(AgentSecretBinding::llm_api_key(
            &mapping.api_key_key,
            request.vendor_code.trim(),
            request.api_key_secret_ref.trim(),
        ))
        .activate();
        let validation = profile.validate_against(&Self::spec_for_mapping(&self.agent_id, mapping));
        if !validation.is_valid() {
            return Err(KernelError::validation(
                "provider model configuration does not satisfy the configuration schema",
            ));
        }

        let application = AgentModelConfigurationApplication::new(
            &request.request_id,
            &mapping.provider_scope,
            profile,
        );
        // Per-provider wrappers materialize the applied configuration into the
        // external CLI's native config surface (config file, env file, settings
        // store) so the CLI actually uses the applied base URL, credential, and
        // model at request time. The base implementation is a no-op.
        self.materialize_model_configuration(request, &application)?;
        Ok(application)
    }

    fn apply_model_selection(
        &self,
        request: &AgentModelSelectionRequest,
    ) -> KernelResult<AgentModelConfigurationApplication> {
        if request.agent_id != self.agent_id {
            return Err(KernelError::CapabilityMissing {
                capability_id: request.agent_id.clone(),
            });
        }
        request.validate()?;

        let mapping = &self.model_mapping;
        let (mut configuration, configuration_version, secret_bindings) =
            if let Some(profile) = &request.current_profile {
                (
                    profile.configuration.clone(),
                    profile.configuration_version.clone(),
                    profile.secret_bindings.clone(),
                )
            } else {
                let provider_default_secret_ref =
                    format!("provider-default.{}.model.api_key", mapping.provider_scope);
                (
                    AgentConfiguration::new(&request.agent_id, &request.profile_id)
                        .set(
                            "agent.display_name",
                            AgentConfigValue::string(&request.agent_id),
                        )
                        .set(
                            &mapping.api_key_key,
                            AgentConfigValue::secret_ref(&provider_default_secret_ref),
                        )
                        .set(
                            "runtime.external.backend",
                            AgentConfigValue::string("process_adapter"),
                        )
                        .set("security.fail_closed", AgentConfigValue::string("true")),
                    "0.2.0".to_string(),
                    vec![AgentSecretBinding::llm_api_key(
                        &mapping.api_key_key,
                        &mapping.provider_scope,
                        provider_default_secret_ref,
                    )],
                )
            };

        let mut supported_model_ids = match configuration.value(&mapping.supported_models_key) {
            Some(AgentConfigValue::StringList(model_ids)) => model_ids.clone(),
            Some(_) => {
                return Err(KernelError::validation(
                    "provider supported model configuration has an invalid value kind",
                ))
            }
            None => Vec::new(),
        };
        let selected_model_id = request.model_id.trim();
        let model_is_supported = supported_model_ids
            .iter()
            .any(|model_id| model_id.trim() == selected_model_id);
        if request.enforce_supported_models && !model_is_supported {
            return Err(KernelError::validation(
                "selected model is not included in the configured supported models",
            ));
        }
        if !model_is_supported {
            supported_model_ids.push(selected_model_id.to_string());
        }

        configuration = configuration
            .set(
                &mapping.default_model_key,
                AgentConfigValue::string(selected_model_id),
            )
            .set(
                &mapping.supported_models_key,
                AgentConfigValue::string_list(supported_model_ids),
            );
        let mut profile = sdkwork_agent_kernel::AgentConfigurationProfile::new(
            &request.profile_id,
            &request.agent_id,
            configuration_version,
            configuration,
        )
        .activate();
        for binding in secret_bindings {
            profile = profile.add_secret_binding(binding);
        }
        if !profile
            .validate_against(&Self::spec_for_mapping(&self.agent_id, mapping))
            .is_valid()
        {
            return Err(KernelError::validation(
                "provider model selection does not satisfy the configuration schema",
            ));
        }

        let application = AgentModelConfigurationApplication::new(
            &request.request_id,
            &mapping.provider_scope,
            profile,
        );
        // Per-provider wrappers materialize the selected model into the
        // external CLI's native config surface. The base implementation is a
        // no-op (most providers receive the model id per turn).
        self.materialize_model_selection(request, &application)?;
        Ok(application)
    }

    fn health(&self) -> ProviderHealth {
        ProviderHealth::available()
    }
}

#[cfg(test)]
mod tests;
