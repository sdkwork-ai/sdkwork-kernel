//! Fail-closed policy for mock/stub provider responses in production profiles.

use sdkwork_agent_kernel::{
    is_production_kernel_profile_from_env, mock_provider_invocation_allowed_from_env, KernelError,
    KernelResult, ModelResponse, ModelStreamChunk, ToolResult,
};
use serde_json::Value;

pub fn kernel_profile_id() -> Option<String> {
    sdkwork_agent_kernel::kernel_profile_id_from_env()
}

pub fn is_production_kernel_profile() -> bool {
    is_production_kernel_profile_from_env()
}

pub fn mock_provider_invocation_allowed() -> bool {
    mock_provider_invocation_allowed_from_env()
}

pub fn reject_direct_mock_provider_invocation(operation: &str) -> KernelResult<()> {
    if mock_provider_invocation_allowed() {
        return Ok(());
    }

    Err(KernelError::ProviderUnavailable {
        provider_id: operation.to_string(),
    })
}

/// Model invoke must run through the provider transport worker, not in-process stubs.
pub fn reject_in_process_model_invoke(provider_id: &str) -> KernelResult<ModelResponse> {
    reject_direct_mock_provider_invocation(&format!("{provider_id}.invoke"))?;
    Err(KernelError::ProviderUnavailable {
        provider_id: provider_id.to_string(),
    })
}

/// Model streaming must run through the provider transport worker, not in-process stubs.
pub fn reject_in_process_model_stream(provider_id: &str) -> KernelResult<Vec<ModelStreamChunk>> {
    reject_direct_mock_provider_invocation(&format!("{provider_id}.stream"))?;
    Err(KernelError::ProviderUnavailable {
        provider_id: provider_id.to_string(),
    })
}

/// Tool invocation must run through the provider transport worker, not in-process stubs.
pub fn reject_in_process_tool_invoke(provider_id: &str) -> KernelResult<ToolResult> {
    reject_direct_mock_provider_invocation(&format!("{provider_id}.tool.invoke"))?;
    Err(KernelError::ProviderUnavailable {
        provider_id: provider_id.to_string(),
    })
}

pub fn validate_runtime_model_payload(payload: &Value) -> Result<(), String> {
    if mock_provider_invocation_allowed() {
        return Ok(());
    }

    if payload
        .get("mode")
        .and_then(Value::as_str)
        .is_some_and(|mode| mode == "stub" || mode == "sdk_probe" || mode == "sdk_live_failed")
    {
        return Err(format!(
            "runtime backend mode {:?} is not allowed in production profile",
            payload.get("mode")
        ));
    }

    if payload.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("runtime invoke returned ok=false")
            .to_string());
    }

    if let Some(messages) = payload.get("messages").and_then(Value::as_array) {
        for message in messages {
            if let Some(text) = message.as_str() {
                if is_mock_response_text(text) {
                    return Err(
                        "runtime returned mock provider text in production profile".to_string()
                    );
                }
            }
        }
    }

    Ok(())
}

pub fn is_mock_response_text(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    normalized.contains("mock response")
        || normalized.contains(" stub]")
        || normalized.contains("streaming mock")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdkwork_agent_kernel::{
        ALLOW_MOCK_PROVIDERS_ENV, KERNEL_ENVIRONMENT_ENV, KERNEL_PROFILE_ID_ENV,
    };
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("mock policy env lock")
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
    fn production_topology_profile_rejects_mock_invocation() {
        let _lock = env_lock();
        let _profile = EnvVarGuard::set(KERNEL_PROFILE_ID_ENV, Some("cloud.production"));
        let _environment = EnvVarGuard::set(KERNEL_ENVIRONMENT_ENV, None);
        let _allow = EnvVarGuard::set(ALLOW_MOCK_PROVIDERS_ENV, None);

        assert!(!mock_provider_invocation_allowed());
        assert!(reject_direct_mock_provider_invocation("model.invoke").is_err());
    }

    #[test]
    fn production_environment_rejects_mock_invocation() {
        let _lock = env_lock();
        let _profile = EnvVarGuard::set(KERNEL_PROFILE_ID_ENV, None);
        let _environment = EnvVarGuard::set(KERNEL_ENVIRONMENT_ENV, Some("production"));
        let _allow = EnvVarGuard::set(ALLOW_MOCK_PROVIDERS_ENV, None);

        assert!(!mock_provider_invocation_allowed());
    }

    #[test]
    fn production_profile_rejects_mock_override_at_kernel_level() {
        // Kernel-level production gate is unconditional; the env override is
        // development-only (see runtime_topology::mock_provider_invocation_allowed).
        let _lock = env_lock();
        let _profile = EnvVarGuard::set(KERNEL_PROFILE_ID_ENV, Some("standalone.production"));
        let _environment = EnvVarGuard::set(KERNEL_ENVIRONMENT_ENV, Some("production"));
        let _allow = EnvVarGuard::set(ALLOW_MOCK_PROVIDERS_ENV, Some("1"));

        assert!(!mock_provider_invocation_allowed());
    }

    #[test]
    fn rejects_stub_runtime_payload_in_production_profile() {
        let _lock = env_lock();
        let _profile = EnvVarGuard::set(KERNEL_PROFILE_ID_ENV, Some("cloud.production"));
        let _environment = EnvVarGuard::set(KERNEL_ENVIRONMENT_ENV, Some("production"));
        let _allow = EnvVarGuard::set(ALLOW_MOCK_PROVIDERS_ENV, None);

        let payload = serde_json::json!({
            "mode": "stub",
            "messages": ["[pkg stub] hello"]
        });
        assert!(validate_runtime_model_payload(&payload).is_err());
    }

    #[test]
    fn allows_sdk_live_runtime_payload_in_production_profile() {
        let _lock = env_lock();
        let _profile = EnvVarGuard::set(KERNEL_PROFILE_ID_ENV, Some("cloud.production"));
        let _environment = EnvVarGuard::set(KERNEL_ENVIRONMENT_ENV, Some("production"));
        let _allow = EnvVarGuard::set(ALLOW_MOCK_PROVIDERS_ENV, None);

        let payload = serde_json::json!({
            "mode": "sdk_live",
            "messages": ["official sdk response"]
        });
        assert!(validate_runtime_model_payload(&payload).is_ok());
    }

    #[test]
    fn in_process_tool_invoke_rejects_without_runtime_worker() {
        let _lock = env_lock();
        let _profile = EnvVarGuard::set(KERNEL_PROFILE_ID_ENV, None);
        let _environment = EnvVarGuard::set(KERNEL_ENVIRONMENT_ENV, None);
        let _allow = EnvVarGuard::set(ALLOW_MOCK_PROVIDERS_ENV, None);

        let error = reject_in_process_tool_invoke("provider.tool.codex")
            .expect_err("direct tool invocation must fail closed");

        // The fail-closed guard reports the fully qualified operation id
        // (`{provider_id}.tool.invoke`), matching `reject_direct_mock_provider_invocation`.
        assert!(matches!(
            error,
            KernelError::ProviderUnavailable { provider_id } if provider_id == "provider.tool.codex.tool.invoke"
        ));
    }
}
