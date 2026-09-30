//! Canonical internal-api runtime route tree for `application.public-ingress`.
//!
//! Route timeout strategy:
//! - Standard JSON request/response routes receive a `request_timeout_secs`
//!   timeout (default 30 s) to prevent slow-client resource exhaustion.
//! - The SSE streaming route receives a `sse_request_timeout_secs` timeout
//!   (default 3600 s) because long-lived event streams are expected.
//!
//! The two timeout layers are applied to **disjoint** sub-routers and then
//! merged, so the shorter standard timeout never fires on SSE connections.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    http::StatusCode,
    routing::{get, post},
    Router,
};
use tower_http::timeout::TimeoutLayer;

use crate::api::internal_runtime;

/// Canonical internal-api mount prefix on `application.public-ingress`.
pub const INTERNAL_RUNTIME_MOUNT_PREFIX: &str = "/internal/v3/api/intelligence/runtime";

/// Builds the internal-api runtime route tree nested under [`INTERNAL_RUNTIME_MOUNT_PREFIX`].
///
/// The returned router already contains the appropriate per-route timeout
/// layers, so callers must NOT apply an additional outer `TimeoutLayer`
/// — doing so would cause the shorter timeout to fire on SSE connections.
pub fn build_internal_runtime_routes(
    state: Arc<internal_runtime::InternalRuntimeApiState>,
) -> Router {
    let standard_timeout = Duration::from_secs(state.config.request_timeout_secs);
    let sse_timeout = Duration::from_secs(state.config.sse_request_timeout_secs);

    // --- Standard JSON routes (short timeout) ---
    let standard_routes = Router::new()
        .route("/manifest", get(internal_runtime::get_runtime_manifest))
        .route("/health", get(internal_runtime::get_runtime_health))
        .route(
            "/diagnostics",
            get(internal_runtime::get_runtime_diagnostics),
        )
        .route("/snapshot", get(internal_runtime::load_snapshot))
        .route(
            "/permissions/{permission_request_id}",
            post(internal_runtime::decide_permission),
        )
        .route(
            "/sessions",
            post(internal_runtime::create_session).get(internal_runtime::list_sessions),
        )
        .route(
            "/sessions/{session_id}",
            get(internal_runtime::get_session).delete(internal_runtime::delete_session),
        )
        .route(
            "/sessions/{session_id}/close",
            post(internal_runtime::close_session),
        )
        .route(
            "/sessions/{session_id}/messages",
            post(internal_runtime::send_message).get(internal_runtime::get_messages),
        )
        .route(
            "/sessions/{session_id}/tasks",
            post(internal_runtime::submit_task).get(internal_runtime::list_tasks),
        )
        .route("/tasks/{task_id}", get(internal_runtime::get_task))
        .route("/runs/{run_id}", get(internal_runtime::get_run))
        .route("/runs/{run_id}/pause", post(internal_runtime::pause_run))
        .route("/runs/{run_id}/resume", post(internal_runtime::resume_run))
        .route("/runs/{run_id}/cancel", post(internal_runtime::cancel_run))
        .route(
            "/tasks/{task_id}/cancel",
            post(internal_runtime::cancel_task),
        )
        .route("/tasks/{task_id}/retry", post(internal_runtime::retry_task))
        .route("/models", get(internal_runtime::list_models))
        .route(
            "/sessions/{session_id}/model/invoke",
            post(internal_runtime::invoke_model),
        )
        .route(
            "/sessions/{session_id}/model/cancel",
            post(internal_runtime::cancel_model),
        )
        .route(
            "/sessions/{session_id}/tools",
            get(internal_runtime::list_tools),
        )
        .route(
            "/sessions/{session_id}/tools/{tool_name}/execute",
            post(internal_runtime::execute_tool),
        )
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            standard_timeout,
        ));

    // --- SSE streaming route (long timeout) ---
    let sse_routes = Router::new()
        .route(
            "/sessions/{session_id}/model/stream",
            post(internal_runtime::stream_model),
        )
        .route(
            "/sessions/{session_id}/events/stream",
            get(internal_runtime::stream_session_events),
        )
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            sse_timeout,
        ));

    standard_routes.merge(sse_routes).with_state(state)
}
