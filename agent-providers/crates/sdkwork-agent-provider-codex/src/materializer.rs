//! Codex provider config materialization.
//!
//! The Codex CLI reads `~/.codex/config.toml` (or `$CODEX_HOME/config.toml`)
//! at request time: `model`, `model_provider`, and `[model_providers.<id>]`
//! entries with `base_url` + `experimental_bearer_token` (or `env_key`).
//! Applied model configurations are materialized into that file so the CLI
//! actually routes through the configured relay endpoint with the configured
//! credential, exactly matching the upstream config management surface.

use sdkwork_agent_kernel::{
    AgentModelConfigurationApplication, AgentModelConfigurationRequest, AgentModelSelectionRequest,
    KernelError, KernelResult, ProviderModelConfigurationStatus, ProviderModelMaterializationState,
};
use sdkwork_agent_provider_core::{
    dematerialize_provider_config_named, read_provider_config, update_provider_config_file_named,
};

use crate::codex_config_path;

/// Provider id materialized into `model_providers` for SDKWork-managed configs.
pub const SDKWORK_CODEX_MODEL_PROVIDER_ID: &str = "sdkwork";
const CONFIG_PROVIDER_NAME: &str = "SDKWork BirdCoder";

/// Resolves the plaintext API key for materialization: the transient request
/// field first, then the host secret surface (`SDKWORK_SECRET_*` env vars and
/// `SDKWORK_SECRETS_DIR` files). Returns `None` when the key is unavailable so
/// the CLI keeps its own credential (the pre-existing behavior) instead of
/// failing the applied profile.
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

/// True when the current config.toml is already materialized by this provider
/// for the exact base URL and model, which makes repeated apply calls
/// idempotent (no backup churn). A changed base URL or model still rewrites.
fn current_is_sdkwork_materialized(
    path: &std::path::Path,
    request: &AgentModelConfigurationRequest,
) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(document) = content.parse::<toml::Value>() else {
        return false;
    };
    if !document
        .get("model_provider")
        .and_then(toml::Value::as_str)
        .is_some_and(|provider| provider == SDKWORK_CODEX_MODEL_PROVIDER_ID)
    {
        return false;
    }
    let materialized_base_url = document
        .get("model_providers")
        .and_then(|providers| providers.get(SDKWORK_CODEX_MODEL_PROVIDER_ID))
        .and_then(|provider| provider.get("base_url"))
        .and_then(toml::Value::as_str);
    let materialized_model = document.get("model").and_then(toml::Value::as_str);
    materialized_base_url.is_some_and(|base_url| base_url == request.base_url.trim())
        && materialized_model.is_some_and(|model| model == request.default_model_id.trim())
}

fn build_materialized_config(
    current: Option<&str>,
    request: &AgentModelConfigurationRequest,
    api_key: Option<&str>,
) -> KernelResult<String> {
    let mut document = match current {
        Some(content) => content.parse::<toml::Value>().map_err(|error| {
            KernelError::provider_error(
                "codex_config_parse",
                format!("~/.codex/config.toml could not be parsed: {error}"),
            )
        })?,
        None => toml::Value::Table(Default::default()),
    };
    let table = document.as_table_mut().ok_or_else(|| {
        KernelError::provider_error("codex_config_shape", "~/.codex/config.toml is not a table")
    })?;
    table.insert(
        "model".to_string(),
        toml::Value::String(request.default_model_id.trim().to_string()),
    );
    table.insert(
        "model_provider".to_string(),
        toml::Value::String(SDKWORK_CODEX_MODEL_PROVIDER_ID.to_string()),
    );
    let mut provider = toml::map::Map::new();
    provider.insert(
        "name".to_string(),
        toml::Value::String(CONFIG_PROVIDER_NAME.to_string()),
    );
    provider.insert(
        "base_url".to_string(),
        toml::Value::String(request.base_url.trim().to_string()),
    );
    if let Some(api_key) = api_key {
        provider.insert(
            "experimental_bearer_token".to_string(),
            toml::Value::String(api_key.to_string()),
        );
    }
    // Codex only ships the Responses wire API; pin it so a future Codex
    // release cannot silently fall back to a different protocol.
    provider.insert(
        "wire_api".to_string(),
        toml::Value::String("responses".to_string()),
    );
    // Merge into the existing `model_providers` table (user-defined relay
    // entries must survive) and upsert the SDKWork-managed provider entry.
    let mut providers = match table.get("model_providers") {
        Some(toml::Value::Table(existing)) => existing.clone(),
        Some(_) | None => toml::map::Map::new(),
    };
    providers.insert(
        SDKWORK_CODEX_MODEL_PROVIDER_ID.to_string(),
        toml::Value::Table(provider),
    );
    table.insert("model_providers".to_string(), toml::Value::Table(providers));
    toml::to_string_pretty(&document).map_err(|error| {
        KernelError::provider_error(
            "codex_config_serialize",
            format!("~/.codex/config.toml could not be serialized: {error}"),
        )
    })
}

fn update_selected_model(current: Option<&str>, model_id: &str) -> KernelResult<String> {
    let mut document = match current {
        Some(content) => content.parse::<toml::Value>().map_err(|error| {
            KernelError::provider_error(
                "codex_config_parse",
                format!("~/.codex/config.toml could not be parsed: {error}"),
            )
        })?,
        None => toml::Value::Table(Default::default()),
    };
    let table = document.as_table_mut().ok_or_else(|| {
        KernelError::provider_error("codex_config_shape", "~/.codex/config.toml is not a table")
    })?;
    table.insert(
        "model".to_string(),
        toml::Value::String(model_id.to_string()),
    );
    toml::to_string_pretty(&document).map_err(|error| {
        KernelError::provider_error(
            "codex_config_serialize",
            format!("~/.codex/config.toml could not be serialized: {error}"),
        )
    })
}

/// Materializes a Codex model configuration into the resolved Codex config
/// file (`CODEX_HOME`/`~/.codex/config.toml`).
pub fn materialize_codex_model_configuration(
    request: &AgentModelConfigurationRequest,
    application: &AgentModelConfigurationApplication,
) -> KernelResult<()> {
    let Some(path) = codex_config_path() else {
        return Err(KernelError::provider_error(
                "provider_config_path",
                "could not resolve the Codex config path: CODEX_HOME is set to a missing directory or the user home is unavailable",
            ));
    };
    materialize_codex_model_configuration_at(&path, request, application)
}

/// Materializes a Codex model configuration into an explicit config file.
pub(crate) fn materialize_codex_model_configuration_at(
    path: &std::path::Path,
    request: &AgentModelConfigurationRequest,
    _application: &AgentModelConfigurationApplication,
) -> KernelResult<()> {
    if current_is_sdkwork_materialized(path, request)
        && request.api_key_materialization.is_none()
        && sdkwork_agent_kernel::lookup_env_file_secret(&request.api_key_secret_ref).is_none()
    {
        return Ok(());
    }
    let api_key = resolve_materialization_api_key(request);
    update_provider_config_file_named(path, "codex", |current| {
        build_materialized_config(current, request, api_key.as_deref())
    })
}

/// Materializes a Codex model selection (updates the `model` key).
pub fn materialize_codex_model_selection(
    request: &AgentModelSelectionRequest,
    application: &AgentModelConfigurationApplication,
) -> KernelResult<()> {
    let Some(path) = codex_config_path() else {
        return Err(KernelError::provider_error(
                "provider_config_path",
                "could not resolve the Codex config path: CODEX_HOME is set to a missing directory or the user home is unavailable",
            ));
    };
    materialize_codex_model_selection_at(&path, request, application)
}

/// Materializes a Codex model selection into an explicit config file.
pub(crate) fn materialize_codex_model_selection_at(
    path: &std::path::Path,
    request: &AgentModelSelectionRequest,
    _application: &AgentModelConfigurationApplication,
) -> KernelResult<()> {
    update_provider_config_file_named(path, "codex", |current| {
        update_selected_model(current, request.model_id.trim())
    })
}

/// Restores the pre-materialization `~/.codex/config.toml` backup.
pub fn dematerialize_codex_model_configuration(
    _agent_id: &str,
    _profile_id: &str,
) -> KernelResult<()> {
    let Some(path) = codex_config_path() else {
        return Err(KernelError::provider_error(
                "provider_config_path",
                "could not resolve the Codex config path: CODEX_HOME is set to a missing directory or the user home is unavailable",
            ));
    };
    dematerialize_codex_model_configuration_at(&path)
}

/// Reads the currently effective Codex model configuration back from the
/// Codex config file (`CODEX_HOME`/`~/.codex/config.toml`) and reports the
/// materialization state, so callers can detect drift and stale CLI state.
pub fn read_codex_model_configuration() -> KernelResult<ProviderModelConfigurationStatus> {
    let Some(path) = codex_config_path() else {
        return Ok(ProviderModelConfigurationStatus::not_materialized("codex"));
    };
    read_codex_model_configuration_at(&path)
}

/// Reads the effective Codex model configuration back from an explicit config
/// file (used by tests and by config surfaces with a known path).
pub(crate) fn read_codex_model_configuration_at(
    path: &std::path::Path,
) -> KernelResult<ProviderModelConfigurationStatus> {
    let Some(content) = read_provider_config(path)? else {
        return Ok(ProviderModelConfigurationStatus::not_materialized("codex"));
    };
    let document = match content.parse::<toml::Value>() {
        Ok(document) => document,
        Err(error) => {
            return Ok(ProviderModelConfigurationStatus {
                provider_scope: "codex".to_string(),
                materialization: ProviderModelMaterializationState::Diverged,
                effective_base_url: None,
                effective_default_model: None,
                credential_configured: false,
                issues: vec![format!(
                    "{} could not be parsed as TOML: {error}",
                    path.display()
                )],
            });
        }
    };
    let is_sdkwork_routed = document
        .get("model_provider")
        .and_then(toml::Value::as_str)
        .is_some_and(|provider| provider == SDKWORK_CODEX_MODEL_PROVIDER_ID);
    let sdkwork_entry = document
        .get("model_providers")
        .and_then(|providers| providers.get(SDKWORK_CODEX_MODEL_PROVIDER_ID));
    let effective_base_url = sdkwork_entry
        .and_then(|entry| entry.get("base_url"))
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    // The routing marker without its provider entry is drift: the CLI would
    // fail to resolve the SDKWork provider at request time.
    let (materialization, issues) = if !is_sdkwork_routed {
        (
            ProviderModelMaterializationState::NotMaterialized,
            vec![format!(
                "codex model_provider is not routed through the {} provider entry",
                SDKWORK_CODEX_MODEL_PROVIDER_ID
            )],
        )
    } else if sdkwork_entry.is_none() || effective_base_url.is_none() {
        (
            ProviderModelMaterializationState::Diverged,
            vec![format!(
                "codex is routed through the {} provider entry but the entry is missing or carries no base_url",
                SDKWORK_CODEX_MODEL_PROVIDER_ID
            )],
        )
    } else {
        (ProviderModelMaterializationState::Materialized, Vec::new())
    };
    let status = ProviderModelConfigurationStatus {
        provider_scope: "codex".to_string(),
        materialization,
        effective_base_url,
        effective_default_model: document
            .get("model")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        credential_configured: sdkwork_entry
            .and_then(|entry| entry.get("experimental_bearer_token"))
            .and_then(toml::Value::as_str)
            .is_some_and(|token| !token.is_empty()),
        issues,
    };
    Ok(status)
}

/// Restores the pre-materialization backup for an explicit config file.
pub(crate) fn dematerialize_codex_model_configuration_at(
    path: &std::path::Path,
) -> KernelResult<()> {
    dematerialize_provider_config_named(path, "codex")
}

#[cfg(test)] // WORKSPACE-PATH:allow-fixture-block: this module is the file's #[cfg(test)] unit-test fixture data
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn request_with(
        api_key: Option<&str>,
        base_url: &str,
        model: &str,
    ) -> AgentModelConfigurationRequest {
        let mut request = AgentModelConfigurationRequest::new(
            "request-1",
            "agent.agent-engine.codex",
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
    fn build_config_sets_provider_entry_and_model() {
        let request = request_with(
            Some("token-abc"),
            "https://api.birdcoder.com/v1",
            "gpt-5.4-mini",
        );
        let content =
            build_materialized_config(None, &request, Some("token-abc")).expect("build config");
        let document: toml::Value = content.parse().expect("parse");
        assert_eq!(document["model"].as_str(), Some("gpt-5.4-mini"));
        assert_eq!(document["model_provider"].as_str(), Some("sdkwork"));
        assert_eq!(
            document["model_providers"]["sdkwork"]["base_url"].as_str(),
            Some("https://api.birdcoder.com/v1")
        );
        assert_eq!(
            document["model_providers"]["sdkwork"]["experimental_bearer_token"].as_str(),
            Some("token-abc")
        );
        assert_eq!(
            document["model_providers"]["sdkwork"]["wire_api"].as_str(),
            Some("responses")
        );
    }

    #[test]
    fn build_config_merges_existing_tables() {
        let existing = "model = \"old\"\n[model_providers.custom]\nbase_url = \"https://x\"\n";
        let request = request_with(None, "https://api.birdcoder.com/v1", "gpt-5");
        let content = build_materialized_config(Some(existing), &request, None).expect("merge");
        let document: toml::Value = content.parse().expect("parse");
        assert_eq!(
            document["model_providers"]["custom"]["base_url"].as_str(),
            Some("https://x")
        );
        assert_eq!(document["model_provider"].as_str(), Some("sdkwork"));
    }

    #[test]
    fn update_selected_model_keeps_other_keys() {
        let existing = "model = \"a\"\nmodel_provider = \"sdkwork\"\n";
        let content = update_selected_model(Some(existing), "gpt-5.5").expect("update");
        let document: toml::Value = content.parse().expect("parse");
        assert_eq!(document["model"].as_str(), Some("gpt-5.5"));
        assert_eq!(document["model_provider"].as_str(), Some("sdkwork"));
    }

    #[test]
    fn materialization_rewrites_when_base_url_changes() {
        let _guard = test_guard();
        let dir =
            std::env::temp_dir().join(format!("sdkwork-codex-relay-change-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "model = \"original\"\n").expect("seed");

        let first = request_with(None, "https://relay.old.test/v1", "gpt-5");
        materialize_codex_model_configuration_at(
            &path,
            &first,
            &AgentModelConfigurationApplication::new(
                "request-1",
                "codex",
                sdkwork_agent_kernel::AgentConfigurationProfile::new(
                    "profile.test",
                    "agent.agent-engine.codex",
                    "0.2.0",
                    sdkwork_agent_kernel::AgentConfiguration::new(
                        "agent.agent-engine.codex",
                        "profile.test",
                    ),
                ),
            ),
        )
        .expect("first materialize");
        // Same config again (no key) must be a no-op; a changed relay base URL
        // must rewrite even without a fresh key.
        let second = request_with(None, "https://api.birdcoder.com/v1", "gpt-5");
        materialize_codex_model_configuration_at(
            &path,
            &second,
            &AgentModelConfigurationApplication::new(
                "request-1",
                "codex",
                sdkwork_agent_kernel::AgentConfigurationProfile::new(
                    "profile.test",
                    "agent.agent-engine.codex",
                    "0.2.0",
                    sdkwork_agent_kernel::AgentConfiguration::new(
                        "agent.agent-engine.codex",
                        "profile.test",
                    ),
                ),
            ),
        )
        .expect("second materialize");
        let content = std::fs::read_to_string(&path).expect("read");
        assert!(content.contains("base_url = \"https://api.birdcoder.com/v1\""));
        assert!(!content.contains("https://relay.old.test"));
        // The backup still holds the ORIGINAL pre-materialization state.
        dematerialize_codex_model_configuration_at(&path).expect("dematerialize");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read restored"),
            "model = \"original\"\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn materialization_round_trip_writes_and_dematerializes() {
        let _guard = test_guard();
        let dir =
            std::env::temp_dir().join(format!("sdkwork-codex-materialize-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let config_path = dir.join("config.toml");
        std::fs::write(&config_path, "model = \"original\"\n").expect("seed");

        let request = request_with(Some("token-xyz"), "https://api.birdcoder.com/v1", "gpt-5.4");
        let application = AgentModelConfigurationApplication::new(
            "request-1",
            "codex",
            sdkwork_agent_kernel::AgentConfigurationProfile::new(
                "profile.test",
                "agent.agent-engine.codex",
                "0.2.0",
                sdkwork_agent_kernel::AgentConfiguration::new(
                    "agent.agent-engine.codex",
                    "profile.test",
                ),
            ),
        );
        materialize_codex_model_configuration_at(&config_path, &request, &application)
            .expect("materialize");
        let content = std::fs::read_to_string(&config_path).expect("read");
        assert!(content.contains("model_provider = \"sdkwork\""));
        assert!(content.contains("experimental_bearer_token = \"token-xyz\""));

        dematerialize_codex_model_configuration_at(&config_path).expect("dematerialize");
        let restored = std::fs::read_to_string(&config_path).expect("read restored");
        assert_eq!(restored, "model = \"original\"\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_back_reports_materialized_effective_values() {
        let _guard = test_guard();
        let dir =
            std::env::temp_dir().join(format!("sdkwork-codex-readback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let config_path = dir.join("config.toml");
        std::fs::write(
            &config_path,
            "model = \"gpt-5.4\"\nmodel_provider = \"sdkwork\"\n\n[model_providers.sdkwork]\nbase_url = \"https://api.birdcoder.com/v1\"\nexperimental_bearer_token = \"token-abc\"\n",
        )
        .expect("seed");

        let status = read_codex_model_configuration_at(&config_path).expect("read back");
        assert_eq!(
            status.materialization,
            sdkwork_agent_kernel::ProviderModelMaterializationState::Materialized
        );
        assert_eq!(
            status.effective_base_url.as_deref(),
            Some("https://api.birdcoder.com/v1")
        );
        assert_eq!(status.effective_default_model.as_deref(), Some("gpt-5.4"));
        assert!(status.credential_configured);
        assert!(status.issues.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_back_detects_missing_and_non_sdkwork_configs() {
        let _guard = test_guard();
        let dir = std::env::temp_dir().join(format!(
            "sdkwork-codex-readback-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let config_path = dir.join("config.toml");

        // Absent file: nothing has been materialized.
        let status = read_codex_model_configuration_at(&config_path).expect("read back");
        assert_eq!(
            status.materialization,
            sdkwork_agent_kernel::ProviderModelMaterializationState::NotMaterialized
        );

        // Existing file routed through a different provider: diverged surface.
        std::fs::write(
            &config_path,
            "model = \"gpt-4\"\nmodel_provider = \"openai\"\n",
        )
        .expect("seed");
        let status = read_codex_model_configuration_at(&config_path).expect("read back");
        assert_eq!(
            status.materialization,
            sdkwork_agent_kernel::ProviderModelMaterializationState::NotMaterialized
        );
        assert_eq!(status.effective_default_model.as_deref(), Some("gpt-4"));
        assert!(!status.issues.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
