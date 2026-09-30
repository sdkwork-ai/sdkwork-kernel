use crate::error::{DatabaseError, DatabaseResult};
use crate::postgres::PostgresDatabase;
use crate::postgres_pool::map_sqlx_error;
use crate::postgres_repository::map_message_row;
use crate::traits::RuntimeExecutionRepository;
use crate::types::{
    ActionKind, ClaimedRun, EventRow, RunControlAction, RunRow, RunState, StepRow, StepState,
    TaskRow,
};
use sqlx::{PgConnection, Row};
use std::str::FromStr;

pub(crate) const RUN_COLUMNS: &str = "run_id, task_id, session_id, attempt, state,
    next_attempt_at, lease_owner, lease_expires_at, fencing_token,
    cancel_requested_at, started_at, finished_at, error_kind, error_code,
    error_detail, created_at, updated_at";
pub(crate) const STEP_COLUMNS: &str = "step_id, run_id, sequence_no, action_kind, state,
    provider_id, descriptor_revision, policy_revision, causation_step_id,
    idempotency_key_hash, result_json, error_kind, error_code, error_detail,
    started_at, finished_at, created_at, updated_at";

pub(crate) fn map_run_row(row: &sqlx::postgres::PgRow) -> DatabaseResult<RunRow> {
    let state: String = row.try_get("state").map_err(map_sqlx_error)?;
    Ok(RunRow {
        run_id: row.try_get("run_id").map_err(map_sqlx_error)?,
        task_id: row.try_get("task_id").map_err(map_sqlx_error)?,
        session_id: row.try_get("session_id").map_err(map_sqlx_error)?,
        attempt: row.try_get("attempt").map_err(map_sqlx_error)?,
        state: RunState::from_str(&state).map_err(DatabaseError::Serialization)?,
        next_attempt_at: row.try_get("next_attempt_at").map_err(map_sqlx_error)?,
        lease_owner: row.try_get("lease_owner").map_err(map_sqlx_error)?,
        lease_expires_at: row.try_get("lease_expires_at").map_err(map_sqlx_error)?,
        fencing_token: row.try_get("fencing_token").map_err(map_sqlx_error)?,
        cancel_requested_at: row.try_get("cancel_requested_at").map_err(map_sqlx_error)?,
        started_at: row.try_get("started_at").map_err(map_sqlx_error)?,
        finished_at: row.try_get("finished_at").map_err(map_sqlx_error)?,
        error_kind: row.try_get("error_kind").map_err(map_sqlx_error)?,
        error_code: row.try_get("error_code").map_err(map_sqlx_error)?,
        error_detail: row.try_get("error_detail").map_err(map_sqlx_error)?,
        created_at: row.try_get("created_at").map_err(map_sqlx_error)?,
        updated_at: row.try_get("updated_at").map_err(map_sqlx_error)?,
    })
}

pub(crate) fn map_step_row(row: &sqlx::postgres::PgRow) -> DatabaseResult<StepRow> {
    let action_kind: String = row.try_get("action_kind").map_err(map_sqlx_error)?;
    let state: String = row.try_get("state").map_err(map_sqlx_error)?;
    Ok(StepRow {
        step_id: row.try_get("step_id").map_err(map_sqlx_error)?,
        run_id: row.try_get("run_id").map_err(map_sqlx_error)?,
        sequence_no: row.try_get("sequence_no").map_err(map_sqlx_error)?,
        action_kind: ActionKind::from_str(&action_kind).map_err(DatabaseError::Serialization)?,
        state: StepState::from_str(&state).map_err(DatabaseError::Serialization)?,
        provider_id: row.try_get("provider_id").map_err(map_sqlx_error)?,
        descriptor_revision: row.try_get("descriptor_revision").map_err(map_sqlx_error)?,
        policy_revision: row.try_get("policy_revision").map_err(map_sqlx_error)?,
        causation_step_id: row.try_get("causation_step_id").map_err(map_sqlx_error)?,
        idempotency_key_hash: row
            .try_get("idempotency_key_hash")
            .map_err(map_sqlx_error)?,
        result_json: row.try_get("result_json").map_err(map_sqlx_error)?,
        error_kind: row.try_get("error_kind").map_err(map_sqlx_error)?,
        error_code: row.try_get("error_code").map_err(map_sqlx_error)?,
        error_detail: row.try_get("error_detail").map_err(map_sqlx_error)?,
        started_at: row.try_get("started_at").map_err(map_sqlx_error)?,
        finished_at: row.try_get("finished_at").map_err(map_sqlx_error)?,
        created_at: row.try_get("created_at").map_err(map_sqlx_error)?,
        updated_at: row.try_get("updated_at").map_err(map_sqlx_error)?,
    })
}

pub(crate) async fn insert_run(connection: &mut PgConnection, run: &RunRow) -> DatabaseResult<()> {
    sqlx::query(
        "INSERT INTO runs (
            run_id, task_id, session_id, attempt, state, next_attempt_at,
            lease_owner, lease_expires_at, fencing_token, cancel_requested_at,
            started_at, finished_at, error_kind, error_code, error_detail,
            created_at, updated_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                   $13, $14, $15, $16, $17)",
    )
    .bind(&run.run_id)
    .bind(&run.task_id)
    .bind(&run.session_id)
    .bind(run.attempt)
    .bind(run.state.as_str())
    .bind(&run.next_attempt_at)
    .bind(&run.lease_owner)
    .bind(&run.lease_expires_at)
    .bind(run.fencing_token)
    .bind(&run.cancel_requested_at)
    .bind(&run.started_at)
    .bind(&run.finished_at)
    .bind(&run.error_kind)
    .bind(&run.error_code)
    .bind(&run.error_detail)
    .bind(&run.created_at)
    .bind(&run.updated_at)
    .execute(connection)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

pub(crate) async fn insert_step(
    connection: &mut PgConnection,
    step: &StepRow,
) -> DatabaseResult<()> {
    sqlx::query(
        "INSERT INTO steps (
            step_id, run_id, sequence_no, action_kind, state, provider_id,
            descriptor_revision, policy_revision, causation_step_id,
            idempotency_key_hash, result_json, error_kind, error_code,
            error_detail, started_at, finished_at, created_at, updated_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                   $13, $14, $15, $16, $17, $18)",
    )
    .bind(&step.step_id)
    .bind(&step.run_id)
    .bind(step.sequence_no)
    .bind(step.action_kind.as_str())
    .bind(step.state.as_str())
    .bind(&step.provider_id)
    .bind(&step.descriptor_revision)
    .bind(&step.policy_revision)
    .bind(&step.causation_step_id)
    .bind(&step.idempotency_key_hash)
    .bind(&step.result_json)
    .bind(&step.error_kind)
    .bind(&step.error_code)
    .bind(&step.error_detail)
    .bind(&step.started_at)
    .bind(&step.finished_at)
    .bind(&step.created_at)
    .bind(&step.updated_at)
    .execute(connection)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

fn ensure_claim_identity(claim: &ClaimedRun) -> DatabaseResult<(String, i64)> {
    let owner = claim.run.lease_owner.clone().ok_or_else(|| {
        DatabaseError::ConstraintViolation("claimed run has no lease owner".to_string())
    })?;
    if claim.run.fencing_token < 1 || claim.step.run_id != claim.run.run_id {
        return Err(DatabaseError::ConstraintViolation(
            "invalid claimed run identity".to_string(),
        ));
    }
    Ok((owner, claim.run.fencing_token))
}

impl RuntimeExecutionRepository for PostgresDatabase {
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
        let pool = self.pool.pool().clone();
        let task = task.clone();
        let run = run.clone();
        let step = step.clone();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let session_state = sqlx::query_scalar::<_, String>(
                "SELECT state FROM sessions WHERE session_id = $1 FOR UPDATE",
            )
            .bind(&task.session_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .ok_or_else(|| {
                DatabaseError::NotFound(format!("session not found: {}", task.session_id))
            })?;
            if crate::types::session_state_is_terminal(&session_state) {
                return Err(DatabaseError::ConstraintViolation(format!(
                    "session {} is terminal ({session_state})",
                    task.session_id
                )));
            }
            sqlx::query(
                "INSERT INTO tasks (
                    task_id, session_id, instruction, state, created_at, updated_at
                 ) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(&task.task_id)
            .bind(&task.session_id)
            .bind(&task.instruction)
            .bind(&task.state)
            .bind(&task.created_at)
            .bind(&task.updated_at)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            insert_run(&mut tx, &run).await?;
            insert_step(&mut tx, &step).await?;
            crate::postgres_repository::postgres_save_event_idempotent(&mut *tx, &event).await?;
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(())
        })
    }

    fn load_run(&self, run_id: &str) -> DatabaseResult<Option<RunRow>> {
        let pool = self.pool.pool().clone();
        let run_id = run_id.to_string();
        self.pool.run_db(async move {
            let row = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {RUN_COLUMNS} FROM runs WHERE run_id = $1"
            )))
            .bind(&run_id)
            .fetch_optional(&pool)
            .await
            .map_err(map_sqlx_error)?;
            row.as_ref().map(map_run_row).transpose()
        })
    }

    fn load_steps(&self, run_id: &str) -> DatabaseResult<Vec<StepRow>> {
        let pool = self.pool.pool().clone();
        let run_id = run_id.to_string();
        self.pool.run_db(async move {
            let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {STEP_COLUMNS} FROM steps
                 WHERE run_id = $1 ORDER BY sequence_no LIMIT 201"
            )))
            .bind(&run_id)
            .fetch_all(&pool)
            .await
            .map_err(map_sqlx_error)?;
            rows.iter().map(map_step_row).collect()
        })
    }

    fn next_task_attempt(&self, task_id: &str) -> DatabaseResult<i64> {
        let pool = self.pool.pool().clone();
        let task_id = task_id.to_string();
        self.pool.run_db(async move {
            let exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM tasks WHERE task_id = $1)",
            )
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .map_err(map_sqlx_error)?;
            if !exists {
                return Err(DatabaseError::NotFound(format!(
                    "task not found: {task_id}"
                )));
            }
            sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(MAX(attempt), 0) + 1 FROM runs WHERE task_id = $1",
            )
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .map_err(map_sqlx_error)
        })
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
        let pool = self.pool.pool().clone();
        let worker_id = worker_id.to_string();
        let now = now.to_string();
        let lease_expires_at = lease_expires_at.to_string();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let row = sqlx::query(sqlx::AssertSqlSafe(format!(
                "WITH candidate AS (
                    SELECT run_id FROM runs
                    WHERE state IN ('created', 'planning', 'executing')
                      AND cancel_requested_at IS NULL
                      AND (next_attempt_at IS NULL OR next_attempt_at <= $1)
                      AND (lease_expires_at IS NULL OR lease_expires_at <= $1)
                    ORDER BY COALESCE(next_attempt_at, created_at), created_at, run_id
                    FOR UPDATE SKIP LOCKED
                    LIMIT 1
                 )
                 UPDATE runs AS claimed
                 SET state = 'planning', lease_owner = $2, lease_expires_at = $3,
                     fencing_token = claimed.fencing_token + 1,
                     started_at = COALESCE(claimed.started_at, $1), updated_at = $1
                 FROM candidate
                 WHERE claimed.run_id = candidate.run_id
                 RETURNING {}",
                RUN_COLUMNS
                    .split(", ")
                    .map(|column| format!("claimed.{column}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
            .bind(&now)
            .bind(&worker_id)
            .bind(&lease_expires_at)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            let Some(row) = row else {
                tx.commit().await.map_err(map_sqlx_error)?;
                return Ok(None);
            };
            let run = map_run_row(&row)?;
            sqlx::query(
                "UPDATE steps SET state = 'ready', updated_at = $1
                 WHERE run_id = $2 AND state IN ('created', 'ready', 'running')",
            )
            .bind(&now)
            .bind(&run.run_id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            sqlx::query(
                "UPDATE tasks SET state = 'accepted', updated_at = $1
                 WHERE task_id = $2
                   AND lower(state) NOT IN ('completed', 'failed', 'cancelled', 'canceled')",
            )
            .bind(&now)
            .bind(&run.task_id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            let step_row = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {STEP_COLUMNS} FROM steps
                 WHERE run_id = $1 AND state IN ('ready', 'running')
                 ORDER BY sequence_no LIMIT 1"
            )))
            .bind(&run.run_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .ok_or_else(|| {
                DatabaseError::ConstraintViolation(format!(
                    "run {} has no claimable step",
                    run.run_id
                ))
            })?;
            let step = map_step_row(&step_row)?;
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(Some(ClaimedRun { run, step }))
        })
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
        let pool = self.pool.pool().clone();
        let run_id = run_id.to_string();
        let worker_id = worker_id.to_string();
        let now = now.to_string();
        let lease_expires_at = lease_expires_at.to_string();
        self.pool.run_db(async move {
            let changed = sqlx::query(
                "UPDATE runs SET lease_expires_at = $1, updated_at = $2
                 WHERE run_id = $3 AND lease_owner = $4 AND fencing_token = $5
                   AND lease_expires_at > $2 AND cancel_requested_at IS NULL
                   AND state IN ('planning', 'executing', 'awaiting_permission')",
            )
            .bind(&lease_expires_at)
            .bind(&now)
            .bind(&run_id)
            .bind(&worker_id)
            .bind(fencing_token)
            .execute(&pool)
            .await
            .map_err(map_sqlx_error)?
            .rows_affected();
            Ok(changed == 1)
        })
    }

    fn start_claimed_run(
        &self,
        claim: &ClaimedRun,
        started_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<()> {
        let (owner, fence) = ensure_claim_identity(claim)?;
        crate::event_identity::ensure_event_session(event, &claim.run.session_id, "run start")?;
        let pool = self.pool.pool().clone();
        let claim = claim.clone();
        let started_at = started_at.to_string();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let run_changed = sqlx::query(
                "UPDATE runs SET state = 'executing', started_at = COALESCE(started_at, $1),
                     updated_at = $1
                 WHERE run_id = $2 AND lease_owner = $3 AND fencing_token = $4
                   AND cancel_requested_at IS NULL AND state = 'planning'",
            )
            .bind(&started_at)
            .bind(&claim.run.run_id)
            .bind(&owner)
            .bind(fence)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .rows_affected();
            let step_changed = sqlx::query(
                "UPDATE steps SET state = 'running', started_at = COALESCE(started_at, $1),
                     updated_at = $1
                 WHERE step_id = $2 AND run_id = $3 AND state = 'ready'",
            )
            .bind(&started_at)
            .bind(&claim.step.step_id)
            .bind(&claim.run.run_id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .rows_affected();
            let task_changed = sqlx::query(
                "UPDATE tasks SET state = 'running', updated_at = $1
                 WHERE task_id = $2 AND lower(state) IN ('created', 'accepted', 'planned')",
            )
            .bind(&started_at)
            .bind(&claim.run.task_id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .rows_affected();
            if run_changed != 1 || step_changed != 1 || task_changed != 1 {
                return Err(DatabaseError::ConstraintViolation(
                    "run start rejected for stale lease, fence, or state".to_string(),
                ));
            }
            crate::postgres_repository::postgres_save_event_idempotent(&mut *tx, &event).await?;
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(())
        })
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
        let pool = self.pool.pool().clone();
        let claim = claim.clone();
        let error_kind = error_kind.to_string();
        let error_code = error_code.map(str::to_string);
        let error_detail = error_detail.to_string();
        let next_attempt_at = next_attempt_at.to_string();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let run_changed = sqlx::query(
                "UPDATE runs SET state = 'created', lease_owner = NULL, lease_expires_at = NULL,
                     next_attempt_at = $1, error_kind = $2, error_code = $3, error_detail = $4,
                     attempt = attempt + 1, fencing_token = fencing_token + 1, updated_at = $1
                 WHERE run_id = $5 AND lease_owner = $6 AND fencing_token = $7
                   AND cancel_requested_at IS NULL AND state IN ('planning', 'executing')",
            )
            .bind(&next_attempt_at)
            .bind(&error_kind)
            .bind(&error_code)
            .bind(&error_detail)
            .bind(&claim.run.run_id)
            .bind(&owner)
            .bind(fence)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .rows_affected();
            if run_changed != 1 {
                return Err(DatabaseError::ConstraintViolation(
                    "run retry rejected for stale lease, fence, or state".to_string(),
                ));
            }
            let step_changed = sqlx::query(
                "UPDATE steps SET state = 'ready', error_kind = NULL, error_code = NULL,
                     error_detail = NULL, updated_at = $1
                 WHERE step_id = $2 AND run_id = $3",
            )
            .bind(&next_attempt_at)
            .bind(&claim.step.step_id)
            .bind(&claim.run.run_id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .rows_affected();
            if step_changed != 1 {
                return Err(DatabaseError::ConstraintViolation(
                    "run step retry rejected for stale state".to_string(),
                ));
            }
            crate::postgres_repository::postgres_save_event_idempotent(&mut *tx, &event).await?;
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(())
        })
    }

    fn request_task_cancellation(
        &self,
        task_id: &str,
        requested_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<(TaskRow, bool)> {
        let pool = self.pool.pool().clone();
        let task_id = task_id.to_string();
        let requested_at = requested_at.to_string();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let row = sqlx::query(
                "SELECT task_id, session_id, instruction, state, created_at, updated_at
                 FROM tasks WHERE task_id = $1 FOR UPDATE",
            )
            .bind(&task_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .ok_or_else(|| DatabaseError::NotFound(format!("task not found: {task_id}")))?;
            let mut task = crate::postgres_repository::map_task_row(&row)?;
            crate::event_identity::ensure_event_session(
                &event,
                &task.session_id,
                "task cancellation",
            )?;
            if matches!(
                task.state.to_ascii_lowercase().as_str(),
                "cancelled" | "canceled"
            ) {
                tx.commit().await.map_err(map_sqlx_error)?;
                return Ok((task, false));
            }
            if task.state.eq_ignore_ascii_case("completed")
                || task.state.eq_ignore_ascii_case("failed")
            {
                return Err(DatabaseError::ConstraintViolation(
                    "completed or failed task cannot be cancelled".to_string(),
                ));
            }
            sqlx::query(
                "UPDATE runs SET state = 'cancelled', cancel_requested_at = $1,
                     lease_owner = NULL, lease_expires_at = NULL,
                     fencing_token = fencing_token + 1, finished_at = $1,
                     updated_at = $1
                 WHERE task_id = $2 AND state NOT IN ('completed', 'failed', 'cancelled')",
            )
            .bind(&requested_at)
            .bind(&task_id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            sqlx::query(
                "UPDATE steps SET state = 'cancelled', finished_at = $1, updated_at = $1
                 WHERE run_id IN (SELECT run_id FROM runs WHERE task_id = $2)
                   AND state NOT IN ('completed', 'failed', 'skipped', 'cancelled')",
            )
            .bind(&requested_at)
            .bind(&task_id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            // Fence out any permission worker holding a live claim for this
            // task: cancel the operation, drop its lease, bump its fencing
            // token, and crypto-erase the payload so a stale claim can no
            // longer complete.
            sqlx::query(
                "UPDATE permission_operations SET state = 'cancelled',
                     payload_ref = '', payload_digest = '', encryption_key_id = NULL,
                     lease_owner = NULL, lease_expires_at = NULL,
                     fencing_token = fencing_token + 1, updated_at = $1
                 WHERE run_id IN (SELECT run_id FROM runs WHERE task_id = $2)
                   AND state IN ('pending', 'claimable', 'executing')",
            )
            .bind(&requested_at)
            .bind(&task_id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            sqlx::query("UPDATE tasks SET state = 'cancelled', updated_at = $1 WHERE task_id = $2")
                .bind(&requested_at)
                .bind(&task_id)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
            crate::postgres_repository::postgres_save_event_idempotent(&mut *tx, &event).await?;
            task.state = "cancelled".to_string();
            task.updated_at = Some(requested_at);
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok((task, true))
        })
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
        let pool = self.pool.pool().clone();
        let task_id = task_id.to_string();
        let run = run.clone();
        let step = step.clone();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let row = sqlx::query(
                "SELECT task_id, session_id, instruction, state, created_at, updated_at
                 FROM tasks WHERE task_id = $1 FOR UPDATE",
            )
            .bind(&task_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .ok_or_else(|| DatabaseError::NotFound(format!("task not found: {task_id}")))?;
            let mut task = crate::postgres_repository::map_task_row(&row)?;
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
            let next_attempt = sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(MAX(attempt), 0) + 1 FROM runs WHERE task_id = $1",
            )
            .bind(&task_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            if run.attempt != next_attempt {
                return Err(DatabaseError::ConstraintViolation(format!(
                    "retry attempt must be {next_attempt}"
                )));
            }
            crate::event_identity::ensure_event_session(&event, &task.session_id, "task retry")?;
            insert_run(&mut tx, &run).await?;
            insert_step(&mut tx, &step).await?;
            sqlx::query("UPDATE tasks SET state = 'accepted', updated_at = $1 WHERE task_id = $2")
                .bind(&run.created_at)
                .bind(&task_id)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
            crate::postgres_repository::postgres_save_event_idempotent(&mut *tx, &event).await?;
            task.state = "accepted".to_string();
            task.updated_at = Some(run.created_at);
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(task)
        })
    }

    fn control_run(
        &self,
        run_id: &str,
        action: RunControlAction,
        changed_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<RunRow> {
        let pool = self.pool.pool().clone();
        let run_id = run_id.to_string();
        let changed_at = changed_at.to_string();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let current_row = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {RUN_COLUMNS} FROM runs WHERE run_id = $1 FOR UPDATE"
            )))
            .bind(&run_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .ok_or_else(|| DatabaseError::NotFound(format!("run not found: {run_id}")))?;
            let current = map_run_row(&current_row)?;
            crate::event_identity::ensure_event_session(
                &event,
                &current.session_id,
                "run control",
            )?;
            if (action == RunControlAction::Pause && current.state == RunState::Paused)
                || (action == RunControlAction::Cancel && current.state == RunState::Cancelled)
            {
                tx.commit().await.map_err(map_sqlx_error)?;
                return Ok(current);
            }
            let (from_states, run_state, task_state, step_state, is_cancel) = match action {
                RunControlAction::Pause => (
                    "'created','planning','executing'",
                    "paused",
                    "paused",
                    "ready",
                    false,
                ),
                RunControlAction::Resume => ("'paused'", "created", "accepted", "ready", false),
                RunControlAction::Cancel => (
                    "'created','planning','executing','awaiting_permission','paused'",
                    "cancelled",
                    "cancelled",
                    "cancelled",
                    true,
                ),
            };
            let sql = format!(
                "UPDATE runs SET state = $1, lease_owner = NULL, lease_expires_at = NULL,
                     fencing_token = fencing_token + 1,
                     cancel_requested_at = CASE WHEN $2 THEN $3 ELSE NULL END,
                     finished_at = CASE WHEN $2 THEN $3 ELSE NULL END,
                     updated_at = $3
                 WHERE run_id = $4 AND state IN ({from_states})
                 RETURNING {RUN_COLUMNS}"
            );
            let updated_row = sqlx::query(sqlx::AssertSqlSafe(sql.clone()))
                .bind(run_state)
                .bind(is_cancel)
                .bind(&changed_at)
                .bind(&run_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_sqlx_error)?
                .ok_or_else(|| {
                    DatabaseError::ConstraintViolation(format!(
                        "run cannot transition from {} through {}",
                        current.state, action
                    ))
                })?;
            sqlx::query(
                "UPDATE steps SET state = $1,
                     finished_at = CASE WHEN $2 THEN $3 ELSE NULL END,
                     updated_at = $3
                 WHERE run_id = $4
                   AND state NOT IN ('completed','failed','skipped','cancelled')",
            )
            .bind(step_state)
            .bind(is_cancel)
            .bind(&changed_at)
            .bind(&run_id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            if is_cancel {
                // Fence out permission workers holding a live claim on this
                // run so a stale lease cannot resurrect the cancelled task.
                sqlx::query(
                    "UPDATE permission_operations SET state = 'cancelled',
                         payload_ref = '', payload_digest = '', encryption_key_id = NULL,
                         lease_owner = NULL, lease_expires_at = NULL,
                         fencing_token = fencing_token + 1, updated_at = $1
                     WHERE run_id = $2 AND state IN ('pending', 'claimable', 'executing')",
                )
                .bind(&changed_at)
                .bind(&run_id)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
            }
            sqlx::query("UPDATE tasks SET state = $1, updated_at = $2 WHERE task_id = $3")
                .bind(task_state)
                .bind(&changed_at)
                .bind(&current.task_id)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
            crate::postgres_repository::postgres_save_event_idempotent(&mut *tx, &event).await?;
            let updated = map_run_row(&updated_row)?;
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(updated)
        })
    }
}

impl PostgresDatabase {
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
        let pool = self.pool.pool().clone();
        let claim = claim.clone();
        let messages = messages.to_vec();
        let terminal_state = terminal_state.to_string();
        let step_state = step_state.to_string();
        let result_json = result_json.map(str::to_string);
        let error_kind = error_kind.map(str::to_string);
        let error_code = error_code.map(str::to_string);
        let error_detail = error_detail.map(str::to_string);
        let finished_at = finished_at.to_string();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let changed = sqlx::query(
                "UPDATE runs SET state = $1, lease_owner = NULL, lease_expires_at = NULL,
                     error_kind = $2, error_code = $3, error_detail = $4,
                     finished_at = $5, updated_at = $5
                 WHERE run_id = $6 AND lease_owner = $7 AND fencing_token = $8
                   AND cancel_requested_at IS NULL AND state IN ('planning', 'executing')",
            )
            .bind(&terminal_state)
            .bind(&error_kind)
            .bind(&error_code)
            .bind(&error_detail)
            .bind(&finished_at)
            .bind(&claim.run.run_id)
            .bind(&owner)
            .bind(fence)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .rows_affected();
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
            for message in &messages {
                let existing = sqlx::query(
                    "SELECT message_id, session_id, role, content, created_at, metadata_json
                     FROM messages WHERE message_id = $1",
                )
                .bind(&message.message_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_sqlx_error)?
                .map(|row| map_message_row(&row))
                .transpose()?;
                if let Some(existing) = existing {
                    crate::message_identity::ensure_message_retry_matches(&existing, message)?;
                    continue;
                }
                sqlx::query(
                    "INSERT INTO messages (
                        message_id, session_id, role, content, created_at, metadata_json
                     ) VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(&message.message_id)
                .bind(&message.session_id)
                .bind(&message.role)
                .bind(&message.content)
                .bind(&message.created_at)
                .bind(&message.metadata_json)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                inserted += 1;
            }
            if inserted > 0 {
                let added = i64::try_from(inserted).map_err(|_| {
                    DatabaseError::ConstraintViolation("message turn size overflow".to_string())
                })?;
                let session_changed = sqlx::query(
                    "UPDATE sessions
                     SET message_count = message_count + $1, updated_at = $2
                     WHERE session_id = $3 AND lower(state) = 'active'
                       AND message_count <= $4",
                )
                .bind(added)
                .bind(&finished_at)
                .bind(&claim.run.session_id)
                .bind(i64::MAX - added)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?
                .rows_affected();
                if session_changed != 1 {
                    return Err(DatabaseError::ConstraintViolation(
                        "run session is not active or message count is exhausted".to_string(),
                    ));
                }
            }
            let step_changed = sqlx::query(
                "UPDATE steps SET state = $1, result_json = $2, error_kind = $3,
                     error_code = $4, error_detail = $5, finished_at = $6,
                     updated_at = $6
                 WHERE step_id = $7 AND run_id = $8 AND state IN ('ready', 'running')",
            )
            .bind(&step_state)
            .bind(&result_json)
            .bind(&error_kind)
            .bind(&error_code)
            .bind(&error_detail)
            .bind(&finished_at)
            .bind(&claim.step.step_id)
            .bind(&claim.run.run_id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .rows_affected();
            if step_changed != 1 {
                return Err(DatabaseError::ConstraintViolation(
                    "run step finish rejected for stale state".to_string(),
                ));
            }
            let task_changed = sqlx::query(
                "UPDATE tasks SET state = $1, updated_at = $2
                 WHERE task_id = $3 AND lower(state) NOT IN ('cancelled', 'canceled')",
            )
            .bind(&terminal_state)
            .bind(&finished_at)
            .bind(&claim.run.task_id)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .rows_affected();
            if task_changed != 1 {
                return Err(DatabaseError::ConstraintViolation(
                    "task finish rejected for stale state".to_string(),
                ));
            }
            crate::postgres_repository::postgres_save_event_idempotent(&mut *tx, &event).await?;
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(())
        })
    }
}
