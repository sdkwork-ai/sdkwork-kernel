use crate::backend::{default_backend_priority, SdkBackendKind};
use crate::binding::{BackendCandidate, CapabilityBinding};
use crate::driver::SdkDriverHealth;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendSelection<'a> {
    pub capability_id: &'a str,
    pub backend: &'a BackendCandidate,
}

/// Select the first backend candidate that matches priority order and passes
/// health. A `None` health probe (driver registered but never probed, or
/// absent from the health source) is treated as unhealthy: backend selection
/// is fail-closed, and a driver that has not proven health cannot win a
/// binding.
pub fn select_backend<'a>(
    capability: &'a CapabilityBinding,
    priority: &[SdkBackendKind],
    driver_health: impl Fn(&str) -> Option<SdkDriverHealth>,
) -> Option<BackendSelection<'a>> {
    let probed_health =
        |driver_id: &str| -> Option<SdkDriverHealth> {
            driver_health(driver_id).or_else(|| {
                Some(SdkDriverHealth {
                    status: crate::driver::SdkDriverStatus::Unhealthy,
                    message: Some("driver health is unknown (never probed)".to_string()),
                })
            })
        };
    for kind in priority {
        for backend in &capability.backends {
            if backend.kind != *kind {
                continue;
            }
            let health = probed_health(&backend.driver_id);
            if health.is_some_and(|health| health.is_usable()) {
                return Some(BackendSelection {
                    capability_id: capability.capability_id.as_str(),
                    backend,
                });
            }
        }
    }

    // Fall back to manifest order when priority reordering yields no healthy match.
    for backend in &capability.backends {
        let health = probed_health(&backend.driver_id);
        if health.is_some_and(|health| health.is_usable()) {
            return Some(BackendSelection {
                capability_id: capability.capability_id.as_str(),
                backend,
            });
        }
    }

    None
}

/// Merge manifest backend order with global default priority.
pub fn effective_backend_priority(
    manifest_priority: &[SdkBackendKind],
    capability: &CapabilityBinding,
) -> Vec<SdkBackendKind> {
    let mut ordered = Vec::new();
    for backend in &capability.backends {
        if !ordered.contains(&backend.kind) {
            ordered.push(backend.kind);
        }
    }

    for kind in manifest_priority {
        if !ordered.contains(kind) {
            ordered.push(*kind);
        }
    }

    for kind in default_backend_priority() {
        if !ordered.contains(kind) {
            ordered.push(*kind);
        }
    }

    ordered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{BackendCandidate, CapabilityExecutionScope};
    use crate::driver::SdkDriverStatus;
    use crate::runtime::SdkRuntimeOperationKind;

    fn candidate(kind: SdkBackendKind, driver_id: &str) -> BackendCandidate {
        BackendCandidate {
            kind,
            driver_id: driver_id.to_string(),
            runtime_operations: vec![SdkRuntimeOperationKind::Ping],
            rust_crate: None,
            package: None,
            python_module: None,
            openapi_authority: None,
            transport: None,
        }
    }

    #[test]
    fn prefers_rust_over_typescript_when_both_declared() {
        let capability = CapabilityBinding {
            capability_id: "sdk.model.chat".to_string(),
            required: true,
            execution_scope: CapabilityExecutionScope::TransportRuntime,
            backends: vec![
                candidate(SdkBackendKind::TypeScriptNode, "driver.test.model.chat.ts"),
                candidate(SdkBackendKind::RustNative, "driver.test.model.chat.rust"),
            ],
        };

        let selection = select_backend(&capability, default_backend_priority(), |_| {
            Some(SdkDriverHealth::healthy())
        })
        .expect("expected selection");

        assert_eq!(selection.backend.driver_id, "driver.test.model.chat.rust");
    }

    #[test]
    fn skips_unhealthy_backends() {
        let capability = CapabilityBinding {
            capability_id: "sdk.model.chat".to_string(),
            required: true,
            execution_scope: CapabilityExecutionScope::TransportRuntime,
            backends: vec![
                candidate(SdkBackendKind::RustNative, "driver.test.model.chat.rust"),
                candidate(SdkBackendKind::TypeScriptNode, "driver.test.model.chat.ts"),
            ],
        };

        let selection = select_backend(&capability, default_backend_priority(), |driver_id| {
            if driver_id.ends_with(".rust") {
                Some(SdkDriverHealth {
                    status: SdkDriverStatus::Unhealthy,
                    message: Some("offline".to_string()),
                })
            } else {
                Some(SdkDriverHealth::healthy())
            }
        })
        .expect("expected fallback selection");

        assert_eq!(selection.backend.driver_id, "driver.test.model.chat.ts");
    }
}
