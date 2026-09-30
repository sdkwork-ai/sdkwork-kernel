//! Topology profile detection and fail-closed mock provider policy.
//!
//! Canonical source for production profile classification shared by
//! `sdkwork-agent-server`, adapter plugins, and SDK backend workers.

pub const KERNEL_PROFILE_ID_ENV: &str = "SDKWORK_KERNEL_PROFILE_ID";
pub const KERNEL_ENVIRONMENT_ENV: &str = "SDKWORK_KERNEL_ENVIRONMENT";
pub const ALLOW_MOCK_PROVIDERS_ENV: &str = "SDKWORK_KERNEL_ALLOW_MOCK_PROVIDERS";

pub fn normalize_kernel_profile_id(profile_id: &str) -> Option<String> {
    let normalized = profile_id.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

pub fn kernel_profile_id_from_env() -> Option<String> {
    std::env::var(KERNEL_PROFILE_ID_ENV)
        .ok()
        .and_then(|value| normalize_kernel_profile_id(&value))
}

pub fn is_production_kernel_profile(environment: &str, profile_id: Option<&str>) -> bool {
    if environment.eq_ignore_ascii_case("production") {
        return true;
    }

    profile_id
        .and_then(normalize_kernel_profile_id)
        .is_some_and(|profile| profile.ends_with(".production"))
}

pub fn is_production_kernel_profile_from_env() -> bool {
    let environment = std::env::var(KERNEL_ENVIRONMENT_ENV).unwrap_or_default();
    is_production_kernel_profile(&environment, kernel_profile_id_from_env().as_deref())
}

pub fn mock_provider_override_enabled_from_env() -> bool {
    std::env::var(ALLOW_MOCK_PROVIDERS_ENV)
        .ok()
        .is_some_and(|value| matches_env_truthy(&value))
}

pub fn mock_provider_override_disabled_from_env() -> bool {
    std::env::var(ALLOW_MOCK_PROVIDERS_ENV)
        .ok()
        .is_some_and(|value| matches_env_falsy(&value))
}

pub fn mock_provider_invocation_allowed(environment: &str, profile_id: Option<&str>) -> bool {
    // Production is an unconditional fail-closed gate at the kernel level:
    // the `SDKWORK_KERNEL_ALLOW_MOCK_PROVIDERS` override exists for
    // development environments only. Production deployments of embedded
    // runtimes (no server preflight in front) must not be able to flip into
    // mock responses by environment variable.
    if is_production_kernel_profile(environment, profile_id) {
        return false;
    }

    if mock_provider_override_disabled_from_env() {
        return false;
    }

    if mock_provider_override_enabled_from_env() {
        return true;
    }

    // Fail closed: mock providers are only ever enabled by the explicit
    // `SDKWORK_KERNEL_ALLOW_MOCK_PROVIDERS=1` override. A debug build must
    // not silently accept mock responses when it is deployed to a
    // non-production environment without the override.
    false
}

pub fn mock_provider_invocation_allowed_from_env() -> bool {
    let environment = std::env::var(KERNEL_ENVIRONMENT_ENV).unwrap_or_default();
    mock_provider_invocation_allowed(&environment, kernel_profile_id_from_env().as_deref())
}

fn matches_env_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn matches_env_falsy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("runtime topology env lock")
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
    fn production_profile_id_suffix_is_production() {
        assert!(is_production_kernel_profile(
            "development",
            Some("cloud.production")
        ));
    }

    #[test]
    fn production_environment_is_production() {
        assert!(is_production_kernel_profile("production", None));
    }

    #[test]
    fn development_profile_is_not_production() {
        assert!(!is_production_kernel_profile(
            "development",
            Some("standalone.development")
        ));
    }

    #[test]
    fn production_profile_rejects_mock_without_override() {
        let _lock = env_lock();
        let _allow = EnvVarGuard::set(ALLOW_MOCK_PROVIDERS_ENV, None);
        assert!(!mock_provider_invocation_allowed(
            "production",
            Some("cloud.production")
        ));
    }

    #[test]
    fn production_profile_rejects_mock_override_at_kernel_level() {
        // Production is an unconditional fail-closed gate: the env override
        // exists for development profiles only, so embedded runtimes without
        // a server preflight cannot be flipped into mock responses.
        let _lock = env_lock();
        let _allow = EnvVarGuard::set(ALLOW_MOCK_PROVIDERS_ENV, Some("1"));
        assert!(!mock_provider_invocation_allowed(
            "production",
            Some("standalone.production")
        ));
        assert!(!mock_provider_invocation_allowed(
            "production",
            Some("cloud.production")
        ));
    }
}
