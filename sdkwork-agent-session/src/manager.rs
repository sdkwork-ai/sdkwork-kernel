use crate::conversation::ConversationManager;
use crate::types::{MessageConfig, SessionConfig, SessionQuery};
use sdkwork_agent_database::{
    session_owner_fields_from_metadata_json, AgentDatabase, EventRepository, EventRow,
    MessageRepository, MessageRow, RuntimeSessionWrites, SessionRepository, SessionRow,
    TaskRepository, TaskRow,
};
use sdkwork_agent_kernel::AgentSession;
use std::collections::HashMap;
use std::sync::Arc;

/// Unified session manager backed by one database type implementing all runtime repositories.
pub struct UnifiedSessionManager<DB>
where
    DB: AgentDatabase
        + SessionRepository
        + MessageRepository
        + TaskRepository
        + EventRepository
        + RuntimeSessionWrites
        + Clone,
{
    db: DB,
    event_listener: Option<Arc<dyn Fn(EventRow) + Send + Sync>>,
}

impl<DB> UnifiedSessionManager<DB>
where
    DB: AgentDatabase
        + SessionRepository
        + MessageRepository
        + TaskRepository
        + EventRepository
        + RuntimeSessionWrites
        + Clone,
{
    pub fn new(db: DB) -> Self {
        Self {
            db,
            event_listener: None,
        }
    }

    /// Register a listener invoked after each persisted session event.
    pub fn set_event_listener(&mut self, listener: Arc<dyn Fn(EventRow) + Send + Sync>) {
        self.event_listener = Some(listener);
    }

    /// Create a new session
    pub fn create_session(&self, config: SessionConfig) -> Result<SessionRow, String> {
        let now = sdkwork_agent_database::runtime_now_timestamp();
        let session_id = format!("session.{}", generate_id());

        let metadata_json = config
            .metadata
            .as_ref()
            .and_then(|value| serde_json::to_string(value).ok());
        let (owner_tenant_id, owner_user_ref) =
            session_owner_fields_from_metadata_json(&metadata_json);

        let row = SessionRow {
            session_id: session_id.clone(),
            agent_id: config.agent_id,
            kind: config.kind.unwrap_or_else(|| "main".to_string()),
            source: config.source.unwrap_or_else(|| "api".to_string()),
            state: "active".to_string(),
            title: config.title,
            model: config.model,
            cwd: config.cwd,
            provider_id: None,
            bridge_id: None,
            token_usage_json: None,
            message_count: 0,
            owner_tenant_id,
            owner_user_ref,
            created_at: now.clone(),
            updated_at: Some(now),
            metadata_json,
        };

        let event = self.build_event(&session_id, "session.created", "info", None);
        self.db
            .save_session_with_event(&row, &event)
            .map_err(|e| format!("failed to create session: {e}"))?;
        self.notify_event(event);

        Ok(row)
    }

    /// Get a session by ID
    pub fn get_session(&self, session_id: &str) -> Result<SessionRow, String> {
        self.find_session(session_id)?
            .ok_or_else(|| format!("session not found: {}", session_id))
    }

    pub(crate) fn find_session(&self, session_id: &str) -> Result<Option<SessionRow>, String> {
        self.db
            .load_session(session_id)
            .map_err(|e| format!("failed to load session: {}", e))
    }

    /// List sessions with optional filters
    pub fn list_sessions(&self, query: SessionQuery) -> Result<Vec<SessionRow>, String> {
        let db_query = sdkwork_agent_database::SessionQuery {
            agent_id: query.agent_id,
            state: query.state,
            kind: query.kind,
            provider_id: query.provider_id,
            bridge_id: query.bridge_id,
            owner_tenant_id: query.owner_tenant_id,
            owner_user_ref: query.owner_user_ref,
            after_session_id: query.after_session_id,
            after_session_sort_at: query.after_session_sort_at,
            limit: query.limit,
            offset: query.offset,
        };

        self.db
            .list_sessions(&db_query)
            .map_err(|e| format!("failed to list sessions: {}", e))
    }

    /// Update a session
    pub fn update_session(&self, session: &SessionRow) -> Result<(), String> {
        let existing = self.get_session(&session.session_id)?;
        if existing.provider_id != session.provider_id {
            return Err(format!(
                "cannot change provider ownership for session {}",
                session.session_id
            ));
        }
        if sdkwork_agent_database::session_state_regresses_from_terminal(session, &existing) {
            return Err(format!(
                "cannot transition terminal session {} from {} to {}",
                session.session_id, existing.state, session.state
            ));
        }
        let mut session = session.clone();
        let state_changed = !session.state.eq_ignore_ascii_case(&existing.state);
        let updated_at = sdkwork_agent_database::runtime_now_timestamp();
        session.updated_at = Some(updated_at.clone());
        ensure_session_row_lifecycle_timestamps(&mut session, &updated_at)?;
        let event_type = if state_changed && session.state.eq_ignore_ascii_case("closed") {
            "session.closed"
        } else {
            "session.updated"
        };
        let event = self.build_event(&session.session_id, event_type, "info", None);
        self.db
            .save_session_with_event(&session, &event)
            .map_err(|e| format!("failed to update session: {}", e))?;
        self.notify_event(event);
        Ok(())
    }

    /// Atomically upsert a provider session snapshot into unified runtime state.
    pub fn synchronize_provider_session(
        &self,
        provider_id: &str,
        bridge_id: Option<&str>,
        session: &AgentSession,
    ) -> Result<SessionRow, String> {
        if provider_id.trim().is_empty() {
            return Err("provider_id must not be empty".to_string());
        }
        if session.session_id.trim().is_empty() {
            return Err("session_id must not be empty".to_string());
        }

        let existing = self
            .db
            .load_session(&session.session_id)
            .map_err(|e| format!("failed to load provider session: {e}"))?;
        if let Some(existing_provider) =
            existing.as_ref().and_then(|row| row.provider_id.as_deref())
        {
            if existing_provider != provider_id {
                return Err(format!(
                    "session {} already belongs to provider {}",
                    session.session_id, existing_provider
                ));
            }
        }

        let row = provider_session_to_row(provider_id, bridge_id, session, existing.as_ref())?;
        if let Some(existing) = existing.as_ref() {
            if sdkwork_agent_database::session_state_regresses_from_terminal(&row, existing) {
                return Err(format!(
                    "cannot transition terminal session {} from {} to {}",
                    row.session_id, existing.state, row.state
                ));
            }
            if session_row_is_older(&row, existing) {
                return Ok(existing.clone());
            }
            if session_rows_equivalent(&row, existing) {
                return Ok(existing.clone());
            }
        }
        let event = self.build_event(
            &row.session_id,
            "session.synchronized",
            "info",
            Some(provider_id),
        );
        let applied = self
            .db
            .save_session_with_event_if_newer(&row, &event)
            .map_err(|e| format!("failed to synchronize provider session: {e}"))?;
        if applied {
            self.notify_event(event);
            Ok(row)
        } else {
            let current = self.get_session(&row.session_id)?;
            if let Some(existing_provider) = current.provider_id.as_deref() {
                if existing_provider != provider_id {
                    return Err(format!(
                        "session {} already belongs to provider {}",
                        row.session_id, existing_provider
                    ));
                }
            }
            if sdkwork_agent_database::session_state_regresses_from_terminal(&row, &current) {
                return Err(format!(
                    "cannot transition terminal session {} from {} to {}",
                    row.session_id, current.state, row.state
                ));
            }
            Ok(current)
        }
    }

    /// Close a session
    pub fn close_session(&self, session_id: &str) -> Result<SessionRow, String> {
        let mut session = self.get_session(session_id)?;
        if session.state.eq_ignore_ascii_case("closed") {
            return Ok(session);
        }
        if matches!(
            session.state.to_ascii_lowercase().as_str(),
            "failed" | "archived"
        ) {
            return Err(format!(
                "cannot close terminal session {} from state {}",
                session_id, session.state
            ));
        }
        session.state = "closed".to_string();
        let updated_at = sdkwork_agent_database::runtime_now_timestamp();
        session.updated_at = Some(updated_at.clone());
        ensure_session_row_lifecycle_timestamps(&mut session, &updated_at)?;

        let event = self.build_event(session_id, "session.closed", "info", None);
        self.db
            .save_session_with_event(&session, &event)
            .map_err(|e| format!("failed to close session: {e}"))?;
        self.notify_event(event);

        Ok(session)
    }

    /// Delete a session and all associated data
    pub fn delete_session(&self, session_id: &str) -> Result<(), String> {
        self.get_session(session_id)?;
        let event = self.build_event(session_id, "session.deleted", "info", None);
        self.db
            .delete_session_cascade(session_id)
            .map_err(|e| format!("failed to delete session: {}", e))?;
        self.notify_event(event);
        Ok(())
    }

    /// Send a message in a session
    pub fn send_message(
        &self,
        session_id: &str,
        config: MessageConfig,
    ) -> Result<MessageRow, String> {
        let session = self.get_session(session_id)?;
        if sdkwork_agent_database::session_state_is_terminal(&session.state) {
            return Err(format!(
                "session {session_id} is terminal ({})",
                session.state
            ));
        }

        let now = sdkwork_agent_database::runtime_now_timestamp();
        let message_id = format!("msg.{}", generate_id());

        let row = MessageRow {
            message_id,
            session_id: session_id.to_string(),
            role: config.role,
            content: config.content,
            created_at: now,
            metadata_json: config
                .metadata
                .as_ref()
                .and_then(|value| serde_json::to_string(value).ok()),
        };

        let event = EventRow {
            event_id: format!("evt.{}", generate_id()),
            session_id: Some(session_id.to_string()),
            event_type: "message.sent".to_string(),
            severity: "info".to_string(),
            payload: Some(format!("role={}", row.role)),
            created_at: sdkwork_agent_database::runtime_now_timestamp(),
        };

        self.db
            .append_message_with_event(&row, &event)
            .map_err(|e| format!("failed to append message: {}", e))?;

        if let Some(listener) = &self.event_listener {
            listener(event);
        }

        Ok(row)
    }

    /// Atomically persist a completed user turn, optional assistant reply, and lifecycle events.
    pub fn append_completed_turn(
        &self,
        session_id: &str,
        user_content: String,
        assistant_content: Option<String>,
    ) -> Result<(MessageRow, Option<MessageRow>), String> {
        let session = self.get_session(session_id)?;
        if !session.state.eq_ignore_ascii_case("active") {
            return Err(format!("session {session_id} is not active"));
        }

        let turn_started = sdkwork_utils_rust::now();
        let user_row = MessageRow {
            message_id: format!("msg.{}", generate_id()),
            session_id: session_id.to_string(),
            role: "user".to_string(),
            content: user_content,
            created_at: sdkwork_agent_database::format_runtime_timestamp(turn_started),
            metadata_json: None,
        };
        let assistant_row = assistant_content.map(|content| MessageRow {
            message_id: format!("msg.{}", generate_id()),
            session_id: session_id.to_string(),
            role: "assistant".to_string(),
            content,
            created_at: sdkwork_agent_database::format_runtime_timestamp(
                turn_started + chrono::Duration::nanoseconds(1),
            ),
            metadata_json: None,
        });

        let mut messages = vec![user_row.clone()];
        if let Some(row) = &assistant_row {
            messages.push(row.clone());
        }
        let mut events = Vec::with_capacity(messages.len() + 1);
        for (index, message) in messages.iter().enumerate() {
            events.push(EventRow {
                event_id: format!("evt.{}", generate_id()),
                session_id: Some(session_id.to_string()),
                event_type: "message.sent".to_string(),
                severity: "info".to_string(),
                payload: Some(format!("role={}", message.role)),
                created_at: sdkwork_agent_database::format_runtime_timestamp(
                    turn_started
                        + chrono::Duration::nanoseconds(2 + i64::try_from(index).unwrap_or(0)),
                ),
            });
        }
        events.push(EventRow {
            event_id: format!("evt.{}", generate_id()),
            session_id: Some(session_id.to_string()),
            event_type: "turn.completed".to_string(),
            severity: "info".to_string(),
            payload: Some(format!("user_message_id={}", user_row.message_id)),
            created_at: sdkwork_agent_database::format_runtime_timestamp(
                turn_started
                    + chrono::Duration::nanoseconds(2 + i64::try_from(messages.len()).unwrap_or(0)),
            ),
        });

        self.db
            .append_message_turn_with_events(&messages, &events)
            .map_err(|e| format!("failed to append completed message turn: {e}"))?;
        for event in events {
            self.notify_event(event);
        }

        Ok((user_row, assistant_row))
    }

    /// Delete all messages in a session and reset the cached message count.
    pub fn delete_messages(&self, session_id: &str) -> Result<(), String> {
        self.get_session(session_id)?;
        let updated_at = sdkwork_agent_database::runtime_now_timestamp();
        let event = self.build_event(
            session_id,
            "session.updated",
            "info",
            Some("messages_cleared=true"),
        );
        self.db
            .delete_messages_and_reset_count_with_event(session_id, &updated_at, &event)
            .map_err(|e| format!("failed to delete messages: {}", e))?;
        self.notify_event(event);
        Ok(())
    }

    /// List messages with full query parameters (offset or keyset continuation).
    pub fn list_messages(
        &self,
        session_id: &str,
        query: sdkwork_agent_database::MessageQuery,
    ) -> Result<Vec<MessageRow>, String> {
        self.db
            .load_messages(session_id, &query)
            .map_err(|e| format!("failed to load messages: {}", e))
    }

    /// Get message history for a session
    pub fn get_messages(
        &self,
        session_id: &str,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Vec<MessageRow>, String> {
        let query = sdkwork_agent_database::MessageQuery {
            limit,
            offset,
            ..Default::default()
        };
        self.db
            .load_messages(session_id, &query)
            .map_err(|e| format!("failed to load messages: {}", e))
    }

    /// Load the most recent bounded message window in chronological order.
    pub fn load_recent_messages(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<MessageRow>, String> {
        self.db
            .load_recent_messages(session_id, limit)
            .map_err(|e| format!("failed to load recent messages: {e}"))
    }

    /// Get message count for a session
    pub fn message_count(&self, session_id: &str) -> Result<i64, String> {
        self.db
            .message_count(session_id)
            .map_err(|e| format!("failed to count messages: {}", e))
    }

    /// Get a conversation manager for a session
    pub fn conversation(&self) -> ConversationManager<DB> {
        let mut conversation = ConversationManager::new(self.db.clone());
        if let Some(listener) = &self.event_listener {
            conversation.set_event_listener(listener.clone());
        }
        conversation
    }

    /// Emit a persisted session event to storage and optional listeners.
    pub fn emit_session_event(
        &self,
        session_id: &str,
        event_type: &str,
        severity: &str,
        payload: Option<&str>,
    ) -> Result<(), String> {
        self.record_event(session_id, event_type, severity, payload)
    }

    /// Record an event
    fn record_event(
        &self,
        session_id: &str,
        event_type: &str,
        severity: &str,
        payload: Option<&str>,
    ) -> Result<(), String> {
        self.get_session(session_id)?;
        let event = self.build_event(session_id, event_type, severity, payload);

        self.db
            .save_event(&event)
            .map_err(|e| format!("failed to save event: {}", e))?;

        self.notify_event(event);

        Ok(())
    }

    fn build_event(
        &self,
        session_id: &str,
        event_type: &str,
        severity: &str,
        payload: Option<&str>,
    ) -> EventRow {
        EventRow {
            event_id: format!("evt.{}", generate_id()),
            session_id: Some(session_id.to_string()),
            event_type: event_type.to_string(),
            severity: severity.to_string(),
            payload: payload.map(str::to_string),
            created_at: sdkwork_agent_database::runtime_now_timestamp(),
        }
    }

    fn notify_event(&self, event: EventRow) {
        if let Some(listener) = &self.event_listener {
            listener(event);
        }
    }

    /// Create a task in a session.
    pub fn create_task(&self, session_id: &str, instruction: &str) -> Result<TaskRow, String> {
        let session = self.get_session(session_id)?;
        if sdkwork_agent_database::session_state_is_terminal(&session.state) {
            return Err(format!(
                "cannot create a task for terminal session {session_id} ({})",
                session.state
            ));
        }
        let now = sdkwork_agent_database::runtime_now_timestamp();
        let task = TaskRow {
            task_id: format!("task.{}", generate_id()),
            session_id: session_id.to_string(),
            instruction: instruction.to_string(),
            state: "created".to_string(),
            created_at: now.clone(),
            updated_at: Some(now),
        };
        let event = self.build_event(session_id, "task.created", "info", Some(&task.task_id));
        self.db
            .save_task_with_event(&task, &event)
            .map_err(|e| format!("failed to create task: {e}"))?;
        self.notify_event(event);
        Ok(task)
    }

    /// Load a task by id.
    pub fn get_task(&self, task_id: &str) -> Result<TaskRow, String> {
        self.db
            .load_task(task_id)
            .map_err(|e| format!("failed to load task: {}", e))?
            .ok_or_else(|| format!("task not found: {}", task_id))
    }

    /// List tasks for a session.
    pub fn list_tasks(
        &self,
        session_id: &str,
        query: sdkwork_agent_database::TaskQuery,
    ) -> Result<Vec<TaskRow>, String> {
        self.get_session(session_id)?;
        self.db
            .load_tasks(session_id, &query)
            .map_err(|e| format!("failed to load tasks: {}", e))
    }

    /// Cancel a task.
    pub fn cancel_task(&self, task_id: &str) -> Result<TaskRow, String> {
        let task = self.get_task(task_id)?;
        let updated_at = sdkwork_agent_database::runtime_now_timestamp();
        let event = self.build_event(
            &task.session_id,
            "task.cancelled",
            "info",
            Some(&task.task_id),
        );
        let (task, changed) = self
            .db
            .cancel_task_with_event(task_id, &updated_at, &event)
            .map_err(|e| format!("failed to cancel task: {e}"))?;
        if changed {
            self.notify_event(event);
        }
        Ok(task)
    }

    /// Load recent events for a session.
    pub fn load_session_events(
        &self,
        session_id: &str,
        limit: Option<i64>,
        after_event_id: Option<&str>,
    ) -> Result<Vec<EventRow>, String> {
        self.get_session(session_id)?;
        let query = sdkwork_agent_database::EventQuery {
            event_type: None,
            severity: None,
            owner_tenant_id: None,
            owner_user_ref: None,
            after_event_id: after_event_id.map(str::to_string),
            limit,
            offset: None,
        };
        self.db
            .load_events(session_id, &query)
            .map_err(|e| format!("failed to load events: {}", e))
    }

    /// Load recent events across all sessions (newest first).
    pub fn list_recent_events(
        &self,
        query: sdkwork_agent_database::EventQuery,
    ) -> Result<Vec<EventRow>, String> {
        self.db
            .list_recent_events(&query)
            .map_err(|e| format!("failed to list recent events: {}", e))
    }

    /// Health check
    pub fn health(&self) -> Result<bool, String> {
        self.db
            .health()
            .map_err(|e| format!("health check failed: {}", e))
    }
}

/// Generate a collision-resistant runtime ID.
fn generate_id() -> String {
    sdkwork_utils_rust::uuid()
}

fn ensure_session_row_lifecycle_timestamps(
    session: &mut SessionRow,
    updated_at: &str,
) -> Result<(), String> {
    if !sdkwork_agent_database::session_state_is_terminal(&session.state) {
        return Ok(());
    }
    let mut metadata: HashMap<String, String> = session
        .metadata_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| format!("failed to parse session metadata: {error}"))?
        .unwrap_or_default();
    metadata
        .entry("endedAt".to_string())
        .or_insert_with(|| updated_at.to_string());
    if session.state.eq_ignore_ascii_case("archived") {
        metadata
            .entry("archivedAt".to_string())
            .or_insert_with(|| updated_at.to_string());
    }
    session.metadata_json = Some(
        serde_json::to_string(&metadata)
            .map_err(|error| format!("failed to serialize session metadata: {error}"))?,
    );
    Ok(())
}

fn provider_session_to_row(
    provider_id: &str,
    bridge_id: Option<&str>,
    session: &AgentSession,
    existing: Option<&SessionRow>,
) -> Result<SessionRow, String> {
    let now = sdkwork_agent_database::runtime_now_timestamp();
    let created_at = normalized_provider_timestamp(session.created_at.as_deref(), "created_at")?;
    let updated_at = normalized_provider_timestamp(session.updated_at.as_deref(), "updated_at")?;
    let mut ended_at = normalized_provider_timestamp(session.ended_at.as_deref(), "ended_at")?;
    let mut archived_at =
        normalized_provider_timestamp(session.archived_at.as_deref(), "archived_at")?;
    let existing_metadata: HashMap<String, String> = existing
        .and_then(|row| row.metadata_json.as_deref())
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default();
    let parent_session_id = stable_provider_session_reference(
        session.parent_session_id.as_deref(),
        existing_metadata.get("parentSessionId").map(String::as_str),
        &session.session_id,
        "parent_session_id",
    )?;
    let forked_from_id = stable_provider_session_reference(
        session.forked_from_id.as_deref(),
        existing_metadata.get("forkedFromId").map(String::as_str),
        &session.session_id,
        "forked_from_id",
    )?;
    let child_session_ids = normalized_provider_child_session_ids(session)?;
    ended_at = ended_at.or_else(|| existing_metadata.get("endedAt").cloned());
    archived_at = archived_at.or_else(|| existing_metadata.get("archivedAt").cloned());
    let effective_created_at = existing
        .map(|row| row.created_at.clone())
        .or(created_at)
        .unwrap_or_else(|| now.clone());
    let effective_updated_at = updated_at
        .or_else(|| existing.and_then(|row| row.updated_at.clone()))
        .unwrap_or_else(|| now.clone());
    if session.state.is_terminal() && ended_at.is_none() {
        ended_at = Some(effective_updated_at.clone());
    }
    if session.state == sdkwork_agent_kernel::SessionState::Archived && archived_at.is_none() {
        archived_at = Some(effective_updated_at.clone());
    }
    validate_runtime_session_timestamp_order(
        &effective_created_at,
        &effective_updated_at,
        ended_at.as_deref(),
        archived_at.as_deref(),
    )?;
    let tool_call_count = session
        .tool_call_count
        .max(metadata_u32(&existing_metadata, "toolCallCount"));
    let compression_count = session
        .compression_count
        .max(metadata_u32(&existing_metadata, "compressionCount"));
    let cost_cents = session
        .cost_cents
        .into_iter()
        .chain(metadata_u64_optional(&existing_metadata, "costCents"))
        .max();
    let existing_change_summary = existing_metadata
        .get("changeSummary")
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok());
    let change_additions = session.change_summary.additions.max(
        existing_change_summary
            .as_ref()
            .and_then(|value| value.get("additions"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0),
    );
    let change_deletions = session.change_summary.deletions.max(
        existing_change_summary
            .as_ref()
            .and_then(|value| value.get("deletions"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0),
    );
    let files_changed = session.change_summary.files_changed.max(
        existing_change_summary
            .as_ref()
            .and_then(|value| value.get("filesChanged"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0),
    );
    let mut metadata: HashMap<String, String> = session.metadata.iter().cloned().collect();
    for key in [
        "providerId",
        "providerSessionId",
        "parentSessionId",
        "forkedFromId",
        "slug",
        "userRef",
        "tenantId",
        "ownerUserRef",
        "ownerTenantId",
        "preview",
        "goal",
        "summary",
        "endedAt",
        "archivedAt",
        "modelProvider",
        "instructions",
        "personality",
        "reasoningEffort",
        "approvalPolicy",
        "permissionProfile",
        "workspaceRoots",
        "costCents",
        "contextFrom",
        "contextWatermark",
        "summaryMessageId",
        "childSessionIds",
        "agentNickname",
        "agentRole",
        "timeoutMs",
        "toolCallCount",
        "compressionCount",
        "changeSummary",
    ] {
        metadata.remove(key);
    }
    metadata.insert("providerId".to_string(), provider_id.to_string());
    metadata.insert("providerSessionId".to_string(), session.session_id.clone());
    insert_optional(
        &mut metadata,
        "parentSessionId",
        parent_session_id.as_deref(),
    );
    insert_optional(&mut metadata, "forkedFromId", forked_from_id.as_deref());
    insert_optional(&mut metadata, "slug", session.slug.as_deref());
    insert_optional(&mut metadata, "userRef", session.user_ref.as_deref());
    insert_optional(&mut metadata, "tenantId", session.tenant_id.as_deref());
    insert_optional(&mut metadata, "ownerUserRef", session.user_ref.as_deref());
    insert_optional(&mut metadata, "ownerTenantId", session.tenant_id.as_deref());
    insert_optional(&mut metadata, "preview", session.preview.as_deref());
    insert_optional(&mut metadata, "goal", session.goal.as_deref());
    insert_optional(&mut metadata, "summary", session.summary.as_deref());
    insert_optional(&mut metadata, "endedAt", ended_at.as_deref());
    insert_optional(&mut metadata, "archivedAt", archived_at.as_deref());
    insert_optional(
        &mut metadata,
        "modelProvider",
        session.model_provider.as_deref(),
    );
    insert_optional(
        &mut metadata,
        "instructions",
        session.instructions.as_deref(),
    );
    insert_optional(&mut metadata, "personality", session.personality.as_deref());
    insert_optional(
        &mut metadata,
        "reasoningEffort",
        session.reasoning_effort.as_deref(),
    );
    insert_optional(
        &mut metadata,
        "approvalPolicy",
        session.approval_policy.as_deref(),
    );
    insert_optional(
        &mut metadata,
        "permissionProfile",
        session.permission_profile.as_deref(),
    );
    if !session.workspace_roots.is_empty() {
        metadata.insert(
            "workspaceRoots".to_string(),
            serde_json::to_string(&session.workspace_roots)
                .map_err(|e| format!("failed to serialize workspace roots: {e}"))?,
        );
    }
    if !child_session_ids.is_empty() {
        metadata.insert(
            "childSessionIds".to_string(),
            serde_json::to_string(&child_session_ids)
                .map_err(|e| format!("failed to serialize child sessions: {e}"))?,
        );
    }
    if let Some(cost_cents) = cost_cents {
        metadata.insert("costCents".to_string(), cost_cents.to_string());
    }
    insert_optional(
        &mut metadata,
        "contextFrom",
        session.context_from.as_deref(),
    );
    insert_optional(
        &mut metadata,
        "contextWatermark",
        session.context_watermark.as_deref(),
    );
    insert_optional(
        &mut metadata,
        "summaryMessageId",
        session.summary_message_id.as_deref(),
    );
    insert_optional(
        &mut metadata,
        "agentNickname",
        session.agent_nickname.as_deref(),
    );
    insert_optional(&mut metadata, "agentRole", session.agent_role.as_deref());
    if let Some(timeout_ms) = session.timeout_ms {
        metadata.insert("timeoutMs".to_string(), timeout_ms.to_string());
    }
    metadata.insert("toolCallCount".to_string(), tool_call_count.to_string());
    metadata.insert(
        "compressionCount".to_string(),
        compression_count.to_string(),
    );
    metadata.insert(
        "changeSummary".to_string(),
        serde_json::json!({
            "additions": change_additions,
            "deletions": change_deletions,
            "filesChanged": files_changed,
        })
        .to_string(),
    );

    let metadata_json = serde_json::to_string(&metadata)
        .map_err(|e| format!("failed to serialize provider session metadata: {e}"))?;
    let existing_token_usage = existing
        .and_then(|row| row.token_usage_json.as_deref())
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok());
    let existing_token = |key| {
        existing_token_usage
            .as_ref()
            .and_then(|value| value.get(key))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let input_tokens = session
        .token_usage
        .input_tokens
        .max(existing_token("inputTokens"));
    let output_tokens = session
        .token_usage
        .output_tokens
        .max(existing_token("outputTokens"));
    let cached_tokens = session
        .token_usage
        .cached_tokens
        .max(existing_token("cachedTokens"));
    let reasoning_tokens = session
        .token_usage
        .reasoning_tokens
        .max(existing_token("reasoningTokens"));
    let counted_total = input_tokens
        .checked_add(output_tokens)
        .ok_or_else(|| "provider session token usage overflow".to_string())?;
    let total_tokens = session
        .token_usage
        .total_tokens
        .max(existing_token("totalTokens"))
        .max(counted_total);
    let token_usage_json = serde_json::json!({
        "inputTokens": input_tokens,
        "outputTokens": output_tokens,
        "cachedTokens": cached_tokens,
        "reasoningTokens": reasoning_tokens,
        "totalTokens": total_tokens,
    })
    .to_string();
    let (owner_tenant_id, owner_user_ref) =
        session_owner_fields_from_metadata_json(&Some(metadata_json.clone()));

    Ok(SessionRow {
        session_id: session.session_id.clone(),
        agent_id: session
            .agent_id
            .clone()
            .or_else(|| existing.map(|row| row.agent_id.clone()))
            .unwrap_or_else(|| format!("agent.{provider_id}")),
        kind: session.kind.as_str().to_string(),
        source: session.source.as_str().to_string(),
        state: session.state.as_str().to_string(),
        title: session.title.clone(),
        model: session.model.clone(),
        cwd: session.cwd.clone(),
        provider_id: Some(provider_id.to_string()),
        bridge_id: bridge_id
            .map(str::to_string)
            .or_else(|| existing.and_then(|row| row.bridge_id.clone())),
        token_usage_json: Some(token_usage_json),
        message_count: i64::from(session.message_count)
            .max(existing.map(|row| row.message_count).unwrap_or(0)),
        owner_tenant_id,
        owner_user_ref,
        created_at: effective_created_at,
        updated_at: Some(effective_updated_at),
        metadata_json: Some(metadata_json),
    })
}

fn insert_optional(metadata: &mut HashMap<String, String>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        metadata.insert(key.to_string(), value.to_string());
    }
}

fn metadata_u32(metadata: &HashMap<String, String>, key: &str) -> u32 {
    metadata
        .get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn metadata_u64_optional(metadata: &HashMap<String, String>, key: &str) -> Option<u64> {
    metadata.get(key).and_then(|value| value.parse().ok())
}

fn stable_provider_session_reference(
    incoming: Option<&str>,
    existing: Option<&str>,
    session_id: &str,
    field: &str,
) -> Result<Option<String>, String> {
    let incoming = incoming.map(str::trim).filter(|value| !value.is_empty());
    if incoming == Some(session_id) {
        return Err(format!("{field} must not reference the session itself"));
    }
    if let Some(existing) = existing {
        if incoming.is_some_and(|incoming| incoming != existing) {
            return Err(format!("{field} cannot change from {existing}"));
        }
        return Ok(Some(existing.to_string()));
    }
    Ok(incoming.map(str::to_string))
}

fn normalized_provider_child_session_ids(session: &AgentSession) -> Result<Vec<String>, String> {
    let mut seen = std::collections::HashSet::with_capacity(session.child_session_ids.len());
    let mut normalized = Vec::with_capacity(session.child_session_ids.len());
    for child_session_id in &session.child_session_ids {
        let child_session_id = child_session_id.trim();
        if child_session_id.is_empty() {
            return Err("child_session_ids must not contain an empty session id".to_string());
        }
        if child_session_id == session.session_id {
            return Err("child_session_ids must not reference the session itself".to_string());
        }
        if seen.insert(child_session_id.to_string()) {
            normalized.push(child_session_id.to_string());
        }
    }
    Ok(normalized)
}

fn normalized_provider_timestamp(
    value: Option<&str>,
    field: &str,
) -> Result<Option<String>, String> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let parsed = sdkwork_utils_rust::parse_datetime(value, None)
        .ok_or_else(|| format!("{field} must be an RFC 3339 timestamp"))?;
    Ok(Some(sdkwork_utils_rust::format_datetime(
        parsed,
        Some("%Y-%m-%dT%H:%M:%S%.9fZ"),
    )))
}

fn validate_runtime_session_timestamp_order(
    created_at: &str,
    updated_at: &str,
    ended_at: Option<&str>,
    archived_at: Option<&str>,
) -> Result<(), String> {
    let parse = |field: &str, value: &str| {
        sdkwork_utils_rust::parse_datetime(value, None)
            .ok_or_else(|| format!("{field} must be an RFC 3339 timestamp"))
    };
    let created_at = parse("created_at", created_at)?;
    let updated_at = parse("updated_at", updated_at)?;
    let ended_at = ended_at.map(|value| parse("ended_at", value)).transpose()?;
    let archived_at = archived_at
        .map(|value| parse("archived_at", value))
        .transpose()?;
    if updated_at < created_at {
        return Err("updated_at must not be earlier than created_at".to_string());
    }
    if ended_at.is_some_and(|ended_at| ended_at < created_at) {
        return Err("ended_at must not be earlier than created_at".to_string());
    }
    if archived_at.is_some_and(|archived_at| archived_at < created_at) {
        return Err("archived_at must not be earlier than created_at".to_string());
    }
    if matches!((ended_at, archived_at), (Some(ended), Some(archived)) if archived < ended) {
        return Err("archived_at must not be earlier than ended_at".to_string());
    }
    Ok(())
}

fn session_row_is_older(incoming: &SessionRow, existing: &SessionRow) -> bool {
    let incoming_at = incoming
        .updated_at
        .as_deref()
        .unwrap_or(incoming.created_at.as_str());
    let existing_at = existing
        .updated_at
        .as_deref()
        .unwrap_or(existing.created_at.as_str());
    match (
        sdkwork_utils_rust::parse_datetime(incoming_at, None),
        sdkwork_utils_rust::parse_datetime(existing_at, None),
    ) {
        (Some(incoming_at), Some(existing_at)) => incoming_at < existing_at,
        _ => false,
    }
}

fn session_rows_equivalent(left: &SessionRow, right: &SessionRow) -> bool {
    fn normalized_json(raw: Option<&str>) -> Option<serde_json::Value> {
        raw.and_then(|value| serde_json::from_str(value).ok())
    }

    left.session_id == right.session_id
        && left.agent_id == right.agent_id
        && left.kind == right.kind
        && left.source == right.source
        && left.state == right.state
        && left.title == right.title
        && left.model == right.model
        && left.cwd == right.cwd
        && left.provider_id == right.provider_id
        && left.bridge_id == right.bridge_id
        && left.message_count == right.message_count
        && left.owner_tenant_id == right.owner_tenant_id
        && left.owner_user_ref == right.owner_user_ref
        && left.created_at == right.created_at
        && left.updated_at == right.updated_at
        && normalized_json(left.token_usage_json.as_deref())
            == normalized_json(right.token_usage_json.as_deref())
        && normalized_json(left.metadata_json.as_deref())
            == normalized_json(right.metadata_json.as_deref())
}

#[cfg(test)] // WORKSPACE-PATH:allow-fixture-block: this module is the file's #[cfg(test)] unit-test fixture data
mod tests {
    use super::*;
    use sdkwork_agent_database::InMemoryDatabase;
    use sdkwork_agent_kernel::{SessionKind, SessionSource, SessionState};
    use std::sync::Mutex;

    fn create_manager() -> UnifiedSessionManager<InMemoryDatabase> {
        UnifiedSessionManager::new(InMemoryDatabase::new())
    }

    #[test]
    fn create_and_get_session() {
        let manager = create_manager();
        let config = SessionConfig::new("agent.1").with_title("Test Session");

        let session = manager.create_session(config).expect("created");
        assert_eq!(session.agent_id, "agent.1");
        assert_eq!(session.title, Some("Test Session".to_string()));
        assert_eq!(session.state, "active");

        let loaded = manager.get_session(&session.session_id).expect("loaded");
        assert_eq!(loaded.session_id, session.session_id);
    }

    #[test]
    fn list_sessions() {
        let manager = create_manager();
        manager
            .create_session(SessionConfig::new("agent.1"))
            .expect("created");
        manager
            .create_session(SessionConfig::new("agent.2"))
            .expect("created");

        let sessions = manager
            .list_sessions(SessionQuery::default())
            .expect("listed");
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn close_session() {
        let manager = create_manager();
        let session = manager
            .create_session(SessionConfig::new("agent.1"))
            .expect("created");
        let closed = manager.close_session(&session.session_id).expect("closed");
        assert_eq!(closed.state, "closed");
        let metadata: HashMap<String, String> =
            serde_json::from_str(closed.metadata_json.as_deref().expect("metadata"))
                .expect("metadata JSON");
        assert_eq!(
            metadata.get("endedAt").map(String::as_str),
            closed.updated_at.as_deref()
        );
        manager
            .close_session(&session.session_id)
            .expect("repeated close is idempotent");
        let events = manager
            .load_session_events(&session.session_id, Some(20), None)
            .expect("events");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "session.closed")
                .count(),
            1
        );
    }

    #[test]
    fn generic_update_preserves_terminal_lifecycle_semantics() {
        let manager = create_manager();
        let mut session = manager
            .create_session(SessionConfig::new("agent.1"))
            .expect("created");
        session.state = "closed".to_string();
        manager
            .update_session(&session)
            .expect("close through update");
        let mut closed = manager.get_session(&session.session_id).expect("closed");
        let metadata: HashMap<String, String> =
            serde_json::from_str(closed.metadata_json.as_deref().expect("metadata"))
                .expect("metadata JSON");
        let ended_at = metadata.get("endedAt").cloned().expect("endedAt");
        assert_eq!(
            manager
                .load_session_events(&session.session_id, Some(20), None)
                .expect("events")
                .iter()
                .filter(|event| event.event_type == "session.closed")
                .count(),
            1
        );

        closed.title = Some("retitled".to_string());
        manager
            .update_session(&closed)
            .expect("same-state terminal update");
        let updated = manager.get_session(&session.session_id).expect("updated");
        let updated_metadata: HashMap<String, String> =
            serde_json::from_str(updated.metadata_json.as_deref().expect("metadata"))
                .expect("metadata JSON");
        assert_eq!(updated_metadata.get("endedAt"), Some(&ended_at));
        assert_eq!(
            manager
                .load_session_events(&session.session_id, Some(20), None)
                .expect("events")
                .iter()
                .filter(|event| event.event_type == "session.closed")
                .count(),
            1
        );
    }

    #[test]
    fn send_message() {
        let manager = create_manager();
        let session = manager
            .create_session(SessionConfig::new("agent.1"))
            .expect("created");

        let msg = manager
            .send_message(&session.session_id, MessageConfig::user("Hello"))
            .expect("sent");

        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, "Hello");

        let messages = manager
            .get_messages(&session.session_id, None, None)
            .expect("loaded");
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn append_completed_turn_persists_messages_and_events_together() {
        let manager = create_manager();
        let session = manager
            .create_session(SessionConfig::new("agent.1"))
            .expect("created");

        let (user, assistant) = manager
            .append_completed_turn(
                &session.session_id,
                "Hello".to_string(),
                Some("Hi".to_string()),
            )
            .expect("completed turn");
        assert_eq!(user.role, "user");
        assert_eq!(
            assistant.as_ref().map(|row| row.role.as_str()),
            Some("assistant")
        );

        let messages = manager
            .get_messages(&session.session_id, None, None)
            .expect("messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].message_id, user.message_id);
        assert_eq!(
            messages[1].message_id,
            assistant.expect("assistant").message_id
        );
        assert_eq!(
            manager.message_count(&session.session_id).expect("count"),
            2
        );

        let events = manager
            .load_session_events(&session.session_id, Some(20), None)
            .expect("events");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "message.sent")
                .count(),
            2
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "turn.completed")
                .count(),
            1
        );
    }

    #[test]
    fn delete_session() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let mut manager = create_manager();
        manager.set_event_listener(Arc::new(move |event| {
            captured.lock().expect("events lock").push(event);
        }));
        let session = manager
            .create_session(SessionConfig::new("agent.1"))
            .expect("created");
        manager
            .send_message(&session.session_id, MessageConfig::user("Hello"))
            .expect("sent");

        manager
            .delete_session(&session.session_id)
            .expect("deleted");

        let result = manager.get_session(&session.session_id);
        assert!(result.is_err());
        assert!(events
            .lock()
            .expect("events lock")
            .iter()
            .any(|event| event.event_type == "session.deleted"));
    }

    #[test]
    fn synchronizes_provider_session_into_unified_store() {
        let manager = create_manager();
        let mut provider_session = AgentSession::new("codex.thread.1")
            .with_agent_id("agent.codex")
            .with_user_ref("user.1")
            .with_tenant_id("tenant.1")
            .with_source(SessionSource::Cli)
            .with_kind(SessionKind::Main)
            .with_title("Native thread")
            .with_model("gpt-5-codex")
            .created_at("2026-07-15T00:00:00Z");
        provider_session.updated_at = Some("2026-07-15T00:01:00Z".to_string());
        provider_session.parent_session_id = Some("codex.thread.parent".to_string());
        provider_session.forked_from_id = Some("codex.thread.source".to_string());
        provider_session.slug = Some("native-thread".to_string());
        provider_session.preview = Some("preview".to_string());
        provider_session.goal = Some("goal".to_string());
        provider_session.summary = Some("summary".to_string());
        provider_session.ended_at = Some("2026-07-15T08:03:00+08:00".to_string());
        provider_session.archived_at = Some("2026-07-15T08:04:00+08:00".to_string());
        provider_session.model_provider = Some("openai".to_string());
        provider_session.instructions = Some("instructions".to_string());
        provider_session.personality = Some("precise".to_string());
        provider_session.reasoning_effort = Some("high".to_string());
        provider_session.approval_policy = Some("on-request".to_string());
        provider_session.permission_profile = Some("workspace-write".to_string());
        provider_session.workspace_roots = vec!["E:/workspace".to_string()];
        provider_session.token_usage.input_tokens = 11;
        provider_session.token_usage.output_tokens = 7;
        provider_session.token_usage.cached_tokens = 3;
        provider_session.token_usage.reasoning_tokens = 5;
        provider_session.token_usage.total_tokens = 18;
        provider_session.tool_call_count = 4;
        provider_session.compression_count = 2;
        provider_session.cost_cents = Some(99);
        provider_session.change_summary.additions = 21;
        provider_session.change_summary.deletions = 8;
        provider_session.change_summary.files_changed = 3;
        provider_session.context_from = Some("msg.context".to_string());
        provider_session.context_watermark = Some("watermark.1".to_string());
        provider_session.summary_message_id = Some("msg.summary".to_string());
        provider_session.child_session_ids = vec![
            " codex.thread.child ".to_string(),
            "codex.thread.child".to_string(),
        ];
        provider_session.agent_nickname = Some("builder".to_string());
        provider_session.agent_role = Some("worker".to_string());
        provider_session.timeout_ms = Some(30_000);
        provider_session
            .metadata
            .push(("providerId".to_string(), "spoofed".to_string()));
        provider_session.state = SessionState::Working;
        provider_session.message_count = 12;

        let row = manager
            .synchronize_provider_session("codex", Some("bridge.codex"), &provider_session)
            .expect("synchronized");
        assert_eq!(row.session_id, "codex.thread.1");
        assert_eq!(row.provider_id.as_deref(), Some("codex"));
        assert_eq!(row.bridge_id.as_deref(), Some("bridge.codex"));
        assert_eq!(row.state, "working");
        assert_eq!(row.message_count, 12);
        assert_eq!(row.owner_tenant_id.as_deref(), Some("tenant.1"));
        let metadata: HashMap<String, String> =
            serde_json::from_str(row.metadata_json.as_deref().expect("metadata"))
                .expect("metadata JSON");
        for (key, expected) in [
            ("providerId", "codex"),
            ("providerSessionId", "codex.thread.1"),
            ("parentSessionId", "codex.thread.parent"),
            ("forkedFromId", "codex.thread.source"),
            ("slug", "native-thread"),
            ("preview", "preview"),
            ("goal", "goal"),
            ("summary", "summary"),
            ("endedAt", "2026-07-15T00:03:00.000000000Z"),
            ("archivedAt", "2026-07-15T00:04:00.000000000Z"),
            ("modelProvider", "openai"),
            ("instructions", "instructions"),
            ("personality", "precise"),
            ("reasoningEffort", "high"),
            ("approvalPolicy", "on-request"),
            ("permissionProfile", "workspace-write"),
            ("costCents", "99"),
            ("contextFrom", "msg.context"),
            ("contextWatermark", "watermark.1"),
            ("summaryMessageId", "msg.summary"),
            ("agentNickname", "builder"),
            ("agentRole", "worker"),
            ("timeoutMs", "30000"),
            ("toolCallCount", "4"),
            ("compressionCount", "2"),
        ] {
            assert_eq!(metadata.get(key).map(String::as_str), Some(expected));
        }
        assert_eq!(
            metadata.get("workspaceRoots").map(String::as_str),
            Some("[\"E:/workspace\"]")
        );
        assert_eq!(
            metadata.get("childSessionIds").map(String::as_str),
            Some("[\"codex.thread.child\"]")
        );

        provider_session.updated_at = Some("2026-07-15T08:02:00+08:00".to_string());
        provider_session.created_at = Some("2027-01-01T00:00:00Z".to_string());
        provider_session.message_count = 18;
        provider_session.token_usage = Default::default();
        provider_session.tool_call_count = 0;
        provider_session.compression_count = 0;
        provider_session.cost_cents = None;
        provider_session.change_summary = Default::default();
        provider_session.parent_session_id = None;
        provider_session.forked_from_id = None;
        provider_session.ended_at = None;
        provider_session.archived_at = None;
        provider_session.tenant_id = Some("tenant.2".to_string());
        let updated = manager
            .synchronize_provider_session("codex", Some("bridge.codex"), &provider_session)
            .expect("updated provider aggregate");
        assert_eq!(updated.message_count, 18);
        assert_eq!(updated.owner_tenant_id.as_deref(), Some("tenant.2"));
        let updated_tokens: serde_json::Value = serde_json::from_str(
            updated
                .token_usage_json
                .as_deref()
                .expect("updated token usage"),
        )
        .expect("updated token JSON");
        assert_eq!(updated_tokens["inputTokens"], 11);
        assert_eq!(updated_tokens["outputTokens"], 7);
        assert_eq!(updated_tokens["cachedTokens"], 3);
        assert_eq!(updated_tokens["reasoningTokens"], 5);
        assert_eq!(updated_tokens["totalTokens"], 18);
        let updated_metadata: HashMap<String, String> =
            serde_json::from_str(updated.metadata_json.as_deref().expect("updated metadata"))
                .expect("updated metadata JSON");
        assert_eq!(
            updated_metadata.get("toolCallCount").map(String::as_str),
            Some("4")
        );
        assert_eq!(
            updated_metadata.get("compressionCount").map(String::as_str),
            Some("2")
        );
        assert_eq!(
            updated_metadata.get("costCents").map(String::as_str),
            Some("99")
        );
        let updated_change_summary: serde_json::Value = serde_json::from_str(
            updated_metadata
                .get("changeSummary")
                .expect("change summary"),
        )
        .expect("change summary JSON");
        assert_eq!(updated_change_summary["additions"], 21);
        assert_eq!(updated_change_summary["deletions"], 8);
        assert_eq!(updated_change_summary["filesChanged"], 3);
        assert_eq!(updated.created_at, row.created_at);
        assert_eq!(
            updated_metadata.get("parentSessionId").map(String::as_str),
            Some("codex.thread.parent")
        );
        assert_eq!(
            updated_metadata.get("forkedFromId").map(String::as_str),
            Some("codex.thread.source")
        );
        assert_eq!(
            updated_metadata.get("endedAt").map(String::as_str),
            Some("2026-07-15T00:03:00.000000000Z")
        );
        assert_eq!(
            updated_metadata.get("archivedAt").map(String::as_str),
            Some("2026-07-15T00:04:00.000000000Z")
        );
        assert_eq!(
            updated.updated_at.as_deref(),
            Some("2026-07-15T00:02:00.000000000Z")
        );
        assert_eq!(
            manager
                .get_session(&row.session_id)
                .expect("persisted aggregate")
                .message_count,
            18
        );

        let events = manager
            .load_session_events(&row.session_id, Some(20), None)
            .expect("events");
        assert!(events
            .iter()
            .any(|event| event.event_type == "session.synchronized"));

        provider_session.parent_session_id = Some("codex.thread.other".to_string());
        assert!(manager
            .synchronize_provider_session("codex", Some("bridge.codex"), &provider_session)
            .expect_err("lineage must not move")
            .contains("parent_session_id cannot change"));

        let mut self_parent = AgentSession::new("codex.thread.self");
        self_parent.parent_session_id = Some(self_parent.session_id.clone());
        assert!(manager
            .synchronize_provider_session("codex", None, &self_parent)
            .expect_err("self parent")
            .contains("must not reference the session itself"));

        let collision =
            manager.synchronize_provider_session("claude-code", None, &provider_session);
        assert!(collision
            .expect_err("provider collision must fail")
            .contains("already belongs to provider codex"));
    }

    #[test]
    fn unified_store_rejects_stale_provider_snapshot() {
        let manager = create_manager();
        let mut newer = AgentSession::new("opencode.session.1")
            .with_agent_id("agent.opencode")
            .created_at("2026-07-15T00:00:00Z");
        newer.updated_at = Some("2026-07-15T00:02:00Z".to_string());
        newer.state = SessionState::Working;
        manager
            .synchronize_provider_session("opencode", None, &newer)
            .expect("newer");

        let mut stale = newer.clone();
        stale.updated_at = Some("2026-07-15T00:01:00Z".to_string());
        stale.state = SessionState::Paused;
        let retained = manager
            .synchronize_provider_session("opencode", None, &stale)
            .expect("stale ignored");
        assert_eq!(retained.state, "working");
        assert_eq!(
            manager
                .load_session_events("opencode.session.1", Some(20), None)
                .expect("events")
                .iter()
                .filter(|event| event.event_type == "session.synchronized")
                .count(),
            1
        );
    }

    #[test]
    fn unified_store_rejects_terminal_provider_state_regression() {
        let manager = create_manager();
        let mut session = AgentSession::new("openclaw.session.terminal")
            .with_agent_id("agent.openclaw")
            .created_at("2026-07-15T00:00:00Z");
        session.updated_at = Some("2026-07-15T00:01:00Z".to_string());
        session.state = SessionState::Working;
        manager
            .synchronize_provider_session("openclaw", None, &session)
            .expect("working");

        session.updated_at = Some("2026-07-15T00:02:00Z".to_string());
        session.state = SessionState::Closed;
        let closed = manager
            .synchronize_provider_session("openclaw", None, &session)
            .expect("closed");
        let closed_metadata: HashMap<String, String> =
            serde_json::from_str(closed.metadata_json.as_deref().expect("closed metadata"))
                .expect("closed metadata JSON");
        assert_eq!(
            closed_metadata.get("endedAt").map(String::as_str),
            Some("2026-07-15T00:02:00.000000000Z")
        );

        session.updated_at = Some("2026-07-15T00:02:30Z".to_string());
        session.state = SessionState::Archived;
        let archived = manager
            .synchronize_provider_session("openclaw", None, &session)
            .expect("archived");
        let archived_metadata: HashMap<String, String> = serde_json::from_str(
            archived
                .metadata_json
                .as_deref()
                .expect("archived metadata"),
        )
        .expect("archived metadata JSON");
        assert_eq!(
            archived_metadata.get("endedAt").map(String::as_str),
            Some("2026-07-15T00:02:00.000000000Z")
        );
        assert_eq!(
            archived_metadata.get("archivedAt").map(String::as_str),
            Some("2026-07-15T00:02:30.000000000Z")
        );

        session.updated_at = Some("2026-07-15T00:03:00Z".to_string());
        session.state = SessionState::Active;
        assert!(manager
            .synchronize_provider_session("openclaw", None, &session)
            .expect_err("terminal regression")
            .contains("cannot transition terminal session"));
        assert_eq!(
            manager
                .get_session(&session.session_id)
                .expect("retained terminal state")
                .state,
            "archived"
        );
        let mut update = manager
            .get_session(&session.session_id)
            .expect("terminal update source");
        update.state = "active".to_string();
        assert!(manager
            .update_session(&update)
            .expect_err("generic update terminal regression")
            .contains("cannot transition terminal session"));
        update.state = "archived".to_string();
        update.provider_id = Some("codex".to_string());
        assert!(manager
            .update_session(&update)
            .expect_err("provider ownership mutation")
            .contains("cannot change provider ownership"));
        assert!(manager
            .send_message(&session.session_id, MessageConfig::user("late"))
            .expect_err("terminal message")
            .contains("terminal"));
        assert!(manager
            .create_task(&session.session_id, "late task")
            .expect_err("terminal task")
            .contains("terminal session"));
    }

    #[test]
    fn health_check() {
        let manager = create_manager();
        assert!(manager.health().expect("health"));
    }

    #[test]
    fn create_and_cancel_task() {
        let manager = create_manager();
        let session = manager
            .create_session(SessionConfig::new("agent.1"))
            .expect("created");
        let task = manager
            .create_task(&session.session_id, "run tests")
            .expect("task");
        assert_eq!(task.state, "created");
        let cancelled = manager.cancel_task(&task.task_id).expect("cancelled");
        assert_eq!(cancelled.state, "cancelled");
        let retried = manager
            .cancel_task(&task.task_id)
            .expect("repeated cancellation is idempotent");
        assert_eq!(retried.updated_at, cancelled.updated_at);
        let events = manager
            .load_session_events(&session.session_id, Some(20), None)
            .expect("events");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "task.cancelled")
                .count(),
            1
        );
    }

    #[test]
    fn replaying_identical_provider_snapshot_is_event_idempotent() {
        let manager = create_manager();
        let mut provider_session = AgentSession::new("codex.thread.replay")
            .with_agent_id("agent.codex")
            .created_at("2026-07-15T00:00:00Z");
        provider_session.updated_at = Some("2026-07-15T00:01:00Z".to_string());
        provider_session.state = SessionState::Working;

        manager
            .synchronize_provider_session("codex", None, &provider_session)
            .expect("first sync");
        manager
            .synchronize_provider_session("codex", None, &provider_session)
            .expect("replayed sync");
        assert_eq!(
            manager
                .load_session_events("codex.thread.replay", Some(20), None)
                .expect("events")
                .iter()
                .filter(|event| event.event_type == "session.synchronized")
                .count(),
            1
        );
    }

    #[test]
    fn unified_store_rejects_invalid_provider_timestamp() {
        let manager = create_manager();
        let mut session = AgentSession::new("gemini.session.invalid-time");
        session.updated_at = Some("yesterday-ish".to_string());
        assert!(manager
            .synchronize_provider_session("gemini-cli", None, &session)
            .expect_err("invalid timestamp")
            .contains("RFC 3339"));
        assert!(manager.get_session(&session.session_id).is_err());

        let mut invalid_order = AgentSession::new("gemini.session.invalid-order");
        invalid_order.created_at = Some("2026-07-16T00:02:00Z".to_string());
        invalid_order.updated_at = Some("2026-07-16T00:01:00Z".to_string());
        assert!(manager
            .synchronize_provider_session("gemini-cli", None, &invalid_order)
            .expect_err("invalid timestamp order")
            .contains("earlier than created_at"));
        assert!(manager.get_session(&invalid_order.session_id).is_err());
    }
}
