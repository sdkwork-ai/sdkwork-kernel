//! Local provider configuration helpers shared by provider adapter crates.
//!
//! Provider adapters surface the model configured for the locally installed
//! application (Codex config.toml, opencode config.json, Claude settings.json,
//! Hermes config.yaml) through their `ModelProvider::list_models()` so default
//! model selection always matches what the installed CLI would use. Only the
//! user-home resolution is shared here; each provider parses its own config
//! format because the upstream applications use different file schemas.

use std::path::PathBuf;

/// Resolves the current user's home directory.
///
/// Windows prefers `USERPROFILE`; other platforms use `HOME`. Returns `None`
/// when neither variable is set so callers can fall back to their built-in
/// defaults.
pub fn provider_user_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

#[cfg(test)] // WORKSPACE-PATH:allow-fixture-block: this module is the file's #[cfg(test)] unit-test fixture data
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

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
    fn user_home_prefers_windows_profile_variable() {
        let _lock = env_lock();
        let _profile = EnvVarGuard::set("USERPROFILE", Some("C:/Users/test"));
        let _home = EnvVarGuard::set("HOME", Some("/home/test"));
        assert_eq!(provider_user_home(), Some(PathBuf::from("C:/Users/test")));
    }

    #[test]
    fn user_home_falls_back_to_home() {
        let _lock = env_lock();
        let _profile = EnvVarGuard::set("USERPROFILE", None);
        let _home = EnvVarGuard::set("HOME", Some("/home/test"));
        assert_eq!(provider_user_home(), Some(PathBuf::from("/home/test")));
    }

    #[test]
    fn user_home_is_none_when_unset() {
        let _lock = env_lock();
        let _profile = EnvVarGuard::set("USERPROFILE", None);
        let _home = EnvVarGuard::set("HOME", None);
        assert_eq!(provider_user_home(), None);
    }
}
