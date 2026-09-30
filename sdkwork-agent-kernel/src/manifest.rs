use crate::{KernelError, KernelResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentManifest {
    pub schema_version: String,
    pub manifest_type: String,
    pub agent_id: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub version: String,
    pub domain: String,
    pub required_capabilities: Vec<String>,
    pub optional_capabilities: Vec<String>,
    pub required_capability_requirements: Vec<CapabilityRequirement>,
    pub optional_capability_requirements: Vec<CapabilityRequirement>,
    pub event_families: Vec<String>,
    pub owner_name: String,
    pub status: String,
}

impl AgentManifest {
    pub fn from_json(input: &str) -> KernelResult<Self> {
        let manifest_type = extract_string(input, "manifest_type")?;
        if manifest_type == "agent_definition" {
            return Self::from_json(&extract_object_body(input, "agent")?);
        }
        if manifest_type != "agent" {
            return Err(KernelError::validation("manifest_type must be agent"));
        }

        let required_capability_requirements =
            extract_capability_requirements(input, "required_capabilities")?;
        let optional_capability_requirements =
            extract_capability_requirements(input, "optional_capabilities")?;

        Ok(Self {
            schema_version: extract_string(input, "schema_version")?,
            manifest_type,
            agent_id: extract_string(input, "agent_id")?,
            name: extract_string(input, "name")?,
            display_name: extract_string(input, "display_name")?,
            description: extract_string(input, "description")?,
            version: extract_string(input, "version")?,
            domain: extract_string(input, "domain")?,
            required_capabilities: required_capability_requirements
                .iter()
                .map(|requirement| requirement.capability_id.clone())
                .collect(),
            optional_capabilities: optional_capability_requirements
                .iter()
                .map(|requirement| requirement.capability_id.clone())
                .collect(),
            required_capability_requirements,
            optional_capability_requirements,
            event_families: extract_string_array(input, "event_families")?,
            owner_name: extract_owner_name(input)?,
            status: extract_string(input, "status")?,
        })
    }

    pub fn requires_capability(&self, capability_id: &str) -> bool {
        self.required_capabilities
            .iter()
            .any(|required| required == capability_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRequirement {
    pub capability_id: String,
    pub min_version: Option<String>,
}

impl CapabilityRequirement {
    pub fn new(capability_id: impl Into<String>) -> Self {
        Self {
            capability_id: capability_id.into(),
            min_version: None,
        }
    }

    pub fn with_min_version(mut self, min_version: impl Into<String>) -> Self {
        self.min_version = Some(min_version.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderManifest {
    pub provider_id: String,
    pub provider_family: String,
    pub name: String,
    pub version: String,
    pub capabilities: Vec<String>,
}

impl ProviderManifest {
    pub fn new(
        provider_id: impl Into<String>,
        provider_family: impl Into<String>,
        name: impl Into<String>,
        version: impl Into<String>,
        capabilities: Vec<String>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            provider_family: provider_family.into(),
            name: name.into(),
            version: version.into(),
            capabilities,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHealth {
    pub status: String,
}

impl ProviderHealth {
    pub fn available() -> Self {
        Self {
            status: "available".to_string(),
        }
    }

    /// Express an unavailable provider with a human-readable reason.
    ///
    /// The kernel requires providers to fail closed when required security
    /// policy is unavailable; this constructor standardizes the unhealthy
    /// side of the health contract for manifests, diagnostics, and plugins.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            status: format!("unavailable: {}", reason.into()),
        }
    }

    /// Express a degraded provider.
    ///
    /// Degraded health reports a working-but-limited provider (partial
    /// capability registration, reduced capacity). The status vocabulary is
    /// the bare `"degraded"` token shared with backend-health, execution,
    /// and rate-limit state reporting; any non-`available` status keeps
    /// readiness fail-closed for profiles that require full provider health.
    pub fn degraded(_reason: impl Into<String>) -> Self {
        Self {
            status: "degraded".to_string(),
        }
    }

    /// Whether this health snapshot reports an available provider.
    pub fn is_available(&self) -> bool {
        self.status == "available"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub capability_id: String,
    pub version: String,
    pub provider_id: String,
    pub status: String,
    pub required: bool,
    pub operations: Vec<String>,
    pub side_effect_level: Option<String>,
    pub policy_categories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityManifest {
    pub schema_version: String,
    pub manifest_type: String,
    pub runtime_id: String,
    pub agent_id: String,
    pub kernel_version: String,
    pub providers: Vec<ProviderManifest>,
    pub capabilities: Vec<Capability>,
    pub missing_required_capabilities: Vec<String>,
    pub degraded_capabilities: Vec<String>,
    pub protocol_adapters: Vec<String>,
    pub security_profile: String,
    pub generated_at: String,
}

impl CapabilityManifest {
    pub fn is_ready(&self) -> bool {
        self.missing_required_capabilities.is_empty()
    }

    pub fn missing_required_capabilities(&self) -> &[String] {
        &self.missing_required_capabilities
    }
}

fn extract_string(input: &str, key: &str) -> KernelResult<String> {
    let pattern = format!("\"{key}\"");
    let key_start = input
        .find(&pattern)
        .ok_or_else(|| KernelError::validation(format!("missing field: {key}")))?;
    let after_key = &input[key_start + pattern.len()..];
    let colon = after_key
        .find(':')
        .ok_or_else(|| KernelError::validation(format!("missing ':' after field: {key}")))?;
    let after_colon = after_key[colon + 1..].trim_start();
    let value_start = after_colon
        .find('"')
        .ok_or_else(|| KernelError::validation(format!("field is not a string: {key}")))?;
    let rest = &after_colon[value_start + 1..];
    let value_end = rest
        .find('"')
        .ok_or_else(|| KernelError::validation(format!("unterminated string field: {key}")))?;
    Ok(rest[..value_end].to_string())
}

fn extract_owner_name(input: &str) -> KernelResult<String> {
    let owner_start = input
        .find("\"owner\"")
        .ok_or_else(|| KernelError::validation("missing field: owner"))?;
    extract_string(&input[owner_start..], "name")
}

fn extract_capability_requirements(
    input: &str,
    key: &str,
) -> KernelResult<Vec<CapabilityRequirement>> {
    let array = extract_array_body(input, key)?;
    let mut requirements = Vec::new();
    let mut remaining = array.as_str();
    let capability_key = "\"capability_id\"";

    while let Some(index) = remaining.find(capability_key) {
        let slice = &remaining[index..];
        let after_capability_key = &slice[capability_key.len()..];
        let next_capability_index = after_capability_key.find(capability_key);
        let item_slice = match next_capability_index {
            Some(next_index) => &slice[..capability_key.len() + next_index],
            None => slice,
        };

        let mut requirement =
            CapabilityRequirement::new(extract_string(item_slice, "capability_id")?);
        if let Some(min_version) = extract_optional_string(item_slice, "min_version")? {
            requirement = requirement.with_min_version(min_version);
        }
        requirements.push(requirement);

        remaining = match next_capability_index {
            Some(next_index) => &after_capability_key[next_index..],
            None => "",
        };
    }

    Ok(requirements)
}

fn extract_optional_string(input: &str, key: &str) -> KernelResult<Option<String>> {
    let pattern = format!("\"{key}\"");
    if !input.contains(&pattern) {
        return Ok(None);
    }

    extract_string(input, key).map(Some)
}

fn extract_string_array(input: &str, key: &str) -> KernelResult<Vec<String>> {
    let array = extract_array_body(input, key)?;
    let mut values = Vec::new();
    let mut rest = array.as_str();

    while let Some(start) = rest.find('"') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('"') else {
            return Err(KernelError::validation(format!(
                "unterminated string array field: {key}"
            )));
        };
        values.push(after_start[..end].to_string());
        rest = &after_start[end + 1..];
    }

    Ok(values)
}

fn extract_array_body(input: &str, key: &str) -> KernelResult<String> {
    let pattern = format!("\"{key}\"");
    let key_start = input
        .find(&pattern)
        .ok_or_else(|| KernelError::validation(format!("missing array field: {key}")))?;
    let after_key = &input[key_start + pattern.len()..];
    let bracket_start = after_key
        .find('[')
        .ok_or_else(|| KernelError::validation(format!("field is not an array: {key}")))?;
    let mut depth = 0usize;
    let mut end = None;

    for (index, ch) in after_key[bracket_start..].char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(bracket_start + index);
                    break;
                }
            }
            _ => {}
        }
    }

    let end = end.ok_or_else(|| KernelError::validation(format!("unterminated array: {key}")))?;
    Ok(after_key[bracket_start + 1..end].to_string())
}

fn extract_object_body(input: &str, key: &str) -> KernelResult<String> {
    let pattern = format!("\"{key}\"");
    let key_start = input
        .find(&pattern)
        .ok_or_else(|| KernelError::validation(format!("missing object field: {key}")))?;
    let after_key = &input[key_start + pattern.len()..];
    let object_start = after_key
        .find('{')
        .ok_or_else(|| KernelError::validation(format!("field is not an object: {key}")))?;
    let mut depth = 0usize;
    let mut end = None;

    for (index, ch) in after_key[object_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(object_start + index);
                    break;
                }
            }
            _ => {}
        }
    }

    let end = end.ok_or_else(|| KernelError::validation(format!("unterminated object: {key}")))?;
    Ok(after_key[object_start..=end].to_string())
}
