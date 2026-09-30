//! HTTP surface classification for observability labels (`OBSERVABILITY_SPEC.md`).

use crate::runtime_routes::INTERNAL_RUNTIME_MOUNT_PREFIX;

/// Classifies request paths into SDKWork `api_surface` labels for logs and metrics.
pub fn classify_api_surface(path: &str) -> Option<&'static str> {
    if path.starts_with(INTERNAL_RUNTIME_MOUNT_PREFIX) {
        return Some("internal-api");
    }

    None
}

/// Maps raw request paths to stable route templates for logs and metrics.
pub fn route_template(path: &str) -> String {
    match path {
        "/healthz" | "/readyz" | "/livez" | "/metrics" => path.to_string(),
        _ if path.starts_with(INTERNAL_RUNTIME_MOUNT_PREFIX) => {
            internal_runtime_route_template(path).to_string()
        }
        _ => "unknown".to_string(),
    }
}

fn internal_runtime_route_template(path: &str) -> &'static str {
    let relative = path
        .strip_prefix(INTERNAL_RUNTIME_MOUNT_PREFIX)
        .unwrap_or(path);

    match relative {
        "/snapshot" => "/internal/v3/api/intelligence/runtime/snapshot",
        p if p.starts_with("/permissions/") => {
            "/internal/v3/api/intelligence/runtime/permissions/{permission_request_id}"
        }
        "/sessions" => "/internal/v3/api/intelligence/runtime/sessions",
        p if p.starts_with("/sessions/") && p.ends_with("/close") => {
            "/internal/v3/api/intelligence/runtime/sessions/{session_id}/close"
        }
        p if p.starts_with("/sessions/") && p.ends_with("/messages") => {
            "/internal/v3/api/intelligence/runtime/sessions/{session_id}/messages"
        }
        p if p.starts_with("/sessions/") && p.ends_with("/tasks") => {
            "/internal/v3/api/intelligence/runtime/sessions/{session_id}/tasks"
        }
        p if p.starts_with("/sessions/") && p.ends_with("/model/invoke") => {
            "/internal/v3/api/intelligence/runtime/sessions/{session_id}/model/invoke"
        }
        p if p.starts_with("/sessions/") && p.ends_with("/model/stream") => {
            "/internal/v3/api/intelligence/runtime/sessions/{session_id}/model/stream"
        }
        p if p.starts_with("/sessions/") && p.ends_with("/tools") => {
            "/internal/v3/api/intelligence/runtime/sessions/{session_id}/tools"
        }
        p if p.starts_with("/sessions/") && p.contains("/tools/") && p.ends_with("/execute") => {
            "/internal/v3/api/intelligence/runtime/sessions/{session_id}/tools/{tool_name}/execute"
        }
        p if p.starts_with("/sessions/") && p.ends_with("/events/stream") => {
            "/internal/v3/api/intelligence/runtime/sessions/{session_id}/events/stream"
        }
        p if p.starts_with("/sessions/") => {
            "/internal/v3/api/intelligence/runtime/sessions/{session_id}"
        }
        p if p.starts_with("/tasks/") && p.ends_with("/cancel") => {
            "/internal/v3/api/intelligence/runtime/tasks/{task_id}/cancel"
        }
        p if p.starts_with("/tasks/") && p.ends_with("/retry") => {
            "/internal/v3/api/intelligence/runtime/tasks/{task_id}/retry"
        }
        p if p.starts_with("/tasks/") => "/internal/v3/api/intelligence/runtime/tasks/{task_id}",
        p if p.starts_with("/runs/") && p.ends_with("/pause") => {
            "/internal/v3/api/intelligence/runtime/runs/{run_id}/pause"
        }
        p if p.starts_with("/runs/") && p.ends_with("/resume") => {
            "/internal/v3/api/intelligence/runtime/runs/{run_id}/resume"
        }
        p if p.starts_with("/runs/") && p.ends_with("/cancel") => {
            "/internal/v3/api/intelligence/runtime/runs/{run_id}/cancel"
        }
        p if p.starts_with("/runs/") => "/internal/v3/api/intelligence/runtime/runs/{run_id}",
        "/models" => "/internal/v3/api/intelligence/runtime/models",
        _ => "/internal/v3/api/intelligence/runtime/*",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_runtime_paths_label_internal_api() {
        assert_eq!(
            classify_api_surface("/internal/v3/api/intelligence/runtime/snapshot"),
            Some("internal-api")
        );
    }

    #[test]
    fn retired_kernel_alias_paths_are_unlabeled() {
        assert_eq!(classify_api_surface("/api/kernel/snapshot"), None);
    }

    #[test]
    fn retired_session_paths_are_unlabeled() {
        assert_eq!(classify_api_surface("/api/sessions"), None);
        assert_eq!(classify_api_surface("/api/chat/send"), None);
    }

    #[test]
    fn route_templates_use_stable_placeholders() {
        assert_eq!(
            route_template("/internal/v3/api/intelligence/runtime/sessions/sess.1/messages"),
            "/internal/v3/api/intelligence/runtime/sessions/{session_id}/messages"
        );
        assert_eq!(
            route_template("/internal/v3/api/intelligence/runtime/permissions/perm.1"),
            "/internal/v3/api/intelligence/runtime/permissions/{permission_request_id}"
        );
        assert_eq!(route_template("/metrics"), "/metrics");
        assert_eq!(route_template("/healthz"), "/healthz");
    }
}
