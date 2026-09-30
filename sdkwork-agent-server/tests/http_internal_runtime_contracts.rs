//! HTTP contract tests for canonical internal-api runtime routes.
//! These contracts exercise the in-memory (SQLite) persistence path and only
//! compile when the client-local `sqlite` feature is enabled.

#![cfg(feature = "sqlite")]

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header::AUTHORIZATION, header::CONTENT_TYPE, Request, StatusCode};
use axum::Router;
use sdkwork_agent_server::{
    app,
    config::{ServerConfig, TenantTokenQuotaOverride},
    ingress_identity::{self, IDENTITY_MAC_HEADER},
    runtime_routes::INTERNAL_RUNTIME_MOUNT_PREFIX,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const TEST_INGRESS_TOKEN: &str = "kernel-test-token";

/// Enables the explicit mock-provider override once for the whole test
/// binary: these contracts exercise model/provider paths that are
/// fail-closed by default (see ).
static SET_MOCK_ENV: std::sync::Once = std::sync::Once::new();

fn ensure_mock_provider_override() {
    SET_MOCK_ENV.call_once(|| {
        std::env::set_var("SDKWORK_KERNEL_ALLOW_MOCK_PROVIDERS", "1");
    });
}

fn open_test_app() -> Router {
    ensure_mock_provider_override();
    app::build_test_app(Arc::new(ServerConfig::default()))
}

fn token_test_app(token: &str) -> Router {
    let config = ServerConfig {
        ingress_auth_mode: "token".to_string(),
        ingress_token: Some(token.to_string()),
        ..Default::default()
    };
    app::build_test_app(Arc::new(config))
}

fn quota_test_app(tenant_id: &str, daily_tokens: u64) -> Router {
    let mut overrides = HashMap::new();
    overrides.insert(
        tenant_id.to_string(),
        TenantTokenQuotaOverride { daily_tokens },
    );
    let config = ServerConfig {
        ingress_auth_mode: "token".to_string(),
        ingress_token: Some(TEST_INGRESS_TOKEN.to_string()),
        tenant_token_quota_overrides: overrides,
        ..Default::default()
    };
    app::build_test_app(Arc::new(config))
}

fn identity_mac(token: &str, tenant: &str, user: &str) -> String {
    ingress_identity::compute_identity_mac(token, tenant, user).expect("identity mac")
}

fn with_signed_identity(
    builder: axum::http::request::Builder,
    token: &str,
    tenant: &str,
    user: &str,
) -> axum::http::request::Builder {
    builder
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header("x-sdkwork-tenant-id", tenant)
        .header("x-sdkwork-user-id", user)
        .header(IDENTITY_MAC_HEADER, identity_mac(token, tenant, user))
}

fn runtime_path(relative: &str) -> String {
    format!("{INTERNAL_RUNTIME_MOUNT_PREFIX}{relative}")
}

fn internal_snapshot_path() -> String {
    runtime_path("/snapshot")
}

async fn read_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response body should be valid json")
}

fn assert_sdkwork_success_envelope(payload: &Value) {
    assert_eq!(
        payload.get("code").and_then(Value::as_i64),
        Some(0),
        "success responses must use numeric code 0"
    );
    assert!(
        payload.get("traceId").and_then(Value::as_str).is_some(),
        "success responses must include traceId"
    );
}

fn list_items(payload: &Value) -> &[Value] {
    assert_sdkwork_success_envelope(payload);
    payload
        .get("data")
        .and_then(|data| data.get("items"))
        .and_then(|value| value.as_array())
        .map(|items| items.as_slice())
        .expect("list response should expose data.items[]")
}

fn assert_cursor_page_info(payload: &Value, expected_page_size: i64) {
    let page_info = payload
        .get("data")
        .and_then(|data| data.get("pageInfo"))
        .expect("list response should expose data.pageInfo");
    assert_eq!(
        page_info.get("mode").and_then(Value::as_str),
        Some("cursor")
    );
    assert_eq!(
        page_info.get("pageSize").and_then(Value::as_i64),
        Some(expected_page_size)
    );
    assert!(page_info.get("hasMore").is_some());
}

fn next_cursor(payload: &Value) -> String {
    payload
        .get("data")
        .and_then(|data| data.get("pageInfo"))
        .and_then(|page_info| page_info.get("nextCursor"))
        .and_then(Value::as_str)
        .expect("cursor page should expose pageInfo.nextCursor")
        .to_string()
}

fn item_value(payload: &Value) -> &Value {
    assert_sdkwork_success_envelope(payload);
    payload
        .get("data")
        .and_then(|data| data.get("item"))
        .expect("resource response should expose data.item")
}

#[tokio::test]
async fn internal_runtime_snapshot_returns_runtime_health() {
    let app = open_test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri(internal_snapshot_path())
                .body(Body::empty())
                .expect("request should be built"),
        )
        .await
        .expect("snapshot request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = read_json(response).await;
    assert_eq!(item_value(&snapshot)["runtime"]["health"], "healthy");
}

#[tokio::test]
async fn internal_runtime_manifest_returns_capability_manifest() {
    let app = open_test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri(runtime_path("/manifest"))
                .body(Body::empty())
                .expect("request should be built"),
        )
        .await
        .expect("manifest request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let manifest = read_json(response).await;
    let manifest = item_value(&manifest);
    assert!(
        manifest["runtimeId"].is_string(),
        "runtimeId must be present"
    );
    assert!(manifest["agentId"].is_string(), "agentId must be present");
    assert!(
        manifest["kernelVersion"].is_string(),
        "kernelVersion must be present"
    );
    assert!(
        manifest["securityProfile"].is_string(),
        "securityProfile must be present"
    );
    assert!(
        manifest["capabilities"].is_array(),
        "capabilities must be an array"
    );
    assert!(
        manifest["providers"].is_array(),
        "providers must be an array"
    );
    assert!(
        manifest["missingRequiredCapabilities"].is_array(),
        "missingRequiredCapabilities must be an array"
    );
    assert!(
        manifest["degradedCapabilities"].is_array(),
        "degradedCapabilities must be an array"
    );
}

#[tokio::test]
async fn internal_runtime_health_returns_health_status() {
    let app = open_test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri(runtime_path("/health"))
                .body(Body::empty())
                .expect("request should be built"),
        )
        .await
        .expect("health request should succeed");

    match response.status() {
        StatusCode::OK => {
            let health = read_json(response).await;
            let health = item_value(&health);
            assert!(health["runtimeId"].is_string(), "runtimeId must be present");
            assert!(health["state"].is_string(), "state must be present");
            assert_eq!(health["health"].as_str(), Some("healthy"));
            assert!(
                health["persistenceHealthy"].is_boolean(),
                "persistenceHealthy must be boolean"
            );
            assert!(
                health["degradedCapabilities"].is_array(),
                "degradedCapabilities must be an array"
            );
        }
        StatusCode::SERVICE_UNAVAILABLE => {
            let problem = read_json(response).await;
            assert_eq!(problem.get("status").and_then(Value::as_i64), Some(503));
            assert!(
                problem.get("code").and_then(Value::as_i64).is_some(),
                "degraded health must return ProblemDetail.code"
            );
            assert!(
                problem.get("traceId").and_then(Value::as_str).is_some(),
                "degraded health must return traceId"
            );
        }
        other => panic!("unexpected runtime health status: {other}"),
    }
}

#[tokio::test]
async fn internal_runtime_diagnostics_returns_provider_diagnostics() {
    let app = open_test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri(runtime_path("/diagnostics"))
                .body(Body::empty())
                .expect("request should be built"),
        )
        .await
        .expect("diagnostics request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let diagnostics = read_json(response).await;
    let diagnostics = item_value(&diagnostics);
    assert!(
        diagnostics["runtimeId"].is_string(),
        "runtimeId must be present"
    );
    assert!(
        diagnostics["agentId"].is_string(),
        "agentId must be present"
    );
    assert!(diagnostics["state"].is_string(), "state must be present");
    assert!(
        diagnostics["providerCount"].is_number(),
        "providerCount must be a number"
    );
    assert!(
        diagnostics["capabilityCount"].is_number(),
        "capabilityCount must be a number"
    );
    assert!(
        diagnostics["typedProviderCount"].is_number(),
        "typedProviderCount must be a number"
    );
    assert!(
        diagnostics["manifestOnlyProviderCount"].is_number(),
        "manifestOnlyProviderCount must be a number"
    );
    assert!(
        diagnostics["providerDiagnostics"].is_array(),
        "providerDiagnostics must be an array"
    );
}

#[tokio::test]
async fn retired_kernel_alias_snapshot_returns_not_found() {
    let app = open_test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/kernel/snapshot")
                .body(Body::empty())
                .expect("retired alias request should be built"),
        )
        .await
        .expect("retired alias request should succeed");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn retired_legacy_session_paths_return_not_found() {
    let app = open_test_app();
    for path in ["/api/sessions", "/api/chat/send", "/api/chat/stream"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(if path.ends_with("/send") || path.ends_with("/stream") {
                        "POST"
                    } else {
                        "GET"
                    })
                    .uri(path)
                    .header(CONTENT_TYPE, "application/json")
                    .body(if path.ends_with("/send") || path.ends_with("/stream") {
                        Body::from(json!({ "session_id": "session.1", "content": "x" }).to_string())
                    } else {
                        Body::empty()
                    })
                    .expect("retired path request should be built"),
            )
            .await
            .expect("retired path request should succeed");
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "path {path} should be retired"
        );
    }
}

#[tokio::test]
async fn internal_runtime_session_roundtrip_uses_items_list_envelope() {
    let app = open_test_app();
    let create = Request::builder()
        .method("POST")
        .uri(runtime_path("/sessions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "agentId": "agent.1",
                "tenantId": "tenant.1",
                "title": "internal contract"
            })
            .to_string(),
        ))
        .expect("create request should be built");

    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("internal create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);

    let session_payload = read_json(response).await;
    let session = item_value(&session_payload);
    let session_id = session["sessionId"]
        .as_str()
        .expect("sessionId should be present");

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(runtime_path("/sessions"))
                .body(Body::empty())
                .expect("list request should be built"),
        )
        .await
        .expect("internal list request should succeed");
    assert_eq!(list.status(), StatusCode::OK);
    let sessions = read_json(list).await;
    let session_items = list_items(&sessions);
    assert_cursor_page_info(&sessions, 20);
    assert!(session_items
        .iter()
        .any(|row| row["sessionId"] == session_id));

    let send = Request::builder()
        .method("POST")
        .uri(runtime_path(&format!("/sessions/{session_id}/messages")))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "content": "hello internal" }).to_string(),
        ))
        .expect("send request should be built");
    let response = app
        .clone()
        .oneshot(send)
        .await
        .expect("internal send request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);

    let messages = app
        .oneshot(
            Request::builder()
                .uri(runtime_path(&format!("/sessions/{session_id}/messages")))
                .body(Body::empty())
                .expect("messages request should be built"),
        )
        .await
        .expect("internal messages request should succeed");
    assert_eq!(messages.status(), StatusCode::OK);
    let payload = read_json(messages).await;
    let message_items = list_items(&payload);
    assert_cursor_page_info(&payload, 20);
    assert_eq!(message_items.len(), 2);
    assert_eq!(message_items[0]["role"], "user");
    assert_eq!(message_items[1]["role"], "assistant");
}

#[tokio::test]
async fn internal_runtime_messages_support_cursor_pagination() {
    let app = open_test_app();
    let create = Request::builder()
        .method("POST")
        .uri(runtime_path("/sessions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "agentId": "agent.1",
                "tenantId": "tenant.1",
                "title": "cursor pagination"
            })
            .to_string(),
        ))
        .expect("create request should be built");
    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let session_id = item_value(&read_json(response).await)["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    for content in ["one", "two", "three"] {
        let send = Request::builder()
            .method("POST")
            .uri(runtime_path(&format!("/sessions/{session_id}/messages")))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "content": content }).to_string()))
            .expect("send request should be built");
        let response = app
            .clone()
            .oneshot(send)
            .await
            .expect("send should succeed");
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    let first_page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(runtime_path(&format!(
                    "/sessions/{session_id}/messages?page_size=1"
                )))
                .body(Body::empty())
                .expect("messages request should be built"),
        )
        .await
        .expect("messages request should succeed");
    assert_eq!(first_page.status(), StatusCode::OK);
    let first_payload = read_json(first_page).await;
    let first_items = list_items(&first_payload);
    assert_cursor_page_info(&first_payload, 1);
    assert_eq!(first_items.len(), 1);
    let first_message_id = first_items[0]["messageId"]
        .as_str()
        .expect("messageId")
        .to_string();
    let cursor = next_cursor(&first_payload);

    let cursor_page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(runtime_path(&format!(
                    "/sessions/{session_id}/messages?cursor={cursor}&page_size=1"
                )))
                .body(Body::empty())
                .expect("cursor request should be built"),
        )
        .await
        .expect("cursor request should succeed");
    assert_eq!(cursor_page.status(), StatusCode::OK);
    let cursor_payload = read_json(cursor_page).await;
    let cursor_items = list_items(&cursor_payload);
    let page_info = cursor_payload
        .get("data")
        .and_then(|data| data.get("pageInfo"))
        .expect("pageInfo");
    assert_eq!(
        page_info.get("mode").and_then(Value::as_str),
        Some("cursor")
    );
    assert_eq!(cursor_items.len(), 1);
    assert_ne!(cursor_items[0]["messageId"], first_message_id);
}

#[tokio::test]
async fn internal_runtime_sessions_support_cursor_pagination() {
    let app = open_test_app();
    let mut session_ids = Vec::new();
    for title in ["session-alpha", "session-beta", "session-gamma"] {
        let create = Request::builder()
            .method("POST")
            .uri(runtime_path("/sessions"))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "agentId": "agent.1",
                    "tenantId": "tenant.1",
                    "title": title
                })
                .to_string(),
            ))
            .expect("create request should be built");
        let response = app
            .clone()
            .oneshot(create)
            .await
            .expect("create should succeed");
        assert_eq!(response.status(), StatusCode::CREATED);
        let session_id = item_value(&read_json(response).await)["sessionId"]
            .as_str()
            .expect("sessionId")
            .to_string();
        session_ids.push(session_id);
    }

    let first_page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(runtime_path("/sessions?page_size=1"))
                .body(Body::empty())
                .expect("list request should be built"),
        )
        .await
        .expect("list request should succeed");
    assert_eq!(first_page.status(), StatusCode::OK);
    let first_payload = read_json(first_page).await;
    let first_items = list_items(&first_payload);
    assert_cursor_page_info(&first_payload, 1);
    assert_eq!(first_items.len(), 1);
    let first_session_id = first_items[0]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    let cursor = next_cursor(&first_payload);

    let deleted_anchor = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(runtime_path(&format!("/sessions/{first_session_id}")))
                .body(Body::empty())
                .expect("delete cursor anchor request should be built"),
        )
        .await
        .expect("delete cursor anchor should succeed");
    assert_eq!(deleted_anchor.status(), StatusCode::NO_CONTENT);

    let cursor_page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(runtime_path(&format!(
                    "/sessions?cursor={cursor}&page_size=1"
                )))
                .body(Body::empty())
                .expect("cursor request should be built"),
        )
        .await
        .expect("cursor request should succeed");
    assert_eq!(cursor_page.status(), StatusCode::OK);
    let cursor_payload = read_json(cursor_page).await;
    let cursor_items = list_items(&cursor_payload);
    let page_info = cursor_payload
        .get("data")
        .and_then(|data| data.get("pageInfo"))
        .expect("pageInfo");
    assert_eq!(
        page_info.get("mode").and_then(Value::as_str),
        Some("cursor")
    );
    assert_eq!(cursor_items.len(), 1);
    assert_ne!(cursor_items[0]["sessionId"], first_session_id);
    let cursor_session_id = cursor_items[0]["sessionId"]
        .as_str()
        .expect("sessionId on cursor page");
    assert!(session_ids.iter().any(|id| id == cursor_session_id));
}

#[tokio::test]
async fn internal_runtime_list_queries_reject_forbidden_pagination_aliases() {
    let app = open_test_app();

    for query in [
        "pageSize=1",
        "limit=1",
        "page_no=1",
        "pageNo=1",
        "per_page=1",
        "size=1",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(runtime_path(&format!("/sessions?{query}")))
                    .body(Body::empty())
                    .expect("list sessions request should be built"),
            )
            .await
            .expect("list sessions request should succeed");

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "forbidden pagination alias should be rejected: {query}"
        );
        // PAGINATION_SPEC §10.1: alias rejections must be structured
        // `application/problem+json` with numeric code 40003, not axum's
        // default text/plain body.
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .map(|value| value.to_str().unwrap_or("")),
            Some("application/problem+json"),
            "alias rejection must use the problem+json contract: {query}"
        );
        let bytes = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("problem body");
        let problem: Value = serde_json::from_slice(&bytes).expect("problem json");
        assert_eq!(
            problem["code"],
            json!(40003),
            "alias rejection code: {query}"
        );
        assert!(
            problem["traceId"].is_string(),
            "alias rejection must carry a traceId: {query}"
        );
    }
}

#[tokio::test]
async fn internal_runtime_tasks_support_cursor_pagination() {
    let app = open_test_app();
    let create = Request::builder()
        .method("POST")
        .uri(runtime_path("/sessions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "agentId": "agent.1",
                "tenantId": "tenant.1",
                "title": "task cursor pagination"
            })
            .to_string(),
        ))
        .expect("create request should be built");
    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let session_id = item_value(&read_json(response).await)["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    for instruction in ["task-one", "task-two", "task-three"] {
        let submit = Request::builder()
            .method("POST")
            .uri(runtime_path(&format!(
                "/sessions/{session_id}/tasks"
            )))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "instruction": instruction }).to_string(),
            ))
            .expect("submit request should be built");
        let response = app
            .clone()
            .oneshot(submit)
            .await
            .expect("submit should succeed");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    let first_page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(runtime_path(&format!(
                    "/sessions/{session_id}/tasks?page_size=1"
                )))
                .body(Body::empty())
                .expect("tasks request should be built"),
        )
        .await
        .expect("tasks request should succeed");
    assert_eq!(first_page.status(), StatusCode::OK);
    let first_payload = read_json(first_page).await;
    let first_items = list_items(&first_payload);
    assert_cursor_page_info(&first_payload, 1);
    assert_eq!(first_items.len(), 1);
    let first_task_id = first_items[0]["taskId"]
        .as_str()
        .expect("taskId")
        .to_string();
    let cursor = next_cursor(&first_payload);

    let cursor_page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(runtime_path(&format!(
                    "/sessions/{session_id}/tasks?cursor={cursor}&page_size=1"
                )))
                .body(Body::empty())
                .expect("cursor request should be built"),
        )
        .await
        .expect("cursor request should succeed");
    assert_eq!(cursor_page.status(), StatusCode::OK);
    let cursor_payload = read_json(cursor_page).await;
    let cursor_items = list_items(&cursor_payload);
    let page_info = cursor_payload
        .get("data")
        .and_then(|data| data.get("pageInfo"))
        .expect("pageInfo");
    assert_eq!(
        page_info.get("mode").and_then(Value::as_str),
        Some("cursor")
    );
    assert_eq!(cursor_items.len(), 1);
    assert_ne!(cursor_items[0]["taskId"], first_task_id);
}

#[tokio::test]
async fn internal_runtime_list_rejects_offset_page_parameter() {
    let app = open_test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri(runtime_path("/sessions?page=1"))
                .body(Body::empty())
                .expect("list request should be built"),
        )
        .await
        .expect("list request should complete");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn internal_runtime_session_create_and_read_roundtrip() {
    let app = open_test_app();
    let create = Request::builder()
        .method("POST")
        .uri(runtime_path("/sessions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "agentId": "agent.1",
                "tenantId": "tenant.1",
                "title": "HTTP contract"
            })
            .to_string(),
        ))
        .expect("create request should be built");

    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);

    let session_payload = read_json(response).await;
    let session = item_value(&session_payload);
    let session_id = session["sessionId"]
        .as_str()
        .expect("sessionId should be present");
    assert_eq!(session["agentId"], "agent.1");
    assert_eq!(session["tenantId"], "tenant.1");

    let response = app
        .oneshot(
            Request::builder()
                .uri(runtime_path(&format!("/sessions/{session_id}")))
                .body(Body::empty())
                .expect("get request should be built"),
        )
        .await
        .expect("get request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let loaded_payload = read_json(response).await;
    let loaded = item_value(&loaded_payload);
    assert_eq!(loaded["sessionId"], session_id);
}

#[tokio::test]
async fn readiness_probe_checks_persistence() {
    let app = open_test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .expect("ready request should be built"),
        )
        .await
        .expect("ready request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload = read_json(response).await;
    assert_eq!(payload["status"], "ready");
}

#[tokio::test]
async fn ingress_token_auth_rejects_missing_credentials() {
    let app = token_test_app(TEST_INGRESS_TOKEN);
    let response = app
        .oneshot(
            Request::builder()
                .uri(internal_snapshot_path())
                .body(Body::empty())
                .expect("snapshot request should be built"),
        )
        .await
        .expect("snapshot request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ingress_token_auth_accepts_bearer_token() {
    let app = token_test_app(TEST_INGRESS_TOKEN);
    let response = app
        .oneshot(
            with_signed_identity(
                Request::builder().uri(internal_snapshot_path()),
                TEST_INGRESS_TOKEN,
                "tenant.owner",
                "user.owner",
            )
            .body(Body::empty())
            .expect("snapshot request should be built"),
        )
        .await
        .expect("snapshot request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn health_probes_bypass_ingress_token_auth() {
    let app = token_test_app(TEST_INGRESS_TOKEN);
    for (path, expected_status) in [("/healthz", "ok"), ("/readyz", "ready"), ("/livez", "ok")] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("health request should be built"),
            )
            .await
            .expect("health request should succeed");
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "path {path} should bypass auth"
        );
        let payload = read_json(response).await;
        assert_eq!(
            payload["status"], expected_status,
            "path {path} should return the canonical infrastructure probe body"
        );
    }
}

#[tokio::test]
async fn legacy_health_probe_paths_are_not_mounted() {
    let app = open_test_app();
    for path in ["/health", "/ready", "/live"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("legacy health request should be built"),
            )
            .await
            .expect("legacy health request should succeed");
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "legacy health path {path} must not remain mounted"
        );
    }
}

#[tokio::test]
async fn metrics_requires_token_when_metrics_auth_mode_is_token() {
    let config = ServerConfig {
        ingress_auth_mode: "token".to_string(),
        ingress_token: Some(TEST_INGRESS_TOKEN.to_string()),
        metrics_auth_mode: "token".to_string(),
        metrics_token: Some(TEST_INGRESS_TOKEN.to_string()),
        ..Default::default()
    };
    let app = app::build_test_app(Arc::new(config));

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .expect("metrics request should be built"),
        )
        .await
        .expect("metrics request should complete");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let allowed = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .header(AUTHORIZATION, format!("Bearer {TEST_INGRESS_TOKEN}"))
                .body(Body::empty())
                .expect("metrics request should be built"),
        )
        .await
        .expect("metrics request should succeed");
    assert_eq!(allowed.status(), StatusCode::OK);
}

#[tokio::test]
async fn token_policy_blocks_foreign_session_access() {
    let app = token_test_app(TEST_INGRESS_TOKEN);
    let create = with_signed_identity(
        Request::builder()
            .method("POST")
            .uri(runtime_path("/sessions"))
            .header(CONTENT_TYPE, "application/json"),
        TEST_INGRESS_TOKEN,
        "tenant.owner",
        "user.owner",
    )
    .body(Body::from(
        json!({
            "agentId": "agent.1",
            "title": "owned session"
        })
        .to_string(),
    ))
    .expect("create request should be built");

    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let session_payload = read_json(response).await;
    let session = item_value(&session_payload);
    let session_id = session["sessionId"]
        .as_str()
        .expect("sessionId should be present");

    let response = app
        .oneshot(
            with_signed_identity(
                Request::builder().uri(runtime_path(&format!("/sessions/{session_id}"))),
                TEST_INGRESS_TOKEN,
                "tenant.other",
                "user.other",
            )
            .body(Body::empty())
            .expect("get request should be built"),
        )
        .await
        .expect("get request should succeed");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn logging_middleware_emits_server_request_id() {
    let app = open_test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri(internal_snapshot_path())
                .header("x-request-id", "req.contract.1")
                .body(Body::empty())
                .expect("snapshot request should be built"),
        )
        .await
        .expect("snapshot request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(!request_id.is_empty());
    assert_ne!(request_id, "req.contract.1");
}

#[tokio::test]
async fn token_policy_blocks_foreign_task_access() {
    let app = token_test_app(TEST_INGRESS_TOKEN);
    let create = with_signed_identity(
        Request::builder()
            .method("POST")
            .uri(runtime_path("/sessions"))
            .header(CONTENT_TYPE, "application/json"),
        TEST_INGRESS_TOKEN,
        "tenant.owner",
        "user.owner",
    )
    .body(Body::from(
        json!({
            "agentId": "agent.1",
            "title": "task owner session"
        })
        .to_string(),
    ))
    .expect("create request should be built");

    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let session_payload = read_json(response).await;
    let session = item_value(&session_payload);
    let session_id = session["sessionId"]
        .as_str()
        .expect("sessionId should be present");

    let submit = with_signed_identity(
        Request::builder()
            .method("POST")
            .uri(runtime_path(&format!(
                "/sessions/{session_id}/tasks"
            )))
            .header(CONTENT_TYPE, "application/json"),
        TEST_INGRESS_TOKEN,
        "tenant.owner",
        "user.owner",
    )
    .body(Body::from(
        json!({ "instruction": "run contract task" }).to_string(),
    ))
    .expect("submit request should be built");

    let response = app
        .clone()
        .oneshot(submit)
        .await
        .expect("submit request should succeed");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let submit_payload = read_json(response).await;
    let run_id = submit_payload["data"]["operationId"]
        .as_str()
        .expect("operationId should be present");

    let foreign_run = app
        .clone()
        .oneshot(
            with_signed_identity(
                Request::builder().uri(runtime_path(&format!("/runs/{run_id}"))),
                TEST_INGRESS_TOKEN,
                "tenant.other",
                "user.other",
            )
            .body(Body::empty())
            .expect("get run request should be built"),
        )
        .await
        .expect("get run request should complete");
    assert_eq!(foreign_run.status(), StatusCode::FORBIDDEN);

    let owner_run = app
        .clone()
        .oneshot(
            with_signed_identity(
                Request::builder().uri(runtime_path(&format!("/runs/{run_id}"))),
                TEST_INGRESS_TOKEN,
                "tenant.owner",
                "user.owner",
            )
            .body(Body::empty())
            .expect("get owner run request should be built"),
        )
        .await
        .expect("get owner run request should succeed");
    assert_eq!(owner_run.status(), StatusCode::OK);
    let run_payload = read_json(owner_run).await;
    let run_item = item_value(&run_payload);
    // Int64 wire contract (API_SPEC §13.6): int64 fields cross the JSON
    // boundary as decimal strings so JS clients never lose precision.
    assert!(
        run_item["attempt"].is_string(),
        "attempt must serialize as an int64 string, got: {}",
        run_item["attempt"]
    );
    assert!(
        run_item["fencingToken"].is_string(),
        "fencingToken must serialize as an int64 string, got: {}",
        run_item["fencingToken"]
    );
    assert!(run_item["steps"].is_array());
    for step in run_item["steps"].as_array().expect("steps array") {
        assert!(
            step["sequenceNo"].is_string(),
            "step sequenceNo must serialize as an int64 string, got: {}",
            step["sequenceNo"]
        );
    }
    let task_id = run_item["taskId"]
        .as_str()
        .expect("taskId should be present");

    let response = app
        .oneshot(
            with_signed_identity(
                Request::builder().uri(runtime_path(&format!("/tasks/{task_id}"))),
                TEST_INGRESS_TOKEN,
                "tenant.other",
                "user.other",
            )
            .body(Body::empty())
            .expect("get task request should be built"),
        )
        .await
        .expect("get task request should succeed");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn closed_session_rejects_messages() {
    let app = open_test_app();
    let create = Request::builder()
        .method("POST")
        .uri(runtime_path("/sessions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "agentId": "agent.1",
                "title": "close contract"
            })
            .to_string(),
        ))
        .expect("create request should be built");

    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let session_payload = read_json(response).await;
    let session = item_value(&session_payload);
    let session_id = session["sessionId"]
        .as_str()
        .expect("sessionId should be present");

    let close = Request::builder()
        .method("POST")
        .uri(runtime_path(&format!("/sessions/{session_id}/close")))
        .body(Body::empty())
        .expect("close request should be built");
    let response = app
        .clone()
        .oneshot(close)
        .await
        .expect("close request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let send = Request::builder()
        .method("POST")
        .uri(runtime_path(&format!("/sessions/{session_id}/messages")))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "content": "after close" }).to_string()))
        .expect("send request should be built");
    let response = app
        .oneshot(send)
        .await
        .expect("send request should succeed");
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn closed_session_rejects_model_invoke() {
    let app = open_test_app();
    let session_id = create_session_on_open_app(&app, "agent.1").await;

    let close = Request::builder()
        .method("POST")
        .uri(runtime_path(&format!("/sessions/{session_id}/close")))
        .body(Body::empty())
        .expect("close request should be built");
    let response = app
        .clone()
        .oneshot(close)
        .await
        .expect("close request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let invoke = Request::builder()
        .method("POST")
        .uri(runtime_path(&format!(
            "/sessions/{session_id}/model/invoke"
        )))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(json!({}).to_string()))
        .expect("invoke request should be built");
    let response = app
        .oneshot(invoke)
        .await
        .expect("invoke request should succeed");
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn internal_runtime_send_message_persists_turn() {
    let app = open_test_app();
    let create = Request::builder()
        .method("POST")
        .uri(runtime_path("/sessions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "agentId": "agent.1",
                "title": "runtime turn contract"
            })
            .to_string(),
        ))
        .expect("create request should be built");

    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let session_payload = read_json(response).await;
    let session = item_value(&session_payload);
    let session_id = session["sessionId"]
        .as_str()
        .expect("sessionId should be present");

    let send = Request::builder()
        .method("POST")
        .uri(runtime_path(&format!("/sessions/{session_id}/messages")))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "content": "hello kernel" }).to_string()))
        .expect("send request should be built");
    let response = app
        .clone()
        .oneshot(send)
        .await
        .expect("send request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let turn_payload = read_json(response).await;
    let turn = item_value(&turn_payload);
    assert_eq!(turn["status"], "completed");
    assert_eq!(turn["userMessage"]["role"], "user");
    assert_eq!(turn["assistantMessage"]["role"], "assistant");

    let response = app
        .oneshot(
            Request::builder()
                .uri(runtime_path(&format!("/sessions/{session_id}/messages")))
                .body(Body::empty())
                .expect("list messages request should be built"),
        )
        .await
        .expect("list messages request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let messages = read_json(response).await;
    let message_items = list_items(&messages);
    assert_eq!(message_items.len(), 2);
    assert_eq!(message_items[0]["role"], "user");
    assert_eq!(message_items[1]["role"], "assistant");
}

#[tokio::test]
async fn internal_runtime_create_rejects_unknown_agent_id() {
    let app = open_test_app();
    let create = Request::builder()
        .method("POST")
        .uri(runtime_path("/sessions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "agentId": "agent.unregistered",
                "title": "should fail"
            })
            .to_string(),
        ))
        .expect("create request should be built");

    let response = app
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn internal_runtime_send_message_emits_turn_completed_event() {
    let app = open_test_app();
    let create = Request::builder()
        .method("POST")
        .uri(runtime_path("/sessions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "agentId": "agent.1",
                "title": "turn event contract"
            })
            .to_string(),
        ))
        .expect("create request should be built");

    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let session_payload = read_json(response).await;
    let session = item_value(&session_payload);
    let session_id = session["sessionId"]
        .as_str()
        .expect("sessionId should be present");

    let send = Request::builder()
        .method("POST")
        .uri(runtime_path(&format!("/sessions/{session_id}/messages")))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "content": "hello kernel" }).to_string()))
        .expect("send request should be built");
    let response = app
        .clone()
        .oneshot(send)
        .await
        .expect("send request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .oneshot(
            Request::builder()
                .uri(runtime_path(&format!(
                    "/sessions/{session_id}/events/stream?live=false"
                )))
                .header("Accept", "text/event-stream")
                .body(Body::empty())
                .expect("events request should be built"),
        )
        .await
        .expect("events request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("events body should be readable");
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("turn.completed"));
}

#[tokio::test]
async fn session_event_stream_honors_last_event_id() {
    let app = open_test_app();
    let create = Request::builder()
        .method("POST")
        .uri(runtime_path("/sessions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "agentId": "agent.1",
                "title": "event stream contract"
            })
            .to_string(),
        ))
        .expect("create request should be built");

    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let session_payload = read_json(response).await;
    let session = item_value(&session_payload);
    let session_id = session["sessionId"]
        .as_str()
        .expect("sessionId should be present");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(runtime_path(&format!(
                    "/sessions/{session_id}/events/stream?live=false"
                )))
                .header("Accept", "text/event-stream")
                .body(Body::empty())
                .expect("stream request should be built"),
        )
        .await
        .expect("stream request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("stream body should be readable");
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("data:"),
        "stream should contain SSE data frames"
    );
    let first_event_id = text
        .lines()
        .find_map(|line| line.strip_prefix("id:").map(str::trim))
        .expect("stream should include SSE id fields");

    let response = app
        .oneshot(
            Request::builder()
                .uri(runtime_path(&format!(
                    "/sessions/{session_id}/events/stream?live=false"
                )))
                .header("Accept", "text/event-stream")
                .header("Last-Event-ID", first_event_id)
                .body(Body::empty())
                .expect("resume stream request should be built"),
        )
        .await
        .expect("resume stream request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let resumed = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("resume stream body should be readable");
    let resumed_text = String::from_utf8_lossy(&resumed);
    assert!(
        !resumed_text.contains(&format!("id: {first_event_id}")),
        "resumed stream should skip events at or before Last-Event-ID"
    );
}

#[tokio::test]
async fn internal_runtime_session_api_enforces_token_scope() {
    let app = token_test_app(TEST_INGRESS_TOKEN);
    let create = with_signed_identity(
        Request::builder()
            .method("POST")
            .uri(runtime_path("/sessions"))
            .header(CONTENT_TYPE, "application/json"),
        TEST_INGRESS_TOKEN,
        "tenant.owner",
        "user.owner",
    )
    .body(Body::from(
        json!({
            "agentId": "agent.1",
            "title": "legacy owned session"
        })
        .to_string(),
    ))
    .expect("create request should be built");

    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let session_payload = read_json(response).await;
    let session = item_value(&session_payload);
    let session_id = session["sessionId"]
        .as_str()
        .or(session["session_id"].as_str())
        .expect("session id should be present");

    let response = app
        .oneshot(
            with_signed_identity(
                Request::builder().uri(runtime_path(&format!("/sessions/{session_id}"))),
                TEST_INGRESS_TOKEN,
                "tenant.other",
                "user.other",
            )
            .body(Body::empty())
            .expect("get request should be built"),
        )
        .await
        .expect("get request should succeed");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn internal_runtime_send_message_rejects_foreign_session_in_token_mode() {
    let app = token_test_app(TEST_INGRESS_TOKEN);
    let create = with_signed_identity(
        Request::builder()
            .method("POST")
            .uri(runtime_path("/sessions"))
            .header(CONTENT_TYPE, "application/json"),
        TEST_INGRESS_TOKEN,
        "tenant.owner",
        "user.owner",
    )
    .body(Body::from(
        json!({
            "agentId": "agent.1",
            "title": "owned stream session"
        })
        .to_string(),
    ))
    .expect("create request should be built");

    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let session_payload = read_json(response).await;
    let session = item_value(&session_payload);
    let session_id = session["sessionId"]
        .as_str()
        .expect("sessionId should be present");

    let stream = with_signed_identity(
        Request::builder()
            .method("POST")
            .uri(runtime_path(&format!("/sessions/{session_id}/messages")))
            .header(CONTENT_TYPE, "application/json"),
        TEST_INGRESS_TOKEN,
        "tenant.other",
        "user.other",
    )
    .body(Body::from(
        json!({
            "content": "hello"
        })
        .to_string(),
    ))
    .expect("message request should be built");

    let response = app
        .oneshot(stream)
        .await
        .expect("stream request should succeed");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn token_mode_rejects_session_missing_owner_metadata() {
    let persistence = Arc::new(
        sdkwork_agent_server::persistence::PersistenceState::memory()
            .expect("in-memory persistence should initialize for tests"),
    );
    let health_state = Arc::new(sdkwork_agent_server::health::HealthState::new());

    let open_config = Arc::new(ServerConfig {
        ingress_auth_mode: "open".to_string(),
        ..Default::default()
    });
    let open_runtime = Arc::new(
        sdkwork_agent_server::api::internal_runtime::InternalRuntimeApiState::new(
            persistence.clone(),
            open_config.clone(),
        )
        .expect("runtime state should initialize for tests"),
    );
    let open_app = app::build_app(
        open_config,
        health_state.clone(),
        persistence.clone(),
        open_runtime,
    );

    let create = Request::builder()
        .method("POST")
        .uri(runtime_path("/sessions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "agentId": "agent.1",
                "title": "open-mode session without owner metadata"
            })
            .to_string(),
        ))
        .expect("create request should be built");

    let response = open_app
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let session_payload = read_json(response).await;
    let session = item_value(&session_payload);
    let session_id = session["sessionId"]
        .as_str()
        .expect("session id should be present");

    let token_config = Arc::new(ServerConfig {
        ingress_auth_mode: "token".to_string(),
        ingress_token: Some(TEST_INGRESS_TOKEN.to_string()),
        ..Default::default()
    });
    let token_runtime = Arc::new(
        sdkwork_agent_server::api::internal_runtime::InternalRuntimeApiState::new(
            persistence.clone(),
            token_config.clone(),
        )
        .expect("runtime state should initialize for tests"),
    );
    let token_app = app::build_app(token_config, health_state, persistence, token_runtime);

    let response = token_app
        .oneshot(
            with_signed_identity(
                Request::builder().uri(runtime_path(&format!("/sessions/{session_id}"))),
                TEST_INGRESS_TOKEN,
                "tenant.owner",
                "user.owner",
            )
            .body(Body::empty())
            .expect("get request should be built"),
        )
        .await
        .expect("get request should succeed");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn internal_runtime_delete_session_returns_no_content() {
    let app = open_test_app();
    let create = Request::builder()
        .method("POST")
        .uri(runtime_path("/sessions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "agentId": "agent.1",
                "title": "delete contract"
            })
            .to_string(),
        ))
        .expect("create request should be built");

    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let session_payload = read_json(response).await;
    let session = item_value(&session_payload);
    let session_id = session["sessionId"]
        .as_str()
        .expect("sessionId should be present");

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(runtime_path(&format!("/sessions/{session_id}")))
                .body(Body::empty())
                .expect("delete request should be built"),
        )
        .await
        .expect("delete request should succeed");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn metrics_endpoint_exposes_prometheus_families_without_auth() {
    let app = open_test_app();
    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .expect("health request should be built"),
        )
        .await
        .expect("health request should succeed");
    assert_eq!(health.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .expect("metrics request should be built"),
        )
        .await
        .expect("metrics request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("metrics body should be readable");
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("sdkwork_kernel_health_status"));
    assert!(text.contains("sdkwork_kernel_http_requests_total"));
    assert!(text.contains("sdkwork_kernel_http_auth_failures_total"));
    assert!(text.contains("sdkwork_kernel_http_rate_limited_total"));
    assert!(text.contains("sdkwork_kernel_provider_admission_capacity"));
    assert!(text.contains("sdkwork_kernel_provider_admission_wait_capacity"));
    assert!(text.contains("sdkwork_kernel_provider_admission_active"));
    assert!(text.contains("sdkwork_kernel_provider_admission_waiting"));
    assert!(text.contains("sdkwork_kernel_provider_admission_rejected_total"));
    assert!(text.contains("sdkwork_kernel_provider_admission_acquire_duration_seconds_bucket"));
    assert!(text.contains("sdkwork_kernel_runtime_persistence_backend_info"));
    assert!(text.contains("sdkwork_kernel_rate_limit_backend_info"));
    assert!(text.contains("backend=\"sqlite\""));
    assert!(text.contains("backend=\"memory\""));
}

fn jwt_test_app(secret: &str) -> Router {
    let config = ServerConfig {
        ingress_auth_mode: "jwt".to_string(),
        ingress_jwt_secret: Some(secret.to_string()),
        ingress_jwt_issuer: Some("sdkwork-kernel".to_string()),
        ingress_jwt_audience: Some("internal-api".to_string()),
        ..Default::default()
    };
    app::build_test_app(Arc::new(config))
}

fn mint_ingress_jwt(secret: &str, tenant: &str, user: &str) -> String {
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    let exp = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;
    let claims = serde_json::json!({
        "sub": user,
        "tenant_id": tenant,
        "user_id": user,
        "exp": exp,
        "iss": "sdkwork-kernel",
        "aud": "internal-api",
    });
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("jwt should encode in contract test")
}

#[tokio::test]
async fn ingress_jwt_auth_accepts_bearer_jwt_without_identity_mac() {
    let secret = "jwt-contract-secret";
    let app = jwt_test_app(secret);
    let token = mint_ingress_jwt(secret, "tenant-jwt", "user-jwt");
    let response = app
        .oneshot(
            Request::builder()
                .uri(internal_snapshot_path())
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .expect("snapshot request should be built"),
        )
        .await
        .expect("snapshot request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
}

fn rs256_jwt_test_app() -> Router {
    let pem = include_str!("fixtures/ingress_jwt_rs256_public.pem");
    let config = ServerConfig {
        ingress_auth_mode: "jwt".to_string(),
        ingress_jwt_algorithm: "rs256".to_string(),
        ingress_jwt_rsa_public_key_pem: Some(pem.to_string()),
        ingress_jwt_issuer: Some("sdkwork-kernel".to_string()),
        ingress_jwt_audience: Some("internal-api".to_string()),
        ..Default::default()
    };
    app::build_test_app(Arc::new(config))
}

fn mint_rs256_ingress_jwt(tenant: &str, user: &str) -> String {
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    let private_pem = include_str!("fixtures/ingress_jwt_rs256_private.pem");
    let exp = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;
    let claims = serde_json::json!({
        "sub": user,
        "tenant_id": tenant,
        "user_id": user,
        "exp": exp,
        "iss": "sdkwork-kernel",
        "aud": "internal-api",
    });
    encode(
        &Header::new(Algorithm::RS256),
        &claims,
        &EncodingKey::from_rsa_pem(private_pem.as_bytes()).expect("rsa private key"),
    )
    .expect("rs256 jwt should encode in contract test")
}

#[tokio::test]
async fn ingress_rs256_jwt_auth_accepts_bearer_without_identity_mac() {
    let app = rs256_jwt_test_app();
    let token = mint_rs256_ingress_jwt("tenant-rs256", "user-rs256");
    let response = app
        .oneshot(
            Request::builder()
                .uri(internal_snapshot_path())
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .expect("snapshot request should be built"),
        )
        .await
        .expect("snapshot request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn internal_runtime_model_invoke_rejects_exhausted_tenant_token_quota() {
    let tenant = "tenant-quota";
    let app = quota_test_app(tenant, 0);
    let create = with_signed_identity(
        Request::builder()
            .method("POST")
            .uri(runtime_path("/sessions"))
            .header(CONTENT_TYPE, "application/json"),
        TEST_INGRESS_TOKEN,
        tenant,
        "user-quota",
    )
    .body(Body::from(
        json!({
            "agentId": "agent.1",
            "title": "quota contract session"
        })
        .to_string(),
    ))
    .expect("create request should be built");

    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let session_payload = read_json(response).await;
    let session = item_value(&session_payload);
    let session_id = session["sessionId"]
        .as_str()
        .expect("sessionId should be present");

    let invoke = with_signed_identity(
        Request::builder()
            .method("POST")
            .uri(runtime_path(&format!(
                "/sessions/{session_id}/model/invoke"
            )))
            .header(CONTENT_TYPE, "application/json"),
        TEST_INGRESS_TOKEN,
        tenant,
        "user-quota",
    )
    .body(Body::from(json!({}).to_string()))
    .expect("invoke request should be built");

    let response = app
        .oneshot(invoke)
        .await
        .expect("invoke request should succeed");
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}

/// Helper: create a session on the open test app and return its session id.
async fn create_session_on_open_app(app: &Router, agent_id: &str) -> String {
    let create = Request::builder()
        .method("POST")
        .uri(runtime_path("/sessions"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "agentId": agent_id,
                "title": "stream contract session"
            })
            .to_string(),
        ))
        .expect("create request should be built");

    let response = app
        .clone()
        .oneshot(create)
        .await
        .expect("create request should succeed");
    assert_eq!(response.status(), StatusCode::CREATED);
    let session_payload = read_json(response).await;
    let session = item_value(&session_payload);
    session["sessionId"]
        .as_str()
        .expect("sessionId should be present")
        .to_string()
}

/// Read the full SSE response body as a UTF-8 string.
async fn read_sse_body(response: axum::response::Response) -> String {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("sse body should be readable");
    String::from_utf8(body.to_vec()).expect("sse body should be valid utf-8")
}

#[tokio::test]
async fn internal_runtime_model_stream_returns_sse_chunks() {
    let app = open_test_app();
    let session_id = create_session_on_open_app(&app, "agent.1").await;

    let stream_request = Request::builder()
        .method("POST")
        .uri(runtime_path(&format!(
            "/sessions/{session_id}/model/stream"
        )))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "modelId": "gpt-4",
                "messages": ["Hello, tell me a joke"]
            })
            .to_string(),
        ))
        .expect("stream request should be built");

    let response = app
        .oneshot(stream_request)
        .await
        .expect("stream request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(
        content_type.starts_with("text/event-stream"),
        "response content-type should be text/event-stream, got: {content_type}"
    );

    let body = read_sse_body(response).await;
    assert!(
        body.contains("event:model.chunk") || body.contains("event: model.chunk"),
        "sse body should contain model.chunk events: {body}"
    );
    let chunk_count = body
        .matches("event:model.chunk")
        .chain(body.matches("event: model.chunk"))
        .count();
    assert!(
        chunk_count >= 2,
        "mock stream should emit multiple incremental chunks, got {chunk_count}: {body}"
    );
    assert!(
        body.contains("event:model.done") || body.contains("event: model.done"),
        "sse body should contain model.done terminator: {body}"
    );
}

#[tokio::test]
async fn internal_runtime_model_stream_uses_sse_timeout_not_standard_timeout() {
    let config = Arc::new(ServerConfig {
        request_timeout_secs: 0,
        sse_request_timeout_secs: 5,
        ..Default::default()
    });
    let persistence = Arc::new(
        sdkwork_agent_server::persistence::PersistenceState::memory()
            .expect("in-memory persistence should initialize for tests"),
    );
    let session = persistence
        .create_session(sdkwork_agent_session::SessionConfig::new("agent.1"))
        .expect("session should be created");
    let runtime_state = Arc::new(
        sdkwork_agent_server::api::internal_runtime::InternalRuntimeApiState::new(
            persistence.clone(),
            config.clone(),
        )
        .expect("runtime state should initialize for tests"),
    );
    let app = app::build_app(
        config,
        Arc::new(sdkwork_agent_server::health::HealthState::new()),
        persistence,
        runtime_state,
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(runtime_path(&format!(
                    "/sessions/{}/model/stream",
                    session.session_id
                )))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "messages": ["hello"] }).to_string()))
                .expect("stream request should be built"),
        )
        .await
        .expect("stream request should succeed");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "model stream must use the long SSE timeout, not the standard JSON timeout"
    );
}

#[tokio::test]
async fn internal_runtime_model_cancel_returns_cancelled_response() {
    let app = open_test_app();
    let session_id = create_session_on_open_app(&app, "agent.1").await;

    let cancel_request = Request::builder()
        .method("POST")
        .uri(runtime_path(&format!(
            "/sessions/{session_id}/model/cancel"
        )))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "modelRequestId": "model-req.test-cancel-001",
                "providerId": null
            })
            .to_string(),
        ))
        .expect("cancel request should be built");

    let response = app
        .oneshot(cancel_request)
        .await
        .expect("cancel request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let body = read_json(response).await;
    let body = item_value(&body);
    assert_eq!(
        body["modelRequestId"], "model-req.test-cancel-001",
        "cancel response should echo the model request id"
    );
    assert!(
        body["providerId"].is_string(),
        "cancel response should include providerId"
    );
    assert_eq!(
        body["status"], "cancelled",
        "cancel response status should be 'cancelled'"
    );
    assert_eq!(
        body["finishReason"], "cancelled",
        "cancel response finishReason should be 'cancelled'"
    );
}

#[tokio::test]
async fn internal_runtime_model_stream_rejects_unknown_session() {
    let app = open_test_app();

    let stream_request = Request::builder()
        .method("POST")
        .uri(runtime_path("/sessions/session.nonexistent/model/stream"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "messages": ["Hello"]
            })
            .to_string(),
        ))
        .expect("stream request should be built");

    let response = app
        .oneshot(stream_request)
        .await
        .expect("stream request should succeed");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn internal_runtime_model_cancel_rejects_unknown_session() {
    let app = open_test_app();

    let cancel_request = Request::builder()
        .method("POST")
        .uri(runtime_path("/sessions/session.nonexistent/model/cancel"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "modelRequestId": "model-req.unknown"
            })
            .to_string(),
        ))
        .expect("cancel request should be built");

    let response = app
        .oneshot(cancel_request)
        .await
        .expect("cancel request should succeed");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
