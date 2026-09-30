//! Request idempotency for internal runtime mutations.
//!
//! The server owns the request fingerprint.  A client supplied digest is never
//! trusted: the bounded request body is read once and hashed before the
//! handler is invoked.  Reservation and replay semantics are delegated to the
//! SDKWork web-framework stores so the same atomic protocol is used by memory
//! development profiles and Redis production profiles.

use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, HeaderName, HeaderValue, Method},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sdkwork_utils_rust::{sha256_hash, SdkWorkResultCode, SDKWORK_TRACE_ID_HEADER};
use sdkwork_web_core::{
    idempotency_replay_response, IdempotencyBeginOutcome, IdempotencyGuard,
    IdempotencyResponseRecord, IdempotencyStore, WebFrameworkError, WebFrameworkErrorKind,
};
use sdkwork_web_store_redis::shared_idempotency_store;
use std::{sync::Arc, time::Duration};
use tracing::{error, warn};

use crate::{
    config::ServerConfig, http_response::ApiError, middleware::RequestContext,
    runtime_routes::INTERNAL_RUNTIME_MOUNT_PREFIX,
};

const IDEMPOTENCY_KEY_HEADER: HeaderName = HeaderName::from_static("idempotency-key");
const REPLAY_HEADER: HeaderName = HeaderName::from_static("idempotent-replayed");
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 255;
#[cfg(test)]
const DEFAULT_IDEMPOTENCY_TTL_SECS: u64 = 24 * 60 * 60;
#[cfg(test)]
const DEFAULT_MAX_CACHED_RESPONSE_BYTES: usize = 512 * 1024;

/// Response cache ceiling for model-output-bearing idempotent routes.
///
/// Message turns and model invokes can carry up to the model output ceiling
/// (`sdkwork-agent-api-bridge` `MAX_MODEL_OUTPUT_BYTES`, 3 MiB) plus the user
/// message and the response envelope. Caching at the generic limit would turn
/// a committed mutation into a `413` with a stuck reservation that blocks
/// retries for the whole retention window. 4 MiB bounds the cache while
/// keeping every legitimate response replayable.
pub const MODEL_ROUTE_MAX_CACHED_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Runtime state for the idempotency middleware.
#[derive(Clone)]
pub struct IdempotencyState {
    pub store: Arc<dyn IdempotencyStore>,
    pub retention: Duration,
    pub max_request_bytes: usize,
    pub max_cached_response_bytes: usize,
    /// Development loopback requests may use a local anonymous principal. A
    /// production profile must always have a verified tenant and principal.
    pub allow_local_anonymous: bool,
    /// Whether mutation routes reject requests that omit `Idempotency-Key`.
    pub require_key: bool,
}

impl IdempotencyState {
    pub fn from_config(config: &ServerConfig) -> anyhow::Result<Self> {
        if !(60..=7 * 24 * 60 * 60).contains(&config.idempotency_ttl_secs) {
            anyhow::bail!("idempotency retention must be between 60 seconds and 7 days");
        }
        if config.idempotency_max_cached_response_bytes == 0
            || config.idempotency_max_cached_response_bytes > 1024 * 1024
            || config.idempotency_max_cached_response_bytes > config.max_body_size
        {
            anyhow::bail!(
                "idempotency response cache limit must be positive, at most 1 MiB, and no larger than max body size"
            );
        }

        let store: Arc<dyn IdempotencyStore> =
            if let Some(redis_url) = config.effective_idempotency_redis_url() {
                shared_idempotency_store(redis_url, "sdkwork:agent-server:v3")?
            } else {
                sdkwork_web_core::memory_idempotency_store()
            };

        if config.requires_distributed_idempotency() && !store.is_distributed_ha() {
            anyhow::bail!(
                "production scale-out requires a distributed idempotency store via SDKWORK_IDEMPOTENCY_REDIS_URL"
            );
        }

        let retention = Duration::from_secs(config.idempotency_ttl_secs);
        let max_cached_response_bytes = config.idempotency_max_cached_response_bytes;

        Ok(Self {
            store,
            retention,
            max_request_bytes: config.max_body_size.max(1),
            max_cached_response_bytes,
            allow_local_anonymous: config.is_development()
                && config.is_loopback_bind()
                && config.ingress_auth_mode.eq_ignore_ascii_case("open"),
            require_key: config.idempotency_require_key
                || config.is_production_kernel_profile()
                || !config.is_loopback_bind(),
        })
    }

    #[cfg(test)]
    fn memory_for_tests() -> Self {
        Self {
            store: sdkwork_web_core::memory_idempotency_store(),
            retention: Duration::from_secs(DEFAULT_IDEMPOTENCY_TTL_SECS),
            max_request_bytes: 1024 * 1024,
            max_cached_response_bytes: DEFAULT_MAX_CACHED_RESPONSE_BYTES,
            allow_local_anonymous: true,
            require_key: true,
        }
    }
}

impl IdempotencyState {
    /// Returns the response cache limit for a request path.
    ///
    /// Model-output-bearing routes (message turns and model invokes) use
    /// [`MODEL_ROUTE_MAX_CACHED_RESPONSE_BYTES`] so a committed mutation with
    /// a large assistant message stays replayable; all other routes use the
    /// configured generic limit.
    pub fn max_cached_response_bytes_for(&self, path: &str) -> usize {
        if path.ends_with("/messages") || path.ends_with("/model/invoke") {
            MODEL_ROUTE_MAX_CACHED_RESPONSE_BYTES.max(self.max_cached_response_bytes)
        } else {
            self.max_cached_response_bytes
        }
    }
}

/// Returns true only for mutation routes whose result must be replayable.
/// Streaming routes are intentionally excluded because an SSE body is not a
/// bounded JSON response and cannot be safely replayed from this store.
pub fn route_requires_idempotency(method: &Method, path: &str) -> bool {
    if method != Method::POST || !path.starts_with(INTERNAL_RUNTIME_MOUNT_PREFIX) {
        return false;
    }

    let relative = path
        .strip_prefix(INTERNAL_RUNTIME_MOUNT_PREFIX)
        .unwrap_or(path);
    match relative {
        "/sessions" => true,
        p if p.starts_with("/permissions/") => true,
        p if p.starts_with("/sessions/") && p.ends_with("/close") => true,
        p if p.starts_with("/sessions/") && p.ends_with("/messages") => true,
        p if p.starts_with("/sessions/") && p.ends_with("/tasks") => true,
        p if p.starts_with("/sessions/") && p.ends_with("/model/invoke") => true,
        p if p.starts_with("/sessions/") && p.ends_with("/model/cancel") => true,
        p if p.starts_with("/sessions/") && p.contains("/tools/") && p.ends_with("/execute") => {
            true
        }
        p if p.starts_with("/tasks/") && p.ends_with("/cancel") => true,
        p if p.starts_with("/tasks/") && p.ends_with("/retry") => true,
        p if p.starts_with("/runs/")
            && (p.ends_with("/pause") || p.ends_with("/resume") || p.ends_with("/cancel")) =>
        {
            true
        }
        _ => false,
    }
}

fn parse_client_key(request: &Request) -> Result<Option<String>, SdkWorkResultCode> {
    let values = request.headers().get_all(&IDEMPOTENCY_KEY_HEADER);
    let mut iter = values.iter();
    let Some(value) = iter.next() else {
        return Ok(None);
    };
    if iter.next().is_some() {
        return Err(SdkWorkResultCode::InvalidParameter);
    }
    let value = value
        .to_str()
        .map_err(|_| SdkWorkResultCode::InvalidParameter)?
        .trim();
    if value.is_empty() || value.len() > MAX_IDEMPOTENCY_KEY_BYTES {
        return Err(SdkWorkResultCode::InvalidParameter);
    }
    // Header values are ASCII by construction. Restrict the key grammar so a
    // caller cannot create unbounded or ambiguous Redis key material.
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
    }) {
        return Err(SdkWorkResultCode::InvalidParameter);
    }
    Ok(Some(value.to_owned()))
}

fn local_context(request: &Request) -> RequestContext {
    request
        .extensions()
        .get::<RequestContext>()
        .cloned()
        .unwrap_or_else(|| {
            RequestContext::from_headers_and_path(request.headers(), request.uri().path())
        })
}

fn mutation_scope(
    state: &IdempotencyState,
    context: &RequestContext,
) -> Result<String, SdkWorkResultCode> {
    let tenant = context
        .tenant_id
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let principal = context
        .user_id
        .as_deref()
        .or(context.subject_id.as_deref())
        .filter(|value| !value.trim().is_empty());

    match (tenant, principal) {
        (Some(tenant), Some(principal)) => Ok(format!(
            "tenant={}:principal={}",
            sha256_hash(tenant.as_bytes()),
            sha256_hash(principal.as_bytes())
        )),
        _ if state.allow_local_anonymous => Ok("tenant=local:principal=local".to_owned()),
        _ => Err(SdkWorkResultCode::AuthenticationRequired),
    }
}

fn request_fingerprint(request: &Request, body: &[u8]) -> String {
    let query = request.uri().query().unwrap_or_default();
    let material = format!(
        "sdkwork-idempotency-v1\nmethod={}\npath={}\nquery={}\nbody_sha256={}",
        request.method().as_str(),
        request.uri().path(),
        query,
        sha256_hash(body)
    );
    sha256_hash(material.as_bytes())
}

fn store_key(
    state: &IdempotencyState,
    context: &RequestContext,
    request: &Request,
    client_key: &str,
) -> Result<String, SdkWorkResultCode> {
    let scope = mutation_scope(state, context)?;
    let route = format!(
        "method={} path={}",
        request.method().as_str(),
        request.uri().path()
    );
    Ok(format!(
        "v1:{}:{}:{}:{}",
        sha256_hash(scope.as_bytes()),
        sha256_hash(route.as_bytes()),
        sha256_hash(client_key.as_bytes()),
        sha256_hash(request.uri().query().unwrap_or_default().as_bytes())
    ))
}

fn json_content_type(response: &Response) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            let media_type = value.split(';').next().unwrap_or_default().trim();
            media_type.eq_ignore_ascii_case("application/json") || media_type.ends_with("+json")
        })
        .unwrap_or(false)
}

fn trace_id(context: &RequestContext) -> String {
    context.problem_trace_id()
}

fn error_response(
    context: &RequestContext,
    code: SdkWorkResultCode,
    detail: &'static str,
) -> Response {
    ApiError::new(code, detail, trace_id(context)).into_response()
}

fn store_error_response(context: &RequestContext, error: &WebFrameworkError) -> Response {
    let code = match error.kind {
        WebFrameworkErrorKind::Conflict => SdkWorkResultCode::Conflict,
        WebFrameworkErrorKind::PayloadTooLarge => SdkWorkResultCode::PayloadTooLarge,
        WebFrameworkErrorKind::DependencyUnavailable => SdkWorkResultCode::ServiceUnavailable,
        WebFrameworkErrorKind::RequestTimeout => SdkWorkResultCode::GatewayTimeout,
        _ => SdkWorkResultCode::ServiceUnavailable,
    };
    let detail = match code {
        SdkWorkResultCode::Conflict => "idempotency key conflicts with an existing request",
        SdkWorkResultCode::PayloadTooLarge => {
            "idempotent response exceeds the configured cache limit"
        }
        _ => "idempotency service is temporarily unavailable",
    };
    error_response(context, code, detail)
}

fn replay_response(record: &IdempotencyResponseRecord, context: &RequestContext) -> Response {
    let mut response = idempotency_replay_response(record, Some(&context.request_id))
        .unwrap_or_else(|_| {
            error_response(
                context,
                SdkWorkResultCode::ServiceUnavailable,
                "cached idempotent response is invalid",
            )
        });
    response
        .headers_mut()
        .insert(REPLAY_HEADER, HeaderValue::from_static("true"));

    // Preserve the original SDKWork trace id when the cached body is a normal
    // response envelope. The current request still receives its own request id.
    if let Some(value) = serde_json::from_slice::<serde_json::Value>(&record.body)
        .ok()
        .and_then(|json| {
            json.get("traceId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
    {
        if let Ok(header_value) = HeaderValue::from_str(&value) {
            if let Ok(header_name) = HeaderName::from_bytes(SDKWORK_TRACE_ID_HEADER.as_bytes()) {
                response.headers_mut().insert(header_name, header_value);
            }
        }
    }
    response
}

/// Enforce idempotency for selected JSON mutation routes.
pub async fn middleware(
    State(state): State<Arc<IdempotencyState>>,
    request: Request,
    next: Next,
) -> Response {
    if !route_requires_idempotency(request.method(), request.uri().path()) {
        return next.run(request).await;
    }

    let context = local_context(&request);
    let client_key = match parse_client_key(&request) {
        Ok(Some(value)) => value,
        Ok(None) if !state.require_key => return next.run(request).await,
        Ok(None) => {
            return error_response(
                &context,
                SdkWorkResultCode::MissingRequiredField,
                "Idempotency-Key header is required for this mutation",
            )
        }
        Err(code) => return error_response(&context, code, "Idempotency-Key header is invalid"),
    };

    let scope_key = match store_key(&state, &context, &request, &client_key) {
        Ok(value) => value,
        Err(code) => {
            return error_response(
                &context,
                code,
                "authenticated tenant and principal are required",
            )
        }
    };

    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, state.max_request_bytes).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error_response(
                &context,
                SdkWorkResultCode::PayloadTooLarge,
                "request body exceeds the configured limit",
            )
        }
    };
    let request = Request::from_parts(parts, Body::from(body.clone()));
    let fingerprint = request_fingerprint(&request, &body);

    let outcome = match state
        .store
        .begin(&scope_key, &fingerprint, state.retention)
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => return store_error_response(&context, &error),
    };

    if let IdempotencyBeginOutcome::Replay(record) = outcome {
        return replay_response(&record, &context);
    }

    let mut guard =
        IdempotencyGuard::new(state.store.clone(), scope_key.clone(), fingerprint.clone());
    // The response cache limit depends on the route; capture the path before
    // the request is moved into the handler chain.
    let request_path = request.uri().path().to_string();
    let mut response = next.run(request).await;
    let status = response.status();

    if !(200..300).contains(&status.as_u16()) {
        if status.as_u16() < 500 {
            if let Err(error) = state.store.release(&scope_key, &fingerprint).await {
                warn!(error = ?error, "failed to release idempotency reservation after client/redirection response");
            }
            guard.mark_completed();
            return response;
        }

        // A server error may be returned after an external provider or a
        // database transaction has committed an unknown side effect. Cache a
        // bounded JSON error exactly as returned so retries replay the first
        // result instead of executing the command again.
        if !json_content_type(&response) {
            warn!(status = %status, "retaining idempotency reservation after non-JSON server error");
            guard.mark_completed();
            return response;
        }
        let (response_parts, response_body) = response.into_parts();
        let response_bytes = match to_bytes(response_body, state.max_request_bytes).await {
            Ok(bytes) => bytes,
            Err(_) => {
                guard.mark_completed();
                return error_response(
                    &context,
                    SdkWorkResultCode::ServiceUnavailable,
                    "idempotent server error could not be cached",
                );
            }
        };
        response = Response::from_parts(response_parts, Body::from(response_bytes.clone()));
        if response_bytes.len() > state.max_cached_response_bytes {
            warn!(status = %status, bytes = response_bytes.len(), "retaining idempotency reservation after oversized server error");
            guard.mark_completed();
            return response;
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let record = IdempotencyResponseRecord {
            status_code: status.as_u16(),
            body: response_bytes.to_vec(),
            content_type,
        };
        if let Err(error) = state
            .store
            .complete(&scope_key, &fingerprint, record, state.retention)
            .await
        {
            warn!(error = ?error, "failed to persist idempotent server error response");
        }
        guard.mark_completed();
        return response;
    }

    if !json_content_type(&response) {
        error!(status = %status, "idempotent mutation returned a non-JSON success response");
        // The handler has already reported success and may have committed a
        // side effect. Keep the reservation fail-closed so a retry cannot run
        // the mutation again, even though this contract-violating response
        // cannot be replayed.
        guard.mark_completed();
        return response;
    }

    let (response_parts, response_body) = response.into_parts();
    let response_bytes = match to_bytes(
        response_body,
        state.max_cached_response_bytes_for(&request_path),
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(_) => {
            // The business response was successful, so releasing here could
            // duplicate a committed side effect. The consumed oversized body
            // cannot be returned, but the reservation remains fail-closed.
            guard.mark_completed();
            return error_response(
                &context,
                SdkWorkResultCode::PayloadTooLarge,
                "idempotent response exceeds the configured cache limit",
            );
        }
    };

    let content_type = response_parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let record = IdempotencyResponseRecord {
        status_code: response_parts.status.as_u16(),
        body: response_bytes.to_vec(),
        content_type,
    };
    if let Err(error) = state
        .store
        .complete(&scope_key, &fingerprint, record, state.retention)
        .await
    {
        // The business side effect may already have committed. Preserve the
        // original success response and leave the reservation in progress;
        // releasing it would allow a retry to duplicate the mutation.
        warn!(error = ?error, "failed to persist idempotency response");
        response = Response::from_parts(response_parts, Body::from(response_bytes));
        guard.mark_completed();
        return response;
    }

    response = Response::from_parts(response_parts, Body::from(response_bytes));
    guard.mark_completed();
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        response::IntoResponse,
        routing::post,
        Router,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    #[test]
    fn only_json_mutations_are_idempotent() {
        assert!(route_requires_idempotency(
            &Method::POST,
            "/internal/v3/api/intelligence/runtime/sessions"
        ));
        assert!(route_requires_idempotency(
            &Method::POST,
            "/internal/v3/api/intelligence/runtime/sessions/sess/messages"
        ));
        assert!(route_requires_idempotency(
            &Method::POST,
            "/internal/v3/api/intelligence/runtime/tasks/task/cancel"
        ));
        assert!(!route_requires_idempotency(
            &Method::POST,
            "/internal/v3/api/intelligence/runtime/sessions/sess/model/stream"
        ));
        assert!(!route_requires_idempotency(
            &Method::GET,
            "/internal/v3/api/intelligence/runtime/sessions"
        ));
    }

    #[test]
    fn model_output_routes_use_a_larger_response_cache_limit() {
        let state = IdempotencyState::memory_for_tests();
        assert!(MODEL_ROUTE_MAX_CACHED_RESPONSE_BYTES > state.max_cached_response_bytes);
        assert_eq!(
            state.max_cached_response_bytes_for(
                "/internal/v3/api/intelligence/runtime/sessions/sess/messages"
            ),
            MODEL_ROUTE_MAX_CACHED_RESPONSE_BYTES
        );
        assert_eq!(
            state.max_cached_response_bytes_for(
                "/internal/v3/api/intelligence/runtime/sessions/sess/model/invoke"
            ),
            MODEL_ROUTE_MAX_CACHED_RESPONSE_BYTES
        );
        assert_eq!(
            state.max_cached_response_bytes_for(
                "/internal/v3/api/intelligence/runtime/sessions/sess/tasks"
            ),
            state.max_cached_response_bytes,
            "non-model routes keep the configured generic limit"
        );
    }

    #[tokio::test]
    async fn same_key_and_body_replays_without_running_handler() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_handler = calls.clone();
        let state = Arc::new(IdempotencyState::memory_for_tests());
        let app = Router::new()
            .route(
                "/internal/v3/api/intelligence/runtime/sessions",
                post(move || {
                    let calls = calls_for_handler.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::CREATED,
                            [(header::CONTENT_TYPE, "application/json")],
                            r#"{"code":0,"traceId":"00000000-0000-0000-0000-000000000001"}"#,
                        )
                            .into_response()
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(state, middleware));

        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/internal/v3/api/intelligence/runtime/sessions")
                        .header(IDEMPOTENCY_KEY_HEADER, "test-key")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"name":"demo"}"#))
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::CREATED);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn different_body_with_same_key_is_conflict() {
        let state = Arc::new(IdempotencyState::memory_for_tests());
        let app = Router::new()
            .route(
                "/internal/v3/api/intelligence/runtime/sessions",
                post(|| async {
                    (
                        StatusCode::CREATED,
                        [(header::CONTENT_TYPE, "application/json")],
                        "{}",
                    )
                }),
            )
            .layer(axum::middleware::from_fn_with_state(state, middleware));
        let send = |body: &'static str| {
            app.clone().oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/internal/v3/api/intelligence/runtime/sessions")
                    .header(IDEMPOTENCY_KEY_HEADER, "same-key")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .expect("request"),
            )
        };
        assert_eq!(
            send("a").await.expect("response").status(),
            StatusCode::CREATED
        );
        assert_eq!(
            send("b").await.expect("response").status(),
            StatusCode::CONFLICT
        );
    }

    #[tokio::test]
    async fn non_json_success_keeps_the_reservation_fail_closed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_handler = calls.clone();
        let state = Arc::new(IdempotencyState::memory_for_tests());
        let app = Router::new()
            .route(
                "/internal/v3/api/intelligence/runtime/sessions",
                post(move || {
                    let calls = calls_for_handler.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        (StatusCode::CREATED, "created")
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(state, middleware));

        let request = || {
            Request::builder()
                .method(Method::POST)
                .uri("/internal/v3/api/intelligence/runtime/sessions")
                .header(IDEMPOTENCY_KEY_HEADER, "non-json-key")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name":"demo"}"#))
                .expect("request")
        };

        let first = app
            .clone()
            .oneshot(request())
            .await
            .expect("first response");
        assert_eq!(first.status(), StatusCode::CREATED);
        let replay = app.oneshot(request()).await.expect("replay response");
        assert_eq!(replay.status(), StatusCode::CONFLICT);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn server_error_is_cached_and_replayed_fail_closed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_handler = calls.clone();
        let state = Arc::new(IdempotencyState::memory_for_tests());
        let app = Router::new()
            .route(
                "/internal/v3/api/intelligence/runtime/sessions",
                post(move || {
                    let calls = calls_for_handler.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            [(header::CONTENT_TYPE, "application/json")],
                            r#"{"code":50000,"traceId":"00000000-0000-0000-0000-000000000002"}"#,
                        )
                            .into_response()
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(state, middleware));

        let request = || {
            Request::builder()
                .method(Method::POST)
                .uri("/internal/v3/api/intelligence/runtime/sessions")
                .header(IDEMPOTENCY_KEY_HEADER, "server-error-key")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name":"demo"}"#))
                .expect("request")
        };

        let first = app
            .clone()
            .oneshot(request())
            .await
            .expect("first response");
        assert_eq!(first.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let replay = app.oneshot(request()).await.expect("replay response");
        assert_eq!(replay.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            replay
                .headers()
                .get(REPLAY_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
