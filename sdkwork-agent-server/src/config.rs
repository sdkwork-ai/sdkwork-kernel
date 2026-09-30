use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Per-tenant rate limit override for commercial quota profiles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TenantRateLimitOverride {
    pub rps: u32,
    pub burst: u32,
}

/// Per-tenant daily model token quota for commercial billing enforcement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TenantTokenQuotaOverride {
    pub daily_tokens: u64,
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Server bind address
    pub bind_address: String,
    /// Server port
    pub port: u16,
    /// Log level (trace, debug, info, warn, error)
    pub log_level: String,
    /// Enable CORS
    pub cors_enabled: bool,
    /// Allowed CORS origins
    pub cors_origins: Vec<String>,
    /// Request timeout in seconds
    pub request_timeout_secs: u64,
    /// Max request body size in bytes
    pub max_body_size: usize,
    /// Health check endpoint path
    pub health_path: String,
    /// Canonical workspace database engine. The server accepts PostgreSQL only.
    pub runtime_database_engine: String,
    /// Retention window for transient sessions, messages, tasks, events, and permissions.
    pub runtime_retention_days: u32,
    /// Maximum rows selected per table in one cleanup transaction.
    pub runtime_cleanup_batch_size: u32,
    /// Interval between runtime cleanup cycles.
    pub runtime_cleanup_interval_secs: u64,
    /// Topology deployment profile: `standalone` | `cloud`.
    pub deployment_profile: Option<String>,
    /// Runtime target (for example server, desktop).
    pub kernel_runtime_target: Option<String>,
    /// Runtime environment: development | production
    pub environment: String,
    /// Topology profile id (`SDKWORK_KERNEL_PROFILE_ID`) captured at config load.
    pub kernel_profile_id: Option<String>,
    /// Ingress auth mode: open | token | jwt
    pub ingress_auth_mode: String,
    /// Required bearer/static token when ingress_auth_mode is token
    pub ingress_token: Option<String>,
    /// HS256 secret for ingress_auth_mode jwt
    pub ingress_jwt_secret: Option<String>,
    /// Optional issuer (`iss`) for ingress JWT validation
    pub ingress_jwt_issuer: Option<String>,
    /// Optional audience (`aud`) for ingress JWT validation
    pub ingress_jwt_audience: Option<String>,
    /// Ingress JWT signing algorithm profile: hs256 | rs256 (when not using JWKS file)
    pub ingress_jwt_algorithm: String,
    /// RS256 public key PEM for ingress JWT validation
    pub ingress_jwt_rsa_public_key_pem: Option<String>,
    /// Local JWKS JSON file for RS256 ingress JWT validation (kid lookup)
    pub ingress_jwt_jwks_file: Option<String>,
    /// Remote JWKS URL fetched once at startup for RS256 ingress JWT validation (kid lookup)
    pub ingress_jwt_jwks_url: Option<String>,
    /// Fixed tenant identity for bound ingress identity mode (non-loopback token deployments).
    pub ingress_bound_tenant_id: Option<String>,
    /// Fixed user identity for bound ingress identity mode (non-loopback token deployments).
    pub ingress_bound_user_id: Option<String>,
    /// Per-client request rate limit (requests/sec). Zero disables limiting.
    pub rate_limit_rps: u32,
    /// Burst capacity for the token-bucket rate limiter.
    pub rate_limit_burst: u32,
    /// Redis URL for distributed rate limiting (`redis_cache` profile).
    pub rate_limit_redis_url: Option<String>,
    /// Dedicated Redis URL for distributed HTTP idempotency records.
    /// This credential is intentionally independent from rate-limit Redis.
    pub idempotency_redis_url: Option<String>,
    /// Dedicated Redis URL for the cross-pod SSE event-wakeup fan-out.
    /// Falls back to `SDKWORK_REDIS_URL` when unset; absent entirely, SSE
    /// streams rely on their durable poll schedule alone.
    pub event_fanout_redis_url: Option<String>,
    /// Runtime coordination mode: `single` (default) or `cluster`. Cluster
    /// mode requires the cross-pod event fan-out to be configured.
    pub coordination_mode: String,
    /// Completed idempotency response retention in seconds.
    pub idempotency_ttl_secs: u64,
    /// Maximum JSON response body retained for idempotent replay.
    pub idempotency_max_cached_response_bytes: usize,
    /// Require `Idempotency-Key` on retry-sensitive mutation routes.
    pub idempotency_require_key: bool,
    /// Optional per-tenant rate limit overrides keyed by tenant id.
    pub tenant_rate_limit_overrides: HashMap<String, TenantRateLimitOverride>,
    /// Optional per-tenant daily model token quotas keyed by tenant id.
    pub tenant_token_quota_overrides: HashMap<String, TenantTokenQuotaOverride>,
    /// Metrics scrape auth mode: open | token
    pub metrics_auth_mode: String,
    /// Bearer token for `/metrics` when metrics_auth_mode is token.
    pub metrics_token: Option<String>,
    /// Dedicated HMAC secret for opaque pagination cursor signatures.
    pub cursor_signing_secret: Option<String>,
    /// Dedicated base64url-encoded 256-bit key for resumable approval payloads.
    pub approval_payload_encryption_key: Option<String>,
    /// Maximum lifetime of an unexecuted approval-gated operation.
    pub permission_operation_ttl_secs: u64,
    /// Optional OTLP HTTP endpoint for distributed tracing export.
    pub otel_exporter_otlp_endpoint: Option<String>,
    /// SSE/streaming request timeout in seconds (long-lived connections).
    pub sse_request_timeout_secs: u64,
    /// Maximum concurrent synchronous provider executions per server instance.
    pub provider_max_concurrency: usize,
    /// Maximum provider invocations allowed to wait for execution capacity.
    pub provider_max_waiters: usize,
    /// Maximum time a provider invocation may wait for execution capacity.
    pub provider_admission_timeout_ms: u64,
    /// Maximum concurrent blocking persistence operations per server instance.
    pub persistence_max_concurrency: usize,
    /// Maximum persistence operations allowed to wait for blocking capacity.
    pub persistence_max_waiters: usize,
    /// Maximum time a persistence operation may wait for blocking capacity.
    pub persistence_admission_timeout_ms: u64,
    /// Number of durable task workers per server instance.
    pub task_worker_max_concurrency: usize,
    /// Idle polling interval for durable task claims.
    pub task_worker_poll_interval_ms: u64,
    /// Lease duration for one claimed task run.
    pub task_worker_lease_secs: u64,
    /// Maximum execution attempts per durable task run before a transient
    /// failure becomes permanent.
    pub task_worker_max_attempts: u64,
    /// Base exponential backoff (seconds) for transiently failed task runs.
    pub task_worker_retry_backoff_base_secs: u64,
    /// Upper bound (seconds) for the retry backoff.
    pub task_worker_retry_backoff_max_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1".to_string(),
            port: 8080,
            log_level: "info".to_string(),
            cors_enabled: true,
            cors_origins: vec![
                "http://127.0.0.1:5173".to_string(),
                "http://localhost:5173".to_string(),
            ],
            request_timeout_secs: 30,
            max_body_size: 1024 * 1024, // 1 MiB hard envelope; fields have stricter limits.
            health_path: "/healthz".to_string(),
            runtime_database_engine: "postgresql".to_string(),
            runtime_retention_days: 7,
            runtime_cleanup_batch_size: 500,
            runtime_cleanup_interval_secs: 300,
            deployment_profile: None,
            kernel_runtime_target: None,
            environment: "development".to_string(),
            kernel_profile_id: None,
            ingress_auth_mode: "open".to_string(),
            ingress_token: None,
            ingress_jwt_secret: None,
            ingress_jwt_issuer: None,
            ingress_jwt_audience: None,
            ingress_jwt_algorithm: "hs256".to_string(),
            ingress_jwt_rsa_public_key_pem: None,
            ingress_jwt_jwks_file: None,
            ingress_jwt_jwks_url: None,
            ingress_bound_tenant_id: None,
            ingress_bound_user_id: None,
            rate_limit_rps: 0,
            rate_limit_burst: 200,
            rate_limit_redis_url: None,
            idempotency_redis_url: None,
            event_fanout_redis_url: None,
            coordination_mode: "single".to_string(),
            idempotency_ttl_secs: 24 * 60 * 60,
            idempotency_max_cached_response_bytes: 512 * 1024,
            idempotency_require_key: false,
            tenant_rate_limit_overrides: HashMap::new(),
            tenant_token_quota_overrides: HashMap::new(),
            metrics_auth_mode: "open".to_string(),
            metrics_token: None,
            cursor_signing_secret: None,
            approval_payload_encryption_key: None,
            permission_operation_ttl_secs: 15 * 60,
            otel_exporter_otlp_endpoint: None,
            sse_request_timeout_secs: 3600,
            provider_max_concurrency: 64,
            provider_max_waiters: 64,
            provider_admission_timeout_ms: 5_000,
            persistence_max_concurrency: 64,
            persistence_max_waiters: 128,
            persistence_admission_timeout_ms: 2_000,
            task_worker_max_concurrency: 4,
            task_worker_poll_interval_ms: 250,
            task_worker_lease_secs: 120,
            task_worker_max_attempts: 3,
            task_worker_retry_backoff_base_secs: 5,
            task_worker_retry_backoff_max_secs: 300,
        }
    }
}

impl ServerConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> anyhow::Result<Self> {
        let mut config = Self::default();

        if let Ok(bind) = std::env::var("SDKWORK_KERNEL_APPLICATION_PUBLIC_INGRESS_BIND") {
            let (bind_address, port) =
                parse_ingress_bind("SDKWORK_KERNEL_APPLICATION_PUBLIC_INGRESS_BIND", &bind)?;
            config.bind_address = bind_address;
            config.port = port;
        }
        if let Ok(level) = std::env::var("SDKWORK_LOG_LEVEL") {
            config.log_level = level;
        }
        if let Ok(cors) = std::env::var("SDKWORK_CORS_ENABLED") {
            config.cors_enabled = cors.trim().parse().map_err(|error| {
                anyhow::anyhow!("SDKWORK_CORS_ENABLED must be true or false: {error}")
            })?;
        }
        if let Ok(origins) = std::env::var("SDKWORK_CORS_ALLOWED_ORIGINS") {
            config.cors_origins = origins.split(',').map(|s| s.trim().to_string()).collect();
        }
        if let Ok(timeout) = std::env::var("SDKWORK_REQUEST_TIMEOUT") {
            config.request_timeout_secs = timeout.parse()?;
        }
        let database = sdkwork_database_config::DatabaseConfig::from_env("agent_runtime").map_err(
            |error| anyhow::anyhow!("invalid SDKWORK_DATABASE_* configuration: {error}"),
        )?;
        config.runtime_database_engine = match database.engine {
            sdkwork_database_config::DatabaseEngine::Postgres => "postgresql".to_string(),
            sdkwork_database_config::DatabaseEngine::Sqlite => anyhow::bail!(
                "sdkwork-agent-server requires SDKWORK_DATABASE_ENGINE=postgresql; SQLite is limited to declared client-local stores and test fixtures"
            ),
        };
        if let Ok(days) = std::env::var("SDKWORK_AGENT_RUNTIME_RETENTION_DAYS") {
            config.runtime_retention_days = days.parse()?;
        }
        if let Ok(batch_size) = std::env::var("SDKWORK_AGENT_RUNTIME_CLEANUP_BATCH_SIZE") {
            config.runtime_cleanup_batch_size = batch_size.parse()?;
        }
        if let Ok(interval) = std::env::var("SDKWORK_AGENT_RUNTIME_CLEANUP_INTERVAL_SECS") {
            config.runtime_cleanup_interval_secs = interval.parse()?;
        }
        if let Ok(profile) = std::env::var("SDKWORK_KERNEL_DEPLOYMENT_PROFILE") {
            let trimmed = profile.trim().to_string();
            if !trimmed.is_empty() {
                config.deployment_profile = Some(trimmed);
            }
        }
        if let Ok(target) = std::env::var("SDKWORK_KERNEL_RUNTIME_TARGET") {
            let trimmed = target.trim().to_string();
            if !trimmed.is_empty() {
                config.kernel_runtime_target = Some(trimmed);
            }
        }
        if let Ok(environment) = std::env::var("SDKWORK_KERNEL_ENVIRONMENT") {
            config.environment = environment;
        }
        if let Ok(profile_id) = std::env::var("SDKWORK_KERNEL_PROFILE_ID") {
            config.kernel_profile_id =
                sdkwork_agent_kernel::normalize_kernel_profile_id(&profile_id);
        }
        if let Ok(auth_mode) = std::env::var("SDKWORK_KERNEL_INGRESS_AUTH_MODE") {
            config.ingress_auth_mode = auth_mode;
        }
        if let Ok(token) = std::env::var("SDKWORK_KERNEL_INGRESS_TOKEN") {
            let trimmed = token.trim().to_string();
            if !trimmed.is_empty() {
                config.ingress_token = Some(trimmed);
            }
        }
        if let Ok(tenant_id) = std::env::var("SDKWORK_KERNEL_INGRESS_BOUND_TENANT_ID") {
            let trimmed = tenant_id.trim().to_string();
            if !trimmed.is_empty() {
                config.ingress_bound_tenant_id = Some(trimmed);
            }
        }
        if let Ok(user_id) = std::env::var("SDKWORK_KERNEL_INGRESS_BOUND_USER_ID") {
            let trimmed = user_id.trim().to_string();
            if !trimmed.is_empty() {
                config.ingress_bound_user_id = Some(trimmed);
            }
        }
        if let Ok(secret) = std::env::var("SDKWORK_KERNEL_INGRESS_JWT_SECRET") {
            let trimmed = secret.trim().to_string();
            if !trimmed.is_empty() {
                config.ingress_jwt_secret = Some(trimmed);
            }
        }
        if let Ok(issuer) = std::env::var("SDKWORK_KERNEL_INGRESS_JWT_ISSUER") {
            let trimmed = issuer.trim().to_string();
            if !trimmed.is_empty() {
                config.ingress_jwt_issuer = Some(trimmed);
            }
        }
        if let Ok(audience) = std::env::var("SDKWORK_KERNEL_INGRESS_JWT_AUDIENCE") {
            let trimmed = audience.trim().to_string();
            if !trimmed.is_empty() {
                config.ingress_jwt_audience = Some(trimmed);
            }
        }
        if let Ok(algorithm) = std::env::var("SDKWORK_KERNEL_INGRESS_JWT_ALGORITHM") {
            let trimmed = algorithm.trim().to_string();
            if !trimmed.is_empty() {
                config.ingress_jwt_algorithm = trimmed;
            }
        }
        if let Ok(pem) = std::env::var("SDKWORK_KERNEL_INGRESS_JWT_RSA_PUBLIC_KEY_PEM") {
            let trimmed = pem.trim().to_string();
            if !trimmed.is_empty() {
                config.ingress_jwt_rsa_public_key_pem = Some(trimmed);
            }
        }
        if let Ok(jwks_file) = std::env::var("SDKWORK_KERNEL_INGRESS_JWT_JWKS_FILE") {
            let trimmed = jwks_file.trim().to_string();
            if !trimmed.is_empty() {
                config.ingress_jwt_jwks_file = Some(trimmed);
            }
        }
        if let Ok(jwks_url) = std::env::var("SDKWORK_KERNEL_INGRESS_JWT_JWKS_URL") {
            let trimmed = jwks_url.trim().to_string();
            if !trimmed.is_empty() {
                config.ingress_jwt_jwks_url = Some(trimmed);
            }
        }
        if let Ok(overrides) = std::env::var("SDKWORK_TENANT_RATE_LIMIT_OVERRIDES") {
            let trimmed = overrides.trim();
            if !trimmed.is_empty() {
                config.tenant_rate_limit_overrides =
                    serde_json::from_str(trimmed).map_err(|error| {
                        anyhow::anyhow!(
                            "SDKWORK_TENANT_RATE_LIMIT_OVERRIDES must be JSON object: {error}"
                        )
                    })?;
            }
        }
        if let Ok(overrides) = std::env::var("SDKWORK_TENANT_TOKEN_QUOTA_OVERRIDES") {
            let trimmed = overrides.trim();
            if !trimmed.is_empty() {
                config.tenant_token_quota_overrides =
                    serde_json::from_str(trimmed).map_err(|error| {
                        anyhow::anyhow!(
                            "SDKWORK_TENANT_TOKEN_QUOTA_OVERRIDES must be JSON object: {error}"
                        )
                    })?;
            }
        }
        if let Ok(rps) = std::env::var("SDKWORK_RATE_LIMIT_RPS") {
            config.rate_limit_rps = rps.trim().parse().map_err(|error| {
                anyhow::anyhow!("SDKWORK_RATE_LIMIT_RPS must be a non-negative integer: {error}")
            })?;
        }
        if let Ok(burst) = std::env::var("SDKWORK_RATE_LIMIT_BURST") {
            config.rate_limit_burst = burst.trim().parse().map_err(|error| {
                anyhow::anyhow!("SDKWORK_RATE_LIMIT_BURST must be a non-negative integer: {error}")
            })?;
        }
        if let Ok(redis_url) = std::env::var("SDKWORK_RATE_LIMIT_REDIS_URL")
            .or_else(|_| std::env::var("SDKWORK_REDIS_URL"))
        {
            let trimmed = redis_url.trim().to_string();
            if !trimmed.is_empty() {
                config.rate_limit_redis_url = Some(trimmed);
            }
        }
        if let Ok(redis_url) = std::env::var("SDKWORK_IDEMPOTENCY_REDIS_URL") {
            let trimmed = redis_url.trim().to_string();
            if !trimmed.is_empty() {
                config.idempotency_redis_url = Some(trimmed);
            }
        }
        if let Ok(redis_url) = std::env::var("SDKWORK_EVENT_FANOUT_REDIS_URL")
            .or_else(|_| std::env::var("SDKWORK_REDIS_URL"))
        {
            let trimmed = redis_url.trim().to_string();
            if !trimmed.is_empty() {
                config.event_fanout_redis_url = Some(trimmed);
            }
        }
        if let Ok(mode) = std::env::var("SDKWORK_COORDINATION_MODE") {
            let trimmed = mode.trim().to_ascii_lowercase();
            match trimmed.as_str() {
                "" => {}
                "single" | "cluster" => config.coordination_mode = trimmed,
                other => {
                    return Err(anyhow::anyhow!(
                        "SDKWORK_COORDINATION_MODE must be 'single' or 'cluster', got '{other}'"
                    ));
                }
            }
        }
        if let Ok(ttl_secs) = std::env::var("SDKWORK_IDEMPOTENCY_TTL_SECS") {
            config.idempotency_ttl_secs = ttl_secs.parse()?;
        }
        if let Ok(max_bytes) = std::env::var("SDKWORK_IDEMPOTENCY_MAX_RESPONSE_BYTES") {
            config.idempotency_max_cached_response_bytes = max_bytes.parse()?;
        }
        if let Ok(require_key) = std::env::var("SDKWORK_IDEMPOTENCY_REQUIRE_KEY") {
            config.idempotency_require_key = require_key.parse()?;
        }
        if let Ok(sse_timeout) = std::env::var("SDKWORK_SSE_REQUEST_TIMEOUT") {
            config.sse_request_timeout_secs = sse_timeout.parse()?;
        }
        if let Ok(concurrency) = std::env::var("SDKWORK_PROVIDER_MAX_CONCURRENCY") {
            config.provider_max_concurrency = concurrency.parse()?;
        }
        if !(1..=1024).contains(&config.provider_max_concurrency) {
            anyhow::bail!("SDKWORK_PROVIDER_MAX_CONCURRENCY must be between 1 and 1024");
        }
        if let Ok(waiters) = std::env::var("SDKWORK_PROVIDER_MAX_WAITERS") {
            config.provider_max_waiters = waiters.parse()?;
        }
        if config.provider_max_waiters > 4096 {
            anyhow::bail!("SDKWORK_PROVIDER_MAX_WAITERS must be between 0 and 4096");
        }
        if let Ok(timeout_ms) = std::env::var("SDKWORK_PROVIDER_ADMISSION_TIMEOUT_MS") {
            config.provider_admission_timeout_ms = timeout_ms.parse()?;
        }
        if !(1..=60_000).contains(&config.provider_admission_timeout_ms) {
            anyhow::bail!("SDKWORK_PROVIDER_ADMISSION_TIMEOUT_MS must be between 1 and 60000");
        }
        if let Ok(concurrency) = std::env::var("SDKWORK_PERSISTENCE_MAX_CONCURRENCY") {
            config.persistence_max_concurrency = concurrency.parse()?;
        }
        if !(1..=1024).contains(&config.persistence_max_concurrency) {
            anyhow::bail!("SDKWORK_PERSISTENCE_MAX_CONCURRENCY must be between 1 and 1024");
        }
        if let Ok(waiters) = std::env::var("SDKWORK_PERSISTENCE_MAX_WAITERS") {
            config.persistence_max_waiters = waiters.parse()?;
        }
        if config.persistence_max_waiters > 4096 {
            anyhow::bail!("SDKWORK_PERSISTENCE_MAX_WAITERS must be between 0 and 4096");
        }
        if let Ok(timeout_ms) = std::env::var("SDKWORK_PERSISTENCE_ADMISSION_TIMEOUT_MS") {
            config.persistence_admission_timeout_ms = timeout_ms.parse()?;
        }
        if !(1..=60_000).contains(&config.persistence_admission_timeout_ms) {
            anyhow::bail!("SDKWORK_PERSISTENCE_ADMISSION_TIMEOUT_MS must be between 1 and 60000");
        }
        if let Ok(concurrency) = std::env::var("SDKWORK_TASK_WORKER_MAX_CONCURRENCY") {
            config.task_worker_max_concurrency = concurrency.parse()?;
        }
        if !(1..=128).contains(&config.task_worker_max_concurrency) {
            anyhow::bail!("SDKWORK_TASK_WORKER_MAX_CONCURRENCY must be between 1 and 128");
        }
        if let Ok(interval_ms) = std::env::var("SDKWORK_TASK_WORKER_POLL_INTERVAL_MS") {
            config.task_worker_poll_interval_ms = interval_ms.parse()?;
        }
        if !(50..=60_000).contains(&config.task_worker_poll_interval_ms) {
            anyhow::bail!("SDKWORK_TASK_WORKER_POLL_INTERVAL_MS must be between 50 and 60000");
        }
        if let Ok(lease_secs) = std::env::var("SDKWORK_TASK_WORKER_LEASE_SECS") {
            config.task_worker_lease_secs = lease_secs.parse()?;
        }
        if !(10..=3_600).contains(&config.task_worker_lease_secs) {
            anyhow::bail!("SDKWORK_TASK_WORKER_LEASE_SECS must be between 10 and 3600");
        }
        if let Ok(max_attempts) = std::env::var("SDKWORK_TASK_WORKER_MAX_ATTEMPTS") {
            config.task_worker_max_attempts = max_attempts.parse()?;
        }
        if !(1..=10).contains(&config.task_worker_max_attempts) {
            anyhow::bail!("SDKWORK_TASK_WORKER_MAX_ATTEMPTS must be between 1 and 10");
        }
        if let Ok(backoff_secs) = std::env::var("SDKWORK_TASK_WORKER_RETRY_BACKOFF_BASE_SECS") {
            config.task_worker_retry_backoff_base_secs = backoff_secs.parse()?;
        }
        if !(1..=60).contains(&config.task_worker_retry_backoff_base_secs) {
            anyhow::bail!("SDKWORK_TASK_WORKER_RETRY_BACKOFF_BASE_SECS must be between 1 and 60");
        }
        if let Ok(backoff_secs) = std::env::var("SDKWORK_TASK_WORKER_RETRY_BACKOFF_MAX_SECS") {
            config.task_worker_retry_backoff_max_secs = backoff_secs.parse()?;
        }
        if !(config.task_worker_retry_backoff_base_secs..=3_600)
            .contains(&config.task_worker_retry_backoff_max_secs)
        {
            anyhow::bail!("SDKWORK_TASK_WORKER_RETRY_BACKOFF_MAX_SECS must be between the base backoff and 3600");
        }
        if let Ok(mode) = std::env::var("SDKWORK_KERNEL_METRICS_AUTH_MODE") {
            config.metrics_auth_mode = mode;
        }
        if let Ok(token) = std::env::var("SDKWORK_KERNEL_METRICS_TOKEN") {
            let trimmed = token.trim().to_string();
            if !trimmed.is_empty() {
                config.metrics_token = Some(trimmed);
            }
        }
        if let Ok(secret) = std::env::var("SDKWORK_CURSOR_SIGNING_SECRET") {
            let trimmed = secret.trim().to_string();
            if !trimmed.is_empty() {
                config.cursor_signing_secret = Some(trimmed);
            }
        }
        if let Ok(key) = std::env::var("SDKWORK_APPROVAL_PAYLOAD_ENCRYPTION_KEY") {
            let trimmed = key.trim().to_string();
            if !trimmed.is_empty() {
                let decoded = sdkwork_utils_rust::base64url_decode(&trimmed).ok_or_else(|| {
                    anyhow::anyhow!(
                        "SDKWORK_APPROVAL_PAYLOAD_ENCRYPTION_KEY must be base64url encoded"
                    )
                })?;
                if decoded.len() != 32 {
                    anyhow::bail!(
                        "SDKWORK_APPROVAL_PAYLOAD_ENCRYPTION_KEY must decode to exactly 32 bytes"
                    );
                }
                config.approval_payload_encryption_key = Some(trimmed);
            }
        }
        if let Ok(ttl_secs) = std::env::var("SDKWORK_PERMISSION_OPERATION_TTL_SECS") {
            config.permission_operation_ttl_secs = ttl_secs.parse()?;
        }
        if !(60..=86_400).contains(&config.permission_operation_ttl_secs) {
            anyhow::bail!("SDKWORK_PERMISSION_OPERATION_TTL_SECS must be between 60 and 86400");
        }
        if let Ok(endpoint) = std::env::var("SDKWORK_OTEL_EXPORTER_OTLP_ENDPOINT") {
            let trimmed = endpoint.trim().to_string();
            if !trimmed.is_empty() {
                config.otel_exporter_otlp_endpoint = Some(trimmed);
            }
        }
        if config.is_production_kernel_profile()
            && config.ingress_auth_mode.eq_ignore_ascii_case("open")
        {
            config.ingress_auth_mode = "token".to_string();
        }
        config.normalize_security();

        Ok(config)
    }

    fn normalize_security(&mut self) {
        if !self.is_loopback_bind() && self.ingress_auth_mode.eq_ignore_ascii_case("open") {
            self.ingress_auth_mode = "token".to_string();
        }
        if self.is_production_kernel_profile()
            && self.cors_origins.iter().any(|origin| origin == "*")
        {
            self.cors_origins = vec![
                "https://kernel.sdkwork.com".to_string(),
                "https://app.sdkwork.com".to_string(),
            ];
        }
        if self.is_production_kernel_profile()
            && self
                .cors_origins
                .iter()
                .all(|origin| origin.contains("127.0.0.1") || origin.contains("localhost"))
        {
            self.cors_origins = vec![
                "https://kernel.sdkwork.com".to_string(),
                "https://app.sdkwork.com".to_string(),
            ];
        }
        if self.rate_limit_rps == 0
            && (self.is_production_kernel_profile() || !self.is_loopback_bind())
        {
            self.rate_limit_rps = if self.is_production_kernel_profile() {
                100
            } else {
                50
            };
        }
        if self.rate_limit_rps > 0 && self.rate_limit_burst == 0 {
            self.rate_limit_burst = self.rate_limit_rps.saturating_mul(2);
        }
        if (self.is_production_kernel_profile() || !self.is_loopback_bind())
            && self.metrics_auth_mode.eq_ignore_ascii_case("open")
        {
            self.metrics_auth_mode = "token".to_string();
        }
        if self.is_production_kernel_profile() || !self.is_loopback_bind() {
            self.idempotency_require_key = true;
        }
        // Metrics credentials are intentionally independent from ingress
        // credentials. Production must fail closed when the dedicated token
        // is absent rather than widening the ingress token's privilege.
    }

    pub fn metrics_auth_required(&self) -> bool {
        self.metrics_auth_mode.eq_ignore_ascii_case("token")
    }

    pub fn effective_metrics_token(&self) -> Option<&str> {
        self.metrics_token
            .as_deref()
            .filter(|token| !token.is_empty())
    }

    pub fn otel_export_enabled(&self) -> bool {
        self.otel_exporter_otlp_endpoint
            .as_deref()
            .is_some_and(|endpoint| !endpoint.is_empty())
    }

    pub fn uses_postgres_runtime_database(&self) -> bool {
        matches!(
            self.runtime_database_engine.to_ascii_lowercase().as_str(),
            "postgres" | "postgresql"
        )
    }

    pub fn effective_cursor_signing_secret(&self) -> &str {
        self.cursor_signing_secret
            .as_deref()
            .unwrap_or("sdkwork-kernel-local-cursor-v2")
    }

    pub fn effective_rate_limit_redis_url(&self) -> Option<&str> {
        self.rate_limit_redis_url
            .as_deref()
            .filter(|url| !url.is_empty())
    }

    pub fn effective_idempotency_redis_url(&self) -> Option<&str> {
        self.idempotency_redis_url
            .as_deref()
            .filter(|url| !url.is_empty())
    }

    pub fn effective_deployment_profile(&self) -> &'static str {
        match self
            .deployment_profile
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("cloud") => "cloud",
            _ => "standalone",
        }
    }

    pub(crate) fn production_scaleout_profile(&self) -> bool {
        self.effective_deployment_profile() == "cloud"
            || self.kernel_runtime_target.as_deref() == Some("server")
    }

    pub fn requires_distributed_rate_limit(&self) -> bool {
        if self.rate_limit_rps == 0 {
            return false;
        }
        self.is_production_kernel_profile() && self.production_scaleout_profile()
    }

    pub fn requires_distributed_idempotency(&self) -> bool {
        // Every production profile, not only scale-out: replayable mutations
        // must survive replica restarts and stay consistent across replicas,
        // and the bounded-entry memory store has no total-byte budget, so a
        // production deployment caching model-output-sized responses in
        // process could accumulate gigabytes. Startup fails closed instead.
        self.is_production_kernel_profile()
    }

    /// Whether the runtime participates in cluster coordination. Cluster mode
    /// requires the cross-pod event fan-out (preflight enforces it) so SSE
    /// subscribers see events persisted on other replicas without waiting
    /// out the durable poll backoff.
    pub fn is_cluster_coordination(&self) -> bool {
        self.coordination_mode.eq_ignore_ascii_case("cluster")
    }

    /// Redis URL for the cross-pod event fan-out, if configured.
    pub fn effective_event_fanout_redis_url(&self) -> Option<&str> {
        self.event_fanout_redis_url
            .as_deref()
            .filter(|url| !url.is_empty())
    }

    pub fn ingress_auth_secured(&self) -> bool {
        self.ingress_auth_mode.eq_ignore_ascii_case("token")
            || self.ingress_auth_mode.eq_ignore_ascii_case("jwt")
    }

    pub fn has_ingress_jwt_material(&self) -> bool {
        self.ingress_jwt_secret
            .as_deref()
            .is_some_and(|value| !value.is_empty())
            || self
                .ingress_jwt_rsa_public_key_pem
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            || self
                .ingress_jwt_jwks_file
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            || self
                .ingress_jwt_jwks_url
                .as_deref()
                .is_some_and(|value| !value.is_empty())
    }

    pub fn is_development(&self) -> bool {
        self.environment.eq_ignore_ascii_case("development")
    }

    /// When true, typed provider failures fall back to the bridge mock path in development.
    pub fn allow_mock_provider_fallback(&self) -> bool {
        sdkwork_agent_kernel::mock_provider_invocation_allowed(
            &self.environment,
            self.effective_kernel_profile_id().as_deref(),
        )
    }

    pub fn is_production_kernel_profile(&self) -> bool {
        sdkwork_agent_kernel::is_production_kernel_profile(
            &self.environment,
            self.effective_kernel_profile_id().as_deref(),
        )
    }

    fn effective_kernel_profile_id(&self) -> Option<String> {
        self.kernel_profile_id
            .clone()
            .or_else(sdkwork_agent_kernel::kernel_profile_id_from_env)
    }

    /// Get the full bind address (address:port)
    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.bind_address, self.port)
    }

    /// Get the base URL for the server
    pub fn base_url(&self) -> String {
        format!("http://localhost:{}", self.port)
    }
}

fn parse_ingress_bind(env_name: &str, bind: &str) -> anyhow::Result<(String, u16)> {
    let trimmed = bind.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{env_name} cannot be empty");
    }

    if let Ok(socket_addr) = trimmed.parse::<std::net::SocketAddr>() {
        return Ok((socket_addr.ip().to_string(), socket_addr.port()));
    }

    let (host, port) = trimmed
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("{env_name} must be host:port or a socket address"))?;
    let port = port
        .parse::<u16>()
        .map_err(|error| anyhow::anyhow!("{env_name} port is invalid: {error}"))?;
    Ok((host.to_string(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = ServerConfig::default();
        assert_eq!(config.bind_address, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert_eq!(config.log_level, "info");
        assert!(config.cors_enabled);
        assert!(!config.cors_origins.iter().any(|origin| origin == "*"));
        assert_eq!(config.provider_max_concurrency, 64);
        assert_eq!(config.provider_max_waiters, 64);
        assert_eq!(config.provider_admission_timeout_ms, 5_000);
        assert_eq!(config.persistence_max_concurrency, 64);
        assert_eq!(config.persistence_max_waiters, 128);
        assert_eq!(config.persistence_admission_timeout_ms, 2_000);
    }

    #[test]
    fn provider_admission_config_rejects_unbounded_waiters() {
        let _lock = crate::testing::env::lock();
        let _waiters =
            crate::testing::env::VarGuard::set("SDKWORK_PROVIDER_MAX_WAITERS", Some("4097"));
        let error = ServerConfig::from_env().expect_err("unbounded waiters must fail startup");
        assert!(error
            .to_string()
            .contains("SDKWORK_PROVIDER_MAX_WAITERS must be between 0 and 4096"));
    }

    #[test]
    fn provider_admission_config_rejects_invalid_timeout() {
        let _lock = crate::testing::env::lock();
        let _timeout =
            crate::testing::env::VarGuard::set("SDKWORK_PROVIDER_ADMISSION_TIMEOUT_MS", Some("0"));
        let error = ServerConfig::from_env().expect_err("zero admission timeout must fail startup");
        assert!(error
            .to_string()
            .contains("SDKWORK_PROVIDER_ADMISSION_TIMEOUT_MS must be between 1 and 60000"));
    }

    #[test]
    fn persistence_admission_config_rejects_unbounded_waiters() {
        let _lock = crate::testing::env::lock();
        let _waiters =
            crate::testing::env::VarGuard::set("SDKWORK_PERSISTENCE_MAX_WAITERS", Some("4097"));
        let error = ServerConfig::from_env().expect_err("unbounded waiters must fail startup");
        assert!(error
            .to_string()
            .contains("SDKWORK_PERSISTENCE_MAX_WAITERS must be between 0 and 4096"));
    }

    #[test]
    fn persistence_admission_config_rejects_invalid_timeout() {
        let _lock = crate::testing::env::lock();
        let _timeout = crate::testing::env::VarGuard::set(
            "SDKWORK_PERSISTENCE_ADMISSION_TIMEOUT_MS",
            Some("0"),
        );
        let error = ServerConfig::from_env().expect_err("zero admission timeout must fail startup");
        assert!(error
            .to_string()
            .contains("SDKWORK_PERSISTENCE_ADMISSION_TIMEOUT_MS must be between 1 and 60000"));
    }

    #[test]
    fn cloud_production_requires_postgres_and_redis_profiles() {
        let config = ServerConfig {
            environment: "production".to_string(),
            deployment_profile: Some("cloud".to_string()),
            kernel_runtime_target: Some("server".to_string()),
            runtime_database_engine: "postgres".to_string(),
            rate_limit_rps: 100,
            ..Default::default()
        };
        assert!(config.uses_postgres_runtime_database());
        assert!(config.requires_distributed_rate_limit());
        assert!(config.requires_distributed_idempotency());
    }

    #[test]
    fn bind_addr() {
        let config = ServerConfig::default();
        assert_eq!(config.bind_addr(), "127.0.0.1:8080");
    }

    #[test]
    fn base_url() {
        let config = ServerConfig::default();
        assert_eq!(config.base_url(), "http://localhost:8080");
    }

    #[test]
    fn production_normalizes_metrics_auth_to_token() {
        let mut config = ServerConfig {
            environment: "production".to_string(),
            bind_address: "0.0.0.0".to_string(),
            ingress_auth_mode: "token".to_string(),
            ingress_token: Some("secret".to_string()),
            metrics_auth_mode: "token".to_string(),
            metrics_token: Some("metrics-secret".to_string()),
            ..Default::default()
        };
        config.normalize_security();
        assert_eq!(config.metrics_auth_mode, "token");
        assert_eq!(config.effective_metrics_token(), Some("metrics-secret"));
    }

    #[test]
    fn production_topology_profile_blocks_mock_fallback() {
        let _lock = crate::testing::env::lock();
        let _profile = crate::testing::env::VarGuard::set(
            "SDKWORK_KERNEL_PROFILE_ID",
            Some("cloud.production"),
        );
        let _allow =
            crate::testing::env::VarGuard::set("SDKWORK_KERNEL_ALLOW_MOCK_PROVIDERS", None);
        let config = ServerConfig {
            environment: "production".to_string(),
            ..Default::default()
        };
        assert!(!config.allow_mock_provider_fallback());
    }

    #[test]
    fn production_topology_profile_rejects_mock_override() {
        // The kernel gate is unconditionally fail-closed in production: the
        // override environment variable is development-only.
        let _lock = crate::testing::env::lock();
        let _profile = crate::testing::env::VarGuard::set(
            "SDKWORK_KERNEL_PROFILE_ID",
            Some("cloud.production"),
        );
        let _allow =
            crate::testing::env::VarGuard::set("SDKWORK_KERNEL_ALLOW_MOCK_PROVIDERS", Some("1"));
        let config = ServerConfig {
            environment: "production".to_string(),
            ..Default::default()
        };
        assert!(!config.allow_mock_provider_fallback());
    }
}
