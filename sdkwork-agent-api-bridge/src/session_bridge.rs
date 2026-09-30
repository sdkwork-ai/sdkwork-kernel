use crate::types::{generate_id, BridgeSessionConfig};
use sdkwork_agent_kernel::{
    AgentMessage, AgentSession, EventRecorder, KernelError, KernelResult, SessionKind,
    SessionSource,
};
use std::collections::HashMap;
use std::mem::size_of;
use std::time::{Duration, Instant};

use crate::model_bridge::PROVIDER_SESSION_ID_METADATA;

/// Maximum in-bridge message history entries retained per session.
const MAX_SESSION_BRIDGE_HISTORY: usize = 512;
/// Maximum flattened message bytes retained per session.
const MAX_SESSION_BRIDGE_HISTORY_BYTES: usize = 4 * 1024 * 1024;
/// Hard cap for all transient message history retained by one process.
const MAX_GLOBAL_SESSION_BRIDGE_HISTORY_BYTES: usize = 256 * 1024 * 1024;
/// Hard bound for active transient sessions retained by one runtime process.
const MAX_SESSION_BRIDGE_SESSIONS: usize = 4096;
/// Maximum retained configuration bytes for one transient session.
const MAX_SESSION_BRIDGE_SESSION_BYTES: usize = 512 * 1024;
/// Hard cap for all transient session configuration retained by one process.
const MAX_GLOBAL_SESSION_BRIDGE_SESSION_BYTES: usize = 64 * 1024 * 1024;
/// Idle duration after which a session's transient bridge state becomes
/// eligible for lazy eviction. Bridge state is a re-hydratable cache of the
/// durable store, so an evicted session is rebuilt from persistence on its
/// next access; eviction only bounds memory for sessions abandoned without
/// an explicit close.
pub const BRIDGE_IDLE_EVICTION: Duration = Duration::from_secs(60 * 60);

/// Manages session lifecycle and message history
pub struct SessionBridge {
    sessions: HashMap<String, AgentSession>,
    histories: HashMap<String, Vec<AgentMessage>>,
    history_bytes: HashMap<String, usize>,
    total_history_bytes: usize,
    history_revisions: HashMap<String, u64>,
    session_bytes: HashMap<String, usize>,
    total_session_bytes: usize,
    last_activity: HashMap<String, Instant>,
}

impl SessionBridge {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            histories: HashMap::new(),
            history_bytes: HashMap::new(),
            total_history_bytes: 0,
            history_revisions: HashMap::new(),
            session_bytes: HashMap::new(),
            total_session_bytes: 0,
            last_activity: HashMap::new(),
        }
    }

    fn touch(&mut self, session_id: &str) {
        self.last_activity
            .insert(session_id.to_string(), Instant::now());
    }

    /// Evict transient state for sessions idle beyond `idle_for` and return
    /// the evicted session ids so callers can release dependent state (event
    /// snapshots, per-session locks) held elsewhere.
    pub fn sweep_idle_sessions(&mut self, idle_for: Duration) -> Vec<String> {
        let cutoff = Instant::now();
        let evicted: Vec<String> = self
            .last_activity
            .iter()
            .filter(|(_, activity)| cutoff.duration_since(**activity) >= idle_for)
            .map(|(session_id, _)| session_id.clone())
            .collect();
        for session_id in &evicted {
            self.remove_session(session_id);
        }
        evicted
    }

    /// Create a new session with a caller-provided session id.
    pub fn register_session(
        &mut self,
        session_id: &str,
        config: BridgeSessionConfig,
    ) -> KernelResult<AgentSession> {
        if self.sessions.contains_key(session_id) {
            return self.refresh_existing_session(session_id, config);
        }
        if self.sessions.len() >= MAX_SESSION_BRIDGE_SESSIONS {
            return Err(KernelError::resource_exhausted(
                "active session bridge capacity exhausted",
            ));
        }

        let session = build_session(session_id, config)?;
        let retained_bytes = session_retained_bytes(&session)?;
        self.ensure_session_capacity(retained_bytes)?;
        self.sessions
            .insert(session_id.to_string(), session.clone());
        self.histories.insert(session_id.to_string(), Vec::new());
        self.history_bytes.insert(session_id.to_string(), 0);
        self.history_revisions.insert(session_id.to_string(), 0);
        self.session_bytes
            .insert(session_id.to_string(), retained_bytes);
        self.total_session_bytes = self
            .total_session_bytes
            .checked_add(retained_bytes)
            .ok_or_else(|| KernelError::resource_exhausted("session byte count overflow"))?;
        self.touch(session_id);

        Ok(session)
    }

    /// Create a new session
    pub fn create_session(&mut self, config: BridgeSessionConfig) -> KernelResult<AgentSession> {
        if self.sessions.len() >= MAX_SESSION_BRIDGE_SESSIONS {
            return Err(KernelError::resource_exhausted(
                "active session bridge capacity exhausted",
            ));
        }
        let session_id = format!("session.{}", generate_id());

        let session = build_session(&session_id, config)?;
        let retained_bytes = session_retained_bytes(&session)?;
        self.ensure_session_capacity(retained_bytes)?;
        self.sessions.insert(session_id.clone(), session.clone());
        self.histories.insert(session_id.clone(), Vec::new());
        self.history_bytes.insert(session_id.clone(), 0);
        self.history_revisions.insert(session_id.clone(), 0);
        self.session_bytes
            .insert(session_id.clone(), retained_bytes);
        self.total_session_bytes = self
            .total_session_bytes
            .checked_add(retained_bytes)
            .ok_or_else(|| KernelError::resource_exhausted("session byte count overflow"))?;
        self.touch(&session_id);

        Ok(session)
    }

    fn refresh_existing_session(
        &mut self,
        session_id: &str,
        config: BridgeSessionConfig,
    ) -> KernelResult<AgentSession> {
        let existing = self.get_session(session_id)?;
        let expected_tenant = config.tenant_id.as_str();
        if existing.agent_id.as_deref() != Some(config.agent_id.as_str())
            || existing.tenant_id.as_deref() != Some(expected_tenant)
            || existing.user_ref.as_deref() != config.user_ref.as_deref()
        {
            return Err(KernelError::conflict(
                "session identity does not match the registered session",
            ));
        }

        let mut updated = existing.clone();
        updated.model = config.model;
        updated.instructions = config.instructions;
        updated.cwd = config.cwd;
        updated.metadata = config.metadata;
        let retained_bytes = session_retained_bytes(&updated)?;
        let previous_bytes = self.session_bytes.get(session_id).copied().unwrap_or(0);
        let projected_total = self
            .total_session_bytes
            .checked_sub(previous_bytes)
            .and_then(|value| value.checked_add(retained_bytes))
            .ok_or_else(|| KernelError::Internal {
                message: "session byte accounting underflow".to_string(),
            })?;
        if projected_total > MAX_GLOBAL_SESSION_BRIDGE_SESSION_BYTES {
            return Err(KernelError::resource_exhausted(
                "global session configuration byte budget exhausted",
            ));
        }
        self.sessions
            .insert(session_id.to_string(), updated.clone());
        self.session_bytes
            .insert(session_id.to_string(), retained_bytes);
        self.total_session_bytes = projected_total;
        self.touch(session_id);
        Ok(updated)
    }

    fn ensure_session_capacity(&self, retained_bytes: usize) -> KernelResult<()> {
        if retained_bytes > MAX_SESSION_BRIDGE_SESSION_BYTES {
            return Err(KernelError::resource_exhausted(
                "session configuration exceeds the byte budget",
            ));
        }
        let projected_total = self
            .total_session_bytes
            .checked_add(retained_bytes)
            .ok_or_else(|| KernelError::resource_exhausted("session byte count overflow"))?;
        if projected_total > MAX_GLOBAL_SESSION_BRIDGE_SESSION_BYTES {
            return Err(KernelError::resource_exhausted(
                "global session configuration byte budget exhausted",
            ));
        }
        Ok(())
    }

    /// Get a session by ID
    pub fn get_session(&self, session_id: &str) -> KernelResult<AgentSession> {
        self.sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| KernelError::validation(format!("session not found: {}", session_id)))
    }

    pub(crate) fn restore_session(&mut self, session: AgentSession) -> KernelResult<()> {
        let session_id = session.session_id.clone();
        let retained_bytes = session_retained_bytes(&session)?;
        let previous_bytes = self.session_bytes.get(&session_id).copied().unwrap_or(0);
        let projected_total = self
            .total_session_bytes
            .checked_sub(previous_bytes)
            .and_then(|value| value.checked_add(retained_bytes))
            .ok_or_else(|| KernelError::Internal {
                message: "session byte accounting underflow".to_string(),
            })?;
        if projected_total > MAX_GLOBAL_SESSION_BRIDGE_SESSION_BYTES {
            return Err(KernelError::resource_exhausted(
                "global session configuration byte budget exhausted",
            ));
        }
        self.sessions.insert(session_id.clone(), session);
        self.session_bytes.insert(session_id.clone(), retained_bytes);
        self.total_session_bytes = projected_total;
        self.touch(&session_id);
        Ok(())
    }

    /// Atomically bind a provider continuation identity and append a completed turn.
    pub(crate) fn append_completed_turn(
        &mut self,
        session_id: &str,
        messages: Vec<AgentMessage>,
        provider_session_id: Option<&str>,
    ) -> KernelResult<()> {
        let previous_session = self.get_session(session_id)?;
        if let Some(provider_session_id) = provider_session_id {
            self.bind_provider_session_id(session_id, provider_session_id)?;
        }
        if let Err(error) = self.append_messages(session_id, messages) {
            self.restore_session(previous_session)?;
            return Err(error);
        }
        Ok(())
    }
    fn bind_provider_session_id(
        &mut self,
        session_id: &str,
        provider_session_id: &str,
    ) -> KernelResult<()> {
        let provider_session_id = provider_session_id.trim();
        if provider_session_id.is_empty() {
            return Err(KernelError::validation(
                "provider session id must not be empty",
            ));
        }
        let mut session = self.get_session(session_id)?;
        let existing_ids = session
            .metadata
            .iter()
            .filter(|(key, _)| key == PROVIDER_SESSION_ID_METADATA)
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if existing_ids
            .iter()
            .any(|existing| *existing != provider_session_id)
        {
            return Err(KernelError::conflict(
                "provider session id does not match the canonical session binding",
            ));
        }
        session
            .metadata
            .retain(|(key, _)| key != PROVIDER_SESSION_ID_METADATA);
        session.metadata.push((
            PROVIDER_SESSION_ID_METADATA.to_string(),
            provider_session_id.to_string(),
        ));
        self.restore_session(session)
    }

    /// List all sessions
    pub fn list_sessions(&self) -> KernelResult<Vec<AgentSession>> {
        Ok(self.sessions.values().cloned().collect())
    }

    /// Close a session
    pub fn close_session(&mut self, session_id: &str) -> KernelResult<AgentSession> {
        let session = self.get_session(session_id)?;
        let mut recorder = EventRecorder::new();
        let closed = session.close(&mut recorder)?;
        let retained_bytes = session_retained_bytes(&closed)?;
        let previous_bytes = self.session_bytes.get(session_id).copied().unwrap_or(0);
        let projected_total = self
            .total_session_bytes
            .checked_sub(previous_bytes)
            .and_then(|value| value.checked_add(retained_bytes))
            .ok_or_else(|| KernelError::Internal {
                message: "session byte accounting underflow".to_string(),
            })?;
        if projected_total > MAX_GLOBAL_SESSION_BRIDGE_SESSION_BYTES {
            return Err(KernelError::resource_exhausted(
                "global session configuration byte budget exhausted",
            ));
        }
        self.sessions.insert(session_id.to_string(), closed.clone());
        self.session_bytes
            .insert(session_id.to_string(), retained_bytes);
        self.total_session_bytes = projected_total;
        Ok(closed)
    }

    /// Remove a session and its in-bridge message history from transient runtime state.
    pub fn remove_session(&mut self, session_id: &str) -> bool {
        self.last_activity.remove(session_id);
        self.histories.remove(session_id);
        if let Some(removed_bytes) = self.history_bytes.remove(session_id) {
            self.total_history_bytes = self.total_history_bytes.saturating_sub(removed_bytes);
        }
        self.history_revisions.remove(session_id);
        if let Some(removed_bytes) = self.session_bytes.remove(session_id) {
            self.total_session_bytes = self.total_session_bytes.saturating_sub(removed_bytes);
        }
        self.sessions.remove(session_id).is_some()
    }

    /// Append a message to session history
    pub fn append_message(&mut self, session_id: &str, message: AgentMessage) -> KernelResult<()> {
        self.append_messages(session_id, vec![message])
    }

    /// Validate a message against the bounded transient-history contract.
    pub fn validate_message(&self, session_id: &str, message: &AgentMessage) -> KernelResult<()> {
        if !self.sessions.contains_key(session_id) || !self.histories.contains_key(session_id) {
            return Err(KernelError::validation(format!(
                "session not found: {session_id}"
            )));
        }
        message.validate()?;
        if message
            .session_id
            .as_deref()
            .is_some_and(|message_session_id| message_session_id != session_id)
        {
            return Err(KernelError::conflict(
                "message session id does not match the target session",
            ));
        }
        if message_size(message) > MAX_SESSION_BRIDGE_HISTORY_BYTES {
            return Err(KernelError::resource_exhausted(
                "message exceeds session bridge byte budget",
            ));
        }
        Ok(())
    }

    /// Atomically append one completed turn after every message has passed validation.
    pub fn append_messages(
        &mut self,
        session_id: &str,
        messages: Vec<AgentMessage>,
    ) -> KernelResult<()> {
        if !self.sessions.contains_key(session_id) || !self.histories.contains_key(session_id) {
            return Err(KernelError::validation(format!(
                "session not found: {session_id}"
            )));
        }
        if messages.len() > MAX_SESSION_BRIDGE_HISTORY {
            return Err(KernelError::resource_exhausted(
                "message batch exceeds session bridge history entry budget",
            ));
        }

        let mut added_bytes = 0usize;
        for message in &messages {
            self.validate_message(session_id, message)?;
            let bytes = message_size(message);
            if bytes > MAX_SESSION_BRIDGE_HISTORY_BYTES {
                return Err(KernelError::resource_exhausted(
                    "message exceeds session bridge byte budget",
                ));
            }
            added_bytes = added_bytes.checked_add(bytes).ok_or_else(|| {
                KernelError::resource_exhausted("session bridge history byte count overflow")
            })?;
        }

        let current_bytes = self.history_bytes.get(session_id).copied().unwrap_or(0);
        let mut retained_bytes = current_bytes.checked_add(added_bytes).ok_or_else(|| {
            KernelError::resource_exhausted("session bridge history byte count overflow")
        })?;
        let projected_retained_bytes = {
            let history = self
                .histories
                .get(session_id)
                .expect("history existence checked above");
            projected_history_bytes(history, &messages, retained_bytes)
        };
        let projected_total = global_history_total_after_replace(
            self.total_history_bytes,
            current_bytes,
            projected_retained_bytes,
        )?;
        let appended_count = messages.len();
        let next_revision = self
            .history_revisions
            .get(session_id)
            .copied()
            .unwrap_or(0)
            .checked_add(appended_count as u64)
            .ok_or_else(|| KernelError::resource_exhausted("history revision exhausted"))?;
        let history = self
            .histories
            .get_mut(session_id)
            .expect("history existence checked above");
        history.extend(messages);
        while history.len() > MAX_SESSION_BRIDGE_HISTORY
            || retained_bytes > MAX_SESSION_BRIDGE_HISTORY_BYTES
        {
            let removed = history.remove(0);
            retained_bytes = retained_bytes.saturating_sub(message_size(&removed));
        }
        self.history_bytes
            .insert(session_id.to_string(), retained_bytes);
        debug_assert_eq!(retained_bytes, projected_retained_bytes);
        self.total_history_bytes = projected_total;
        self.history_revisions
            .insert(session_id.to_string(), next_revision);

        if let Some(session) = self.sessions.get_mut(session_id) {
            for _ in 0..appended_count {
                session.record_message_received();
            }
        }
        self.touch(session_id);
        Ok(())
    }

    /// Replace transient history with a bounded persisted snapshot.
    pub fn replace_history(
        &mut self,
        session_id: &str,
        messages: Vec<AgentMessage>,
    ) -> KernelResult<()> {
        if !self.sessions.contains_key(session_id) {
            return Err(KernelError::validation(format!(
                "session not found: {session_id}"
            )));
        }

        let mut retained = Vec::new();
        let mut retained_bytes = 0usize;
        for message in messages.into_iter().rev() {
            self.validate_message(session_id, &message)?;
            let message_bytes = message_size(&message);
            if message_bytes > MAX_SESSION_BRIDGE_HISTORY_BYTES {
                return Err(KernelError::resource_exhausted(
                    "persisted message exceeds session bridge byte budget",
                ));
            }
            if retained.len() >= MAX_SESSION_BRIDGE_HISTORY
                || retained_bytes.saturating_add(message_bytes) > MAX_SESSION_BRIDGE_HISTORY_BYTES
            {
                break;
            }
            retained_bytes = retained_bytes.saturating_add(message_bytes);
            retained.push(message);
        }
        retained.reverse();
        let current_bytes = self.history_bytes.get(session_id).copied().unwrap_or(0);
        let projected_total = global_history_total_after_replace(
            self.total_history_bytes,
            current_bytes,
            retained_bytes,
        )?;
        self.histories.insert(session_id.to_string(), retained);
        self.history_bytes
            .insert(session_id.to_string(), retained_bytes);
        self.total_history_bytes = projected_total;
        self.touch(session_id);
        Ok(())
    }

    /// Refresh history only when the supplied persisted snapshot is newer than
    /// the transient snapshot already retained for this session. Persistence
    /// is read before the per-session turn lock is acquired, so a stale result
    /// must not overwrite a turn that completed while the request waited.
    pub fn replace_history_if_newer(
        &mut self,
        session_id: &str,
        messages: Vec<AgentMessage>,
    ) -> KernelResult<bool> {
        let should_replace = {
            let existing = self.histories.get(session_id).ok_or_else(|| {
                KernelError::validation(format!("session not found: {session_id}"))
            })?;
            history_snapshot_is_newer(existing, &messages)
        };
        if !should_replace {
            return Ok(false);
        }
        self.replace_history(session_id, messages)?;
        Ok(true)
    }

    /// Replace history only when persistence supplies a strictly newer
    /// monotonic message-count revision. Equal revisions must describe the
    /// same snapshot; divergent data at one revision is a consistency error.
    pub fn replace_history_if_revision(
        &mut self,
        session_id: &str,
        revision: u64,
        messages: Vec<AgentMessage>,
    ) -> KernelResult<bool> {
        let current_revision = self
            .history_revisions
            .get(session_id)
            .copied()
            .ok_or_else(|| KernelError::validation(format!("session not found: {session_id}")))?;
        if revision < current_revision {
            return Ok(false);
        }
        if revision == current_revision {
            let existing = self.histories.get(session_id).ok_or_else(|| {
                KernelError::validation(format!("session not found: {session_id}"))
            })?;
            if histories_semantically_equal(existing, &messages)
                || (revision == 0 && existing.is_empty() && messages.is_empty())
            {
                return Ok(false);
            }
            return Err(KernelError::conflict(
                "history snapshots diverge at the same revision",
            ));
        }
        self.replace_history(session_id, messages)?;
        self.history_revisions
            .insert(session_id.to_string(), revision);
        Ok(true)
    }

    /// Get message history for a session
    pub fn get_history(&self, session_id: &str) -> KernelResult<Vec<AgentMessage>> {
        self.histories
            .get(session_id)
            .cloned()
            .ok_or_else(|| KernelError::validation(format!("session not found: {}", session_id)))
    }

    /// Clear message history for a session
    pub fn clear_history(&mut self, session_id: &str) -> KernelResult<()> {
        let history = self
            .histories
            .get_mut(session_id)
            .ok_or_else(|| KernelError::validation(format!("session not found: {}", session_id)))?;

        *history = Vec::new();
        if let Some(removed_bytes) = self.history_bytes.insert(session_id.to_string(), 0) {
            self.total_history_bytes = self.total_history_bytes.saturating_sub(removed_bytes);
        }
        self.touch(session_id);
        Ok(())
    }

    /// Get the number of messages in a session
    pub fn message_count(&self, session_id: &str) -> usize {
        self.histories.get(session_id).map(|h| h.len()).unwrap_or(0)
    }
}

fn histories_semantically_equal(left: &[AgentMessage], right: &[AgentMessage]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.role == right.role
                && parts_semantically_equal(&left.parts, &right.parts)
                && left.task_id == right.task_id
                && left.run_id == right.run_id
                && left.step_id == right.step_id
                && left.untrusted == right.untrusted
        })
}

fn parts_semantically_equal(
    left: &[sdkwork_agent_kernel::AgentPart],
    right: &[sdkwork_agent_kernel::AgentPart],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            let mut left = left.clone();
            let mut right = right.clone();
            left.part_id.clear();
            right.part_id.clear();
            left == right
        })
}

impl Default for SessionBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// RFC3339 UTC timestamp for bridge-owned session metadata.
fn chrono_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn build_session(session_id: &str, config: BridgeSessionConfig) -> KernelResult<AgentSession> {
    let mut session = AgentSession::new(session_id)
        .with_agent_id(&config.agent_id)
        .with_tenant_id(config.tenant_id.clone())
        .with_source(SessionSource::Api)
        .with_kind(SessionKind::Main)
        .created_at(chrono_now());
    if let Some(user_ref) = config.user_ref {
        session = session.with_user_ref(user_ref);
    }
    if let Some(model) = config.model {
        session = session.with_model(model);
    }
    if let Some(instructions) = config.instructions {
        session = session.with_instructions(instructions);
    }
    if let Some(cwd) = config.cwd {
        session = session.with_cwd(cwd);
    }
    for (key, value) in config.metadata {
        session = session.with_metadata(key, value);
    }
    let mut recorder = EventRecorder::new();
    session.activate(&mut recorder)
}

fn session_retained_bytes(session: &AgentSession) -> KernelResult<usize> {
    let mut bytes = size_of::<AgentSession>().saturating_add(session.session_id.capacity());
    for value in [
        &session.parent_session_id,
        &session.forked_from_id,
        &session.slug,
        &session.agent_id,
        &session.user_ref,
        &session.tenant_id,
        &session.title,
        &session.preview,
        &session.goal,
        &session.summary,
        &session.created_at,
        &session.updated_at,
        &session.ended_at,
        &session.archived_at,
        &session.model,
        &session.model_provider,
        &session.cwd,
        &session.instructions,
        &session.personality,
        &session.reasoning_effort,
        &session.approval_policy,
        &session.permission_profile,
        &session.context_from,
        &session.context_watermark,
        &session.summary_message_id,
        &session.agent_nickname,
        &session.agent_role,
    ] {
        bytes = bytes.saturating_add(value.as_ref().map_or(0, String::capacity));
    }
    bytes = bytes
        .saturating_add(
            session
                .workspace_roots
                .capacity()
                .saturating_mul(size_of::<String>()),
        )
        .saturating_add(
            session
                .workspace_roots
                .iter()
                .map(String::capacity)
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(
            session
                .child_session_ids
                .capacity()
                .saturating_mul(size_of::<String>()),
        )
        .saturating_add(
            session
                .child_session_ids
                .iter()
                .map(String::capacity)
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(metadata_retained_bytes(
            &session.metadata,
            session.metadata.capacity(),
        ));
    if bytes > MAX_SESSION_BRIDGE_SESSION_BYTES {
        return Err(KernelError::resource_exhausted(
            "session configuration exceeds the byte budget",
        ));
    }
    Ok(bytes)
}

fn message_size(message: &AgentMessage) -> usize {
    let mut bytes = size_of::<AgentMessage>()
        .saturating_add(message.message_id.capacity())
        .saturating_add(
            message
                .parts
                .capacity()
                .saturating_mul(size_of::<sdkwork_agent_kernel::AgentPart>()),
        )
        .saturating_add(metadata_retained_bytes(
            &message.metadata,
            message.metadata.capacity(),
        ));
    for value in [
        &message.session_id,
        &message.task_id,
        &message.run_id,
        &message.step_id,
        &message.created_at,
    ] {
        bytes = bytes.saturating_add(value.as_ref().map_or(0, String::capacity));
    }
    if let Some(trace) = &message.trace_context {
        bytes = bytes
            .saturating_add(size_of::<sdkwork_agent_kernel::TraceContext>())
            .saturating_add(trace.trace_id.capacity())
            .saturating_add(trace.span_id.capacity())
            .saturating_add(trace.parent_span_id.as_ref().map_or(0, String::capacity));
    }
    for part in &message.parts {
        bytes = bytes
            .saturating_add(part.part_id.capacity())
            .saturating_add(metadata_retained_bytes(
                &part.metadata,
                part.metadata.capacity(),
            ));
        for value in [
            &part.text,
            &part.json,
            &part.content_ref,
            &part.artifact_id,
            &part.tool_call_id,
            &part.policy_decision_id,
            &part.error_code,
            &part.mime_type,
            &part.name,
            &part.schema,
            &part.provenance,
        ] {
            bytes = bytes.saturating_add(value.as_ref().map_or(0, String::capacity));
        }
    }
    bytes
}

fn metadata_retained_bytes(metadata: &[(String, String)], capacity: usize) -> usize {
    capacity
        .saturating_mul(size_of::<(String, String)>())
        .saturating_add(metadata.iter().fold(0usize, |bytes, (key, value)| {
            bytes
                .saturating_add(key.capacity())
                .saturating_add(value.capacity())
        }))
}

fn projected_history_bytes(
    existing: &[AgentMessage],
    incoming: &[AgentMessage],
    mut retained_bytes: usize,
) -> usize {
    let mut retained_count = existing.len().saturating_add(incoming.len());
    for message in existing.iter().chain(incoming.iter()) {
        if retained_count <= MAX_SESSION_BRIDGE_HISTORY
            && retained_bytes <= MAX_SESSION_BRIDGE_HISTORY_BYTES
        {
            break;
        }
        retained_count = retained_count.saturating_sub(1);
        retained_bytes = retained_bytes.saturating_sub(message_size(message));
    }
    retained_bytes
}

fn global_history_total_after_replace(
    total_bytes: usize,
    current_session_bytes: usize,
    replacement_bytes: usize,
) -> KernelResult<usize> {
    let other_session_bytes = total_bytes
        .checked_sub(current_session_bytes)
        .ok_or_else(|| KernelError::Internal {
            message: "session bridge global history accounting underflow".to_string(),
        })?;
    let projected_total = other_session_bytes
        .checked_add(replacement_bytes)
        .ok_or_else(|| {
            KernelError::resource_exhausted("session bridge global history byte count overflow")
        })?;
    if projected_total > MAX_GLOBAL_SESSION_BRIDGE_HISTORY_BYTES {
        return Err(KernelError::resource_exhausted(
            "session bridge global history byte budget exhausted",
        ));
    }
    Ok(projected_total)
}

fn history_snapshot_is_newer(existing: &[AgentMessage], incoming: &[AgentMessage]) -> bool {
    if existing.is_empty() {
        return true;
    }
    if incoming.is_empty() || incoming.len() < existing.len() {
        return false;
    }
    if existing.len() == incoming.len()
        && existing
            .iter()
            .map(|message| message.message_id.as_str())
            .eq(incoming.iter().map(|message| message.message_id.as_str()))
    {
        return false;
    }

    // A same-sized snapshot whose last id occurs before the current tail is
    // an older bounded window read before a local turn evicted old entries.
    if existing.len() == incoming.len() {
        if let Some(incoming_last) = incoming.last() {
            if let Some(existing_index) = existing
                .iter()
                .position(|message| message.message_id == incoming_last.message_id)
            {
                return existing_index + 1 >= existing.len();
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdkwork_agent_kernel::SessionState;

    fn test_config() -> BridgeSessionConfig {
        BridgeSessionConfig {
            agent_id: "agent.test".to_string(),
            tenant_id: "tenant.100001".to_string(),
            user_ref: Some("user.1".to_string()),
            model: Some("gpt-4".to_string()),
            instructions: None,
            cwd: None,
            metadata: Vec::new(),
        }
    }

    #[test]
    fn create_and_get_session() {
        let mut bridge = SessionBridge::new();
        let session = bridge.create_session(test_config()).expect("created");
        let retrieved = bridge.get_session(&session.session_id).expect("found");
        assert_eq!(session.session_id, retrieved.session_id);
    }

    #[test]
    fn sweep_evicts_idle_sessions_and_clears_bookkeeping() {
        let mut bridge = SessionBridge::new();
        let first = bridge.create_session(test_config()).expect("created");
        let second = bridge.create_session(test_config()).expect("created");

        // A zero-duration idle threshold sweeps every registered session and
        // clears all satellite bookkeeping (activity entries, byte counters).
        let evicted = bridge.sweep_idle_sessions(Duration::ZERO);
        assert!(evicted.contains(&first.session_id));
        assert!(evicted.contains(&second.session_id));
        assert_eq!(evicted.len(), 2);
        assert!(bridge.get_session(&first.session_id).is_err());
        assert!(bridge.get_session(&second.session_id).is_err());
        assert!(bridge.last_activity.is_empty());
        assert_eq!(bridge.total_session_bytes, 0);
        assert_eq!(bridge.total_history_bytes, 0);
        // Sweeping an empty bridge is a no-op.
        assert!(bridge.sweep_idle_sessions(Duration::ZERO).is_empty());
    }

    #[test]
    fn list_sessions() {
        let mut bridge = SessionBridge::new();
        bridge.create_session(test_config()).expect("created");
        bridge.create_session(test_config()).expect("created");
        let sessions = bridge.list_sessions().expect("listed");
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn close_session() {
        let mut bridge = SessionBridge::new();
        let session = bridge.create_session(test_config()).expect("created");
        let closed = bridge.close_session(&session.session_id).expect("closed");
        assert_eq!(closed.state, SessionState::Closed);
    }

    #[test]
    fn remove_session_deletes_session_and_history() {
        let mut bridge = SessionBridge::new();
        let session = bridge.create_session(test_config()).expect("created");
        let message = AgentMessage::new(
            "msg.remove",
            sdkwork_agent_kernel::AgentMessageRole::User,
            vec![sdkwork_agent_kernel::AgentPart::text(
                "part.remove",
                "remove me",
            )],
        );
        bridge
            .append_message(&session.session_id, message)
            .expect("message appended");
        assert_eq!(bridge.message_count(&session.session_id), 1);

        assert!(
            bridge.remove_session(&session.session_id),
            "existing session should be removed"
        );

        assert!(bridge.get_session(&session.session_id).is_err());
        assert!(bridge.get_history(&session.session_id).is_err());
        assert!(
            !bridge.remove_session(&session.session_id),
            "removing an absent session should be a no-op"
        );
    }

    #[test]
    fn append_and_get_history() {
        let mut bridge = SessionBridge::new();
        let session = bridge.create_session(test_config()).expect("created");

        let msg = AgentMessage::new(
            "msg.1",
            sdkwork_agent_kernel::AgentMessageRole::User,
            vec![sdkwork_agent_kernel::AgentPart::text("part.1", "Hello")],
        );

        bridge
            .append_message(&session.session_id, msg)
            .expect("appended");

        let history = bridge.get_history(&session.session_id).expect("history");
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn registering_existing_session_rejects_identity_mismatch_without_mutation() {
        let mut bridge = SessionBridge::new();
        let original = bridge
            .register_session("session.identity", test_config())
            .expect("registered");

        let mut tenant_mismatch = test_config();
        tenant_mismatch.tenant_id = "tenant.100002".to_string();
        let tenant_error = bridge
            .register_session("session.identity", tenant_mismatch)
            .expect_err("tenant mismatch must be rejected");
        assert_eq!(
            tenant_error.kind(),
            sdkwork_agent_kernel::KernelErrorKind::Conflict
        );

        let mut agent_mismatch = test_config();
        agent_mismatch.agent_id = "agent.other".to_string();
        let agent_error = bridge
            .register_session("session.identity", agent_mismatch)
            .expect_err("agent mismatch must be rejected");
        assert_eq!(
            agent_error.kind(),
            sdkwork_agent_kernel::KernelErrorKind::Conflict
        );

        let mut user_mismatch = test_config();
        user_mismatch.user_ref = Some("user.other".to_string());
        let user_error = bridge
            .register_session("session.identity", user_mismatch)
            .expect_err("user mismatch must be rejected");
        assert_eq!(
            user_error.kind(),
            sdkwork_agent_kernel::KernelErrorKind::Conflict
        );

        let retained = bridge
            .get_session("session.identity")
            .expect("original session retained");
        assert_eq!(retained.agent_id, original.agent_id);
        assert_eq!(retained.tenant_id, original.tenant_id);
        assert_eq!(retained.user_ref, original.user_ref);
        assert_eq!(retained.model, original.model);
    }

    #[test]
    fn nested_metadata_counts_against_the_history_byte_budget() {
        let mut bridge = SessionBridge::new();
        let session = bridge.create_session(test_config()).expect("created");
        let message = AgentMessage::new(
            "msg.metadata",
            sdkwork_agent_kernel::AgentMessageRole::User,
            vec![
                sdkwork_agent_kernel::AgentPart::text("part.metadata", "").with_metadata(
                    "part.payload",
                    "x".repeat(MAX_SESSION_BRIDGE_HISTORY_BYTES / 2),
                ),
            ],
        )
        .with_metadata(
            "message.payload",
            "y".repeat(MAX_SESSION_BRIDGE_HISTORY_BYTES / 2),
        );
        assert!(
            message_size(&message) > MAX_SESSION_BRIDGE_HISTORY_BYTES,
            "metadata retained below message text must still consume the history budget"
        );

        let error = bridge
            .append_message(&session.session_id, message)
            .expect_err("metadata-only retained state must be bounded");
        assert_eq!(
            error.kind(),
            sdkwork_agent_kernel::KernelErrorKind::ResourceExhausted
        );
        assert!(bridge
            .get_history(&session.session_id)
            .expect("history")
            .is_empty());
    }

    #[test]
    fn append_messages_is_atomic_when_one_message_exceeds_the_byte_budget() {
        let mut bridge = SessionBridge::new();
        let session = bridge.create_session(test_config()).expect("created");
        let user = AgentMessage::new(
            "msg.user",
            sdkwork_agent_kernel::AgentMessageRole::User,
            vec![sdkwork_agent_kernel::AgentPart::text("part.user", "hello")],
        );
        let oversized = AgentMessage::new(
            "msg.agent",
            sdkwork_agent_kernel::AgentMessageRole::Agent,
            vec![sdkwork_agent_kernel::AgentPart::text(
                "part.agent",
                "x".repeat(MAX_SESSION_BRIDGE_HISTORY_BYTES + 1),
            )],
        );

        let error = bridge
            .append_messages(&session.session_id, vec![user, oversized])
            .expect_err("oversized turn must be rejected atomically");
        assert_eq!(
            error.kind(),
            sdkwork_agent_kernel::KernelErrorKind::ResourceExhausted
        );
        assert!(bridge
            .get_history(&session.session_id)
            .expect("history")
            .is_empty());
    }

    #[test]
    fn stale_history_snapshot_cannot_overwrite_a_newer_local_turn() {
        let mut bridge = SessionBridge::new();
        let session = bridge.create_session(test_config()).expect("created");
        let first = AgentMessage::new(
            "msg.first",
            sdkwork_agent_kernel::AgentMessageRole::User,
            vec![sdkwork_agent_kernel::AgentPart::text("part.first", "first")],
        );
        let second = AgentMessage::new(
            "msg.second",
            sdkwork_agent_kernel::AgentMessageRole::Agent,
            vec![sdkwork_agent_kernel::AgentPart::text(
                "part.second",
                "second",
            )],
        );
        bridge
            .append_messages(&session.session_id, vec![first.clone(), second.clone()])
            .expect("local turn");

        let replaced = bridge
            .replace_history_if_newer(&session.session_id, vec![first])
            .expect("stale snapshot check");
        assert!(!replaced);
        let history = bridge.get_history(&session.session_id).expect("history");
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].message_id, "msg.second");
    }

    #[test]
    fn same_revision_accepts_persisted_identity_rewrites_but_rejects_content_changes() {
        let mut bridge = SessionBridge::new();
        let session = bridge.create_session(test_config()).expect("created");
        let local = AgentMessage::new(
            "msg.local",
            sdkwork_agent_kernel::AgentMessageRole::User,
            vec![sdkwork_agent_kernel::AgentPart::text("part.local", "hello")],
        )
        .for_session(&session.session_id)
        .mark_untrusted();
        bridge
            .append_message(&session.session_id, local)
            .expect("local message");

        let persisted = AgentMessage::new(
            "msg.persisted",
            sdkwork_agent_kernel::AgentMessageRole::User,
            vec![sdkwork_agent_kernel::AgentPart::text(
                "msg.persisted/part.0",
                "hello",
            )],
        )
        .for_session(&session.session_id)
        .created_at("2026-07-14T00:00:00Z")
        .mark_untrusted();
        assert!(!bridge
            .replace_history_if_revision(&session.session_id, 1, vec![persisted])
            .expect("storage identity rewrite is equivalent"));

        let changed = AgentMessage::new(
            "msg.changed",
            sdkwork_agent_kernel::AgentMessageRole::User,
            vec![sdkwork_agent_kernel::AgentPart::text(
                "msg.changed/part.0",
                "different",
            )],
        )
        .for_session(&session.session_id)
        .mark_untrusted();
        let error = bridge
            .replace_history_if_revision(&session.session_id, 1, vec![changed])
            .expect_err("content divergence must remain a conflict");
        assert_eq!(
            error.kind(),
            sdkwork_agent_kernel::KernelErrorKind::Conflict
        );
    }

    #[test]
    fn newer_history_snapshot_refreshes_a_transient_session() {
        let mut bridge = SessionBridge::new();
        let session = bridge.create_session(test_config()).expect("created");
        let first = AgentMessage::new(
            "msg.first",
            sdkwork_agent_kernel::AgentMessageRole::User,
            vec![sdkwork_agent_kernel::AgentPart::text("part.first", "first")],
        );
        let second = AgentMessage::new(
            "msg.second",
            sdkwork_agent_kernel::AgentMessageRole::Agent,
            vec![sdkwork_agent_kernel::AgentPart::text(
                "part.second",
                "second",
            )],
        );
        assert!(bridge
            .replace_history_if_newer(&session.session_id, vec![first, second])
            .expect("new snapshot check"));
        assert_eq!(bridge.message_count(&session.session_id), 2);
    }

    #[test]
    fn hydration_rejects_a_message_bound_to_a_different_session() {
        let mut bridge = SessionBridge::new();
        let session = bridge.create_session(test_config()).expect("created");
        let cross_session_message = AgentMessage::new(
            "msg.cross-session",
            sdkwork_agent_kernel::AgentMessageRole::User,
            vec![sdkwork_agent_kernel::AgentPart::text(
                "part.cross-session",
                "must not cross session boundaries",
            )],
        )
        .for_session("session.other");

        let error = bridge
            .replace_history(&session.session_id, vec![cross_session_message])
            .expect_err("hydration must preserve message/session isolation");
        assert_eq!(
            error.kind(),
            sdkwork_agent_kernel::KernelErrorKind::Conflict
        );
        assert!(bridge
            .get_history(&session.session_id)
            .expect("history")
            .is_empty());
    }

    #[test]
    fn global_history_budget_fails_before_accounting_can_exceed_the_cap() {
        assert_eq!(
            global_history_total_after_replace(MAX_GLOBAL_SESSION_BRIDGE_HISTORY_BYTES, 10, 10,)
                .expect("same-size replacement"),
            MAX_GLOBAL_SESSION_BRIDGE_HISTORY_BYTES
        );
        let error =
            global_history_total_after_replace(MAX_GLOBAL_SESSION_BRIDGE_HISTORY_BYTES, 0, 1)
                .expect_err("global capacity must be enforced");
        assert_eq!(
            error.kind(),
            sdkwork_agent_kernel::KernelErrorKind::ResourceExhausted
        );
    }

    #[test]
    fn clear_and_remove_release_global_history_accounting() {
        let mut bridge = SessionBridge::new();
        let first = bridge.create_session(test_config()).expect("first session");
        let second = bridge
            .create_session(test_config())
            .expect("second session");
        let message = AgentMessage::new(
            "msg.accounting",
            sdkwork_agent_kernel::AgentMessageRole::User,
            vec![sdkwork_agent_kernel::AgentPart::text(
                "part.accounting",
                "accounted",
            )],
        );
        let bytes = message_size(&message);
        bridge
            .append_message(&first.session_id, message.clone())
            .expect("first append");
        bridge
            .append_message(&second.session_id, message)
            .expect("second append");
        assert_eq!(bridge.total_history_bytes, bytes * 2);

        bridge
            .clear_history(&first.session_id)
            .expect("clear first");
        assert_eq!(bridge.total_history_bytes, bytes);
        assert!(bridge.remove_session(&second.session_id));
        assert_eq!(bridge.total_history_bytes, 0);
    }
}
