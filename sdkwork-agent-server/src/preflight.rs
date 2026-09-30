use crate::config::ServerConfig;
use tracing::info;

/// Preflight validation result
#[derive(Debug)]
pub struct PreflightResult {
    pub checks: Vec<PreflightCheck>,
    pub passed: bool,
}

/// Individual preflight check
#[derive(Debug)]
pub struct PreflightCheck {
    pub name: String,
    pub status: PreflightStatus,
    pub message: String,
}

/// Preflight check status
#[derive(Debug, PartialEq)]
pub enum PreflightStatus {
    Passed,
    Warning,
    Failed,
}

/// Run preflight validation checks
pub fn validate(config: &ServerConfig) -> PreflightResult {
    let mut checks = Vec::new();

    // Check bind address
    checks.push(validate_bind_address(&config.bind_address));

    if !config.is_loopback_bind() && config.ingress_auth_mode.eq_ignore_ascii_case("open") {
        checks.push(PreflightCheck {
            name: "ingress_auth_open".to_string(),
            status: PreflightStatus::Failed,
            message: "Open ingress auth is allowed only on loopback bind addresses".to_string(),
        });
    }

    if config.is_production_kernel_profile() && config.bind_address == "0.0.0.0" {
        checks.push(PreflightCheck {
            name: "production_bind".to_string(),
            status: PreflightStatus::Warning,
            message:
                "Production environment binds to 0.0.0.0; place ingress behind a trusted gateway"
                    .to_string(),
        });
    }

    if config.ingress_auth_mode.eq_ignore_ascii_case("token")
        && config.ingress_token.as_deref().is_none_or(str::is_empty)
    {
        checks.push(PreflightCheck {
            name: "ingress_token".to_string(),
            status: PreflightStatus::Failed,
            message: "SDKWORK_KERNEL_INGRESS_TOKEN is required when ingress auth mode is token"
                .to_string(),
        });
    }

    if config.ingress_auth_mode.eq_ignore_ascii_case("jwt") && !config.has_ingress_jwt_material() {
        checks.push(PreflightCheck {
            name: "ingress_jwt_material".to_string(),
            status: PreflightStatus::Failed,
            message: "JWT ingress requires SDKWORK_KERNEL_INGRESS_JWT_SECRET, SDKWORK_KERNEL_INGRESS_JWT_RSA_PUBLIC_KEY_PEM, SDKWORK_KERNEL_INGRESS_JWT_JWKS_FILE, or SDKWORK_KERNEL_INGRESS_JWT_JWKS_URL"
                .to_string(),
        });
    }

    if config.ingress_auth_mode.eq_ignore_ascii_case("jwt")
        && config.is_production_kernel_profile()
        && config
            .ingress_jwt_jwks_url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        && !config
            .ingress_jwt_jwks_url
            .as_deref()
            .is_some_and(|value| value.trim().starts_with("https://"))
    {
        checks.push(PreflightCheck {
            name: "ingress_jwt_jwks_url_https".to_string(),
            status: PreflightStatus::Failed,
            message: "Production JWT ingress JWKS URL must use https:// (SDKWORK_KERNEL_INGRESS_JWT_JWKS_URL)"
                .to_string(),
        });
    }

    // Signature verification alone accepts any token signed by the trusted
    // key; pinning issuer and audience in production keeps cross-environment
    // or cross-audience token replay out of the trust boundary.
    if config.ingress_auth_mode.eq_ignore_ascii_case("jwt") && config.is_production_kernel_profile()
    {
        let issuer_pinned = config
            .ingress_jwt_issuer
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
        let audience_pinned = config
            .ingress_jwt_audience
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
        if !issuer_pinned || !audience_pinned {
            checks.push(PreflightCheck {
                name: "ingress_jwt_issuer_audience_pinned".to_string(),
                status: PreflightStatus::Failed,
                message: "Production JWT ingress must pin SDKWORK_KERNEL_INGRESS_JWT_ISSUER and SDKWORK_KERNEL_INGRESS_JWT_AUDIENCE".to_string(),
            });
        }
    }

    if config.ingress_identity_mode() == crate::ingress_identity::IngressIdentityMode::Bound
        && !config.has_bound_identity()
    {
        checks.push(PreflightCheck {
            name: "ingress_bound_identity".to_string(),
            status: PreflightStatus::Failed,
            message: "Bound ingress identity requires SDKWORK_KERNEL_INGRESS_BOUND_TENANT_ID and SDKWORK_KERNEL_INGRESS_BOUND_USER_ID"
                .to_string(),
        });
    }

    if config.is_production_kernel_profile() && config.rate_limit_rps == 0 {
        checks.push(PreflightCheck {
            name: "rate_limit".to_string(),
            status: PreflightStatus::Failed,
            message: "Production requires a positive SDKWORK_RATE_LIMIT_RPS (default 100)"
                .to_string(),
        });
    }

    if config.metrics_auth_required() && config.effective_metrics_token().is_none() {
        checks.push(PreflightCheck {
            name: "metrics_token".to_string(),
            status: PreflightStatus::Failed,
            message: "Metrics token auth requires SDKWORK_KERNEL_METRICS_TOKEN".to_string(),
        });
    }

    if config.is_production_kernel_profile() {
        match config.cursor_signing_secret.as_deref() {
            Some(secret) if secret.len() >= 32 => {
                let reused = [
                    config.ingress_token.as_deref(),
                    config.ingress_jwt_secret.as_deref(),
                    config.metrics_token.as_deref(),
                ]
                .into_iter()
                .flatten()
                .any(|other| sdkwork_utils_rust::secure_compare(secret, other));
                if reused {
                    checks.push(PreflightCheck {
                        name: "cursor_signing_secret_isolation".to_string(),
                        status: PreflightStatus::Failed,
                        message: "SDKWORK_CURSOR_SIGNING_SECRET must not reuse ingress, JWT, or metrics credentials"
                            .to_string(),
                    });
                }
            }
            _ => checks.push(PreflightCheck {
                name: "cursor_signing_secret".to_string(),
                status: PreflightStatus::Failed,
                message: "Production requires SDKWORK_CURSOR_SIGNING_SECRET with at least 32 bytes"
                    .to_string(),
            }),
        }
        match config.approval_payload_encryption_key.as_deref() {
            Some(encoded_key) => match sdkwork_utils_rust::base64url_decode(encoded_key) {
                Some(key) if key.len() == 32 => {
                    let reused = [
                        config.ingress_token.as_deref(),
                        config.ingress_jwt_secret.as_deref(),
                        config.metrics_token.as_deref(),
                        config.cursor_signing_secret.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .any(|other| {
                        sdkwork_utils_rust::secure_compare(encoded_key, other)
                            || secure_compare_bytes(&key, other.as_bytes())
                    });
                    if reused {
                        checks.push(PreflightCheck {
                            name: "approval_payload_key_isolation".to_string(),
                            status: PreflightStatus::Failed,
                            message: "SDKWORK_APPROVAL_PAYLOAD_ENCRYPTION_KEY must not reuse ingress, JWT, metrics, or cursor credentials"
                                .to_string(),
                        });
                    }
                }
                _ => checks.push(PreflightCheck {
                    name: "approval_payload_key".to_string(),
                    status: PreflightStatus::Failed,
                    message: "SDKWORK_APPROVAL_PAYLOAD_ENCRYPTION_KEY must be base64url and decode to exactly 32 bytes"
                        .to_string(),
                }),
            },
            None => checks.push(PreflightCheck {
                name: "approval_payload_key".to_string(),
                status: PreflightStatus::Failed,
                message: "Production requires SDKWORK_APPROVAL_PAYLOAD_ENCRYPTION_KEY"
                    .to_string(),
            }),
        }
    }

    if config.otel_export_enabled() && !cfg!(feature = "observability-otel") {
        checks.push(PreflightCheck {
            name: "otel_feature".to_string(),
            status: PreflightStatus::Failed,
            message: "SDKWORK_OTEL_EXPORTER_OTLP_ENDPOINT requires building sdkwork-agent-server with feature observability-otel"
                .to_string(),
        });
    }

    // The kernel gate already refuses mocks in production unconditionally;
    // this preflight check fails fast with an explicit message whenever the
    // override environment variable is set at all in a production profile.
    if config.is_production_kernel_profile()
        && sdkwork_agent_kernel::mock_provider_override_enabled_from_env()
    {
        checks.push(PreflightCheck {
            name: "mock_providers".to_string(),
            status: PreflightStatus::Failed,
            message: "SDKWORK_KERNEL_ALLOW_MOCK_PROVIDERS must be unset in production profiles"
                .to_string(),
        });
    }

    if config.requires_distributed_rate_limit() && config.effective_rate_limit_redis_url().is_none()
    {
        checks.push(PreflightCheck {
            name: "rate_limit_redis".to_string(),
            status: PreflightStatus::Failed,
            message: "Production cloud/server deployments require SDKWORK_RATE_LIMIT_REDIS_URL (or SDKWORK_REDIS_URL) for distributed rate limiting"
                .to_string(),
        });
    }

    if config.is_cluster_coordination() && config.effective_event_fanout_redis_url().is_none() {
        checks.push(PreflightCheck {
            name: "event_fanout_redis".to_string(),
            status: PreflightStatus::Failed,
            message: "Cluster coordination requires SDKWORK_EVENT_FANOUT_REDIS_URL (or SDKWORK_REDIS_URL) for the cross-pod SSE event wakeup"
                .to_string(),
        });
    }

    if config.requires_distributed_idempotency()
        && config.effective_idempotency_redis_url().is_none()
    {
        checks.push(PreflightCheck {
            name: "idempotency_redis".to_string(),
            status: PreflightStatus::Failed,
            message: "Production cloud/server deployments require SDKWORK_IDEMPOTENCY_REDIS_URL for distributed idempotency"
                .to_string(),
        });
    }

    if config.is_production_kernel_profile() && !config.idempotency_require_key {
        checks.push(PreflightCheck {
            name: "idempotency_key_required".to_string(),
            status: PreflightStatus::Failed,
            message:
                "Production profiles must require Idempotency-Key on retry-sensitive mutations"
                    .to_string(),
        });
    }

    if !(60..=7 * 24 * 60 * 60).contains(&config.idempotency_ttl_secs) {
        checks.push(PreflightCheck {
            name: "idempotency_ttl".to_string(),
            status: PreflightStatus::Failed,
            message: "Idempotency retention must be between 60 seconds and 7 days".to_string(),
        });
    }

    if config.idempotency_max_cached_response_bytes == 0
        || config.idempotency_max_cached_response_bytes > 1024 * 1024
        || config.idempotency_max_cached_response_bytes > config.max_body_size
    {
        checks.push(PreflightCheck {
            name: "idempotency_response_limit".to_string(),
            status: PreflightStatus::Failed,
            message: "Idempotency response cache limit must be positive, at most 1 MiB, and no larger than max body size"
                .to_string(),
        });
    }

    if config.is_production_kernel_profile()
        && config.production_scaleout_profile()
        && !config.tenant_token_quota_overrides.is_empty()
        && config.effective_rate_limit_redis_url().is_none()
    {
        checks.push(PreflightCheck {
            name: "tenant_token_quota_redis".to_string(),
            status: PreflightStatus::Failed,
            message: "Tenant token quota overrides require SDKWORK_RATE_LIMIT_REDIS_URL (or SDKWORK_REDIS_URL) in production scale-out profiles"
                .to_string(),
        });
    }

    if !config.uses_postgres_runtime_database() {
        checks.push(PreflightCheck {
            name: "runtime_database_role".to_string(),
            status: PreflightStatus::Failed,
            message: "sdkwork-agent-server authoritative persistence requires SDKWORK_DATABASE_ENGINE=postgresql"
                .to_string(),
        });
    }

    if config.is_production_kernel_profile() && !canonical_workspace_postgres_configured() {
        checks.push(PreflightCheck {
            name: "runtime_postgres".to_string(),
            status: PreflightStatus::Failed,
            message: "Production requires an explicit, valid SDKWORK_DATABASE_* PostgreSQL profile"
                .to_string(),
        });
    }

    if !(1..=365).contains(&config.runtime_retention_days) {
        checks.push(PreflightCheck {
            name: "runtime_retention_days".to_string(),
            status: PreflightStatus::Failed,
            message: "Runtime retention must be between 1 and 365 days".to_string(),
        });
    }

    if !(1..=10_000).contains(&config.runtime_cleanup_batch_size) {
        checks.push(PreflightCheck {
            name: "runtime_cleanup_batch_size".to_string(),
            status: PreflightStatus::Failed,
            message: "Runtime cleanup batch size must be between 1 and 10000 rows".to_string(),
        });
    }

    if !(10..=24 * 60 * 60).contains(&config.runtime_cleanup_interval_secs) {
        checks.push(PreflightCheck {
            name: "runtime_cleanup_interval".to_string(),
            status: PreflightStatus::Failed,
            message: "Runtime cleanup interval must be between 10 seconds and 24 hours".to_string(),
        });
    }

    // Check port availability
    checks.push(validate_port(config.port));

    // Check log level
    checks.push(validate_log_level(&config.log_level));

    // Check request timeout
    checks.push(validate_timeout(config.request_timeout_secs));

    // Check body size
    checks.push(validate_body_size(config.max_body_size));

    let passed = checks.iter().all(|c| c.status != PreflightStatus::Failed);

    PreflightResult { checks, passed }
}

fn secure_compare_bytes(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn canonical_workspace_postgres_configured() -> bool {
    if !sdkwork_database_config::workspace_database::workspace_postgres_env_is_configured() {
        return false;
    }
    sdkwork_database_config::DatabaseConfig::from_env("agent_runtime")
        .is_ok_and(|config| config.engine == sdkwork_database_config::DatabaseEngine::Postgres)
}

fn validate_bind_address(address: &str) -> PreflightCheck {
    if address.parse::<std::net::IpAddr>().is_ok() || address == "localhost" {
        PreflightCheck {
            name: "bind_address".to_string(),
            status: PreflightStatus::Passed,
            message: format!("Bind address '{}' is valid", address),
        }
    } else {
        PreflightCheck {
            name: "bind_address".to_string(),
            status: PreflightStatus::Warning,
            message: format!("Bind address '{}' may not be valid", address),
        }
    }
}

fn validate_port(port: u16) -> PreflightCheck {
    if port > 0 {
        PreflightCheck {
            name: "port".to_string(),
            status: PreflightStatus::Passed,
            message: format!("Port {} is valid", port),
        }
    } else {
        PreflightCheck {
            name: "port".to_string(),
            status: PreflightStatus::Failed,
            message: "Port must be greater than 0".to_string(),
        }
    }
}

fn validate_log_level(level: &str) -> PreflightCheck {
    match level.to_lowercase().as_str() {
        "trace" | "debug" | "info" | "warn" | "error" => PreflightCheck {
            name: "log_level".to_string(),
            status: PreflightStatus::Passed,
            message: format!("Log level '{}' is valid", level),
        },
        _ => PreflightCheck {
            name: "log_level".to_string(),
            status: PreflightStatus::Warning,
            message: format!("Unknown log level '{}', defaulting to 'info'", level),
        },
    }
}

fn validate_timeout(timeout: u64) -> PreflightCheck {
    if timeout > 0 && timeout <= 300 {
        PreflightCheck {
            name: "request_timeout".to_string(),
            status: PreflightStatus::Passed,
            message: format!("Request timeout {}s is valid", timeout),
        }
    } else {
        PreflightCheck {
            name: "request_timeout".to_string(),
            status: PreflightStatus::Warning,
            message: format!("Request timeout {}s is unusual", timeout),
        }
    }
}

fn validate_body_size(size: usize) -> PreflightCheck {
    if size > 0 && size <= 4 * 1024 * 1024 {
        PreflightCheck {
            name: "max_body_size".to_string(),
            status: PreflightStatus::Passed,
            message: format!("Max body size {} bytes is valid", size),
        }
    } else {
        PreflightCheck {
            name: "max_body_size".to_string(),
            status: PreflightStatus::Failed,
            message: format!(
                "Max body size {size} bytes is unsafe; configure a value between 1 byte and 4 MiB"
            ),
        }
    }
}

/// Print preflight results
pub fn print_results(result: &PreflightResult) {
    info!("=== Preflight Checks ===");
    for check in &result.checks {
        let icon = match check.status {
            PreflightStatus::Passed => "✓",
            PreflightStatus::Warning => "⚠",
            PreflightStatus::Failed => "✗",
        };
        info!("  {} {}: {}", icon, check.name, check.message);
    }
    info!("========================");

    if result.passed {
        info!("All preflight checks passed");
    } else {
        tracing::error!("Some preflight checks failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_default_config() {
        let _lock = crate::testing::env::lock();
        let _profile = crate::testing::env::VarGuard::set("SDKWORK_KERNEL_PROFILE_ID", None);
        let _environment = crate::testing::env::VarGuard::set("SDKWORK_KERNEL_ENVIRONMENT", None);
        let config = ServerConfig::default();
        let result = validate(&config);
        assert!(result.passed, "{result:?}");
    }

    #[test]
    fn jwt_preflight_requires_material() {
        let config = ServerConfig {
            ingress_auth_mode: "jwt".to_string(),
            ..Default::default()
        };
        let result = validate(&config);
        assert!(!result.passed);
        assert!(result
            .checks
            .iter()
            .any(|check| check.name == "ingress_jwt_material"));
    }

    #[test]
    fn production_jwt_jwks_url_requires_https() {
        let config = ServerConfig {
            environment: "production".to_string(),
            ingress_auth_mode: "jwt".to_string(),
            ingress_jwt_jwks_url: Some("http://idp.example.com/jwks".to_string()),
            ..Default::default()
        };
        let result = validate(&config);
        assert!(result
            .checks
            .iter()
            .any(|check| check.name == "ingress_jwt_jwks_url_https"));
    }

    #[test]
    fn production_requires_an_isolated_cursor_signing_secret() {
        let base = ServerConfig {
            environment: "production".to_string(),
            ingress_auth_mode: "token".to_string(),
            ingress_token: Some("ingress-secret-with-at-least-32-bytes".to_string()),
            metrics_auth_mode: "token".to_string(),
            metrics_token: Some("metrics-secret-with-at-least-32-bytes".to_string()),
            rate_limit_rps: 100,
            idempotency_require_key: true,
            ..Default::default()
        };
        let missing = validate(&base);
        assert!(missing.checks.iter().any(|check| {
            check.name == "cursor_signing_secret" && check.status == PreflightStatus::Failed
        }));

        let reused = validate(&ServerConfig {
            cursor_signing_secret: base.ingress_token.clone(),
            ..base.clone()
        });
        assert!(reused.checks.iter().any(|check| {
            check.name == "cursor_signing_secret_isolation"
                && check.status == PreflightStatus::Failed
        }));

        let isolated = validate(&ServerConfig {
            cursor_signing_secret: Some(
                "cursor-only-signing-secret-with-at-least-32-bytes".to_string(),
            ),
            ..base
        });
        assert!(!isolated.checks.iter().any(|check| {
            matches!(
                check.name.as_str(),
                "cursor_signing_secret" | "cursor_signing_secret_isolation"
            )
        }));
    }

    #[test]
    fn production_requires_a_valid_isolated_approval_payload_key() {
        let reused_raw_key = "0123456789abcdef0123456789abcdef";
        let base = ServerConfig {
            environment: "production".to_string(),
            ingress_auth_mode: "token".to_string(),
            ingress_token: Some(reused_raw_key.to_string()),
            metrics_auth_mode: "token".to_string(),
            metrics_token: Some("metrics-secret-with-at-least-32-bytes".to_string()),
            cursor_signing_secret: Some(
                "cursor-only-signing-secret-with-at-least-32-bytes".to_string(),
            ),
            rate_limit_rps: 100,
            idempotency_require_key: true,
            ..Default::default()
        };
        let missing = validate(&base);
        assert!(missing.checks.iter().any(|check| {
            check.name == "approval_payload_key" && check.status == PreflightStatus::Failed
        }));

        let invalid = validate(&ServerConfig {
            approval_payload_encryption_key: Some(sdkwork_utils_rust::base64url_encode(&[1; 16])),
            ..base.clone()
        });
        assert!(invalid.checks.iter().any(|check| {
            check.name == "approval_payload_key" && check.status == PreflightStatus::Failed
        }));

        let reused = validate(&ServerConfig {
            approval_payload_encryption_key: Some(sdkwork_utils_rust::base64url_encode(
                reused_raw_key.as_bytes(),
            )),
            ..base.clone()
        });
        assert!(reused.checks.iter().any(|check| {
            check.name == "approval_payload_key_isolation"
                && check.status == PreflightStatus::Failed
        }));

        let isolated = validate(&ServerConfig {
            approval_payload_encryption_key: Some(sdkwork_utils_rust::base64url_encode(&[9; 32])),
            ..base
        });
        assert!(!isolated.checks.iter().any(|check| {
            matches!(
                check.name.as_str(),
                "approval_payload_key" | "approval_payload_key_isolation"
            )
        }));
    }

    #[test]
    fn cloud_production_preflight_requires_redis_and_postgres_uri() {
        let config = ServerConfig {
            environment: "production".to_string(),
            deployment_profile: Some("cloud".to_string()),
            kernel_runtime_target: Some("server".to_string()),
            runtime_database_engine: "postgres".to_string(),
            ingress_auth_mode: "token".to_string(),
            ingress_token: Some("secret".to_string()),
            rate_limit_rps: 100,
            metrics_auth_mode: "token".to_string(),
            metrics_token: Some("metrics-secret".to_string()),
            ..Default::default()
        };
        let result = validate(&config);
        assert!(!result.passed);
        assert!(
            result
                .checks
                .iter()
                .any(|check| check.name == "rate_limit_redis"),
            "expected rate_limit_redis failure"
        );
        assert!(
            result
                .checks
                .iter()
                .any(|check| check.name == "runtime_postgres"),
            "expected runtime_postgres failure"
        );
    }

    #[test]
    fn production_rejects_mock_provider_override() {
        let _lock = crate::testing::env::lock();
        let _profile = crate::testing::env::VarGuard::set(
            "SDKWORK_KERNEL_PROFILE_ID",
            Some("cloud.production"),
        );
        let _mock =
            crate::testing::env::VarGuard::set("SDKWORK_KERNEL_ALLOW_MOCK_PROVIDERS", Some("1"));
        let config = ServerConfig {
            environment: "production".to_string(),
            ingress_auth_mode: "token".to_string(),
            ingress_token: Some("secret".to_string()),
            rate_limit_rps: 100,
            ..Default::default()
        };
        let result = validate(&config);
        assert!(
            result
                .checks
                .iter()
                .any(|check| check.name == "mock_providers"),
            "production must fail when mock override is enabled"
        );
    }

    #[test]
    fn production_topology_profile_requires_rate_limit_without_environment_literal() {
        let _lock = crate::testing::env::lock();
        let _profile = crate::testing::env::VarGuard::set(
            "SDKWORK_KERNEL_PROFILE_ID",
            Some("cloud.production"),
        );
        let config = ServerConfig {
            environment: "development".to_string(),
            deployment_profile: Some("cloud".to_string()),
            kernel_runtime_target: Some("server".to_string()),
            rate_limit_rps: 0,
            ..Default::default()
        };
        let result = validate(&config);
        assert!(
            result
                .checks
                .iter()
                .any(|check| check.name == "rate_limit"),
            "topology production profile must enforce rate limits even when environment is not production"
        );
    }
}
