use crate::error::{DatabaseError, DatabaseResult};
use crate::sqlite::SqliteDatabase;
use crate::sqlite_repository::map_message_row;
use crate::traits::RuntimeExecutionRepository;
use crate::types::{
    ActionKind, ClaimedRun, EventRow, RunControlAction, RunRow, RunState, StepRow, StepState,
    TaskRow,
};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use std::str::FromStr;

pub(crate) const RUN_COLUMNS: &str = "run_id, task_id, session_id, attempt, state,
    next_attempt_at, lease_owner, lease_expires_at, fencing_token,
    cancel_requested_at, started_at, finished_at, error_kind, error_code,
    error_detail, created_at, updated_at";
pub(crate) const STEP_COLUMNS: &str = "step_id, run_id, sequence_no, action_kind, state,
    provider_id, descriptor_revision, policy_revision, causation_step_id,
    idempotency_key_hash, result_json, error_kind, error_code, error_detail,
    started_at, finished_at, created_at, updated_at";

fn enum_sql_error(column: usize, value: String, error: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{error}; value={value}"),
        )),
    )
}

pub(crate) fn map_run_row(row: &Row<'_>) -> rusqlite::Result<RunRow> {
    let state: String = row.get("state")?;
    let state =
        RunState::from_str(&state).map_err(|error| enum_sql_error(4, state.clone(), error))?;
    Ok(RunRow {
        run_id: row.get("run_id")?,
        task_id: row.get("task_id")?,
        session_id: row.get("session_id")?,
        attempt: row.get("attempt")?,
        state,
        next_attempt_at: row.get("next_attempt_at")?,
        lease_owner: row.get("lease_owner")?,
        lease_expires_at: row.get("lease_expires_at")?,
        fencing_token: row.get("fencing_token")?,
        cancel_requested_at: row.get("cancel_requested_at")?,
        started_at: row.get("started_at")?,
        finished_at: row.get("finished_at")?,
        error_kind: row.get("error_kind")?,
        error_code: row.get("error_code")?,
        error_detail: row.get("error_detail")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

pub(crate) fn map_step_row(row: &Row<'_>) -> rusqlite::Result<StepRow> {
    let action_kind: String = row.get("action_kind")?;
    let state: String = row.get("state")?;
    let action_kind = ActionKind::from_str(&action_kind)
        .map_err(|error| enum_sql_error(3, action_kind.clone(), error))?;
    let state =
        StepState::from_str(&state).map_err(|error| enum_sql_error(4, state.clone(), error))?;
    Ok(StepRow {
        step_id: row.get("step_id")?,
        run_id: row.get("run_id")?,
        sequence_no: row.get("sequence_no")?,
        action_kind,
        state,
        provider_id: row.get("provider_id")?,
        descriptor_revision: row.get("descriptor_revision")?,
        policy_revision: row.get("policy_revision")?,
        causation_step_id: row.get("causation_step_id")?,
        idempotency_key_hash: row.get("idempotency_key_hash")?,
        result_json: row.get("result_json")?,
        error_kind: row.get("error_kind")?,
        error_code: row.get("error_code")?,
        error_detail: row.get("error_detail")?,
        started_at: row.get("started_at")?,
        finished_at: row.get("finished_at")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

pub(crate) fn insert_run(tx: &Transaction<'_>, run: &RunRow) -> DatabaseResult<()> {
    tx.execute(
        "INSERT INTO runs (
            run_id, task_id, session_id, attempt, state, next_attempt_at,
            lease_owner, lease_expires_at, fencing_token, cancel_requested_at,
            started_at, finished_at, error_kind, error_code, error_detail,
            created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                   ?13, ?14, ?15, ?16, ?17)",
        params![
            run.run_id,
            run.task_id,
            run.session_id,
            run.attempt,
            run.state.as_str(),
            run.next_attempt_at,
            run.lease_owner,
            run.lease_expires_at,
            run.fencing_token,
            run.cancel_requested_at,
            run.started_at,
            run.finished_at,
            run.error_kind,
            run.error_code,
            run.error_detail,
            run.created_at,
            run.updated_at,
        ],
    )?;
    Ok(())
}

pub(crate) fn insert_step(tx: &Transaction<'_>, step: &StepRow) -> DatabaseResult<()> {
    tx.execute(
        "INSERT INTO steps (
            step_id, run_id, sequence_no, action_kind, state, provider_id,
            descriptor_revision, policy_revision, causation_step_id,
            idempotency_key_hash, result_json, error_kind, error_code,
            error_detail, started_at, finished_at, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                   ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            step.step_id,
            step.run_id,
            step.sequence_no,
            step.action_kind.as_str(),
            step.state.as_str(),
            step.provider_id,
            step.descriptor_revision,
            step.policy_revision,
            step.causation_step_id,
            step.idempotency_key_hash,
            step.result_json,
            step.error_kind,
            step.error_code,
            step.error_detail,
            step.started_at,
            step.finished_at,
            step.created_at,
            step.updated_at,
        ],
    )?;
    Ok(())
}

pub(crate) fn insert_task(tx: &Transaction<'_>, task: &TaskRow) -> DatabaseResult<()> {
    let session_state = tx
        .query_row(
            "SELECT state FROM sessions WHERE session_id = ?1",
            [&task.session_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| {
            DatabaseError::NotFound(format!("session not found: {}", task.session_id))
        })?;
    if crate::types::session_state_is_terminal(&session_state) {
        return Err(DatabaseError::ConstraintViolation(format!(
            "session {} is terminal ({session_state})",
            task.session_id
        )));
    }
    tx.execute(
        "INSERT INTO tasks (
            task_id, session_id, instruction, state, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            task.task_id,
            task.session_id,
            task.instruction,
            task.state,
            task.created_at,
            task.updated_at,
        ],
    )?;
    Ok(())
}

fn ensure_claim_identity(claim: &ClaimedRun) -> DatabaseResult<(&str, i64)> {
    let owner = claim.run.lease_owner.as_deref().ok_or_else(|| {
        DatabaseError::ConstraintViolation("claimed run has no lease owner".to_string())
    })?;
    if claim.run.fencing_token < 1 || claim.step.run_id != claim.run.run_id {
        return Err(DatabaseError::ConstraintViolation(
            "invalid claimed run identity".to_string(),
        ));
    }
    Ok((owner, claim.run.fencing_token))
}

fn load_claimed_rows(conn: &Connection, run_id: &str) -> DatabaseResult<ClaimedRun> {
    let run = conn
        .query_row(
            &format!("SELECT {RUN_COLUMNS} FROM runs WHERE run_id = ?1"),
            [run_id],
            map_run_row,
        )
        .optional()?
        .ok_or_else(|| DatabaseError::NotFound(format!("run not found: {run_id}")))?;
    let step = conn
        .query_row(
            &format!(
                "SELECT {STEP_COLUMNS} FROM steps
                 WHERE run_id = ?1 AND state IN ('ready', 'running')
                 ORDER BY sequence_no LIMIT 1"
            ),
            [run_id],
            map_step_row,
        )
        .optional()?
        .ok_or_else(|| {
            DatabaseError::ConstraintViolation(format!("run {run_id} has no claimable step"))
        })?;
    Ok(ClaimedRun { run, step })
}

impl RuntimeExecutionRepository for SqliteDatabase {
    fn create_task_execution(
        &self,
        task: &TaskRow,
        run: &RunRow,
        step: &StepRow,
        event: &EventRow,
    ) -> DatabaseResult<()> {
        if run.task_id != task.task_id
            || run.session_id != task.session_id
            || step.run_id != run.run_id
            || run.attempt != 1
            || run.state != RunState::Created
            || step.state != StepState::Ready
        {
            return Err(DatabaseError::ConstraintViolation(
                "initial task/run/step identity or state is invalid".to_string(),
            ));
        }
        crate::event_identity::ensure_event_session(event, &task.session_id, "task acceptance")?;
        let mut conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_task(&tx, task)?;
        insert_run(&tx, run)?;
        insert_step(&tx, step)?;
        crate::sqlite_repository::sqlite_save_event_idempotent(&tx, event)?;
        tx.commit()?;
        Ok(())
    }

    fn load_run(&self, run_id: &str) -> DatabaseResult<Option<RunRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        conn.query_row(
            &format!("SELECT {RUN_COLUMNS} FROM runs WHERE run_id = ?1"),
            [run_id],
            map_run_row,
        )
        .optional()
        .map_err(Into::into)
    }

    fn load_steps(&self, run_id: &str) -> DatabaseResult<Vec<StepRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let mut statement = conn.prepare(&format!(
            "SELECT {STEP_COLUMNS} FROM steps
             WHERE run_id = ?1 ORDER BY sequence_no LIMIT 201"
        ))?;
        let rows = statement.query_map([run_id], map_step_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn next_task_attempt(&self, task_id: &str) -> DatabaseResult<i64> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE task_id = ?1)",
            [task_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(DatabaseError::NotFound(format!(
                "task not found: {task_id}"
            )));
        }
        conn.query_row(
            "SELECT COALESCE(MAX(attempt), 0) + 1 FROM runs WHERE task_id = ?1",
            [task_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    fn claim_ready_run(
        &self,
        worker_id: &str,
        now: &str,
        lease_expires_at: &str,
    ) -> DatabaseResult<Option<ClaimedRun>> {
        if worker_id.trim().is_empty() || lease_expires_at <= now {
            return Err(DatabaseError::ConstraintViolation(
                "worker id and future lease expiry are required".to_string(),
            ));
        }
        let mut conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run_id = tx
            .query_row(
                "SELECT run_id FROM runs
                 WHERE state IN ('created', 'planning', 'executing')
                   AND cancel_requested_at IS NULL
                   AND (next_attempt_at IS NULL OR next_attempt_at <= ?1)
                   AND (lease_expires_at IS NULL OR lease_expires_at <= ?1)
                 ORDER BY COALESCE(next_attempt_at, created_at), created_at, run_id
                 LIMIT 1",
                [now],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(run_id) = run_id else {
            tx.commit()?;
            return Ok(None);
        };
        let changed = tx.execute(
            "UPDATE runs
             SET state = 'planning', lease_owner = ?1, lease_expires_at = ?2,
                 fencing_token = fencing_token + 1,
                 started_at = COALESCE(started_at, ?3), updated_at = ?3
             WHERE run_id = ?4
               AND state IN ('created', 'planning', 'executing')
               AND cancel_requested_at IS NULL
               AND (lease_expires_at IS NULL OR lease_expires_at <= ?3)",
            params![worker_id, lease_expires_at, now, run_id],
        )?;
        if changed != 1 {
            return Err(DatabaseError::Transaction(
                "SQLite run claim lost its serialized candidate".to_string(),
            ));
        }
        tx.execute(
            "UPDATE steps SET state = 'ready', updated_at = ?1
             WHERE run_id = ?2 AND state IN ('created', 'ready', 'running')",
            params![now, run_id],
        )?;
        tx.execute(
            "UPDATE tasks SET state = 'accepted', updated_at = ?1
             WHERE task_id = (SELECT task_id FROM runs WHERE run_id = ?2)
               AND lower(state) NOT IN ('completed', 'failed', 'cancelled', 'canceled')",
            params![now, run_id],
        )?;
        let claim = load_claimed_rows(&tx, &run_id)?;
        tx.commit()?;
        Ok(Some(claim))
    }

    fn renew_run_lease(
        &self,
        run_id: &str,
        worker_id: &str,
        fencing_token: i64,
        now: &str,
        lease_expires_at: &str,
    ) -> DatabaseResult<bool> {
        if fencing_token < 1 || lease_expires_at <= now {
            return Ok(false);
        }
        let conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let changed = conn.execute(
            "UPDATE runs SET lease_expires_at = ?1, updated_at = ?2
             WHERE run_id = ?3 AND lease_owner = ?4 AND fencing_token = ?5
               AND lease_expires_at > ?2 AND cancel_requested_at IS NULL
               AND state IN ('planning', 'executing', 'awaiting_permission')",
            params![lease_expires_at, now, run_id, worker_id, fencing_token],
        )?;
        Ok(changed == 1)
    }

    fn start_claimed_run(
        &self,
        claim: &ClaimedRun,
        started_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<()> {
        let (owner, fence) = ensure_claim_identity(claim)?;
        crate::event_identity::ensure_event_session(event, &claim.run.session_id, "run start")?;
        let mut conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run_changed = tx.execute(
            "UPDATE runs SET state = 'executing', started_at = COALESCE(started_at, ?1),
                 updated_at = ?1
             WHERE run_id = ?2 AND lease_owner = ?3 AND fencing_token = ?4
               AND cancel_requested_at IS NULL AND state = 'planning'",
            params![started_at, claim.run.run_id, owner, fence],
        )?;
        let step_changed = tx.execute(
            "UPDATE steps SET state = 'running', started_at = COALESCE(started_at, ?1),
                 updated_at = ?1
             WHERE step_id = ?2 AND run_id = ?3 AND state = 'ready'",
            params![started_at, claim.step.step_id, claim.run.run_id],
        )?;
        let task_changed = tx.execute(
            "UPDATE tasks SET state = 'running', updated_at = ?1
             WHERE task_id = ?2 AND lower(state) IN ('created', 'accepted', 'planned')",
            params![started_at, claim.run.task_id],
        )?;
        if run_changed != 1 || step_changed != 1 || task_changed != 1 {
            return Err(DatabaseError::ConstraintViolation(
                "run start rejected for stale lease, fence, or state".to_string(),
            ));
        }
        crate::sqlite_repository::sqlite_save_event_idempotent(&tx, event)?;
        tx.commit()?;
        Ok(())
    }

    fn complete_claimed_run(
        &self,
        claim: &ClaimedRun,
        result_json: Option<&str>,
        finished_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<()> {
        self.finish_claimed_run(
            claim,
            &[],
            "completed",
            "completed",
            result_json,
            None,
            None,
            None,
            finished_at,
            event,
        )
    }

    fn complete_claimed_run_with_messages(
        &self,
        claim: &ClaimedRun,
        messages: &[crate::types::MessageRow],
        result_json: Option<&str>,
        finished_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<()> {
        self.finish_claimed_run(
            claim,
            messages,
            "completed",
            "completed",
            result_json,
            None,
            None,
            None,
            finished_at,
            event,
        )
    }

    fn fail_claimed_run(
        &self,
        claim: &ClaimedRun,
        error_kind: &str,
        error_code: Option<&str>,
        error_detail: &str,
        finished_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<()> {
        self.finish_claimed_run(
            claim,
            &[],
            "failed",
            "failed",
            None,
            Some(error_kind),
            error_code,
            Some(error_detail),
            finished_at,
            event,
        )
    }

    fn schedule_run_retry(
        &self,
        claim: &ClaimedRun,
        error_kind: &str,
        error_code: Option<&str>,
        error_detail: &str,
        next_attempt_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<()> {
        let (owner, fence) = ensure_claim_identity(claim)?;
        crate::event_identity::ensure_event_session(event, &claim.run.session_id, "run retry")?;
        let mut conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run_changed = tx.execute(
            "UPDATE runs SET state = 'created', lease_owner = NULL, lease_expires_at = NULL,
                 next_attempt_at = ?1, error_kind = ?2, error_code = ?3, error_detail = ?4,
                 attempt = attempt + 1, fencing_token = fencing_token + 1, updated_at = ?1
             WHERE run_id = ?5 AND lease_owner = ?6 AND fencing_token = ?7
               AND cancel_requested_at IS NULL",
            params![
                next_attempt_at,
                error_kind,
                error_code,
                error_detail,
                claim.run.run_id,
                owner,
                fence,
            ],
        )?;
        let step_changed = tx.execute(
            "UPDATE steps SET state = 'ready', error_kind = NULL, error_code = NULL,
                 error_detail = NULL, updated_at = ?1
             WHERE step_id = ?2 AND run_id = ?3",
            params![next_attempt_at, claim.step.step_id, claim.run.run_id],
        )?;
        if run_changed != 1 || step_changed != 1 {
            return Err(DatabaseError::ConstraintViolation(
                "run retry rejected for stale lease, fence, or state".to_string(),
            ));
        }
        crate::sqlite_repository::sqlite_save_event_idempotent(&tx, event)?;
        tx.commit()?;
        Ok(())
    }

    fn request_task_cancellation(
        &self,
        task_id: &str,
        requested_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<(TaskRow, bool)> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = load_task(&tx, task_id)?;
        crate::event_identity::ensure_event_session(event, &task.session_id, "task cancellation")?;
        if matches!(
            task.state.to_ascii_lowercase().as_str(),
            "cancelled" | "canceled"
        ) {
            tx.commit()?;
            return Ok((task, false));
        }
        if task.state.eq_ignore_ascii_case("completed")
            || task.state.eq_ignore_ascii_case("failed")
        {
            return Err(DatabaseError::ConstraintViolation(
                "completed or failed task cannot be cancelled".to_string(),
            ));
        }
        tx.execute(
            "UPDATE runs SET state = 'cancelled', cancel_requested_at = ?1,
                 lease_owner = NULL, lease_expires_at = NULL,
                 fencing_token = fencing_token + 1, finished_at = ?1,
                 updated_at = ?1
             WHERE task_id = ?2 AND state NOT IN ('completed', 'failed', 'cancelled')",
            params![requested_at, task_id],
        )?;
        tx.execute(
            "UPDATE steps SET state = 'cancelled', finished_at = ?1, updated_at = ?1
             WHERE run_id IN (SELECT run_id FROM runs WHERE task_id = ?2)
               AND state NOT IN ('completed', 'failed', 'skipped', 'cancelled')",
            params![requested_at, task_id],
        )?;
        // Fence out any permission worker holding a live claim for this task:
        // cancel the operation, drop its lease, bump its fencing token, and
        // crypto-erase the payload so a stale claim can no longer complete.
        tx.execute(
            "UPDATE permission_operations SET state = 'cancelled',
                 payload_ref = '', payload_digest = '', encryption_key_id = NULL,
                 lease_owner = NULL, lease_expires_at = NULL,
                 fencing_token = fencing_token + 1, updated_at = ?1
             WHERE run_id IN (SELECT run_id FROM runs WHERE task_id = ?2)
               AND state IN ('pending', 'claimable', 'executing')",
            params![requested_at, task_id],
        )?;
        tx.execute(
            "UPDATE tasks SET state = 'cancelled', updated_at = ?1 WHERE task_id = ?2",
            params![requested_at, task_id],
        )?;
        crate::sqlite_repository::sqlite_save_event_idempotent(&tx, event)?;
        let updated = load_task(&tx, task_id)?;
        tx.commit()?;
        Ok((updated, true))
    }

    fn retry_task_execution(
        &self,
        task_id: &str,
        run: &RunRow,
        step: &StepRow,
        event: &EventRow,
    ) -> DatabaseResult<TaskRow> {
        if run.task_id != task_id
            || step.run_id != run.run_id
            || run.state != RunState::Created
            || step.state != StepState::Ready
        {
            return Err(DatabaseError::ConstraintViolation(
                "retry run/step identity or state is invalid".to_string(),
            ));
        }
        let mut conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task(&tx, task_id)?;
        if !matches!(
            task.state.to_ascii_lowercase().as_str(),
            "failed" | "cancelled" | "canceled"
        ) {
            return Err(DatabaseError::ConstraintViolation(
                "only failed or cancelled tasks can be retried".to_string(),
            ));
        }
        if run.session_id != task.session_id {
            return Err(DatabaseError::ConstraintViolation(
                "retry run session does not match task".to_string(),
            ));
        }
        let next_attempt: i64 = tx.query_row(
            "SELECT COALESCE(MAX(attempt), 0) + 1 FROM runs WHERE task_id = ?1",
            [task_id],
            |row| row.get(0),
        )?;
        if run.attempt != next_attempt {
            return Err(DatabaseError::ConstraintViolation(format!(
                "retry attempt must be {next_attempt}"
            )));
        }
        crate::event_identity::ensure_event_session(event, &task.session_id, "task retry")?;
        insert_run(&tx, run)?;
        insert_step(&tx, step)?;
        tx.execute(
            "UPDATE tasks SET state = 'accepted', updated_at = ?1 WHERE task_id = ?2",
            params![run.created_at, task_id],
        )?;
        crate::sqlite_repository::sqlite_save_event_idempotent(&tx, event)?;
        task.state = "accepted".to_string();
        task.updated_at = Some(run.created_at.clone());
        tx.commit()?;
        Ok(task)
    }

    fn control_run(
        &self,
        run_id: &str,
        action: RunControlAction,
        changed_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<RunRow> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = tx
            .query_row(
                &format!("SELECT {RUN_COLUMNS} FROM runs WHERE run_id = ?1"),
                [run_id],
                map_run_row,
            )
            .optional()?
            .ok_or_else(|| DatabaseError::NotFound(format!("run not found: {run_id}")))?;
        crate::event_identity::ensure_event_session(event, &current.session_id, "run control")?;
        let (from_states, run_state, task_state, step_state, cancel_requested_at, finished_at) =
            match action {
                RunControlAction::Pause => (
                    "'created','planning','executing'",
                    "paused",
                    "paused",
                    "ready",
                    None,
                    None,
                ),
                RunControlAction::Resume => {
                    ("'paused'", "created", "accepted", "ready", None, None)
                }
                RunControlAction::Cancel => (
                    "'created','planning','executing','awaiting_permission','paused'",
                    "cancelled",
                    "cancelled",
                    "cancelled",
                    Some(changed_at),
                    Some(changed_at),
                ),
            };
        if (action == RunControlAction::Pause && current.state == RunState::Paused)
            || (action == RunControlAction::Cancel && current.state == RunState::Cancelled)
        {
            tx.commit()?;
            return Ok(current);
        }
        let sql = format!(
            "UPDATE runs SET state = ?1, lease_owner = NULL, lease_expires_at = NULL,
                 fencing_token = fencing_token + 1, cancel_requested_at = ?2,
                 finished_at = ?3, updated_at = ?4
             WHERE run_id = ?5 AND state IN ({from_states})"
        );
        let changed = tx.execute(
            &sql,
            params![
                run_state,
                cancel_requested_at,
                finished_at,
                changed_at,
                run_id
            ],
        )?;
        if changed != 1 {
            return Err(DatabaseError::ConstraintViolation(format!(
                "run cannot transition from {} through {}",
                current.state, action
            )));
        }
        tx.execute(
            "UPDATE steps SET state = ?1, finished_at = ?2, updated_at = ?3
             WHERE run_id = ?4 AND state NOT IN ('completed','failed','skipped','cancelled')",
            params![step_state, finished_at, changed_at, run_id],
        )?;
        if action == RunControlAction::Cancel {
            // Fence out permission workers holding a live claim on this run
            // so a stale lease cannot resurrect the cancelled run's task.
            tx.execute(
                "UPDATE permission_operations SET state = 'cancelled',
                     payload_ref = '', payload_digest = '', encryption_key_id = NULL,
                     lease_owner = NULL, lease_expires_at = NULL,
                     fencing_token = fencing_token + 1, updated_at = ?1
                 WHERE run_id = ?2 AND state IN ('pending', 'claimable', 'executing')",
                params![changed_at, run_id],
            )?;
        }
        tx.execute(
            "UPDATE tasks SET state = ?1, updated_at = ?2 WHERE task_id = ?3",
            params![task_state, changed_at, current.task_id],
        )?;
        crate::sqlite_repository::sqlite_save_event_idempotent(&tx, event)?;
        let updated = tx.query_row(
            &format!("SELECT {RUN_COLUMNS} FROM runs WHERE run_id = ?1"),
            [run_id],
            map_run_row,
        )?;
        tx.commit()?;
        Ok(updated)
    }
}

impl SqliteDatabase {
    #[allow(clippy::too_many_arguments)]
    fn finish_claimed_run(
        &self,
        claim: &ClaimedRun,
        messages: &[crate::types::MessageRow],
        terminal_state: &str,
        step_state: &str,
        result_json: Option<&str>,
        error_kind: Option<&str>,
        error_code: Option<&str>,
        error_detail: Option<&str>,
        finished_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<()> {
        let (owner, fence) = ensure_claim_identity(claim)?;
        crate::event_identity::ensure_event_session(event, &claim.run.session_id, "run finish")?;
        let mut conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE runs SET state = ?1, lease_owner = NULL, lease_expires_at = NULL,
                 error_kind = ?2, error_code = ?3,
                 error_detail = ?4, finished_at = ?5, updated_at = ?5
             WHERE run_id = ?6 AND lease_owner = ?7 AND fencing_token = ?8
               AND cancel_requested_at IS NULL AND state IN ('planning', 'executing')",
            params![
                terminal_state,
                error_kind,
                error_code,
                error_detail,
                finished_at,
                claim.run.run_id,
                owner,
                fence
            ],
        )?;
        if changed != 1 {
            return Err(DatabaseError::ConstraintViolation(
                "run finish rejected for stale lease, fence, or state".to_string(),
            ));
        }
        if messages
            .iter()
            .any(|message| message.session_id != claim.run.session_id)
        {
            return Err(DatabaseError::ConstraintViolation(
                "completed run messages must belong to the run session".to_string(),
            ));
        }
        // The streaming path may already have persisted these messages; an
        // exact retry must not fail the completion, while a conflicting
        // payload for the same id must still be rejected.
        let mut inserted = 0usize;
        for message in messages {
            let existing = tx
                .query_row(
                    "SELECT message_id, session_id, role, content, created_at, metadata_json
                     FROM messages WHERE message_id = ?1",
                    params![message.message_id],
                    map_message_row,
                )
                .optional()
                .map_err(|error| {
                    DatabaseError::Query(format!("failed to load run-message retry: {error}"))
                })?;
            if let Some(existing) = existing {
                crate::message_identity::ensure_message_retry_matches(&existing, message)?;
                continue;
            }
            tx.execute(
                "INSERT INTO messages (
                    message_id, session_id, role, content, created_at, metadata_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    message.message_id,
                    message.session_id,
                    message.role,
                    message.content,
                    message.created_at,
                    message.metadata_json,
                ],
            )?;
            inserted += 1;
        }
        if inserted > 0 {
            let added = i64::try_from(inserted).map_err(|_| {
                DatabaseError::ConstraintViolation("message turn size overflow".to_string())
            })?;
            let session_changed = tx.execute(
                "UPDATE sessions
                 SET message_count = message_count + ?1, updated_at = ?2
                 WHERE session_id = ?3 AND lower(state) = 'active'
                   AND message_count <= ?4",
                params![added, finished_at, claim.run.session_id, i64::MAX - added],
            )?;
            if session_changed != 1 {
                return Err(DatabaseError::ConstraintViolation(
                    "run session is not active or message count is exhausted".to_string(),
                ));
            }
        }
        let step_changed = tx.execute(
            "UPDATE steps SET state = ?1, result_json = ?2, error_kind = ?3,
                 error_code = ?4, error_detail = ?5, finished_at = ?6,
                 updated_at = ?6
             WHERE step_id = ?7 AND run_id = ?8 AND state IN ('ready', 'running')",
            params![
                step_state,
                result_json,
                error_kind,
                error_code,
                error_detail,
                finished_at,
                claim.step.step_id,
                claim.run.run_id
            ],
        )?;
        if step_changed != 1 {
            return Err(DatabaseError::ConstraintViolation(
                "run step finish rejected for stale state".to_string(),
            ));
        }
        let task_changed = tx.execute(
            "UPDATE tasks SET state = ?1, updated_at = ?2
             WHERE task_id = ?3 AND lower(state) NOT IN ('cancelled', 'canceled')",
            params![terminal_state, finished_at, claim.run.task_id],
        )?;
        if task_changed != 1 {
            return Err(DatabaseError::ConstraintViolation(
                "task finish rejected for stale state".to_string(),
            ));
        }
        crate::sqlite_repository::sqlite_save_event_idempotent(&tx, event)?;
        tx.commit()?;
        Ok(())
    }
}

fn load_task(conn: &Connection, task_id: &str) -> DatabaseResult<TaskRow> {
    conn.query_row(
        "SELECT task_id, session_id, instruction, state, created_at, updated_at
         FROM tasks WHERE task_id = ?1",
        [task_id],
        crate::sqlite_repository::map_task_row,
    )
    .optional()?
    .ok_or_else(|| DatabaseError::NotFound(format!("task not found: {task_id}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::{
        MessageRepository, RuntimeSessionWrites, SessionRepository, TaskRepository,
    };
    use crate::types::{runtime_now_timestamp, MessageQuery, MessageRow, SessionRow};

    fn session() -> SessionRow {
        SessionRow {
            session_id: "session.execution".into(),
            agent_id: "agent.test".into(),
            kind: "main".into(),
            source: "api".into(),
            state: "active".into(),
            title: None,
            model: None,
            cwd: None,
            provider_id: None,
            bridge_id: None,
            token_usage_json: None,
            message_count: 0,
            owner_tenant_id: Some("tenant.test".into()),
            owner_user_ref: Some("user.test".into()),
            created_at: runtime_now_timestamp(),
            updated_at: None,
            metadata_json: None,
        }
    }

    fn execution(attempt: i64, suffix: &str) -> (TaskRow, RunRow, StepRow, EventRow) {
        let now = runtime_now_timestamp();
        let task = TaskRow {
            task_id: "task.execution".into(),
            session_id: "session.execution".into(),
            instruction: "perform the requested operation".into(),
            state: "created".into(),
            created_at: now.clone(),
            updated_at: Some(now.clone()),
        };
        let run = RunRow {
            run_id: format!("run.{suffix}"),
            task_id: task.task_id.clone(),
            session_id: task.session_id.clone(),
            attempt,
            state: RunState::Created,
            next_attempt_at: None,
            lease_owner: None,
            lease_expires_at: None,
            fencing_token: 0,
            cancel_requested_at: None,
            started_at: None,
            finished_at: None,
            error_kind: None,
            error_code: None,
            error_detail: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        let step = StepRow {
            step_id: format!("step.{suffix}"),
            run_id: run.run_id.clone(),
            sequence_no: 0,
            action_kind: ActionKind::ModelCall,
            state: StepState::Ready,
            provider_id: None,
            descriptor_revision: None,
            policy_revision: None,
            causation_step_id: None,
            idempotency_key_hash: None,
            result_json: None,
            error_kind: None,
            error_code: None,
            error_detail: None,
            started_at: None,
            finished_at: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        let event = EventRow {
            event_id: format!("event.{suffix}"),
            session_id: Some(task.session_id.clone()),
            event_type: "task.accepted".into(),
            severity: "info".into(),
            payload: None,
            created_at: now,
        };
        (task, run, step, event)
    }

    fn migrated_database() -> SqliteDatabase {
        let database = SqliteDatabase::memory_migrated().expect("migrated database");
        database.save_session(&session()).expect("session");
        database
    }

    #[test]
    fn claim_complete_is_fenced_and_atomic() {
        let database = migrated_database();
        let (task, run, step, accepted) = execution(1, "initial");
        database
            .create_task_execution(&task, &run, &step, &accepted)
            .expect("execution created");

        let claim = database
            .claim_ready_run(
                "worker.one",
                "2026-07-17T00:00:00.000000000Z",
                "2026-07-17T00:01:00.000000000Z",
            )
            .expect("claim")
            .expect("ready run");
        assert_eq!(claim.run.fencing_token, 1);
        assert_eq!(claim.run.lease_owner.as_deref(), Some("worker.one"));
        assert!(!database
            .renew_run_lease(
                &claim.run.run_id,
                "worker.two",
                claim.run.fencing_token,
                "2026-07-17T00:00:10.000000000Z",
                "2026-07-17T00:01:10.000000000Z",
            )
            .expect("foreign renewal rejected"));

        let started = EventRow {
            event_id: "event.started".into(),
            session_id: Some(task.session_id.clone()),
            event_type: "task.started".into(),
            severity: "info".into(),
            payload: None,
            created_at: "2026-07-17T00:00:15.000000000Z".into(),
        };
        database
            .start_claimed_run(&claim, &started.created_at, &started)
            .expect("started");

        let completed = EventRow {
            event_id: "event.completed".into(),
            session_id: Some(task.session_id.clone()),
            event_type: "task.completed".into(),
            severity: "info".into(),
            payload: None,
            created_at: "2026-07-17T00:00:20.000000000Z".into(),
        };
        database
            .complete_claimed_run(
                &claim,
                Some(r#"{"output":"done"}"#),
                &completed.created_at,
                &completed,
            )
            .expect("completed");
        assert_eq!(
            database
                .load_run(&claim.run.run_id)
                .expect("run")
                .expect("present")
                .state,
            RunState::Completed
        );
        assert_eq!(
            database
                .load_task(&task.task_id)
                .expect("task")
                .expect("present")
                .state,
            "completed"
        );
        assert!(database
            .claim_ready_run(
                "worker.two",
                "2026-07-17T00:02:00.000000000Z",
                "2026-07-17T00:03:00.000000000Z",
            )
            .expect("empty claim")
            .is_none());
    }

    #[test]
    fn cancellation_invalidates_in_flight_claim() {
        let database = migrated_database();
        let (task, run, step, accepted) = execution(1, "cancel");
        database
            .create_task_execution(&task, &run, &step, &accepted)
            .expect("execution created");
        let claim = database
            .claim_ready_run(
                "worker.one",
                "2026-07-17T00:00:00.000000000Z",
                "2026-07-17T00:01:00.000000000Z",
            )
            .expect("claim")
            .expect("ready run");
        let cancelled = EventRow {
            event_id: "event.cancelled".into(),
            session_id: Some(task.session_id.clone()),
            event_type: "task.cancelled".into(),
            severity: "info".into(),
            payload: None,
            created_at: "2026-07-17T00:00:10.000000000Z".into(),
        };
        let (_, changed) = database
            .request_task_cancellation(&task.task_id, &cancelled.created_at, &cancelled)
            .expect("cancelled");
        assert!(changed);
        let (_, changed) = database
            .request_task_cancellation(&task.task_id, &cancelled.created_at, &cancelled)
            .expect("idempotent cancellation");
        assert!(!changed);

        let stale_finish = database.complete_claimed_run(
            &claim,
            None,
            "2026-07-17T00:00:20.000000000Z",
            &EventRow {
                event_id: "event.stale".into(),
                session_id: Some(task.session_id),
                event_type: "task.completed".into(),
                severity: "info".into(),
                payload: None,
                created_at: "2026-07-17T00:00:20.000000000Z".into(),
            },
        );
        assert!(matches!(
            stale_finish,
            Err(DatabaseError::ConstraintViolation(_))
        ));
    }

    #[test]
    fn failed_task_retry_requires_next_attempt() {
        let database = migrated_database();
        let (task, run, step, accepted) = execution(1, "failure");
        database
            .create_task_execution(&task, &run, &step, &accepted)
            .expect("execution created");
        let claim = database
            .claim_ready_run(
                "worker.one",
                "2026-07-17T00:00:00.000000000Z",
                "2026-07-17T00:01:00.000000000Z",
            )
            .expect("claim")
            .expect("ready run");
        let failed = EventRow {
            event_id: "event.failed".into(),
            session_id: Some(task.session_id.clone()),
            event_type: "task.failed".into(),
            severity: "error".into(),
            payload: None,
            created_at: "2026-07-17T00:00:10.000000000Z".into(),
        };
        database
            .fail_claimed_run(
                &claim,
                "provider_unavailable",
                Some("50301"),
                "provider unavailable",
                &failed.created_at,
                &failed,
            )
            .expect("failed");

        let (_, mut retry_run, mut retry_step, mut retry_event) = execution(2, "retry");
        retry_run.created_at = "2026-07-17T00:00:20.000000000Z".into();
        retry_run.updated_at = retry_run.created_at.clone();
        retry_step.created_at = retry_run.created_at.clone();
        retry_step.updated_at = retry_run.created_at.clone();
        retry_event.event_type = "task.retried".into();
        retry_event.created_at = retry_run.created_at.clone();
        let retried = database
            .retry_task_execution(&task.task_id, &retry_run, &retry_step, &retry_event)
            .expect("retried");
        assert_eq!(retried.state, "accepted");
        assert_eq!(
            database.load_steps(&retry_run.run_id).expect("steps").len(),
            1
        );
    }

    #[test]
    fn transient_failure_is_rescheduled_and_reclaimable_after_backoff() {
        let database = migrated_database();
        let (task, run, step, accepted) = execution(1, "retryable");
        database
            .create_task_execution(&task, &run, &step, &accepted)
            .expect("execution created");

        let claim = database
            .claim_ready_run(
                "worker.one",
                "2026-07-17T00:00:00.000000000Z",
                "2026-07-17T00:01:00.000000000Z",
            )
            .expect("claim")
            .expect("ready run");
        let started = EventRow {
            event_id: "event.retryable.started".into(),
            session_id: Some(task.session_id.clone()),
            event_type: "task.started".into(),
            severity: "info".into(),
            payload: None,
            created_at: "2026-07-17T00:00:05.000000000Z".into(),
        };
        database
            .start_claimed_run(&claim, &started.created_at, &started)
            .expect("started");

        // Transient failure schedules a retry instead of failing the run.
        let retry_event = EventRow {
            event_id: "event.retryable.retry".into(),
            session_id: Some(task.session_id.clone()),
            event_type: "task.retry_scheduled".into(),
            severity: "warning".into(),
            payload: None,
            created_at: "2026-07-17T00:00:06.000000000Z".into(),
        };
        database
            .schedule_run_retry(
                &claim,
                "provider_unavailable",
                Some("50301"),
                "provider unavailable",
                "2026-07-17T00:01:06.000000000Z",
                &retry_event,
            )
            .expect("retry scheduled");

        let run_row = database
            .load_run(&run.run_id)
            .expect("load")
            .expect("run present");
        assert_eq!(
            run_row.state,
            RunState::Created,
            "run returns to a claimable state"
        );
        assert_eq!(run_row.attempt, 2, "retry increments the attempt");
        assert_eq!(run_row.lease_owner, None, "lease is released");
        assert_eq!(
            run_row.next_attempt_at.as_deref(),
            Some("2026-07-17T00:01:06.000000000Z"),
            "backoff deadline is persisted"
        );

        // Before the backoff deadline the run is not claimable...
        assert!(database
            .claim_ready_run(
                "worker.two",
                "2026-07-17T00:00:30.000000000Z",
                "2026-07-17T00:01:00.000000000Z",
            )
            .expect("early claim")
            .is_none());
        // ...and after the deadline a fresh worker picks it up with a new fence.
        let re_claim = database
            .claim_ready_run(
                "worker.two",
                "2026-07-17T00:01:10.000000000Z",
                "2026-07-17T00:02:10.000000000Z",
            )
            .expect("late claim")
            .expect("reclaimable run");
        assert_eq!(re_claim.run.attempt, 2);
        assert_eq!(re_claim.run.lease_owner.as_deref(), Some("worker.two"));
        assert_eq!(
            re_claim.run.fencing_token, 3,
            "stale claim fence cannot renew or finish"
        );

        // The stale claim's renew and finish attempts are rejected.
        assert!(!database
            .renew_run_lease(
                &run.run_id,
                "worker.one",
                1,
                "2026-07-17T00:01:20.000000000Z",
                "2026-07-17T00:02:20.000000000Z",
            )
            .expect("stale renewal rejected"));
    }

    #[test]
    fn completion_accepts_messages_already_persisted_by_the_streaming_path() {
        let database = migrated_database();
        let (task, run, step, accepted) = execution(1, "idem-messages");
        database
            .create_task_execution(&task, &run, &step, &accepted)
            .expect("execution created");

        let claim = database
            .claim_ready_run(
                "worker.one",
                "2026-07-17T00:00:00.000000000Z",
                "2026-07-17T00:01:00.000000000Z",
            )
            .expect("claim")
            .expect("ready run");
        let started = EventRow {
            event_id: "event.idem.started".into(),
            session_id: Some(task.session_id.clone()),
            event_type: "task.started".into(),
            severity: "info".into(),
            payload: None,
            created_at: "2026-07-17T00:00:05.000000000Z".into(),
        };
        database
            .start_claimed_run(&claim, &started.created_at, &started)
            .expect("started");

        let user_message = MessageRow {
            message_id: "msg.idem.user".into(),
            session_id: task.session_id.clone(),
            role: "user".into(),
            content: task.instruction.clone(),
            created_at: "2026-07-17T00:00:06.000000000Z".into(),
            metadata_json: None,
        };
        let assistant_message = MessageRow {
            message_id: "msg.idem.assistant".into(),
            session_id: task.session_id.clone(),
            role: "assistant".into(),
            content: "streamed partial output".into(),
            created_at: "2026-07-17T00:00:07.000000000Z".into(),
            metadata_json: None,
        };
        // The streaming path persists the assistant message first.
        let stream_event = EventRow {
            event_id: "event.idem.streamed".into(),
            session_id: Some(task.session_id.clone()),
            event_type: "message.sent".into(),
            severity: "info".into(),
            payload: Some("role=assistant".to_string()),
            created_at: "2026-07-17T00:00:07.000000000Z".into(),
        };
        database
            .append_message_with_event(&assistant_message, &stream_event)
            .expect("streamed assistant message");

        // The completion commits the same message id plus the user message.
        let completed = EventRow {
            event_id: "event.idem.completed".into(),
            session_id: Some(task.session_id.clone()),
            event_type: "task.completed".into(),
            severity: "info".into(),
            payload: None,
            created_at: "2026-07-17T00:00:08.000000000Z".into(),
        };
        database
            .complete_claimed_run_with_messages(
                &claim,
                &[user_message, assistant_message.clone()],
                None,
                &completed.created_at,
                &completed,
            )
            .expect("completion accepts the already-persisted assistant message");

        let messages = database
            .load_messages(&task.session_id, &crate::types::MessageQuery::default())
            .expect("messages");
        assert_eq!(messages.len(), 2, "no duplicate assistant message");
        let session_row = database
            .load_session(&task.session_id)
            .expect("session")
            .expect("present");
        assert_eq!(
            session_row.message_count, 2,
            "count counts each message exactly once"
        );
    }
}
