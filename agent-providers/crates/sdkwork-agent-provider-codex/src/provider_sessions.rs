use std::collections::BTreeMap;

use codex_app_server_protocol::{
    SessionSource as CodexSessionSource, Thread, ThreadActiveFlag, ThreadItem, ThreadItemEntry,
    ThreadStatus, ThreadTurnsListResponse, Turn, TurnError, TurnItemsView, TurnStatus, UserInput,
};
use sdkwork_agent_kernel::{
    AgentMessage, AgentMessageRole, AgentPart, AgentSession, KernelError, KernelEventRedaction,
    KernelResult, SessionActivityEvidenceKind, SessionActivityInteractionHint,
    SessionActivitySnapshot, SessionActivityState, SessionKind, SessionSource,
};
use sdkwork_agent_provider_core::{
    finalize_provider_session_snapshot, normalize_provider_session_path, now_iso,
    session_activity_from_provider_observation, MessageAdapter, ProviderSessionActivityAdapter,
    SessionAdapter, DEFAULT_PROVIDER_SESSION_ACTIVITY_TTL,
};
use sdkwork_agent_provider_spi::{
    SdkRuntimeMessagePart, SdkRuntimeMessageRecord, SdkRuntimeSessionRecord,
};
use serde::Serialize;
use serde_json::Value;

const CODEX_PROVIDER_ID: &str = "codex";
const RAW_ITEM_SCHEMA: &str = "codex.app-server.v2.ThreadItem";

#[derive(Debug, Clone, PartialEq)]
pub struct CodexSessionRecord {
    pub session: SdkRuntimeSessionRecord,
    pub provider_thread: Thread,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexSessionPage {
    pub data: Vec<CodexSessionRecord>,
    pub next_cursor: Option<String>,
    pub backwards_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexMessageRecord {
    pub message: SdkRuntimeMessageRecord,
    pub provider_item: ThreadItemEntry,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexMessagePage {
    pub data: Vec<CodexMessageRecord>,
    pub next_cursor: Option<String>,
    pub backwards_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexThreadActivityObservation {
    pub provider_session_id: String,
    pub status: ThreadStatus,
    pub observed_at: String,
}

impl CodexThreadActivityObservation {
    pub fn from_protocol(provider_session_id: String, status: ThreadStatus) -> Self {
        Self {
            provider_session_id,
            status,
            observed_at: now_iso(),
        }
    }
}

#[derive(Debug, Default)]
pub struct CodexAdapter;

impl CodexAdapter {
    pub fn new() -> Self {
        Self
    }

    fn convert_thread(thread: &Thread) -> KernelResult<AgentSession> {
        let record = provider_session_record(thread)?;
        let mut session = AgentSession::new(&record.provider_session_id);
        session.source = map_session_source(&thread.source);
        session.kind = if thread.parent_thread_id.is_some()
            || matches!(thread.source, CodexSessionSource::SubAgent(_))
        {
            SessionKind::Subagent
        } else if thread.ephemeral {
            SessionKind::Ephemeral
        } else {
            SessionKind::Main
        };
        session.parent_session_id = record.parent_provider_session_id.clone();
        session.forked_from_id = thread.forked_from_id.clone();
        session.title = record.title.clone();
        session.summary = record.summary.clone();
        session.preview = record.preview.clone();
        session.model = record.model.clone();
        session.model_provider = record.model_provider.clone();
        session.cwd = record.cwd.clone();
        if let Some(cwd) = record.cwd.clone() {
            session.workspace_roots.push(cwd);
        }
        session.created_at = record.created_at.clone();
        session.updated_at = record.updated_at.clone();
        session.archived_at = record.archived_at.clone();
        session.agent_nickname = thread.agent_nickname.clone();
        session.agent_role = thread.agent_role.clone();
        session.message_count = record.message_count;
        session.tool_call_count = record.tool_call_count;
        session.compression_count = thread
            .turns
            .iter()
            .flat_map(|turn| &turn.items)
            .filter(|item| matches!(item, ThreadItem::ContextCompaction { .. }))
            .count()
            .try_into()
            .unwrap_or(u32::MAX);
        session.change_summary.additions = record.additions;
        session.change_summary.deletions = record.deletions;
        session.change_summary.files_changed = record.files_changed;
        session.metadata.extend(
            record
                .metadata
                .into_iter()
                .map(|(key, value)| (key, provider_value_string(value))),
        );

        let observation =
            CodexThreadActivityObservation::from_protocol(thread.id.clone(), thread.status.clone());
        session.apply_activity(CodexAdapter::new().to_session_activity(&observation)?)?;
        finalize_provider_session_snapshot(CODEX_PROVIDER_ID, session)
    }
}

fn provider_session_record(thread: &Thread) -> KernelResult<SdkRuntimeSessionRecord> {
    let message_count = thread
        .turns
        .iter()
        .flat_map(|turn| &turn.items)
        .count()
        .try_into()
        .unwrap_or(u32::MAX);
    let tool_call_count = thread
        .turns
        .iter()
        .flat_map(|turn| &turn.items)
        .filter(|item| is_tool_item(item))
        .count()
        .try_into()
        .unwrap_or(u32::MAX);
    let (additions, deletions, files_changed) = thread_change_summary(thread);
    let mut metadata = BTreeMap::from([
        (
            "codex.thread_id".to_string(),
            Value::String(thread.id.clone()),
        ),
        (
            "codex.session_id".to_string(),
            Value::String(thread.session_id.clone()),
        ),
        ("codex.ephemeral".to_string(), Value::Bool(thread.ephemeral)),
        (
            "codex.history_mode".to_string(),
            Value::String(provider_scalar(&thread.history_mode)),
        ),
        (
            "codex.cli_version".to_string(),
            Value::String(thread.cli_version.clone()),
        ),
        (
            "codex.source".to_string(),
            Value::String(provider_scalar(&thread.source)),
        ),
        (
            "codex.status".to_string(),
            Value::String(provider_scalar(&thread.status)),
        ),
        (
            "codex.turn_count".to_string(),
            serde_json::json!(thread.turns.len()),
        ),
    ]);
    insert_metadata_string(
        &mut metadata,
        "codex.recency_at",
        thread.recency_at.and_then(epoch_seconds_to_rfc3339),
    );
    // Fork lineage and subagent identity are part of the persisted protocol
    // tree structure; they are preserved as metadata because the session
    // record has no dedicated fork/nickname/role fields.
    insert_metadata_string(
        &mut metadata,
        "codex.forked_from_id",
        thread.forked_from_id.clone(),
    );
    insert_metadata_string(
        &mut metadata,
        "codex.agent_nickname",
        thread.agent_nickname.clone(),
    );
    insert_metadata_string(&mut metadata, "codex.agent_role", thread.agent_role.clone());
    // The durable thread path on disk is provider metadata, not a canonical
    // identity; it is preserved for diagnostics and directory reconciliation.
    insert_metadata_string(
        &mut metadata,
        "codex.path",
        thread.path.as_ref().map(|path| path.display().to_string()),
    );
    if let Some(section) = &thread.section {
        insert_metadata_string(&mut metadata, "codex.section.id", Some(section.id.clone()));
        insert_metadata_string(
            &mut metadata,
            "codex.section.name",
            Some(section.name.clone()),
        );
    }
    insert_metadata_string(
        &mut metadata,
        "codex.section_entered_at",
        thread.section_entered_at.and_then(epoch_seconds_to_rfc3339),
    );
    if let Some(can_accept_direct_input) = thread.can_accept_direct_input {
        metadata.insert(
            "codex.can_accept_direct_input".to_string(),
            Value::Bool(can_accept_direct_input),
        );
    }
    if let Some(thread_source) = &thread.thread_source {
        metadata.insert(
            "codex.thread_source".to_string(),
            Value::String(provider_scalar(thread_source)),
        );
    }
    if let Some(git_info) = &thread.git_info {
        insert_metadata_string(&mut metadata, "codex.git.sha", git_info.sha.clone());
        insert_metadata_string(&mut metadata, "codex.git.branch", git_info.branch.clone());
        insert_metadata_string(
            &mut metadata,
            "codex.git.origin_url",
            git_info.origin_url.clone(),
        );
    }

    SdkRuntimeSessionRecord {
        provider_session_id: thread.id.clone(),
        parent_provider_session_id: thread.parent_thread_id.clone(),
        title: thread
            .name
            .clone()
            .or_else(|| non_empty(&thread.preview).map(str::to_string)),
        summary: None,
        preview: non_empty(&thread.preview).map(str::to_string),
        // Normalize to the SDKWork provider session path form (forward slashes,
        // lowercase drive letter) so project cwd selectors match regardless of
        // the native path separator the app-server stored.
        cwd: Some(normalize_provider_session_path(
            &thread.cwd.display().to_string(),
        )),
        created_at: epoch_seconds_to_rfc3339(thread.created_at),
        updated_at: epoch_seconds_to_rfc3339(thread.updated_at),
        archived_at: None,
        model: None,
        model_provider: non_empty(&thread.model_provider).map(str::to_string),
        message_count,
        tool_call_count,
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
        reasoning_tokens: 0,
        cost_cents: None,
        additions,
        deletions,
        files_changed,
        metadata,
    }
    .validated(CODEX_PROVIDER_ID)
}

impl SessionAdapter for CodexAdapter {
    type ExternalSession = Thread;

    fn to_agent_session(&self, external: &Self::ExternalSession) -> KernelResult<AgentSession> {
        Self::convert_thread(external)
    }
}

impl ProviderSessionActivityAdapter for CodexAdapter {
    type ExternalActivity = CodexThreadActivityObservation;

    fn to_session_activity(
        &self,
        external: &Self::ExternalActivity,
    ) -> KernelResult<SessionActivitySnapshot> {
        let (state, interaction_hint) = match &external.status {
            ThreadStatus::NotLoaded => {
                return Ok(SessionActivitySnapshot::unsupported_with_evidence(
                    &external.provider_session_id,
                    SessionActivityEvidenceKind::ProviderStatus,
                    external.observed_at.clone(),
                ));
            }
            ThreadStatus::Idle => (SessionActivityState::Idle, None),
            ThreadStatus::SystemError => (SessionActivityState::Failed, None),
            ThreadStatus::Active { active_flags } => {
                if active_flags.contains(&ThreadActiveFlag::WaitingOnApproval) {
                    (
                        SessionActivityState::Waiting,
                        Some(SessionActivityInteractionHint::ApprovalRequired),
                    )
                } else if active_flags.contains(&ThreadActiveFlag::WaitingOnUserInput) {
                    (
                        SessionActivityState::Waiting,
                        Some(SessionActivityInteractionHint::UserInputRequired),
                    )
                } else {
                    (SessionActivityState::Working, None)
                }
            }
        };

        session_activity_from_provider_observation(
            &external.provider_session_id,
            state,
            SessionActivityEvidenceKind::ProviderStatus,
            interaction_hint,
            &external.observed_at,
            DEFAULT_PROVIDER_SESSION_ACTIVITY_TTL,
        )
    }
}

#[derive(Debug, Default)]
pub struct CodexMessageAdapter;

impl CodexMessageAdapter {
    pub fn new() -> Self {
        Self
    }

    fn convert_item(entry: &ThreadItemEntry) -> KernelResult<AgentMessage> {
        let item = &entry.item;
        let item_id = item.id();
        let item_type = item_type(item);
        let parts = message_parts(item)?;

        let mut message = AgentMessage::new(item_id, item_role(item), parts)
            .with_metadata("codex.turn_id", &entry.turn_id)
            .with_metadata("codex.item_type", item_type)
            .with_metadata("codex.item_id", item_id);
        if item_is_untrusted(item) {
            message = message.mark_untrusted();
        }
        Ok(message)
    }
}

impl MessageAdapter for CodexMessageAdapter {
    type ExternalMessage = ThreadItemEntry;

    fn to_agent_message(&self, external: &Self::ExternalMessage) -> KernelResult<AgentMessage> {
        Self::convert_item(external)
    }
}

pub fn map_thread_page(
    response: codex_app_server_protocol::ThreadListResponse,
) -> KernelResult<CodexSessionPage> {
    let data = response
        .data
        .into_iter()
        .map(|provider_thread| {
            let session = provider_session_record(&provider_thread)?;
            Ok(CodexSessionRecord {
                session,
                provider_thread,
            })
        })
        .collect::<KernelResult<Vec<_>>>()?;
    Ok(CodexSessionPage {
        data,
        next_cursor: response.next_cursor,
        backwards_cursor: response.backwards_cursor,
    })
}

pub fn map_thread_record(thread: Thread) -> KernelResult<CodexSessionRecord> {
    let session = provider_session_record(&thread)?;
    Ok(CodexSessionRecord {
        session,
        provider_thread: thread,
    })
}

pub fn map_item_page(
    thread_id: &str,
    response: codex_app_server_protocol::ThreadItemsListResponse,
) -> KernelResult<CodexMessagePage> {
    let data = response
        .data
        .into_iter()
        .map(|provider_item| {
            let message = provider_message_record(thread_id, &provider_item, None)?;
            Ok(CodexMessageRecord {
                message,
                provider_item,
            })
        })
        .collect::<KernelResult<Vec<_>>>()?;
    Ok(CodexMessagePage {
        data,
        next_cursor: response.next_cursor,
        backwards_cursor: response.backwards_cursor,
    })
}

/// Turn-level metadata preserved on every message of that turn so the turn
/// status, error, and timeline survive the canonical message projection.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderTurnMeta {
    pub status: TurnStatus,
    pub error: Option<TurnError>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub items_view: TurnItemsView,
}

impl From<&Turn> for ProviderTurnMeta {
    fn from(turn: &Turn) -> Self {
        Self {
            status: turn.status.clone(),
            error: turn.error.clone(),
            started_at: turn.started_at,
            completed_at: turn.completed_at,
            duration_ms: turn.duration_ms,
            items_view: turn.items_view,
        }
    }
}

pub fn map_turn_page(
    thread_id: &str,
    response: ThreadTurnsListResponse,
) -> KernelResult<CodexMessagePage> {
    let data = response
        .data
        .into_iter()
        .flat_map(|turn| {
            let turn_meta = ProviderTurnMeta::from(&turn);
            let turn_id = turn.id;
            turn.items.into_iter().map(move |item| {
                (
                    ThreadItemEntry {
                        turn_id: turn_id.clone(),
                        item,
                    },
                    turn_meta.clone(),
                )
            })
        })
        .map(|(provider_item, turn_meta)| {
            let message = provider_message_record(thread_id, &provider_item, Some(&turn_meta))?;
            Ok(CodexMessageRecord {
                message,
                provider_item,
            })
        })
        .collect::<KernelResult<Vec<_>>>()?;
    Ok(CodexMessagePage {
        data,
        next_cursor: response.next_cursor,
        backwards_cursor: response.backwards_cursor,
    })
}

pub fn normalize_page_limit(limit: &mut Option<u32>) -> KernelResult<()> {
    let requested = limit.unwrap_or(sdkwork_utils_rust::DEFAULT_LIST_PAGE_SIZE as u32);
    let maximum = sdkwork_utils_rust::MAX_LIST_PAGE_SIZE as u32;
    if !(1..=maximum).contains(&requested) {
        return Err(KernelError::validation(format!(
            "Codex page limit must be between 1 and {maximum}"
        )));
    }
    *limit = Some(requested);
    Ok(())
}

fn map_session_source(source: &CodexSessionSource) -> SessionSource {
    match source {
        CodexSessionSource::Cli | CodexSessionSource::Exec => SessionSource::Cli,
        CodexSessionSource::VsCode => SessionSource::Ide,
        CodexSessionSource::AppServer => SessionSource::Api,
        CodexSessionSource::SubAgent(_) => SessionSource::Cli,
        CodexSessionSource::Custom(_) | CodexSessionSource::Unknown => SessionSource::Unknown,
    }
}

fn thread_change_summary(thread: &Thread) -> (u32, u32, u32) {
    let mut total_additions = 0_u32;
    let mut total_deletions = 0_u32;
    let mut files_changed = 0_u32;
    for item in thread.turns.iter().flat_map(|turn| &turn.items) {
        if let ThreadItem::FileChange { changes, .. } = item {
            for change in changes {
                let (additions, deletions) =
                    change
                        .diff
                        .lines()
                        .fold((0_u32, 0_u32), |(additions, deletions), line| {
                            if line.starts_with('+') && !line.starts_with("+++") {
                                (additions.saturating_add(1), deletions)
                            } else if line.starts_with('-') && !line.starts_with("---") {
                                (additions, deletions.saturating_add(1))
                            } else {
                                (additions, deletions)
                            }
                        });
                total_additions = total_additions.saturating_add(additions);
                total_deletions = total_deletions.saturating_add(deletions);
                files_changed = files_changed.saturating_add(1);
            }
        }
    }
    (total_additions, total_deletions, files_changed)
}

fn insert_metadata_string(
    metadata: &mut BTreeMap<String, Value>,
    key: impl Into<String>,
    value: Option<String>,
) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        metadata.insert(key.into(), Value::String(value));
    }
}

fn provider_value_string(value: Value) -> String {
    match value {
        Value::String(value) => value,
        value => value.to_string(),
    }
}

fn epoch_seconds_to_rfc3339(epoch_seconds: i64) -> Option<String> {
    let milliseconds = epoch_seconds.checked_mul(1_000)?;
    sdkwork_utils_rust::from_unix_millis(milliseconds)
        .map(|value| sdkwork_utils_rust::format_datetime(value, None))
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn provider_scalar(value: &impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(value)) => value,
        Ok(value) => value.to_string(),
        Err(_) => "unknown".to_string(),
    }
}

fn item_type(item: &ThreadItem) -> &'static str {
    match item {
        ThreadItem::UserMessage { .. } => "user_message",
        ThreadItem::HookPrompt { .. } => "hook_prompt",
        ThreadItem::AgentMessage { .. } => "agent_message",
        ThreadItem::Plan { .. } => "plan",
        ThreadItem::Reasoning { .. } => "reasoning",
        ThreadItem::CommandExecution { .. } => "command_execution",
        ThreadItem::FileChange { .. } => "file_change",
        ThreadItem::McpToolCall { .. } => "mcp_tool_call",
        ThreadItem::DynamicToolCall { .. } => "dynamic_tool_call",
        ThreadItem::CollabAgentToolCall { .. } => "collab_agent_tool_call",
        ThreadItem::SubAgentActivity { .. } => "sub_agent_activity",
        ThreadItem::WebSearch(_) => "web_search",
        ThreadItem::ImageView { .. } => "image_view",
        ThreadItem::Sleep(_) => "sleep",
        ThreadItem::ImageGeneration(_) => "image_generation",
        ThreadItem::EnteredReviewMode { .. } => "entered_review_mode",
        ThreadItem::ExitedReviewMode { .. } => "exited_review_mode",
        ThreadItem::ContextCompaction { .. } => "context_compaction",
    }
}

fn item_role(item: &ThreadItem) -> AgentMessageRole {
    match item {
        ThreadItem::UserMessage { .. } => AgentMessageRole::User,
        ThreadItem::HookPrompt { .. } => AgentMessageRole::System,
        ThreadItem::AgentMessage { .. }
        | ThreadItem::Plan { .. }
        | ThreadItem::Reasoning { .. } => AgentMessageRole::Agent,
        ThreadItem::CommandExecution { .. }
        | ThreadItem::FileChange { .. }
        | ThreadItem::McpToolCall { .. }
        | ThreadItem::DynamicToolCall { .. }
        | ThreadItem::CollabAgentToolCall { .. }
        | ThreadItem::WebSearch(_)
        | ThreadItem::ImageView { .. }
        | ThreadItem::ImageGeneration(_) => AgentMessageRole::Tool,
        ThreadItem::SubAgentActivity { .. }
        | ThreadItem::Sleep(_)
        | ThreadItem::EnteredReviewMode { .. }
        | ThreadItem::ExitedReviewMode { .. }
        | ThreadItem::ContextCompaction { .. } => AgentMessageRole::Adapter,
    }
}

fn is_tool_item(item: &ThreadItem) -> bool {
    matches!(
        item,
        ThreadItem::CommandExecution { .. }
            | ThreadItem::FileChange { .. }
            | ThreadItem::McpToolCall { .. }
            | ThreadItem::DynamicToolCall { .. }
            | ThreadItem::CollabAgentToolCall { .. }
            | ThreadItem::WebSearch(_)
            | ThreadItem::ImageView { .. }
            | ThreadItem::Sleep(_)
            | ThreadItem::ImageGeneration(_)
    )
}

fn item_is_untrusted(item: &ThreadItem) -> bool {
    matches!(
        item,
        ThreadItem::CommandExecution {
            aggregated_output: Some(_),
            ..
        } | ThreadItem::FileChange { .. }
            | ThreadItem::McpToolCall {
                result: Some(_),
                ..
            }
            | ThreadItem::McpToolCall { error: Some(_), .. }
            | ThreadItem::DynamicToolCall {
                content_items: Some(_),
                ..
            }
            | ThreadItem::WebSearch(_)
            | ThreadItem::ImageView { .. }
            | ThreadItem::ImageGeneration(_)
    )
}

fn provider_message_record(
    provider_session_id: &str,
    entry: &ThreadItemEntry,
    turn_meta: Option<&ProviderTurnMeta>,
) -> KernelResult<SdkRuntimeMessageRecord> {
    let item = &entry.item;
    let item_id = item.id();
    let mut metadata = BTreeMap::from([
        (
            "codex.turn_id".to_string(),
            Value::String(entry.turn_id.clone()),
        ),
        (
            "codex.item_type".to_string(),
            Value::String(item_type(item).to_string()),
        ),
        (
            "codex.item_id".to_string(),
            Value::String(item_id.to_string()),
        ),
    ]);
    if let Some(turn_meta) = turn_meta {
        metadata.insert(
            "codex.turn.status".to_string(),
            Value::String(provider_scalar(&turn_meta.status)),
        );
        metadata.insert(
            "codex.turn.items_view".to_string(),
            Value::String(provider_scalar(&turn_meta.items_view)),
        );
        if let Some(error) = &turn_meta.error {
            metadata.insert(
                "codex.turn.error".to_string(),
                serde_json::to_value(error).unwrap_or(Value::Null),
            );
        }
        if let Some(started_at) = turn_meta.started_at {
            if let Some(started_at) = epoch_seconds_to_rfc3339(started_at) {
                metadata.insert(
                    "codex.turn.started_at".to_string(),
                    Value::String(started_at),
                );
            }
        }
        if let Some(completed_at) = turn_meta.completed_at {
            if let Some(completed_at) = epoch_seconds_to_rfc3339(completed_at) {
                metadata.insert(
                    "codex.turn.completed_at".to_string(),
                    Value::String(completed_at),
                );
            }
        }
        if let Some(duration_ms) = turn_meta.duration_ms {
            metadata.insert(
                "codex.turn.duration_ms".to_string(),
                Value::from(duration_ms),
            );
        }
    }
    if item_is_untrusted(item) {
        metadata.insert("sdkwork.provider.untrusted".to_string(), Value::Bool(true));
    }
    let parts = message_parts(item)?
        .into_iter()
        .map(provider_message_part)
        .collect::<KernelResult<Vec<_>>>()?;
    let created_at = turn_meta
        .and_then(|turn_meta| turn_meta.started_at)
        .and_then(epoch_seconds_to_rfc3339);

    SdkRuntimeMessageRecord {
        provider_message_id: item_id.to_string(),
        provider_session_id: provider_session_id.to_string(),
        parent_provider_message_id: None,
        role: item_role(item).as_str().to_string(),
        parts,
        created_at,
        metadata,
    }
    .validated(provider_session_id)
}

fn message_parts(item: &ThreadItem) -> KernelResult<Vec<AgentPart>> {
    let mut parts = render_parts(item);
    parts.push(raw_item_part(item)?);
    Ok(parts)
}

fn provider_message_part(part: AgentPart) -> KernelResult<SdkRuntimeMessagePart> {
    let json = part
        .json
        .map(|json| {
            serde_json::from_str(&json).map_err(|error| {
                KernelError::provider_error(
                    "codex_thread_item_json_invalid",
                    format!("failed to project typed Codex message part JSON: {error}"),
                )
                .with_provider(CODEX_PROVIDER_ID)
            })
        })
        .transpose()?;
    let mut metadata = part
        .metadata
        .into_iter()
        .map(|(key, value)| (key, Value::String(value)))
        .collect::<BTreeMap<_, _>>();
    insert_metadata_string(&mut metadata, "sdkwork.provider.schema", part.schema);
    insert_metadata_string(
        &mut metadata,
        "sdkwork.provider.provenance",
        part.provenance,
    );
    if !matches!(part.redaction_classification, KernelEventRedaction::Unknown) {
        metadata.insert(
            "sdkwork.provider.redaction".to_string(),
            Value::String(redaction_name(&part.redaction_classification).to_string()),
        );
    }

    Ok(SdkRuntimeMessagePart {
        part_id: part.part_id,
        kind: part.kind.as_str().to_string(),
        text: part.text,
        json,
        content_ref: part.content_ref,
        artifact_id: part.artifact_id,
        tool_call_id: part.tool_call_id,
        policy_decision_id: part.policy_decision_id,
        error_code: part.error_code,
        mime_type: part.mime_type,
        name: part.name,
        metadata,
    })
}

fn redaction_name(redaction: &KernelEventRedaction) -> &'static str {
    match redaction {
        KernelEventRedaction::Public => "public",
        KernelEventRedaction::Internal => "internal",
        KernelEventRedaction::TenantSensitive => "tenant_sensitive",
        KernelEventRedaction::PersonalData => "personal_data",
        KernelEventRedaction::Secret => "secret",
        KernelEventRedaction::Regulated => "regulated",
        KernelEventRedaction::Unknown => "unknown",
    }
}

fn render_parts(item: &ThreadItem) -> Vec<AgentPart> {
    let item_id = item.id();
    match item {
        ThreadItem::UserMessage {
            content, client_id, ..
        } => {
            let mut parts = content
                .iter()
                .enumerate()
                .map(|(index, input)| user_input_part(item_id, index, input))
                .collect::<Vec<_>>();
            // The client-supplied message id (used for turn correlation) is
            // retained on the first part so the message keeps its upstream
            // correlation identity.
            if let Some(client_id) = client_id {
                if let Some(first) = parts.first_mut() {
                    *first = first
                        .clone()
                        .with_metadata("codex.client_message_id", client_id.as_str());
                }
            }
            parts
        }
        ThreadItem::HookPrompt { fragments, .. } => fragments
            .iter()
            .enumerate()
            .map(|(index, fragment)| {
                AgentPart::text(format!("{item_id}.hook.{index}"), &fragment.text)
                    .from_provider(CODEX_PROVIDER_ID)
                    .with_metadata("codex.hook_run_id", &fragment.hook_run_id)
            })
            .collect(),
        ThreadItem::AgentMessage {
            text,
            phase,
            memory_citation,
            ..
        } => {
            let mut part = provider_text(item_id, "agent_message", text);
            if let Some(phase) = phase {
                part = part.with_metadata("codex.message.phase", provider_scalar(phase));
            }
            if let Some(citation) = memory_citation {
                part = part.with_metadata(
                    "codex.message.memory_citation",
                    serde_json::to_string(citation).unwrap_or_default(),
                );
            }
            vec![part]
        }
        ThreadItem::Plan { text, .. } => vec![provider_text(item_id, "plan", text)],
        ThreadItem::Reasoning {
            summary, content, ..
        } => summary
            .iter()
            .map(|text| ("reasoning_summary", text))
            .chain(content.iter().map(|text| ("reasoning_content", text)))
            .enumerate()
            .map(|(index, (content_type, text))| {
                provider_text(&format!("{item_id}.{index}"), content_type, text)
            })
            .collect(),
        ThreadItem::CommandExecution {
            command,
            cwd,
            process_id,
            source,
            status,
            aggregated_output,
            exit_code,
            duration_ms,
            plugin_id,
            ..
        } => {
            let mut parts = vec![provider_tool_call(
                item_id,
                "shell_command",
                "command_execution",
                provider_scalar(status),
            )
            .with_metadata("codex.command", command)
            .with_metadata("codex.command.cwd", cwd.as_str())
            .with_metadata("codex.command.source", provider_scalar(source))];
            if let Some(plugin_id) = plugin_id {
                parts[0] = parts[0]
                    .clone()
                    .with_metadata("codex.command.plugin_id", plugin_id.as_str());
            }
            if let Some(process_id) = process_id {
                parts[0] = parts[0]
                    .clone()
                    .with_metadata("codex.command.process_id", process_id.as_str());
            }
            if let Some(exit_code) = exit_code {
                parts[0] = parts[0]
                    .clone()
                    .with_metadata("codex.command.exit_code", exit_code.to_string().as_str());
            }
            if let Some(duration_ms) = duration_ms {
                parts[0] = parts[0].clone().with_metadata(
                    "codex.command.duration_ms",
                    duration_ms.to_string().as_str(),
                );
            }
            if let Some(output) = aggregated_output {
                parts.push(
                    provider_text(&format!("{item_id}.output"), "tool_output", output)
                        .with_metadata("codex.tool_call_id", item_id)
                        .with_redaction(KernelEventRedaction::TenantSensitive),
                );
            }
            parts
        }
        ThreadItem::FileChange { status, .. } => vec![provider_tool_call(
            item_id,
            "apply_patch",
            "file_change",
            provider_scalar(status),
        )],
        ThreadItem::McpToolCall {
            server,
            tool,
            status,
            read_only_hint,
            result,
            error,
            duration_ms,
            ..
        } => {
            let mut parts =
                vec![
                    provider_tool_call(item_id, tool, "mcp_tool_call", provider_scalar(status))
                        .with_metadata("codex.mcp.server", server),
                ];
            if let Some(read_only_hint) = read_only_hint {
                parts[0] = parts[0]
                    .clone()
                    .with_metadata("codex.mcp.read_only_hint", read_only_hint.to_string());
            }
            if let Some(duration_ms) = duration_ms {
                parts[0] = parts[0]
                    .clone()
                    .with_metadata("codex.mcp.duration_ms", duration_ms.to_string().as_str());
            }
            match (result.as_ref(), error.as_ref()) {
                (Some(result), _) => parts.push(provider_tool_result_part(
                    item_id,
                    "mcp_tool_result",
                    "completed",
                    serde_json::json!({ "result": result }),
                )),
                (None, Some(error)) => parts.push(provider_tool_result_part(
                    item_id,
                    "mcp_tool_result",
                    "failed",
                    serde_json::json!({ "error": error }),
                )),
                (None, None) => {}
            }
            parts
        }
        ThreadItem::DynamicToolCall {
            tool,
            status,
            content_items,
            success,
            namespace,
            duration_ms,
            ..
        } => {
            let mut parts = vec![provider_tool_call(
                item_id,
                tool,
                "dynamic_tool_call",
                provider_scalar(status),
            )];
            if let Some(namespace) = namespace {
                parts[0] = parts[0]
                    .clone()
                    .with_metadata("codex.tool.namespace", namespace.as_str());
            }
            if let Some(duration_ms) = duration_ms {
                parts[0] = parts[0]
                    .clone()
                    .with_metadata("codex.tool.duration_ms", duration_ms.to_string().as_str());
            }
            if content_items.is_some() || success.is_some() {
                parts.push(provider_tool_result_part(
                    item_id,
                    "tool_result",
                    &provider_scalar(status),
                    serde_json::json!({
                        "contentItems": content_items,
                        "success": success,
                    }),
                ));
            }
            parts
        }
        ThreadItem::CollabAgentToolCall {
            tool,
            status,
            sender_thread_id,
            receiver_thread_ids,
            prompt,
            model,
            reasoning_effort,
            agents_states,
            ..
        } => {
            let mut parts = vec![provider_tool_call(
                item_id,
                &provider_scalar(tool),
                "collab_agent_tool_call",
                provider_scalar(status),
            )
            .with_metadata("codex.collab.sender_thread_id", sender_thread_id)];
            // The sub-agent collaboration payload (receiver threads, prompt,
            // model, reasoning effort, and each agent's live state including
            // its final message) is preserved as a structured result part so
            // the sub-agent execution context survives the canonical
            // projection.
            parts.push(provider_tool_result_part(
                item_id,
                "collab_agent_tool_result",
                &provider_scalar(status),
                serde_json::json!({
                    "tool": provider_scalar(tool),
                    "senderThreadId": sender_thread_id,
                    "receiverThreadIds": receiver_thread_ids,
                    "prompt": prompt,
                    "model": model,
                    "reasoningEffort": reasoning_effort,
                    "agentsStates": agents_states,
                }),
            ));
            parts
        }
        ThreadItem::SubAgentActivity {
            kind,
            agent_thread_id,
            agent_path,
            ..
        } => vec![provider_text(
            item_id,
            "sub_agent_activity",
            &format!(
                "{}: {} ({})",
                provider_scalar(kind),
                agent_thread_id,
                agent_path
            ),
        )
        .with_metadata("codex.sub_agent.kind", provider_scalar(kind))
        .with_metadata("codex.sub_agent.thread_id", agent_thread_id)
        .with_metadata("codex.sub_agent.path", agent_path)],
        ThreadItem::WebSearch(item) => {
            let mut parts = vec![provider_tool_call(
                &item.id,
                "web_search",
                "web_search",
                "completed".to_string(),
            )
            .with_metadata("codex.web_search.query", &item.query)];
            // `WebSearchAction` is an internally tagged enum; the type tag is
            // projected as a scalar so consumers get the action kind without
            // re-parsing the raw payload.
            if let Some(action_type) = serde_json::to_value(&item.action)
                .ok()
                .and_then(|value| value.get("type").cloned())
                .and_then(|value| value.as_str().map(str::to_string))
            {
                parts[0] = parts[0]
                    .clone()
                    .with_metadata("codex.web_search.action", action_type);
            }
            if item.results.is_some() {
                parts.push(provider_tool_result_part(
                    &item.id,
                    "web_search_tool_result",
                    "completed",
                    serde_json::json!({
                        "query": item.query,
                        "results": item.results,
                    }),
                ));
            }
            parts
        }
        ThreadItem::ImageView { path, .. } => {
            vec![
                AgentPart::image_ref(format!("{item_id}.image"), path.to_string(), "image/*")
                    .from_provider(CODEX_PROVIDER_ID),
            ]
        }
        ThreadItem::Sleep(item) => {
            vec![
                provider_tool_call(&item.id, "sleep", "sleep", "completed".to_string())
                    .with_metadata("codex.duration_ms", item.duration_ms.to_string()),
            ]
        }
        ThreadItem::ImageGeneration(item) => {
            let mut parts = vec![provider_tool_call(
                &item.id,
                "image_generation",
                "image_generation",
                item.status.clone(),
            )];
            if let Some(revised_prompt) = item.revised_prompt.as_deref() {
                parts[0] = parts[0]
                    .clone()
                    .with_metadata("codex.image_generation.revised_prompt", revised_prompt);
            }
            if let Some(saved_path) = item
                .saved_path
                .as_ref()
                .map(|path| path.display().to_string())
            {
                parts[0] = parts[0]
                    .clone()
                    .with_metadata("codex.image_generation.saved_path", saved_path);
            }
            if !item.result.is_empty() {
                parts.push(
                    AgentPart::image_ref(format!("{}.image", item.id), &item.result, "image/*")
                        .from_provider(CODEX_PROVIDER_ID),
                );
            }
            parts
        }
        ThreadItem::EnteredReviewMode { review, .. } => {
            vec![provider_text(item_id, "entered_review_mode", review)]
        }
        ThreadItem::ExitedReviewMode { review, .. } => {
            vec![provider_text(item_id, "exited_review_mode", review)]
        }
        ThreadItem::ContextCompaction { .. } => vec![provider_text(
            item_id,
            "context_compaction",
            "Codex compacted the thread context.",
        )],
    }
}

fn user_input_part(item_id: &str, index: usize, input: &UserInput) -> AgentPart {
    let part_id = format!("{item_id}.input.{index}");
    match input {
        UserInput::Text { text, .. } => provider_text(&part_id, "input_text", text),
        UserInput::Image { url, .. } => {
            AgentPart::image_ref(part_id, url, "image/*").from_provider(CODEX_PROVIDER_ID)
        }
        UserInput::LocalImage { path, .. } => {
            AgentPart::image_ref(part_id, path.display().to_string(), "image/*")
                .from_provider(CODEX_PROVIDER_ID)
        }
        UserInput::Audio { url } => {
            AgentPart::audio_ref(part_id, url, "audio/*").from_provider(CODEX_PROVIDER_ID)
        }
        UserInput::LocalAudio { path } => {
            AgentPart::audio_ref(part_id, path.display().to_string(), "audio/*")
                .from_provider(CODEX_PROVIDER_ID)
        }
        UserInput::Skill { name, path } => AgentPart::file_ref(
            part_id,
            path.display().to_string(),
            "application/vnd.sdkwork.skill",
        )
        .with_name(name)
        .from_provider(CODEX_PROVIDER_ID),
        UserInput::Mention { name, path } => {
            provider_text(&part_id, "mention", &format!("@{name} ({path})"))
        }
    }
}

fn provider_text(part_id: &str, content_type: &str, text: &str) -> AgentPart {
    AgentPart::text(part_id, text)
        .from_provider(CODEX_PROVIDER_ID)
        .with_metadata("codex.content_type", content_type)
}

fn provider_tool_call(item_id: &str, name: &str, content_type: &str, status: String) -> AgentPart {
    AgentPart::tool_call_ref(format!("{item_id}.tool"), item_id)
        .with_name(name)
        .from_provider(CODEX_PROVIDER_ID)
        .with_metadata("codex.content_type", content_type)
        .with_metadata("codex.status", status)
}

/// Structured tool result part carrying the originating tool call id so the
/// downstream session history reconciler can pair it with its `ToolCall` item
/// (`tool_call_id` match) and persist the call → result parent chain.
fn provider_tool_result_part(
    item_id: &str,
    content_type: &str,
    status: &str,
    payload: serde_json::Value,
) -> AgentPart {
    AgentPart::json(format!("{item_id}.result"), payload.to_string())
        .from_provider(CODEX_PROVIDER_ID)
        .with_metadata("codex.content_type", content_type)
        .with_metadata("codex.status", status)
        .with_metadata("codex.tool_call_id", item_id)
        .with_redaction(KernelEventRedaction::TenantSensitive)
}

fn raw_item_part(item: &ThreadItem) -> KernelResult<AgentPart> {
    let raw = serde_json::to_string(item).map_err(|error| {
        KernelError::provider_error(
            "codex_thread_item_serialization_failed",
            format!("failed to preserve typed Codex thread item: {error}"),
        )
        .with_provider(CODEX_PROVIDER_ID)
    })?;
    Ok(AgentPart::json(format!("{}.raw", item.id()), raw)
        .with_name("codex.thread_item")
        .with_schema(RAW_ITEM_SCHEMA)
        .from_provider(CODEX_PROVIDER_ID)
        .with_redaction(KernelEventRedaction::TenantSensitive)
        .with_metadata("codex.content_type", "raw_provider_item"))
}

#[cfg(test)] // WORKSPACE-PATH:allow-fixture-block: this module is the file's #[cfg(test)] unit-test fixture data
mod tests {
    use super::*;
    use codex_app_server_protocol::ThreadItemsListResponse;

    fn sample_thread() -> Thread {
        serde_json::from_value(serde_json::json!({
            "id": "0198-thread",
            "extra": null,
            "sessionId": "0198-session",
            "forkedFromId": "0197-thread",
            "parentThreadId": "0196-thread",
            "preview": "Review the provider",
            "ephemeral": false,
            "section": {"id": "section-pinned", "name": "Pinned"},
            "sectionEnteredAt": 1_752_000_100,
            "historyMode": "paginated",
            "modelProvider": "openai",
            "createdAt": 1_752_000_000,
            "updatedAt": 1_752_000_200,
            "recencyAt": 1_752_000_300,
            "status": {"type": "active", "activeFlags": ["waitingOnApproval"]},
            "path": "C:/Users/test/.codex/sessions/thread.jsonl",
            "cwd": "E:/workspace/project",
            "cliVersion": "0.99.0",
            "source": "vscode",
            "canAcceptDirectInput": true,
            "threadSource": "subagent",
            "agentNickname": "reviewer",
            "agentRole": "code-review",
            "gitInfo": {"sha": "abc123", "branch": "main", "originUrl": "https://example.invalid/repo.git"},
            "name": "Provider review",
            "turns": []
        }))
        .expect("thread fixture must match upstream protocol")
    }

    #[test]
    fn maps_every_stable_thread_field_and_preserves_typed_thread() {
        let provider_thread = sample_thread();
        let expected_cwd =
            normalize_provider_session_path(&provider_thread.cwd.display().to_string());
        let record = map_thread_record(provider_thread.clone()).expect("thread mapping");
        let session = record.session;

        assert_eq!(record.provider_thread, provider_thread);
        assert_eq!(session.provider_session_id, "0198-thread");
        assert_eq!(
            session.parent_provider_session_id.as_deref(),
            Some("0196-thread")
        );
        assert_eq!(session.title.as_deref(), Some("Provider review"));
        assert_eq!(session.preview.as_deref(), Some("Review the provider"));
        assert_eq!(session.model_provider.as_deref(), Some("openai"));
        assert_eq!(session.cwd.as_deref(), Some(expected_cwd.as_str()));
        assert_eq!(
            session
                .metadata
                .get("codex.session_id")
                .and_then(Value::as_str),
            Some("0198-session")
        );
        assert_eq!(
            session
                .metadata
                .get("codex.section.name")
                .and_then(Value::as_str),
            Some("Pinned")
        );
        assert_eq!(
            session
                .metadata
                .get("codex.history_mode")
                .and_then(Value::as_str),
            Some("paginated")
        );
        assert_eq!(
            session
                .metadata
                .get("codex.cli_version")
                .and_then(Value::as_str),
            Some("0.99.0")
        );
        assert_eq!(
            session
                .metadata
                .get("codex.can_accept_direct_input")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            session
                .metadata
                .get("codex.thread_source")
                .and_then(Value::as_str),
            Some("subagent")
        );
        assert_eq!(
            session
                .metadata
                .get("codex.git.sha")
                .and_then(Value::as_str),
            Some("abc123")
        );
        assert_eq!(
            session
                .metadata
                .get("codex.forked_from_id")
                .and_then(Value::as_str),
            Some("0197-thread")
        );
        assert_eq!(
            session
                .metadata
                .get("codex.agent_nickname")
                .and_then(Value::as_str),
            Some("reviewer")
        );
        assert_eq!(
            session
                .metadata
                .get("codex.agent_role")
                .and_then(Value::as_str),
            Some("code-review")
        );
        assert_eq!(
            session.metadata.get("codex.path").and_then(Value::as_str),
            Some("C:/Users/test/.codex/sessions/thread.jsonl")
        );
        let serialized = serde_json::to_value(&session).expect("serialize provider record");
        assert!(serialized.get("session_id").is_none());
        assert!(serialized.get("parent_session_id").is_none());
    }

    #[test]
    fn maps_tool_item_without_losing_upstream_fields() {
        let response: ThreadItemsListResponse = serde_json::from_value(serde_json::json!({
            "data": [{
                "turnId": "turn-1",
                "item": {
                    "type": "mcpToolCall",
                    "id": "item-1",
                    "server": "docs",
                    "tool": "search",
                    "status": "completed",
                    "arguments": {"query": "Codex"},
                    "appContext": null,
                    "pluginId": null,
                    "readOnlyHint": true,
                    "result": null,
                    "error": null,
                    "durationMs": 42
                }
            }],
            "nextCursor": "next-1",
            "backwardsCursor": "previous-1"
        }))
        .expect("item fixture must match upstream protocol");
        let page = map_item_page("thread-1", response).expect("item page mapping");
        let record = &page.data[0];

        assert_eq!(page.next_cursor.as_deref(), Some("next-1"));
        assert_eq!(record.message.provider_message_id, "item-1");
        assert_eq!(record.message.provider_session_id, "thread-1");
        assert_eq!(record.message.role, "tool");
        assert_eq!(
            record
                .message
                .metadata
                .get("codex.turn_id")
                .and_then(Value::as_str),
            Some("turn-1")
        );
        assert_eq!(
            record.message.parts[0]
                .metadata
                .get("codex.mcp.read_only_hint")
                .and_then(Value::as_str),
            Some("true")
        );
        let raw = record
            .message
            .parts
            .iter()
            .find(|part| {
                part.name.as_deref() == Some("codex.thread_item")
                    && part
                        .metadata
                        .get("sdkwork.provider.schema")
                        .and_then(Value::as_str)
                        == Some(RAW_ITEM_SCHEMA)
            })
            .and_then(|part| part.json.as_ref())
            .expect("raw typed item");
        assert_eq!(raw.get("readOnlyHint").and_then(Value::as_bool), Some(true));
        assert_eq!(
            record
                .message
                .parts
                .iter()
                .find(|part| part.name.as_deref() == Some("codex.thread_item"))
                .and_then(|part| part.metadata.get("sdkwork.provider.redaction"))
                .and_then(Value::as_str),
            Some("tenant_sensitive")
        );
        assert_eq!(record.provider_item.item.id(), "item-1");
    }

    #[test]
    fn maps_turn_page_preserving_turn_status_timeline_and_message_time() {
        let response: ThreadTurnsListResponse = serde_json::from_value(serde_json::json!({
            "data": [{
                "id": "turn-1",
                "items": [
                    {"type": "agentMessage", "id": "agent-1", "text": "answer", "phase": null, "memoryCitation": null}
                ],
                "itemsView": "full",
                "status": "completed",
                "error": null,
                "startedAt": 1_752_000_000,
                "completedAt": 1_752_000_120,
                "durationMs": 120_000
            }],
            "nextCursor": null,
            "backwardsCursor": null
        }))
        .expect("turns page fixture");
        let page = map_turn_page("thread-1", response).expect("turn page mapping");
        let record = &page.data[0].message;

        assert_eq!(
            record.metadata.get("codex.turn_id").and_then(Value::as_str),
            Some("turn-1")
        );
        assert_eq!(
            record
                .metadata
                .get("codex.turn.status")
                .and_then(Value::as_str),
            Some("completed")
        );
        assert_eq!(
            record
                .metadata
                .get("codex.turn.items_view")
                .and_then(Value::as_str),
            Some("full")
        );
        assert_eq!(
            record
                .metadata
                .get("codex.turn.started_at")
                .and_then(Value::as_str),
            Some("2025-07-08T18:40:00.000Z")
        );
        assert_eq!(
            record
                .metadata
                .get("codex.turn.completed_at")
                .and_then(Value::as_str),
            Some("2025-07-08T18:42:00.000Z")
        );
        assert_eq!(
            record
                .metadata
                .get("codex.turn.duration_ms")
                .and_then(Value::as_i64),
            Some(120_000)
        );
        // The turn start time becomes the message timestamp so the canonical
        // message carries a real created_at instead of None.
        // The turn start time becomes the message timestamp so the canonical
        // message carries a real created_at instead of None. SPI validation
        // normalizes the message timestamp to nanosecond precision.
        assert_eq!(
            record.created_at.as_deref(),
            Some("2025-07-08T18:40:00.000000000Z")
        );
    }

    #[test]
    fn maps_turn_error_onto_messages_of_the_failed_turn() {
        let response: ThreadTurnsListResponse = serde_json::from_value(serde_json::json!({
            "data": [{
                "id": "turn-failed",
                "items": [
                    {"type": "agentMessage", "id": "agent-1", "text": "partial", "phase": null, "memoryCitation": null}
                ],
                "itemsView": "full",
                "status": "failed",
                "error": {"message": "model timeout", "codexErrorInfo": null, "additionalDetails": null},
                "startedAt": 1_752_000_000,
                "completedAt": null,
                "durationMs": null
            }],
            "nextCursor": null,
            "backwardsCursor": null
        }))
        .expect("failed turns page fixture");
        let page = map_turn_page("thread-1", response).expect("turn page mapping");
        let record = &page.data[0].message;
        assert_eq!(
            record
                .metadata
                .get("codex.turn.status")
                .and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            record
                .metadata
                .get("codex.turn.error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str),
            Some("model timeout")
        );
    }

    #[test]
    fn maps_every_upstream_thread_item_variant() {
        let fixtures = serde_json::json!([
            {"type":"userMessage","id":"user","clientId":null,"content":[]},
            {"type":"hookPrompt","id":"hook","fragments":[]},
            {"type":"agentMessage","id":"agent","text":"answer"},
            {"type":"plan","id":"plan","text":"plan"},
            {"type":"reasoning","id":"reasoning","summary":["summary"],"content":["detail"]},
            {"type":"commandExecution","id":"command","command":"pwd","cwd":"E:/workspace","processId":null,"status":"completed","commandActions":[],"aggregatedOutput":null,"exitCode":0,"durationMs":1},
            {"type":"fileChange","id":"file","changes":[],"status":"completed"},
            {"type":"mcpToolCall","id":"mcp","server":"docs","tool":"search","status":"completed","arguments":{},"appContext":null,"pluginId":null,"readOnlyHint":true,"result":null,"error":null,"durationMs":1},
            {"type":"dynamicToolCall","id":"dynamic","namespace":null,"tool":"lookup","arguments":{},"status":"completed","contentItems":null,"success":true,"durationMs":1},
            {"type":"collabAgentToolCall","id":"collab","tool":"wait","status":"completed","senderThreadId":"sender","receiverThreadIds":[],"prompt":null,"model":null,"reasoningEffort":null,"agentsStates":{}},
            {"type":"subAgentActivity","id":"subagent","kind":"started","agentThreadId":"child","agentPath":"/root/child"},
            {"type":"webSearch","id":"web","query":"Codex","action":null,"results":null},
            {"type":"imageView","id":"view","path":"E:/workspace/image.png"},
            {"type":"sleep","id":"sleep","durationMs":10},
            {"type":"imageGeneration","id":"image","status":"completed","revisedPrompt":null,"result":"https://example.invalid/image.png"},
            {"type":"enteredReviewMode","id":"review-in","review":"review"},
            {"type":"exitedReviewMode","id":"review-out","review":"review"},
            {"type":"contextCompaction","id":"compaction"}
        ]);
        let expected_types = [
            "user_message",
            "hook_prompt",
            "agent_message",
            "plan",
            "reasoning",
            "command_execution",
            "file_change",
            "mcp_tool_call",
            "dynamic_tool_call",
            "collab_agent_tool_call",
            "sub_agent_activity",
            "web_search",
            "image_view",
            "sleep",
            "image_generation",
            "entered_review_mode",
            "exited_review_mode",
            "context_compaction",
        ];
        let items: Vec<ThreadItem> =
            serde_json::from_value(fixtures).expect("fixtures must match every upstream variant");

        assert_eq!(items.len(), expected_types.len());
        for (item, expected_type) in items.into_iter().zip(expected_types) {
            let item_id = item.id().to_string();
            let message = CodexMessageAdapter::new()
                .to_agent_message(&ThreadItemEntry {
                    turn_id: "turn-all".to_string(),
                    item,
                })
                .expect("every upstream item must map");
            assert_eq!(message.message_id, item_id);
            assert_eq!(
                message.metadata_value("codex.item_type"),
                Some(expected_type)
            );
            assert!(message
                .parts
                .iter()
                .any(|part| part.schema.as_deref() == Some(RAW_ITEM_SCHEMA)));
        }
    }

    #[test]
    fn preserves_every_new_protocol_field_on_structured_parts() {
        let items: Vec<ThreadItem> = serde_json::from_value(serde_json::json!([
            {
                "type": "agentMessage",
                "id": "agent-1",
                "text": "answer",
                "phase": "final_answer",
                "memoryCitation": {"entries": [{"path": "docs/note.md", "lineStart": 1, "lineEnd": 3, "note": "key note"}], "threadIds": ["thread-9"]}
            },
            {
                "type": "userMessage",
                "id": "user-1",
                "clientId": "client-message-1",
                "content": [{"type": "text", "text": "hello"}]
            },
            {
                "type": "commandExecution",
                "id": "command-1",
                "command": "cargo test",
                "cwd": "E:/workspace",
                "processId": "pty-9",
                "source": "agent",
                "pluginId": "plugin.test",
                "status": "completed",
                "commandActions": [],
                "aggregatedOutput": null,
                "exitCode": 7,
                "durationMs": 42
            },
            {
                "type": "mcpToolCall",
                "id": "mcp-1",
                "server": "docs",
                "tool": "search",
                "status": "completed",
                "arguments": {},
                "appContext": null,
                "mcpAppResourceUri": null,
                "pluginId": null,
                "readOnlyHint": null,
                "result": null,
                "error": null,
                "durationMs": 123
            },
            {
                "type": "dynamicToolCall",
                "id": "dynamic-1",
                "namespace": "connector.alpha",
                "tool": "lookup",
                "arguments": {},
                "status": "completed",
                "contentItems": null,
                "success": true,
                "durationMs": 9
            },
            {
                "type": "collabAgentToolCall",
                "id": "collab-1",
                "tool": "wait",
                "status": "completed",
                "senderThreadId": "sender-1",
                "receiverThreadIds": ["child-1"],
                "prompt": "review this",
                "model": "gpt-5",
                "reasoningEffort": "high",
                "agentsStates": {}
            },
            {
                "type": "webSearch",
                "id": "web-1",
                "query": "Codex",
                "action": {"type": "search"},
                "results": null
            },
            {
                "type": "imageGeneration",
                "id": "image-1",
                "status": "completed",
                "revisedPrompt": "revised",
                "result": "https://example.invalid/image.png",
                "savedPath": "E:/workspace/generated.png"
            }
        ]))
        .expect("fixtures must match upstream protocol");
        let mut parts_by_type = std::collections::HashMap::new();
        for item in &items {
            let parts = render_parts(item);
            parts_by_type.insert(item_type(item).to_string(), parts);
        }

        let agent = &parts_by_type["agent_message"][0];
        assert_eq!(
            agent.metadata_value("codex.message.phase"),
            Some("final_answer")
        );
        let citation = agent
            .metadata_value("codex.message.memory_citation")
            .expect("memory citation");
        assert!(citation.contains("docs/note.md"));

        let user = &parts_by_type["user_message"][0];
        assert_eq!(
            user.metadata_value("codex.client_message_id"),
            Some("client-message-1")
        );

        let command = &parts_by_type["command_execution"][0];
        assert_eq!(command.metadata_value("codex.command"), Some("cargo test"));
        assert_eq!(
            command.metadata_value("codex.command.cwd"),
            Some("E:/workspace")
        );
        assert_eq!(
            command.metadata_value("codex.command.process_id"),
            Some("pty-9")
        );
        assert_eq!(
            command.metadata_value("codex.command.source"),
            Some("agent")
        );
        assert_eq!(
            command.metadata_value("codex.command.plugin_id"),
            Some("plugin.test")
        );
        assert_eq!(command.metadata_value("codex.command.exit_code"), Some("7"));
        assert_eq!(
            command.metadata_value("codex.command.duration_ms"),
            Some("42")
        );

        let mcp = &parts_by_type["mcp_tool_call"][0];
        assert_eq!(mcp.metadata_value("codex.mcp.duration_ms"), Some("123"));

        let dynamic = &parts_by_type["dynamic_tool_call"][0];
        assert_eq!(
            dynamic.metadata_value("codex.tool.namespace"),
            Some("connector.alpha")
        );
        assert_eq!(dynamic.metadata_value("codex.tool.duration_ms"), Some("9"));

        let collab_result = parts_by_type["collab_agent_tool_call"]
            .iter()
            .find(|part| {
                part.metadata_value("codex.content_type") == Some("collab_agent_tool_result")
            })
            .expect("collab result part");
        let collab_payload = collab_result.json.as_deref().expect("collab payload");
        assert!(collab_payload.contains("\"reasoningEffort\":\"high\""));
        assert!(collab_payload.contains("\"receiverThreadIds\":[\"child-1\"]"));

        let web = &parts_by_type["web_search"][0];
        assert_eq!(
            web.metadata_value("codex.web_search.action"),
            Some("search")
        );

        let image = &parts_by_type["image_generation"][0];
        assert_eq!(
            image.metadata_value("codex.image_generation.revised_prompt"),
            Some("revised")
        );
        // `saved_path` is displayed exactly as the provider's absolute path
        // type renders it (platform native separators), so the expectation is
        // derived from the parsed item rather than hard-coded.
        let ThreadItem::ImageGeneration(image_item) = &items[7] else {
            unreachable!("image generation fixture");
        };
        let expected_saved_path = image_item
            .saved_path
            .as_ref()
            .map(|path| path.display().to_string());
        assert_eq!(
            image.metadata_value("codex.image_generation.saved_path"),
            expected_saved_path.as_deref()
        );
    }

    #[test]
    fn command_execution_output_part_carries_tool_call_id_for_pairing() {
        let item: ThreadItem = serde_json::from_value(serde_json::json!({
            "type": "commandExecution",
            "id": "command-1",
            "command": "cargo test",
            "cwd": "E:/workspace",
            "processId": null,
            "status": "completed",
            "commandActions": [],
            "aggregatedOutput": "All tests passed",
            "exitCode": 0,
            "durationMs": 42
        }))
        .expect("commandExecution fixture");
        let parts = render_parts(&item);

        let call_part = parts
            .iter()
            .find(|part| part.part_id == "command-1.tool")
            .expect("tool call part");
        assert_eq!(call_part.tool_call_id.as_deref(), Some("command-1"));

        let output_part = parts
            .iter()
            .find(|part| part.part_id == "command-1.output")
            .expect("tool output part");
        assert_eq!(
            output_part.metadata_value("codex.tool_call_id"),
            Some("command-1")
        );
        assert_eq!(
            output_part.metadata_value("codex.content_type"),
            Some("tool_output")
        );
        assert_eq!(output_part.text.as_deref(), Some("All tests passed"));
    }

    #[test]
    fn command_execution_without_output_has_no_result_part() {
        let item: ThreadItem = serde_json::from_value(serde_json::json!({
            "type": "commandExecution",
            "id": "command-2",
            "command": "pwd",
            "cwd": "E:/workspace",
            "processId": null,
            "status": "inProgress",
            "commandActions": [],
            "aggregatedOutput": null,
            "exitCode": null,
            "durationMs": null
        }))
        .expect("commandExecution fixture");
        let parts = render_parts(&item);

        assert_eq!(parts.len(), 1);
        assert!(!parts.iter().any(|part| part.part_id == "command-2.output"));
    }

    #[test]
    fn mcp_tool_call_result_produces_paired_result_part() {
        let item: ThreadItem = serde_json::from_value(serde_json::json!({
            "type": "mcpToolCall",
            "id": "mcp-1",
            "server": "docs",
            "tool": "search",
            "status": "completed",
            "arguments": {"query": "Codex"},
            "appContext": null,
            "pluginId": null,
            "readOnlyHint": true,
            "result": {"content": [{"type": "text", "text": "Found 3 docs"}], "structuredContent": null, "_meta": null},
            "error": null,
            "durationMs": 1
        }))
        .expect("mcpToolCall fixture");
        let parts = render_parts(&item);

        let call_part = parts
            .iter()
            .find(|part| part.part_id == "mcp-1.tool")
            .expect("tool call part");
        assert_eq!(call_part.tool_call_id.as_deref(), Some("mcp-1"));

        let result_part = parts
            .iter()
            .find(|part| part.part_id == "mcp-1.result")
            .expect("mcp tool result part");
        assert_eq!(
            result_part.metadata_value("codex.content_type"),
            Some("mcp_tool_result")
        );
        assert_eq!(
            result_part.metadata_value("codex.tool_call_id"),
            Some("mcp-1")
        );
        assert_eq!(
            result_part.metadata_value("codex.status"),
            Some("completed")
        );
        let result_json = result_part
            .json
            .as_ref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .expect("result part JSON");
        assert_eq!(
            result_json
                .get("result")
                .and_then(|r| r.get("content"))
                .is_some(),
            true
        );
    }

    #[test]
    fn mcp_tool_call_error_produces_failed_result_part() {
        let item: ThreadItem = serde_json::from_value(serde_json::json!({
            "type": "mcpToolCall",
            "id": "mcp-2",
            "server": "docs",
            "tool": "search",
            "status": "failed",
            "arguments": {},
            "appContext": null,
            "pluginId": null,
            "readOnlyHint": null,
            "result": null,
            "error": {"message": "server unreachable"},
            "durationMs": 5
        }))
        .expect("mcpToolCall fixture");
        let parts = render_parts(&item);

        let result_part = parts
            .iter()
            .find(|part| part.part_id == "mcp-2.result")
            .expect("mcp tool error part");
        assert_eq!(result_part.metadata_value("codex.status"), Some("failed"));
        let result_json = result_part
            .json
            .as_ref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .expect("result part JSON");
        assert_eq!(
            result_json
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(serde_json::Value::as_str),
            Some("server unreachable")
        );
    }

    #[test]
    fn mcp_tool_call_without_result_has_no_result_part() {
        let item: ThreadItem = serde_json::from_value(serde_json::json!({
            "type": "mcpToolCall",
            "id": "mcp-3",
            "server": "docs",
            "tool": "search",
            "status": "inProgress",
            "arguments": {},
            "appContext": null,
            "pluginId": null,
            "readOnlyHint": null,
            "result": null,
            "error": null,
            "durationMs": null
        }))
        .expect("mcpToolCall fixture");
        let parts = render_parts(&item);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].tool_call_id.as_deref(), Some("mcp-3"));
    }

    #[test]
    fn collab_agent_tool_call_preserves_sub_agent_state_result_part() {
        let item: ThreadItem = serde_json::from_value(serde_json::json!({
            "type": "collabAgentToolCall",
            "id": "collab-1",
            "tool": "spawnAgent",
            "status": "completed",
            "senderThreadId": "sender-1",
            "receiverThreadIds": ["child-1"],
            "prompt": "review the diff",
            "model": "gpt-5",
            "reasoningEffort": null,
            "agentsStates": {
                "child-1": {"status": "completed", "message": "review done"}
            }
        }))
        .expect("collabAgentToolCall fixture");
        let parts = render_parts(&item);

        let call_part = parts
            .iter()
            .find(|part| part.part_id == "collab-1.tool")
            .expect("collab tool call part");
        assert_eq!(call_part.tool_call_id.as_deref(), Some("collab-1"));
        assert_eq!(
            call_part.metadata_value("codex.collab.sender_thread_id"),
            Some("sender-1")
        );

        let result_part = parts
            .iter()
            .find(|part| part.part_id == "collab-1.result")
            .expect("collab tool result part");
        assert_eq!(
            result_part.metadata_value("codex.content_type"),
            Some("collab_agent_tool_result")
        );
        assert_eq!(
            result_part.metadata_value("codex.tool_call_id"),
            Some("collab-1")
        );
        let result_json = result_part
            .json
            .as_ref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .expect("result part JSON");
        assert_eq!(
            result_json
                .get("receiverThreadIds")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            result_json
                .get("agentsStates")
                .and_then(|states| states.get("child-1"))
                .and_then(|state| state.get("message"))
                .and_then(serde_json::Value::as_str),
            Some("review done")
        );
    }

    #[test]
    fn sub_agent_activity_preserves_structured_metadata() {
        let item: ThreadItem = serde_json::from_value(serde_json::json!({
            "type": "subAgentActivity",
            "id": "sub-1",
            "kind": "started",
            "agentThreadId": "child-1",
            "agentPath": "0.0"
        }))
        .expect("subAgentActivity fixture");
        let parts = render_parts(&item);
        assert_eq!(parts.len(), 1);
        assert_eq!(
            parts[0].metadata_value("codex.sub_agent.kind"),
            Some("started")
        );
        assert_eq!(
            parts[0].metadata_value("codex.sub_agent.thread_id"),
            Some("child-1")
        );
        assert_eq!(parts[0].metadata_value("codex.sub_agent.path"), Some("0.0"));
        assert_eq!(parts[0].text.as_deref(), Some("started: child-1 (0.0)"));
    }

    #[test]
    fn web_search_results_produce_paired_result_part() {
        let item: ThreadItem = serde_json::from_value(serde_json::json!({
            "type": "webSearch",
            "id": "web-1",
            "query": "codex protocol",
            "action": {"type": "search", "query": "codex protocol"},
            "results": [{"title": "Codex"}]
        }))
        .expect("webSearch fixture");
        let parts = render_parts(&item);

        let call_part = parts
            .iter()
            .find(|part| part.part_id == "web-1.tool")
            .expect("tool call part");
        assert_eq!(call_part.tool_call_id.as_deref(), Some("web-1"));

        let result_part = parts
            .iter()
            .find(|part| part.part_id == "web-1.result")
            .expect("web search result part");
        assert_eq!(
            result_part.metadata_value("codex.content_type"),
            Some("web_search_tool_result")
        );
        assert_eq!(
            result_part.metadata_value("codex.tool_call_id"),
            Some("web-1")
        );
        let result_json = result_part
            .json
            .as_ref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .expect("result part JSON");
        assert_eq!(
            result_json.get("query").and_then(serde_json::Value::as_str),
            Some("codex protocol")
        );
        assert_eq!(
            result_json
                .get("results")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn rejects_out_of_contract_page_limits() {
        let mut missing = None;
        normalize_page_limit(&mut missing).expect("default limit");
        assert_eq!(
            missing,
            Some(sdkwork_utils_rust::DEFAULT_LIST_PAGE_SIZE as u32)
        );

        for invalid in [0, sdkwork_utils_rust::MAX_LIST_PAGE_SIZE as u32 + 1] {
            let mut limit = Some(invalid);
            assert!(normalize_page_limit(&mut limit).is_err());
        }
    }
}
