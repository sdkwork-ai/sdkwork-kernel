use crate::binding::AgentSdkBindingManifest;
use crate::driver::{AgentSdkCapabilityDriver, SdkDriverHealth};
use crate::negotiation::{NegotiatedCapability, SdkCapabilityNegotiation, SdkNegotiationError};
use crate::selector::{effective_backend_priority, select_backend};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Default)]
pub struct DriverRegistry {
    drivers: HashMap<String, Arc<dyn AgentSdkCapabilityDriver>>,
}

impl DriverRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, driver: Arc<dyn AgentSdkCapabilityDriver>) {
        self.drivers.insert(driver.driver_id().to_string(), driver);
    }

    pub fn get(&self, driver_id: &str) -> Option<Arc<dyn AgentSdkCapabilityDriver>> {
        self.drivers.get(driver_id).cloned()
    }

    pub fn health(&self, driver_id: &str) -> Option<SdkDriverHealth> {
        self.get(driver_id).map(|driver| driver.health())
    }

    pub fn len(&self) -> usize {
        self.drivers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.drivers.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct RegisteredBinding {
    pub manifest: AgentSdkBindingManifest,
}

#[derive(Debug, Default)]
pub struct BindingRegistry {
    bindings: HashMap<String, RegisteredBinding>,
}

impl BindingRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, manifest: AgentSdkBindingManifest) {
        let key = manifest.binding_id.clone();
        self.bindings.insert(key, RegisteredBinding { manifest });
    }

    pub fn get(&self, binding_id: &str) -> Option<&RegisteredBinding> {
        self.bindings.get(binding_id)
    }

    pub fn negotiate(
        &self,
        binding_id: &str,
        drivers: &DriverRegistry,
    ) -> Result<SdkCapabilityNegotiation, SdkNegotiationError> {
        let binding = self
            .get(binding_id)
            .ok_or_else(|| SdkNegotiationError::binding_not_found(binding_id))?;

        let priority = binding.manifest.backend_priority();
        let mut selected = Vec::new();
        let mut missing_required = Vec::new();
        let mut degraded_optional = Vec::new();

        for capability in &binding.manifest.capabilities {
            let capability_priority = effective_backend_priority(&priority, capability);

            let selection = select_backend(capability, &capability_priority, |driver_id| {
                drivers
                    .health(driver_id)
                    .or_else(|| Some(SdkDriverHealth::unhealthy("driver not registered")))
            });

            let Some(selection) = selection else {
                if capability.required {
                    missing_required.push(capability.capability_id.clone());
                } else {
                    degraded_optional.push(capability.capability_id.clone());
                }
                continue;
            };

            if drivers.get(&selection.backend.driver_id).is_none() {
                if capability.required {
                    missing_required.push(capability.capability_id.clone());
                } else {
                    degraded_optional.push(capability.capability_id.clone());
                }
                continue;
            }

            selected.push(NegotiatedCapability {
                capability_id: capability.capability_id.clone(),
                backend_kind: selection.backend.kind,
                driver_id: selection.backend.driver_id.clone(),
                runtime_operations: selection.backend.runtime_operations.clone(),
                execution_scope: capability.execution_scope,
            });
        }

        if !missing_required.is_empty() {
            return Err(SdkNegotiationError::missing_required_capabilities(
                binding.manifest.agent_id.clone(),
                missing_required,
            ));
        }

        Ok(SdkCapabilityNegotiation {
            agent_id: binding.manifest.agent_id.clone(),
            binding_id: binding.manifest.binding_id.clone(),
            binding_version: binding.manifest.version.clone(),
            selected,
            missing_required,
            degraded_optional,
        })
    }
}
