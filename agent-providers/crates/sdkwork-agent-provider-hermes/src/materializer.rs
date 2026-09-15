//! Hermes Agent provider config materialization.
//!
//! Hermes reads `~/.hermes/config.yaml` (`HERMES_HOME` override) with API keys
//! resolved from `~/.hermes/.env` and per-provider `key_env` entries. Applied
//! model configurations are materialized as a `providers.sdkwork` entry (the
//! native relay surface: `api`, `transport`, `key_env`) plus the key in
//! `~/.hermes/.env`, exactly matching the upstream config management surface.

use sdkwork_agent_kernel::{
    AgentModelConfigurationApplication, AgentModelConfigurationRequest, AgentModelSelectionRequest,
    KernelError, KernelResult, ProviderModelConfigurationStatus, ProviderModelMaterializationState,
};
use sdkwork_agent_provider_core::{
    dematerialize_provider_config_named, provider_user_home, read_provider_config,
    update_provider_config_file_named,
};

const HERMES_PROVIDER_ID: &str = "sdkwork";
const HERMES_PROVIDER_NAME: &str = "SDKWork BirdCoder";
const HERMES_API_KEY_ENV: &str = "SDKWORK_BIRDOODER_RELAY_API_KEY";

/// Resolves `~/.hermes/config.yaml` (mirrors the upstream candidates).
pub fn hermes_config_path() -> Option<std::path::PathBuf> {
    if let Some(home) = std::env::var_os("HERMES_HOME") {
        let home = std::path::PathBuf::from(home);
        if home.is_dir() {
            return Some(home.join("config.yaml"));
        }
    }
    let home = provider_user_home()?;
    #[cfg(windows)]
    let candidates = [
        home.join("AppData")
            .join("Local")
            .join("hermes")
            .join("config.yaml"),
        home.join(".hermes").join("config.yaml"),
    ];
    #[cfg(not(windows))]
    let candidates = [home.join(".hermes").join("config.yaml")];
    // When no config file exists yet, the canonical default path is returned
    // so materialization still writes a fresh config instead of silently
    // skipping a fresh installation.
    let default_path = candidates[0].clone();
    Some(
        candidates
            .into_iter()
            .find(|path| path.is_file())
            .unwrap_or(default_path),
    )
}

/// Resolves `~/.hermes/.env` (the env file Hermes loads for API keys).
fn hermes_env_path() -> Option<std::path::PathBuf> {
    provider_user_home().map(|home| home.join(".hermes").join(".env"))
}

/// Resolves the plaintext API key for materialization: the transient request
/// field first, then the host secret surface. Returns `None` when the key is
/// unavailable so the CLI keeps its own credential instead of failing.
fn resolve_materialization_api_key(request: &AgentModelConfigurationRequest) -> Option<String> {
    if let Some(api_key) = request
        .api_key_materialization
        .as_deref()
        .map(str::trim)
        .filter(|api_key| !api_key.is_empty())
    {
        return Some(api_key.to_string());
    }
    sdkwork_agent_kernel::lookup_env_file_secret(&request.api_key_secret_ref)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn build_materialized_config(
    current: Option<&str>,
    request: &AgentModelConfigurationRequest,
    api_key: Option<&str>,
) -> KernelResult<String> {
    let mut document: serde_yaml::Mapping = match current {
        Some(content) => serde_yaml::from_str(content).map_err(|error| {
            KernelError::provider_error(
                "hermes_config_parse",
                format!("~/.hermes/config.yaml could not be parsed: {error}"),
            )
        })?,
        None => serde_yaml::Mapping::new(),
    };
    let model_id = request.default_model_id.trim();
    // Hermes canonicalizes the chat model into `model.default` (the runtime
    // resolver and the kernel model catalog both read it); preserve any
    // existing model dict keys (provider, base_url, ...).
    let mut model = match document.get("model") {
        Some(serde_yaml::Value::Mapping(existing)) => existing.clone(),
        Some(serde_yaml::Value::String(existing)) if !existing.is_empty() => {
            serde_yaml::Mapping::from_iter([(
                serde_yaml::Value::String("default".to_string()),
                serde_yaml::Value::String(existing.clone()),
            )])
        }
        _ => serde_yaml::Mapping::new(),
    };
    model.insert(
        serde_yaml::Value::String("default".to_string()),
        serde_yaml::Value::String(model_id.to_string()),
    );
    document.insert(
        serde_yaml::Value::String("model".to_string()),
        serde_yaml::Value::Mapping(model),
    );
    let mut providers = match document.get("providers") {
        Some(serde_yaml::Value::Mapping(existing)) => existing.clone(),
        _ => serde_yaml::Mapping::new(),
    };
    let mut provider = serde_yaml::Mapping::new();
    provider.insert(
        serde_yaml::Value::String("name".to_string()),
        serde_yaml::Value::String(HERMES_PROVIDER_NAME.to_string()),
    );
    provider.insert(
        serde_yaml::Value::String("api".to_string()),
        serde_yaml::Value::String(request.base_url.trim().to_string()),
    );
    provider.insert(
        serde_yaml::Value::String("transport".to_string()),
        serde_yaml::Value::String("openai_chat".to_string()),
    );
    if api_key.is_some() {
        // The raw key goes into ~/.hermes/.env; config.yaml references it.
        provider.insert(
            serde_yaml::Value::String("key_env".to_string()),
            serde_yaml::Value::String(HERMES_API_KEY_ENV.to_string()),
        );
    }
    providers.insert(
        serde_yaml::Value::String(HERMES_PROVIDER_ID.to_string()),
        serde_yaml::Value::Mapping(provider),
    );
    document.insert(
        serde_yaml::Value::String("providers".to_string()),
        serde_yaml::Value::Mapping(providers),
    );
    serde_yaml::to_string(&serde_yaml::Value::Mapping(document)).map_err(|error| {
        KernelError::provider_error(
            "hermes_config_serialize",
            format!("~/.hermes/config.yaml could not be serialized: {error}"),
        )
    })
}

fn update_selected_model(current: Option<&str>, model_id: &str) -> KernelResult<String> {
    let mut document: serde_yaml::Mapping = match current {
        Some(content) => serde_yaml::from_str(content).map_err(|error| {
            KernelError::provider_error(
                "hermes_config_parse",
                format!("~/.hermes/config.yaml could not be parsed: {error}"),
            )
        })?,
        None => serde_yaml::Mapping::new(),
    };
    let mut model = match document.get("model") {
        Some(serde_yaml::Value::Mapping(existing)) => existing.clone(),
        _ => serde_yaml::Mapping::new(),
    };
    model.insert(
        serde_yaml::Value::String("default".to_string()),
        serde_yaml::Value::String(model_id.to_string()),
    );
    document.insert(
        serde_yaml::Value::String("model".to_string()),
        serde_yaml::Value::Mapping(model),
    );
    serde_yaml::to_string(&serde_yaml::Value::Mapping(document)).map_err(|error| {
        KernelError::provider_error(
            "hermes_config_serialize",
            format!("~/.hermes/config.yaml could not be serialized: {error}"),
        )
    })
}

fn merge_env_key(current: Option<&str>, api_key: &str) -> KernelResult<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut replaced = false;
    if let Some(content) = current {
        for line in content.lines() {
            if let Some((key, _)) = line.split_once('=') {
                if key.trim() == HERMES_API_KEY_ENV {
                    lines.push(format!("{HERMES_API_KEY_ENV}={api_key}"));
                    replaced = true;
                    continue;
                }
            }
            lines.push(line.to_string());
        }
    }
    if !replaced {
        lines.push(format!("{HERMES_API_KEY_ENV}={api_key}"));
    }
    Ok(format!("{}\n", lines.join("\n")))
}

/// Materializes a Hermes model configuration into `~/.hermes/config.yaml`.
pub fn materialize_hermes_model_configuration(
    request: &AgentModelConfigurationRequest,
    _application: &AgentModelConfigurationApplication,
) -> KernelResult<()> {
    let Some(path) = hermes_config_path() else {
        return Err(KernelError::provider_error(
            "provider_config_path",
            "could not resolve the Hermes config path: user home is unavailable",
        ));
    };
    materialize_hermes_model_configuration_at(&path, request)
}

/// Materializes a Hermes model configuration into explicit files.
pub(crate) fn materialize_hermes_model_configuration_at(
    path: &std::path::Path,
    request: &AgentModelConfigurationRequest,
) -> KernelResult<()> {
    let api_key = resolve_materialization_api_key(request);
    update_provider_config_file_named(path, "hermes", |current| {
        build_materialized_config(current, request, api_key.as_deref())
    })?;
    if let (Some(api_key), Some(env_path)) = (api_key, hermes_env_path()) {
        if let Err(error) = update_provider_config_file_named(&env_path, "hermes", |current| {
            merge_env_key(current, &api_key)
        }) {
            // The config.yaml was already materialized; restore its backup so
            // a partial two-file materialization never persists.
            let _ = dematerialize_provider_config_named(path, "hermes");
            return Err(error);
        }
    }
    Ok(())
}

/// Materializes a Hermes model selection (updates the `model` key).
pub fn materialize_hermes_model_selection(
    request: &AgentModelSelectionRequest,
    _application: &AgentModelConfigurationApplication,
) -> KernelResult<()> {
    let Some(path) = hermes_config_path() else {
        return Err(KernelError::provider_error(
            "provider_config_path",
            "could not resolve the Hermes config path: user home is unavailable",
        ));
    };
    update_provider_config_file_named(&path, "hermes", |current| {
        update_selected_model(current, request.model_id.trim())
    })
}

/// Restores the pre-materialization Hermes config backup (config + env file).
pub fn dematerialize_hermes_model_configuration(
    _agent_id: &str,
    _profile_id: &str,
) -> KernelResult<()> {
    let Some(path) = hermes_config_path() else {
        return Err(KernelError::provider_error(
            "provider_config_path",
            "could not resolve the Hermes config path: user home is unavailable",
        ));
    };
    dematerialize_hermes_model_configuration_at(&path)?;
    if let Some(env_path) = hermes_env_path() {
        dematerialize_provider_config_named(&env_path, "hermes")?;
    }
    Ok(())
}

/// Restores the pre-materialization backup for an explicit config file.
pub(crate) fn dematerialize_hermes_model_configuration_at(
    path: &std::path::Path,
) -> KernelResult<()> {
    dematerialize_provider_config_named(path, "hermes")
}

/// Reads the currently effective Hermes model configuration back from
/// `~/.hermes/config.yaml` (plus the `~/.hermes/.env` key when the provider
/// entry references it) and reports the materialization state.
pub fn read_hermes_model_configuration() -> KernelResult<ProviderModelConfigurationStatus> {
    let Some(path) = hermes_config_path() else {
        return Ok(ProviderModelConfigurationStatus::not_materialized("hermes"));
    };
    read_hermes_model_configuration_at(&path, hermes_env_path().as_deref())
}

/// Reads the effective Hermes model configuration back from an explicit
/// config file, resolving the referenced key from an explicit env file (used
/// by tests and by config surfaces with known paths).
pub(crate) fn read_hermes_model_configuration_at(
    path: &std::path::Path,
    env_path: Option<&std::path::Path>,
) -> KernelResult<ProviderModelConfigurationStatus> {
    let Some(content) = read_provider_config(path)? else {
        return Ok(ProviderModelConfigurationStatus::not_materialized("hermes"));
    };
    let document: serde_yaml::Mapping = match serde_yaml::from_str(&content) {
        Ok(document) => document,
        Err(error) => {
            return Ok(ProviderModelConfigurationStatus {
                provider_scope: "hermes".to_string(),
                materialization: ProviderModelMaterializationState::Diverged,
                effective_base_url: None,
                effective_default_model: None,
                credential_configured: false,
                issues: vec![format!(
                    "{} could not be parsed as YAML: {error}",
                    path.display()
                )],
            });
        }
    };
    let sdkwork_entry = document
        .get("providers")
        .and_then(|providers| providers.as_mapping())
        .and_then(|providers| providers.get(HERMES_PROVIDER_ID))
        .and_then(|provider| provider.as_mapping());
    let effective_base_url = sdkwork_entry
        .and_then(|provider| provider.get("api"))
        .and_then(serde_yaml::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let effective_default_model = document
        .get("model")
        .and_then(|model| model.as_mapping())
        .and_then(|model| model.get("default"))
        .and_then(serde_yaml::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let key_env = sdkwork_entry
        .and_then(|provider| provider.get("key_env"))
        .and_then(serde_yaml::Value::as_str)
        .filter(|value| !value.is_empty());
    // A credential counts as configured only when the referenced env file
    // carries a non-empty value; unreadable env files fail closed (reported as
    // an issue) instead of silently reading as "not configured".
    let mut issues = Vec::new();
    let credential_configured = match key_env {
        Some(key_env) => match env_path {
            Some(env_path) => match read_provider_config(env_path) {
                Ok(Some(env)) => env.lines().any(|line| {
                    line.trim_start()
                        .split_once('=')
                        .is_some_and(|(key, value)| {
                            key.trim() == key_env && !value.trim().is_empty()
                        })
                }),
                Ok(None) => false,
                Err(error) => {
                    issues.push(format!(
                        "referenced env file {} could not be read: {error}",
                        env_path.display()
                    ));
                    false
                }
            },
            None => false,
        },
        None => false,
    };
    let materialized = effective_base_url.is_some();
    let mut status = ProviderModelConfigurationStatus {
        provider_scope: "hermes".to_string(),
        materialization: if materialized {
            ProviderModelMaterializationState::Materialized
        } else {
            ProviderModelMaterializationState::NotMaterialized
        },
        effective_base_url,
        effective_default_model,
        credential_configured,
        issues,
    };
    if !materialized {
        status.issues.push(format!(
            "config.yaml does not carry a {HERMES_PROVIDER_ID} providers entry"
        ));
    }
    Ok(status)
}

#[cfg(test)] // WORKSPACE-PATH:allow-fixture-block: this module is the file's #[cfg(test)] unit-test fixture data
mod tests {
    use super::*;

    fn request_with(
        api_key: Option<&str>,
        base_url: &str,
        model: &str,
    ) -> AgentModelConfigurationRequest {
        let mut request = AgentModelConfigurationRequest::new(
            "request-1",
            "agent.agent-engine.hermes",
            "profile.test",
            "openai",
            base_url,
            "secret.ref",
            model,
        );
        if let Some(api_key) = api_key {
            request = request.with_api_key_materialization(api_key);
        }
        request
    }

    #[test]
    fn build_config_sets_provider_entry() {
        let request = request_with(
            Some("token-abc"),
            "https://api.birdcoder.com/v1",
            "gpt-5.4-mini",
        );
        let content = build_materialized_config(None, &request, Some("token-abc")).expect("build");
        let document: serde_yaml::Value = serde_yaml::from_str(&content).expect("parse");
        assert_eq!(document["model"]["default"].as_str(), Some("gpt-5.4-mini"));
        assert_eq!(
            document["providers"]["sdkwork"]["api"].as_str(),
            Some("https://api.birdcoder.com/v1")
        );
        assert_eq!(
            document["providers"]["sdkwork"]["transport"].as_str(),
            Some("openai_chat")
        );
        assert_eq!(
            document["providers"]["sdkwork"]["key_env"].as_str(),
            Some("SDKWORK_BIRDOODER_RELAY_API_KEY")
        );
    }

    #[test]
    fn build_config_merges_existing_providers() {
        let existing = "model: old\nproviders:\n  deepseek:\n    api: https://x\n";
        let request = request_with(None, "https://api.birdcoder.com/v1", "gpt-5");
        let content = build_materialized_config(Some(existing), &request, None).expect("merge");
        let document: serde_yaml::Value = serde_yaml::from_str(&content).expect("parse");
        assert!(document["providers"]["deepseek"].is_mapping());
        assert!(document["providers"]["sdkwork"].is_mapping());
        assert_eq!(document["model"]["default"].as_str(), Some("gpt-5"));
    }

    #[test]
    fn merge_env_key_replaces_and_appends() {
        let merged = merge_env_key(
            Some("OTHER=1\nSDKWORK_BIRDOODER_RELAY_API_KEY=old\n"),
            "new",
        )
        .expect("merge");
        assert!(merged.contains("SDKWORK_BIRDOODER_RELAY_API_KEY=new"));
        assert!(merged.contains("OTHER=1"));
        assert!(!merged.contains("=old"));
        let appended = merge_env_key(None, "new").expect("append");
        assert!(appended.contains("SDKWORK_BIRDOODER_RELAY_API_KEY=new"));
    }

    #[test]
    fn update_selected_model_keeps_providers() {
        let existing = "model: a\nproviders:\n  sdkwork:\n    api: https://x\n";
        let content = update_selected_model(Some(existing), "gpt-5.5").expect("update");
        let document: serde_yaml::Value = serde_yaml::from_str(&content).expect("parse");
        assert_eq!(document["model"]["default"].as_str(), Some("gpt-5.5"));
        assert!(document["providers"]["sdkwork"].is_mapping());
    }

    #[test]
    fn read_back_reports_materialized_provider_entry() {
        let dir =
            std::env::temp_dir().join(format!("sdkwork-hermes-readback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let config_path = dir.join("config.yaml");
        std::fs::write(
            &config_path,
            "model:\n  default: gpt-5.4\nproviders:\n  sdkwork:\n    name: SDKWork BirdCoder\n    api: https://api.birdcoder.com\n    transport: openai_chat\n    key_env: SDKWORK_BIRDOODER_RELAY_API_KEY\n",
        )
        .expect("seed");
        let env_path = dir.join(".env");
        std::fs::write(&env_path, "SDKWORK_BIRDOODER_RELAY_API_KEY=token-abc\n").expect("seed");

        let status =
            read_hermes_model_configuration_at(&config_path, Some(&env_path)).expect("read back");
        assert_eq!(
            status.materialization,
            sdkwork_agent_kernel::ProviderModelMaterializationState::Materialized
        );
        assert_eq!(
            status.effective_base_url.as_deref(),
            Some("https://api.birdcoder.com")
        );
        assert_eq!(status.effective_default_model.as_deref(), Some("gpt-5.4"));
        assert!(status.credential_configured);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_back_reports_not_materialized_without_provider_entry() {
        let dir = std::env::temp_dir().join(format!(
            "sdkwork-hermes-readback-native-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let config_path = dir.join("config.yaml");
        std::fs::write(&config_path, "model:\n  default: gemini-2.5-pro\n").expect("seed");

        let status = read_hermes_model_configuration_at(&config_path, None).expect("read back");
        assert_eq!(
            status.materialization,
            sdkwork_agent_kernel::ProviderModelMaterializationState::NotMaterialized
        );
        assert_eq!(
            status.effective_default_model.as_deref(),
            Some("gemini-2.5-pro")
        );
        assert!(!status.issues.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
