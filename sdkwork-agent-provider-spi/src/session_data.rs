use std::collections::{BTreeMap, HashSet};

use sdkwork_agent_kernel::{AgentPart, AgentPartKind, KernelError, KernelResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::runtime::{
    SdkRuntimeOperation, SdkRuntimeRequest, SdkRuntimeResponse, SdkRuntimeRouter,
};

pub const SDK_SESSION_PAGE_SIZE: u32 = 200;
const MAX_SDK_SESSION_ITEMS: usize = 10_000;
const SDK_SESSION_LIFECYCLE_CAPABILITY: &str = "sdk.session.lifecycle";

/// Drains an official provider SDK inventory without interpreting its cursor.
///
/// Returned records retain only provider-owned identities. Establishing a
/// canonical SDKWork Session is a separate adoption step owned by the runtime.
pub fn list_all_provider_sessions_from_runtime(
    runtime: &SdkRuntimeRouter,
    provider_id: &str,
    working_directory: Option<&str>,
) -> KernelResult<Vec<SdkRuntimeSessionRecord>> {
    validate_provider_id(provider_id)?;
    let working_directory = normalized(working_directory.map(str::to_string));
    let mut cursor = None;
    let mut seen_cursors = HashSet::new();
    let mut seen_session_ids = HashSet::new();
    let mut sessions = Vec::new();
    loop {
        let response = runtime
            .invoke(&SdkRuntimeRequest {
                capability_id: SDK_SESSION_LIFECYCLE_CAPABILITY.to_string(),
                operation: SdkRuntimeOperation::SessionList {
                    working_directory: working_directory.clone(),
                    cursor: cursor.clone(),
                    limit: SDK_SESSION_PAGE_SIZE,
                    source_kinds: None,
                    section_id: None,
                    archived: None,
                    search_term: None,
                    sort_key: None,
                    sort_direction: None,
                    model_providers: None,
                },
                payload: None,
            })
            .map_err(|error| runtime_error(provider_id, "session_list", error))?;
        let page = SdkRuntimeSessionPage::from_payload(required_payload(
            provider_id,
            "session_list",
            response,
        )?)?;
        validate_page_size(provider_id, "session_list", page.items.len())?;
        let next_cursor = page.next_cursor.clone();
        for session in page.into_provider_sessions(provider_id)? {
            if !seen_session_ids.insert(session.provider_session_id.clone()) {
                return Err(KernelError::provider_error(
                    "provider_sdk_session_identity_repeated",
                    format!(
                        "{provider_id} official SDK repeated provider session {} across inventory pages",
                        session.provider_session_id
                    ),
                ));
            }
            sessions.push(session);
        }
        if sessions.len() > MAX_SDK_SESSION_ITEMS {
            return Err(KernelError::provider_error(
                "provider_sdk_session_inventory_too_large",
                format!(
                    "{provider_id} official SDK inventory exceeds {MAX_SDK_SESSION_ITEMS} sessions"
                ),
            ));
        }
        let Some(next_cursor) = next_cursor else {
            return Ok(sessions);
        };
        if !seen_cursors.insert(next_cursor.clone()) {
            return Err(KernelError::provider_error(
                "provider_sdk_cursor_cycle",
                format!("{provider_id} official SDK repeated a session list cursor"),
            ));
        }
        cursor = Some(next_cursor);
    }
}

/// Drains a provider transcript through the official SDK's pagination contract.
///
/// Returned records retain provider message and Session identities and must not
/// be persisted as canonical SDKWork messages without an explicit identity map.
pub fn load_all_provider_messages_from_runtime(
    runtime: &SdkRuntimeRouter,
    provider_id: &str,
    provider_session_id: &str,
    working_directory: Option<&str>,
) -> KernelResult<Vec<SdkRuntimeMessageRecord>> {
    validate_provider_id(provider_id)?;
    let provider_session_id = required_borrowed_string(
        provider_session_id,
        "provider_sdk_session_identity_invalid",
        "provider_session_id",
    )?
    .to_string();
    let working_directory = normalized(working_directory.map(str::to_string));
    let mut cursor = None;
    let mut seen_cursors = HashSet::new();
    let mut seen_message_ids = HashSet::new();
    let mut messages = Vec::new();
    loop {
        let response = runtime
            .invoke(&SdkRuntimeRequest {
                capability_id: SDK_SESSION_LIFECYCLE_CAPABILITY.to_string(),
                operation: SdkRuntimeOperation::SessionHistory {
                    provider_session_id: provider_session_id.clone(),
                    working_directory: working_directory.clone(),
                    cursor: cursor.clone(),
                    limit: SDK_SESSION_PAGE_SIZE,
                },
                payload: None,
            })
            .map_err(|error| runtime_error(provider_id, "session_history", error))?;
        let page = SdkRuntimeMessagePage::from_payload(required_payload(
            provider_id,
            "session_history",
            response,
        )?)?;
        validate_page_size(provider_id, "session_history", page.items.len())?;
        let next_cursor = page.next_cursor.clone();
        for message in page.into_provider_messages(&provider_session_id)? {
            if !seen_message_ids.insert(message.provider_message_id.clone()) {
                return Err(KernelError::provider_error(
                    "provider_sdk_message_identity_repeated",
                    format!(
                        "{provider_id} official SDK repeated provider message {} across transcript pages",
                        message.provider_message_id
                    ),
                ));
            }
            messages.push(message);
        }
        if messages.len() > MAX_SDK_SESSION_ITEMS {
            return Err(KernelError::provider_error(
                "provider_sdk_session_history_too_large",
                format!(
                    "{provider_id} official SDK transcript exceeds {MAX_SDK_SESSION_ITEMS} messages"
                ),
            ));
        }
        let Some(next_cursor) = next_cursor else {
            return Ok(messages);
        };
        if !seen_cursors.insert(next_cursor.clone()) {
            return Err(KernelError::provider_error(
                "provider_sdk_cursor_cycle",
                format!("{provider_id} official SDK repeated a session history cursor"),
            ));
        }
        cursor = Some(next_cursor);
    }
}

fn required_payload(
    provider_id: &str,
    operation: &str,
    response: SdkRuntimeResponse,
) -> KernelResult<Value> {
    if !response.success {
        let message = response
            .message
            .as_deref()
            .unwrap_or("official SDK runtime returned an unspecified failure");
        return Err(KernelError::provider_error(
            format!("{provider_id}_sdk_{operation}"),
            format!("{provider_id} official SDK {operation} failed: {message}"),
        ));
    }
    response.payload.ok_or_else(|| {
        KernelError::provider_error(
            "provider_sdk_payload_missing",
            format!("{provider_id} official SDK returned no payload for {operation}"),
        )
    })
}

fn runtime_error(
    provider_id: &str,
    operation: &str,
    error: crate::runtime::SdkRuntimeError,
) -> KernelError {
    KernelError::provider_error(
        format!("{provider_id}_sdk_{operation}"),
        format!("{}: {}", error.code, error.message),
    )
}

fn validate_provider_id(provider_id: &str) -> KernelResult<()> {
    if provider_id.trim().is_empty() {
        return Err(KernelError::validation("provider_id must not be empty"));
    }
    Ok(())
}

fn validate_page_size(provider_id: &str, operation: &str, item_count: usize) -> KernelResult<()> {
    if item_count > SDK_SESSION_PAGE_SIZE as usize {
        return Err(KernelError::provider_error(
            "provider_sdk_page_too_large",
            format!(
                "{provider_id} official SDK returned {item_count} items for {operation}, exceeding page size {SDK_SESSION_PAGE_SIZE}"
            ),
        ));
    }
    Ok(())
}

/// Provider-neutral page returned by an official SDK session inventory adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SdkRuntimeSessionPage {
    #[serde(default)]
    pub items: Vec<SdkRuntimeSessionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_cursor: Option<String>,
}

impl SdkRuntimeSessionPage {
    pub fn from_payload(payload: Value) -> KernelResult<Self> {
        serde_json::from_value(payload).map_err(|error| {
            KernelError::provider_error(
                "provider_sdk_session_page_invalid",
                format!("official SDK returned an invalid session page: {error}"),
            )
        })
    }

    pub fn into_provider_sessions(
        self,
        provider_id: &str,
    ) -> KernelResult<Vec<SdkRuntimeSessionRecord>> {
        validate_provider_id(provider_id)?;
        self.items
            .into_iter()
            .map(|record| record.validated(provider_id))
            .collect()
    }
}

/// Stable session projection shared by provider SDK workers and Rust adapters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SdkRuntimeSessionRecord {
    pub provider_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_provider_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(default)]
    pub message_count: u32,
    #[serde(default)]
    pub tool_call_count: u32,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cents: Option<u64>,
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
    #[serde(default)]
    pub files_changed: u32,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

impl SdkRuntimeSessionRecord {
    /// Validates and normalizes one provider-owned Session discovery record.
    pub fn validated(mut self, provider_id: &str) -> KernelResult<Self> {
        validate_provider_id(provider_id)?;
        self.provider_session_id = required_string(
            self.provider_session_id,
            "provider_sdk_session_identity_invalid",
            "provider_session_id",
        )?;
        self.parent_provider_session_id = normalized(self.parent_provider_session_id);
        if self.parent_provider_session_id.as_deref() == Some(self.provider_session_id.as_str()) {
            return Err(KernelError::provider_error(
                "provider_sdk_session_lineage_invalid",
                format!(
                    "{provider_id} official SDK session {} cannot be its own parent",
                    self.provider_session_id
                ),
            ));
        }
        self.title = normalized(self.title);
        self.summary = normalized(self.summary);
        self.preview = normalized(self.preview);
        self.cwd = normalized(self.cwd);
        self.created_at = normalized_timestamp(self.created_at, "created_at")?;
        self.updated_at = normalized_timestamp(self.updated_at, "updated_at")?;
        self.archived_at = normalized_timestamp(self.archived_at, "archived_at")?;
        validate_session_timestamp_order(&self)?;
        self.model = normalized(self.model);
        self.model_provider = normalized(self.model_provider);
        self.metadata = normalized_metadata(self.metadata);
        Ok(self)
    }
}

/// Provider-neutral page returned by an official SDK transcript adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SdkRuntimeMessagePage {
    pub provider_session_id: String,
    #[serde(default)]
    pub items: Vec<SdkRuntimeMessageRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_cursor: Option<String>,
}

impl SdkRuntimeMessagePage {
    pub fn from_payload(payload: Value) -> KernelResult<Self> {
        serde_json::from_value(payload).map_err(|error| {
            KernelError::provider_error(
                "provider_sdk_message_page_invalid",
                format!("official SDK returned an invalid message page: {error}"),
            )
        })
    }

    pub fn into_provider_messages(
        self,
        expected_provider_session_id: &str,
    ) -> KernelResult<Vec<SdkRuntimeMessageRecord>> {
        let expected_provider_session_id = required_borrowed_string(
            expected_provider_session_id,
            "provider_sdk_session_identity_invalid",
            "expected provider_session_id",
        )?;
        let provider_session_id = required_borrowed_string(
            &self.provider_session_id,
            "provider_sdk_session_identity_invalid",
            "page provider_session_id",
        )?;
        if provider_session_id != expected_provider_session_id {
            return Err(KernelError::provider_error(
                "provider_sdk_session_affinity_mismatch",
                format!(
                    "official SDK returned transcript for {}, expected {}",
                    provider_session_id, expected_provider_session_id
                ),
            ));
        }
        self.items
            .into_iter()
            .map(|record| record.validated(expected_provider_session_id))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SdkRuntimeMessageRecord {
    pub provider_message_id: String,
    pub provider_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_provider_message_id: Option<String>,
    pub role: String,
    #[serde(default)]
    pub parts: Vec<SdkRuntimeMessagePart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

impl SdkRuntimeMessageRecord {
    /// Validates and normalizes one provider-owned message discovery record.
    pub fn validated(mut self, expected_provider_session_id: &str) -> KernelResult<Self> {
        let expected_provider_session_id = required_borrowed_string(
            expected_provider_session_id,
            "provider_sdk_session_identity_invalid",
            "expected provider_session_id",
        )?;
        self.provider_message_id = required_string(
            self.provider_message_id,
            "provider_sdk_message_identity_invalid",
            "provider_message_id",
        )?;
        self.provider_session_id = required_string(
            self.provider_session_id,
            "provider_sdk_session_identity_invalid",
            "message provider_session_id",
        )?;
        if self.provider_session_id != expected_provider_session_id {
            return Err(KernelError::provider_error(
                "provider_sdk_message_affinity_mismatch",
                format!(
                    "official SDK message {} belongs to {}, expected {}",
                    self.provider_message_id,
                    self.provider_session_id,
                    expected_provider_session_id
                ),
            ));
        }
        self.parent_provider_message_id = normalized(self.parent_provider_message_id);
        if self.parent_provider_message_id.as_deref() == Some(self.provider_message_id.as_str()) {
            return Err(KernelError::provider_error(
                "provider_sdk_message_lineage_invalid",
                format!(
                    "official SDK message {} cannot be its own parent",
                    self.provider_message_id
                ),
            ));
        }
        self.role = match self.role.trim().to_ascii_lowercase().as_str() {
            "user" => "user",
            "agent" | "assistant" => "agent",
            "model" => "model",
            "system" => "system",
            "tool" => "tool",
            "policy" => "policy",
            "adapter" => "adapter",
            other => {
                return Err(KernelError::provider_error(
                    "provider_sdk_message_role_invalid",
                    format!("official SDK returned unsupported message role: {other}"),
                ))
            }
        }
        .to_string();
        if self.parts.is_empty() {
            return Err(KernelError::provider_error(
                "provider_sdk_message_parts_missing",
                format!(
                    "official SDK message {} did not contain any displayable parts",
                    self.provider_message_id
                ),
            ));
        }
        let mut part_ids = HashSet::with_capacity(self.parts.len());
        self.parts = self
            .parts
            .into_iter()
            .map(SdkRuntimeMessagePart::validated)
            .collect::<KernelResult<Vec<_>>>()?;
        for part in &self.parts {
            if !part_ids.insert(part.part_id.clone()) {
                return Err(KernelError::provider_error(
                    "provider_sdk_part_identity_repeated",
                    format!(
                        "official SDK message {} repeated part {}",
                        self.provider_message_id, part.part_id
                    ),
                ));
            }
        }
        self.created_at = normalized_timestamp(self.created_at, "message.created_at")?;
        self.metadata = normalized_metadata(self.metadata);
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SdkRuntimeMessagePart {
    pub part_id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_decision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

impl SdkRuntimeMessagePart {
    /// Validates and normalizes one provider-owned message part.
    pub fn validated(mut self) -> KernelResult<Self> {
        self.part_id = required_string(
            self.part_id,
            "provider_sdk_part_identity_invalid",
            "part_id",
        )?;
        self.kind = self.kind.trim().to_ascii_lowercase();
        self.text = non_empty_content(self.text);
        self.content_ref = normalized(self.content_ref);
        self.artifact_id = normalized(self.artifact_id);
        self.tool_call_id = normalized(self.tool_call_id);
        self.policy_decision_id = normalized(self.policy_decision_id);
        self.error_code = normalized(self.error_code);
        self.mime_type = normalized(self.mime_type);
        self.name = normalized(self.name);
        self.metadata = normalized_metadata(self.metadata);
        self.clone().into_agent_part()?;
        Ok(self)
    }

    /// Builds a legal kernel part without seeding unrelated text fields.
    pub fn into_agent_part(self) -> KernelResult<AgentPart> {
        let kind = AgentPartKind::parse(&self.kind).map_err(|error| {
            KernelError::provider_error("provider_sdk_part_kind_invalid", error.to_string())
        })?;
        let part_id = required_string(
            self.part_id,
            "provider_sdk_part_identity_invalid",
            "part_id",
        )?;
        let text = non_empty_content(self.text);
        let content_ref = normalized(self.content_ref);
        let artifact_id = normalized(self.artifact_id);
        let tool_call_id = normalized(self.tool_call_id);
        let policy_decision_id = normalized(self.policy_decision_id);
        let error_code = normalized(self.error_code);
        let mime_type = normalized(self.mime_type);
        let json = self.json.map(json_string);
        validate_part_field_shape(
            kind,
            &[
                ("text", text.is_some()),
                ("json", json.is_some()),
                ("content_ref", content_ref.is_some()),
                ("artifact_id", artifact_id.is_some()),
                ("tool_call_id", tool_call_id.is_some()),
                ("policy_decision_id", policy_decision_id.is_some()),
                ("error_code", error_code.is_some()),
                ("mime_type", mime_type.is_some()),
            ],
        )?;
        let mut part = match kind {
            AgentPartKind::Text => AgentPart::text(
                part_id,
                required_option(text, "provider_sdk_part_text_missing", "text")?,
            ),
            AgentPartKind::Json => {
                let mut part = AgentPart::json(
                    part_id,
                    required_option(json, "provider_sdk_part_json_missing", "json")?,
                );
                part.tool_call_id = tool_call_id;
                part
            }
            AgentPartKind::BinaryRef => AgentPart::binary_ref(
                part_id,
                required_option(
                    content_ref,
                    "provider_sdk_part_content_ref_missing",
                    "content_ref",
                )?,
                required_option(
                    mime_type,
                    "provider_sdk_part_mime_type_missing",
                    "mime_type",
                )?,
            ),
            AgentPartKind::FileRef => AgentPart::file_ref(
                part_id,
                required_option(
                    content_ref,
                    "provider_sdk_part_content_ref_missing",
                    "content_ref",
                )?,
                required_option(
                    mime_type,
                    "provider_sdk_part_mime_type_missing",
                    "mime_type",
                )?,
            ),
            AgentPartKind::ArtifactRef => AgentPart::artifact_ref(
                part_id,
                required_option(
                    artifact_id,
                    "provider_sdk_part_artifact_id_missing",
                    "artifact_id",
                )?,
            ),
            AgentPartKind::ImageRef => AgentPart::image_ref(
                part_id,
                required_option(
                    content_ref,
                    "provider_sdk_part_content_ref_missing",
                    "content_ref",
                )?,
                required_option(
                    mime_type,
                    "provider_sdk_part_mime_type_missing",
                    "mime_type",
                )?,
            ),
            AgentPartKind::AudioRef => AgentPart::audio_ref(
                part_id,
                required_option(
                    content_ref,
                    "provider_sdk_part_content_ref_missing",
                    "content_ref",
                )?,
                required_option(
                    mime_type,
                    "provider_sdk_part_mime_type_missing",
                    "mime_type",
                )?,
            ),
            AgentPartKind::VideoRef => AgentPart::video_ref(
                part_id,
                required_option(
                    content_ref,
                    "provider_sdk_part_content_ref_missing",
                    "content_ref",
                )?,
                required_option(
                    mime_type,
                    "provider_sdk_part_mime_type_missing",
                    "mime_type",
                )?,
            ),
            AgentPartKind::ToolCallRef => {
                let mut part = AgentPart::tool_call_ref(
                    part_id,
                    required_option(
                        tool_call_id,
                        "provider_sdk_part_tool_call_id_missing",
                        "tool_call_id",
                    )?,
                );
                part.json = json;
                part.text = text;
                part
            }
            AgentPartKind::PolicyDecisionRef => {
                let mut part = AgentPart::policy_decision_ref(
                    part_id,
                    required_option(
                        policy_decision_id,
                        "provider_sdk_part_policy_decision_id_missing",
                        "policy_decision_id",
                    )?,
                );
                part.json = json;
                part.text = text;
                part
            }
            AgentPartKind::Error => {
                let mut part = AgentPart::error(
                    part_id,
                    required_option(
                        error_code,
                        "provider_sdk_part_error_code_missing",
                        "error_code",
                    )?,
                    required_option(text, "provider_sdk_part_text_missing", "text")?,
                );
                part.json = json;
                part
            }
        };
        part.name = normalized(self.name);
        part.metadata.extend(metadata_pairs(self.metadata));
        Ok(part)
    }
}

fn required_string(value: String, code: &str, field: &str) -> KernelResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(KernelError::provider_error(
            code,
            format!("official SDK returned an empty {field}"),
        ));
    }
    Ok(value.to_string())
}

fn required_borrowed_string<'a>(value: &'a str, code: &str, field: &str) -> KernelResult<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        return Err(KernelError::provider_error(
            code,
            format!("official SDK returned an empty {field}"),
        ));
    }
    Ok(value)
}

fn required_option<T>(value: Option<T>, code: &str, field: &str) -> KernelResult<T> {
    value.ok_or_else(|| {
        KernelError::provider_error(code, format!("official SDK part requires {field}"))
    })
}

fn normalized_timestamp(value: Option<String>, field: &str) -> KernelResult<Option<String>> {
    let Some(value) = normalized(value) else {
        return Ok(None);
    };
    let parsed = sdkwork_utils_rust::parse_datetime(&value, None).ok_or_else(|| {
        KernelError::provider_error(
            "provider_sdk_timestamp_invalid",
            format!("official SDK returned an invalid {field} timestamp"),
        )
    })?;
    Ok(Some(sdkwork_utils_rust::format_datetime(
        parsed,
        Some("%Y-%m-%dT%H:%M:%S%.9fZ"),
    )))
}

fn validate_session_timestamp_order(session: &SdkRuntimeSessionRecord) -> KernelResult<()> {
    if let (Some(created_at), Some(updated_at)) =
        (session.created_at.as_deref(), session.updated_at.as_deref())
    {
        if created_at > updated_at {
            return Err(KernelError::provider_error(
                "provider_sdk_timestamp_order_invalid",
                format!(
                    "official SDK session {} updated_at precedes created_at",
                    session.provider_session_id
                ),
            ));
        }
    }
    if let Some(archived_at) = session.archived_at.as_deref() {
        let lower_bound = session
            .updated_at
            .as_deref()
            .or(session.created_at.as_deref());
        if lower_bound.is_some_and(|lower_bound| archived_at < lower_bound) {
            return Err(KernelError::provider_error(
                "provider_sdk_timestamp_order_invalid",
                format!(
                    "official SDK session {} archived_at precedes its active timestamps",
                    session.provider_session_id
                ),
            ));
        }
    }
    Ok(())
}

fn validate_part_field_shape(kind: AgentPartKind, fields: &[(&str, bool)]) -> KernelResult<()> {
    let allowed = |field: &str| match kind {
        AgentPartKind::Text => field == "text",
        AgentPartKind::Json => matches!(field, "json" | "tool_call_id"),
        AgentPartKind::BinaryRef
        | AgentPartKind::FileRef
        | AgentPartKind::ImageRef
        | AgentPartKind::AudioRef
        | AgentPartKind::VideoRef => matches!(field, "content_ref" | "mime_type"),
        AgentPartKind::ArtifactRef => field == "artifact_id",
        AgentPartKind::ToolCallRef => matches!(field, "tool_call_id" | "json" | "text"),
        AgentPartKind::PolicyDecisionRef => {
            matches!(field, "policy_decision_id" | "json" | "text")
        }
        AgentPartKind::Error => matches!(field, "error_code" | "text" | "json"),
    };
    let unexpected = fields
        .iter()
        .filter_map(|(field, present)| (*present && !allowed(field)).then_some(*field))
        .collect::<Vec<_>>();
    if !unexpected.is_empty() {
        return Err(KernelError::provider_error(
            "provider_sdk_part_shape_invalid",
            format!(
                "official SDK {} part contains incompatible fields: {}",
                kind.as_str(),
                unexpected.join(", ")
            ),
        ));
    }
    Ok(())
}

fn normalized_metadata(metadata: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    metadata
        .into_iter()
        .filter_map(|(key, value)| {
            let key = key.trim();
            (!key.is_empty() && !value.is_null()).then(|| (key.to_string(), value))
        })
        .collect()
}

fn json_string(value: Value) -> String {
    match value {
        Value::String(value) => value,
        value => value.to_string(),
    }
}

fn normalized(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn non_empty_content(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn metadata_pairs(metadata: BTreeMap<String, Value>) -> Vec<(String, String)> {
    metadata
        .into_iter()
        .filter_map(|(key, value)| {
            let key = key.trim();
            if key.is_empty() || value.is_null() {
                return None;
            }
            let value = json_string(value);
            Some((key.to_string(), value))
        })
        .collect()
}

#[cfg(test)] // WORKSPACE-PATH:allow-fixture-block: this module is the file's #[cfg(test)] unit-test fixture data
mod tests {
    use super::*;
    use crate::{
        NegotiatedCapability, SdkBackendKind, SdkBackendRuntime, SdkCapabilityNegotiation,
        SdkDriverHealth, SdkRuntimeError, SdkRuntimeOperationKind, SdkRuntimeResponse,
        SDK_CAPABILITY_SESSION_LIFECYCLE,
    };
    use std::sync::{Arc, Mutex};

    type RuntimeHandler =
        dyn Fn(&SdkRuntimeRequest) -> Result<SdkRuntimeResponse, SdkRuntimeError> + Send + Sync;

    struct TestRuntime {
        handler: Box<RuntimeHandler>,
    }

    impl SdkBackendRuntime for TestRuntime {
        fn backend_kind(&self) -> SdkBackendKind {
            SdkBackendKind::TypeScriptNode
        }

        fn health(&self) -> SdkDriverHealth {
            SdkDriverHealth::healthy()
        }

        fn invoke(
            &self,
            request: &SdkRuntimeRequest,
        ) -> Result<SdkRuntimeResponse, SdkRuntimeError> {
            (self.handler)(request)
        }
    }

    fn runtime(
        handler: impl Fn(&SdkRuntimeRequest) -> Result<SdkRuntimeResponse, SdkRuntimeError>
            + Send
            + Sync
            + 'static,
    ) -> SdkRuntimeRouter {
        let negotiation = SdkCapabilityNegotiation {
            agent_id: "agent.provider-test".to_string(),
            binding_id: "binding.provider-test".to_string(),
            binding_version: "0.1.0".to_string(),
            selected: vec![NegotiatedCapability {
                capability_id: SDK_CAPABILITY_SESSION_LIFECYCLE.to_string(),
                backend_kind: SdkBackendKind::TypeScriptNode,
                driver_id: "driver.provider-test.session-discovery".to_string(),
                runtime_operations: vec![
                    SdkRuntimeOperationKind::SessionList,
                    SdkRuntimeOperationKind::SessionHistory,
                ],
                execution_scope: crate::binding::CapabilityExecutionScope::TransportRuntime,
            }],
            missing_required: Vec::new(),
            degraded_optional: Vec::new(),
        };
        SdkRuntimeRouter::new(negotiation).with_typescript_runtime(Arc::new(TestRuntime {
            handler: Box::new(handler),
        }))
    }

    fn success(request: &SdkRuntimeRequest, payload: Value) -> SdkRuntimeResponse {
        SdkRuntimeResponse::success(
            SdkBackendKind::TypeScriptNode,
            &request.capability_id,
            payload,
        )
    }

    #[test]
    fn maps_sdk_pages_without_claiming_canonical_session_or_message_identity() {
        let session_page = SdkRuntimeSessionPage::from_payload(serde_json::json!({
            "items": [{
                "provider_session_id": " provider-session-1 ",
                "title": "SDK session",
                "cwd": "E:/work/birdcoder",
                "created_at": "2026-08-01T08:00:00+08:00",
                "updated_at": "2026-08-01T01:00:00Z",
                "input_tokens": 4,
                "output_tokens": 5
            }],
            "next_cursor": "opaque-next"
        }))
        .expect("session page");
        assert_eq!(session_page.next_cursor.as_deref(), Some("opaque-next"));
        let sessions = session_page
            .into_provider_sessions("provider-under-test")
            .expect("sessions");
        assert_eq!(sessions[0].provider_session_id, "provider-session-1");
        assert_eq!(
            sessions[0].created_at.as_deref(),
            Some("2026-08-01T00:00:00.000000000Z")
        );
        let session_wire = serde_json::to_value(&sessions[0]).expect("session wire");
        assert!(session_wire.get("session_id").is_none());

        let message_page = SdkRuntimeMessagePage::from_payload(serde_json::json!({
            "provider_session_id": "provider-session-1",
            "items": [{
                "provider_message_id": "provider-message-1",
                "provider_session_id": "provider-session-1",
                "role": "assistant",
                "parts": [{
                    "part_id": "provider-part-1",
                    "kind": "text",
                    "text": "done"
                }]
            }]
        }))
        .expect("message page");
        let messages = message_page
            .into_provider_messages("provider-session-1")
            .expect("messages");
        assert_eq!(messages[0].provider_message_id, "provider-message-1");
        assert_eq!(messages[0].role, "agent");
        let message_wire = serde_json::to_value(&messages[0]).expect("message wire");
        assert!(message_wire.get("message_id").is_none());
        let part = messages[0].parts[0]
            .clone()
            .into_agent_part()
            .expect("kernel part");
        assert_eq!(part.kind, AgentPartKind::Text);
        assert_eq!(part.text.as_deref(), Some("done"));
    }

    #[test]
    fn drains_official_sdk_session_and_history_pages_with_opaque_cursors() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let runtime = runtime(move |request| {
            captured.lock().expect("requests").push(request.clone());
            let payload = match &request.operation {
                SdkRuntimeOperation::SessionList {
                    working_directory,
                    cursor,
                    limit,
                    ..
                } => {
                    assert_eq!(working_directory.as_deref(), Some("E:/workspace"));
                    assert_eq!(*limit, SDK_SESSION_PAGE_SIZE);
                    match cursor.as_deref() {
                        None => serde_json::json!({
                            "items": [{"provider_session_id": "session-1"}],
                            "next_cursor": " list-next "
                        }),
                        Some(" list-next ") => serde_json::json!({
                            "items": [{"provider_session_id": "session-2"}]
                        }),
                        other => panic!("unexpected list cursor: {other:?}"),
                    }
                }
                SdkRuntimeOperation::SessionHistory {
                    provider_session_id,
                    working_directory,
                    cursor,
                    limit,
                } => {
                    assert_eq!(provider_session_id, "session-1");
                    assert_eq!(working_directory.as_deref(), Some("E:/workspace"));
                    assert_eq!(*limit, SDK_SESSION_PAGE_SIZE);
                    let (message_id, next_cursor) = match cursor.as_deref() {
                        None => ("message-1", Some(" history-next ")),
                        Some(" history-next ") => ("message-2", None),
                        other => panic!("unexpected history cursor: {other:?}"),
                    };
                    serde_json::json!({
                        "provider_session_id": "session-1",
                        "items": [{
                            "provider_message_id": message_id,
                            "provider_session_id": "session-1",
                            "role": "assistant",
                            "parts": [{
                                "part_id": format!("{message_id}:text"),
                                "kind": "text",
                                "text": message_id
                            }]
                        }],
                        "next_cursor": next_cursor
                    })
                }
                other => panic!("unexpected operation: {other:?}"),
            };
            Ok(success(request, payload))
        });

        let sessions = list_all_provider_sessions_from_runtime(
            &runtime,
            "provider-test",
            Some("E:/workspace"),
        )
        .expect("session inventory");
        let messages = load_all_provider_messages_from_runtime(
            &runtime,
            "provider-test",
            "session-1",
            Some("E:/workspace"),
        )
        .expect("session history");

        assert_eq!(
            sessions
                .iter()
                .map(|session| session.provider_session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["session-1", "session-2"]
        );
        assert_eq!(
            messages
                .iter()
                .map(|message| message.provider_message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["message-1", "message-2"]
        );
        assert_eq!(requests.lock().expect("requests").len(), 4);
    }

    #[test]
    fn preserves_runtime_failure_details_before_payload_validation() {
        let runtime = runtime(|_| {
            Ok(SdkRuntimeResponse::failure(
                SdkBackendKind::TypeScriptNode,
                "upstream session.list rejected the cursor",
            ))
        });

        let error = list_all_provider_sessions_from_runtime(&runtime, "opencode", None)
            .expect_err("runtime failure");
        let message = error.to_string();
        assert!(message.contains("session_list failed"));
        assert!(message.contains("upstream session.list rejected the cursor"));
        assert!(!message.contains("returned no payload"));
    }

    #[test]
    fn rejects_cursor_cycles_for_inventory_and_history() {
        let inventory = runtime(|request| {
            let suffix = match &request.operation {
                SdkRuntimeOperation::SessionList { cursor, .. } => {
                    cursor.as_deref().unwrap_or("first")
                }
                other => panic!("unexpected operation: {other:?}"),
            };
            Ok(success(
                request,
                serde_json::json!({
                    "items": [{"provider_session_id": format!("session-{suffix}")}],
                    "next_cursor": "repeated"
                }),
            ))
        });
        let inventory_error =
            list_all_provider_sessions_from_runtime(&inventory, "provider-test", None)
                .expect_err("inventory cursor cycle");
        assert!(inventory_error
            .to_string()
            .contains("repeated a session list cursor"));

        let history = runtime(|request| {
            let suffix = match &request.operation {
                SdkRuntimeOperation::SessionHistory { cursor, .. } => {
                    cursor.as_deref().unwrap_or("first")
                }
                other => panic!("unexpected operation: {other:?}"),
            };
            Ok(success(
                request,
                serde_json::json!({
                    "provider_session_id": "session-1",
                    "items": [{
                        "provider_message_id": format!("message-{suffix}"),
                        "provider_session_id": "session-1",
                        "role": "user",
                        "parts": [{
                            "part_id": format!("part-{suffix}"),
                            "kind": "text",
                            "text": "hello"
                        }]
                    }],
                    "next_cursor": "repeated"
                }),
            ))
        });
        let history_error =
            load_all_provider_messages_from_runtime(&history, "provider-test", "session-1", None)
                .expect_err("history cursor cycle");
        assert!(history_error
            .to_string()
            .contains("repeated a session history cursor"));
    }

    #[test]
    fn rejects_page_and_message_session_affinity_mismatches() {
        let page_error = SdkRuntimeMessagePage::from_payload(serde_json::json!({
            "provider_session_id": "session-other",
            "items": []
        }))
        .expect("page")
        .into_provider_messages("session-expected")
        .expect_err("page affinity mismatch");
        assert!(page_error.to_string().contains("session-other"));

        let message_error = SdkRuntimeMessagePage::from_payload(serde_json::json!({
            "provider_session_id": "session-expected",
            "items": [{
                "provider_message_id": "message-1",
                "provider_session_id": "session-other",
                "role": "user",
                "parts": [{"part_id": "part-1", "kind": "text", "text": "hello"}]
            }]
        }))
        .expect("page")
        .into_provider_messages("session-expected")
        .expect_err("message affinity mismatch");
        assert!(message_error.to_string().contains("message-1"));
    }

    #[test]
    fn rejects_repeated_provider_identities_across_pages() {
        let runtime = runtime(|request| {
            let next_cursor = match &request.operation {
                SdkRuntimeOperation::SessionList { cursor: None, .. } => Some("next"),
                SdkRuntimeOperation::SessionList { .. } => None,
                other => panic!("unexpected operation: {other:?}"),
            };
            Ok(success(
                request,
                serde_json::json!({
                    "items": [{"provider_session_id": "session-repeated"}],
                    "next_cursor": next_cursor
                }),
            ))
        });
        let error = list_all_provider_sessions_from_runtime(&runtime, "provider-test", None)
            .expect_err("repeated provider session");
        assert!(error.to_string().contains("session-repeated"));
    }

    #[test]
    fn rejects_pages_larger_than_the_requested_bound() {
        let inventory = runtime(|request| {
            let items = (0..=SDK_SESSION_PAGE_SIZE)
                .map(|index| serde_json::json!({"provider_session_id": format!("session-{index}")}))
                .collect::<Vec<_>>();
            Ok(success(request, serde_json::json!({"items": items})))
        });
        let inventory_error =
            list_all_provider_sessions_from_runtime(&inventory, "provider-test", None)
                .expect_err("oversized inventory page");
        assert!(inventory_error
            .to_string()
            .contains("exceeding page size 200"));

        let history = runtime(|request| {
            let items = (0..=SDK_SESSION_PAGE_SIZE)
                .map(|index| {
                    serde_json::json!({
                        "provider_message_id": format!("message-{index}"),
                        "provider_session_id": "session-1",
                        "role": "user",
                        "parts": [{
                            "part_id": format!("part-{index}"),
                            "kind": "text",
                            "text": "hello"
                        }]
                    })
                })
                .collect::<Vec<_>>();
            Ok(success(
                request,
                serde_json::json!({
                    "provider_session_id": "session-1",
                    "items": items
                }),
            ))
        });
        let history_error =
            load_all_provider_messages_from_runtime(&history, "provider-test", "session-1", None)
                .expect_err("oversized history page");
        assert!(history_error
            .to_string()
            .contains("exceeding page size 200"));
    }

    #[test]
    fn rejects_official_sdk_inventory_above_the_bounded_capacity() {
        let runtime = runtime(|request| {
            let page_index = match &request.operation {
                SdkRuntimeOperation::SessionList { cursor, .. } => cursor
                    .as_deref()
                    .unwrap_or("0")
                    .parse::<usize>()
                    .expect("numeric test cursor"),
                other => panic!("unexpected operation: {other:?}"),
            };
            let item_count = if page_index < 50 { 200 } else { 1 };
            let items = (0..item_count)
                .map(|item_index| {
                    serde_json::json!({
                        "provider_session_id": format!("session-{page_index}-{item_index}")
                    })
                })
                .collect::<Vec<_>>();
            let next_cursor = (page_index < 50).then(|| (page_index + 1).to_string());
            Ok(success(
                request,
                serde_json::json!({"items": items, "next_cursor": next_cursor}),
            ))
        });

        let error = list_all_provider_sessions_from_runtime(&runtime, "provider-test", None)
            .expect_err("bounded inventory");
        assert!(error.to_string().contains("exceeds 10000 sessions"));
    }

    #[test]
    fn rejects_official_sdk_history_above_the_bounded_capacity() {
        let runtime = runtime(|request| {
            let page_index = match &request.operation {
                SdkRuntimeOperation::SessionHistory { cursor, .. } => cursor
                    .as_deref()
                    .unwrap_or("0")
                    .parse::<usize>()
                    .expect("numeric test cursor"),
                other => panic!("unexpected operation: {other:?}"),
            };
            let item_count = if page_index < 50 { 200 } else { 1 };
            let items = (0..item_count)
                .map(|item_index| {
                    let message_id = format!("message-{page_index}-{item_index}");
                    serde_json::json!({
                        "provider_message_id": message_id,
                        "provider_session_id": "session-1",
                        "role": "agent",
                        "parts": [{
                            "part_id": format!("part-{page_index}-{item_index}"),
                            "kind": "text",
                            "text": "hello"
                        }]
                    })
                })
                .collect::<Vec<_>>();
            let next_cursor = (page_index < 50).then(|| (page_index + 1).to_string());
            Ok(success(
                request,
                serde_json::json!({
                    "provider_session_id": "session-1",
                    "items": items,
                    "next_cursor": next_cursor
                }),
            ))
        });

        let error =
            load_all_provider_messages_from_runtime(&runtime, "provider-test", "session-1", None)
                .expect_err("bounded history");
        assert!(error.to_string().contains("exceeds 10000 messages"));
    }

    #[test]
    fn constructs_parts_by_kind_and_rejects_incomplete_shapes() {
        let json_part = SdkRuntimeMessagePart {
            part_id: "part-json".to_string(),
            kind: "json".to_string(),
            text: None,
            json: Some(serde_json::json!({"ok": true})),
            content_ref: None,
            artifact_id: None,
            tool_call_id: None,
            policy_decision_id: None,
            error_code: None,
            mime_type: None,
            name: None,
            metadata: BTreeMap::new(),
        }
        .into_agent_part()
        .expect("json part");
        assert_eq!(json_part.kind, AgentPartKind::Json);
        assert_eq!(json_part.text, None);
        assert_eq!(json_part.json.as_deref(), Some("{\"ok\":true}"));

        let tool_part = SdkRuntimeMessagePart {
            part_id: "part-tool".to_string(),
            kind: "tool_call_ref".to_string(),
            text: None,
            json: Some(serde_json::json!({"path": "README.md"})),
            content_ref: None,
            artifact_id: None,
            tool_call_id: Some("tool-call-1".to_string()),
            policy_decision_id: None,
            error_code: None,
            mime_type: None,
            name: Some("read".to_string()),
            metadata: BTreeMap::new(),
        }
        .into_agent_part()
        .expect("tool part");
        assert_eq!(tool_part.kind, AgentPartKind::ToolCallRef);
        assert_eq!(tool_part.text, None);
        assert_eq!(tool_part.tool_call_id.as_deref(), Some("tool-call-1"));

        let incomplete_file = SdkRuntimeMessagePart {
            part_id: "part-file".to_string(),
            kind: "file_ref".to_string(),
            text: None,
            json: None,
            content_ref: Some("file:///tmp/report.txt".to_string()),
            artifact_id: None,
            tool_call_id: None,
            policy_decision_id: None,
            error_code: None,
            mime_type: None,
            name: None,
            metadata: BTreeMap::new(),
        }
        .into_agent_part()
        .expect_err("file MIME is required");
        assert!(incomplete_file.to_string().contains("mime_type"));

        let incompatible_text = SdkRuntimeMessagePart {
            part_id: "part-text".to_string(),
            kind: "text".to_string(),
            text: Some("visible".to_string()),
            json: Some(serde_json::json!({"hidden": true})),
            content_ref: None,
            artifact_id: None,
            tool_call_id: None,
            policy_decision_id: None,
            error_code: None,
            mime_type: None,
            name: None,
            metadata: BTreeMap::new(),
        }
        .into_agent_part()
        .expect_err("text part must not silently discard JSON");
        assert!(incompatible_text
            .to_string()
            .contains("incompatible fields: json"));

        let whitespace_sensitive = SdkRuntimeMessagePart {
            part_id: "part-whitespace".to_string(),
            kind: "text".to_string(),
            text: Some("  indented\n".to_string()),
            json: None,
            content_ref: None,
            artifact_id: None,
            tool_call_id: None,
            policy_decision_id: None,
            error_code: None,
            mime_type: None,
            name: None,
            metadata: BTreeMap::new(),
        }
        .validated()
        .expect("whitespace-sensitive text");
        assert_eq!(whitespace_sensitive.text.as_deref(), Some("  indented\n"));
    }

    #[test]
    fn rejects_invalid_session_timestamp_order() {
        let error = SdkRuntimeSessionPage::from_payload(serde_json::json!({
            "items": [{
                "provider_session_id": "session-1",
                "created_at": "2026-08-02T00:00:00Z",
                "updated_at": "2026-08-01T00:00:00Z"
            }]
        }))
        .expect("page")
        .into_provider_sessions("provider-test")
        .expect_err("timestamp order");
        assert!(error.to_string().contains("updated_at precedes created_at"));
    }
}
