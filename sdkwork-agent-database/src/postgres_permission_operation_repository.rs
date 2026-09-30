use crate::error::{DatabaseError, DatabaseResult};
use crate::postgres::PostgresDatabase;
use crate::postgres_execution_repository::{
    insert_run, insert_step, map_run_row, map_step_row, RUN_COLUMNS, STEP_COLUMNS,
};
use crate::postgres_pool::map_sqlx_error;
use crate::traits::PermissionOperationRepository;
use crate::types::{
    ClaimedPermissionOperation, EventRow, PermissionOperationRow, PermissionOperationState,
    PermissionPayloadKind, PermissionRow, RunRow, RunState, StepRow, StepState, TaskRow,
};
use sqlx::{PgConnection, Row};
use std::str::FromStr;

const OPERATION_COLUMNS: &str = "permission_request_id, run_id, step_id, tool_call_id,
    provider_id, descriptor_revision, policy_revision, payload_kind, payload_ref,
    payload_digest, encryption_key_id, state, expires_at, lease_owner,
    lease_expires_at, fencing_token, result_json, error_kind, error_code,
    error_detail, created_at, updated_at";

fn map_operation_row(row: &sqlx::postgres::PgRow) -> DatabaseResult<PermissionOperationRow> {
    let payload_kind: String = row.try_get("payload_kind").map_err(map_sqlx_error)?;
    let state: String = row.try_get("state").map_err(map_sqlx_error)?;
    Ok(PermissionOperationRow {
        permission_request_id: row
            .try_get("permission_request_id")
            .map_err(map_sqlx_error)?,
        run_id: row.try_get("run_id").map_err(map_sqlx_error)?,
        step_id: row.try_get("step_id").map_err(map_sqlx_error)?,
        tool_call_id: row.try_get("tool_call_id").map_err(map_sqlx_error)?,
        provider_id: row.try_get("provider_id").map_err(map_sqlx_error)?,
        descriptor_revision: row.try_get("descriptor_revision").map_err(map_sqlx_error)?,
        policy_revision: row.try_get("policy_revision").map_err(map_sqlx_error)?,
        payload_kind: PermissionPayloadKind::from_str(&payload_kind)
            .map_err(DatabaseError::Serialization)?,
        payload_ref: row.try_get("payload_ref").map_err(map_sqlx_error)?,
        payload_digest: row.try_get("payload_digest").map_err(map_sqlx_error)?,
        encryption_key_id: row.try_get("encryption_key_id").map_err(map_sqlx_error)?,
        state: PermissionOperationState::from_str(&state).map_err(DatabaseError::Serialization)?,
        expires_at: row.try_get("expires_at").map_err(map_sqlx_error)?,
        lease_owner: row.try_get("lease_owner").map_err(map_sqlx_error)?,
        lease_expires_at: row.try_get("lease_expires_at").map_err(map_sqlx_error)?,
        fencing_token: row.try_get("fencing_token").map_err(map_sqlx_error)?,
        result_json: row.try_get("result_json").map_err(map_sqlx_error)?,
        error_kind: row.try_get("error_kind").map_err(map_sqlx_error)?,
        error_code: row.try_get("error_code").map_err(map_sqlx_error)?,
        error_detail: row.try_get("error_detail").map_err(map_sqlx_error)?,
        created_at: row.try_get("created_at").map_err(map_sqlx_error)?,
        updated_at: row.try_get("updated_at").map_err(map_sqlx_error)?,
    })
}

async fn insert_permission(
    connection: &mut PgConnection,
    permission: &PermissionRow,
) -> DatabaseResult<()> {
    sqlx::query(
        "INSERT INTO permissions (
            permission_request_id, session_id, category, resource, side_effect_level,
            reason, status, owner_tenant_id, owner_user_ref, created_at, updated_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(&permission.permission_request_id)
    .bind(&permission.session_id)
    .bind(&permission.category)
    .bind(&permission.resource)
    .bind(&permission.side_effect_level)
    .bind(&permission.reason)
    .bind(&permission.status)
    .bind(&permission.owner_tenant_id)
    .bind(&permission.owner_user_ref)
    .bind(&permission.created_at)
    .bind(&permission.updated_at)
    .execute(connection)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

async fn insert_operation(
    connection: &mut PgConnection,
    operation: &PermissionOperationRow,
) -> DatabaseResult<()> {
    sqlx::query(
        "INSERT INTO permission_operations (
            permission_request_id, run_id, step_id, tool_call_id, provider_id,
            descriptor_revision, policy_revision, payload_kind, payload_ref,
            payload_digest, encryption_key_id, state, expires_at, lease_owner,
            lease_expires_at, fencing_token, result_json, error_kind, error_code,
            error_detail, created_at, updated_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                   $13, $14, $15, $16, $17, $18, $19, $20, $21, $22)",
    )
    .bind(&operation.permission_request_id)
    .bind(&operation.run_id)
    .bind(&operation.step_id)
    .bind(&operation.tool_call_id)
    .bind(&operation.provider_id)
    .bind(&operation.descriptor_revision)
    .bind(&operation.policy_revision)
    .bind(operation.payload_kind.as_str())
    .bind(&operation.payload_ref)
    .bind(&operation.payload_digest)
    .bind(&operation.encryption_key_id)
    .bind(operation.state.as_str())
    .bind(&operation.expires_at)
    .bind(&operation.lease_owner)
    .bind(&operation.lease_expires_at)
    .bind(operation.fencing_token)
    .bind(&operation.result_json)
    .bind(&operation.error_kind)
    .bind(&operation.error_code)
    .bind(&operation.error_detail)
    .bind(&operation.created_at)
    .bind(&operation.updated_at)
    .execute(connection)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

fn ensure_claim(claim: &ClaimedPermissionOperation) -> DatabaseResult<(String, i64)> {
    let owner = claim.operation.lease_owner.clone().ok_or_else(|| {
        DatabaseError::ConstraintViolation("permission claim has no lease owner".to_string())
    })?;
    if claim.operation.fencing_token < 1
        || claim.operation.run_id != claim.run.run_id
        || claim.operation.step_id != claim.step.step_id
    {
        return Err(DatabaseError::ConstraintViolation(
            "invalid permission claim identity".to_string(),
        ));
    }
    Ok((owner, claim.operation.fencing_token))
}

impl PermissionOperationRepository for PostgresDatabase {
    fn create_permission_execution(
        &self,
        permission: &PermissionRow,
        task: &TaskRow,
        run: &RunRow,
        step: &StepRow,
        operation: &PermissionOperationRow,
        event: &EventRow,
    ) -> DatabaseResult<()> {
        if permission.status != "pending"
            || permission.session_id.as_deref() != Some(task.session_id.as_str())
            || run.task_id != task.task_id
            || run.session_id != task.session_id
            || run.state != RunState::AwaitingPermission
            || step.run_id != run.run_id
            || step.state != StepState::AwaitingPermission
            || operation.permission_request_id != permission.permission_request_id
            || operation.run_id != run.run_id
            || operation.step_id != step.step_id
            || operation.state != PermissionOperationState::Pending
            || operation.fencing_token != 0
        {
            return Err(DatabaseError::ConstraintViolation(
                "permission execution identity or initial state is invalid".to_string(),
            ));
        }
        crate::event_identity::ensure_event_session(event, &task.session_id, "permission request")?;
        let pool = self.pool.pool().clone();
        let permission = permission.clone();
        let task = task.clone();
        let run = run.clone();
        let step = step.clone();
        let operation = operation.clone();
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
                "INSERT INTO tasks (task_id, session_id, instruction, state, created_at, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6)",
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
            insert_permission(&mut tx, &permission).await?;
            insert_run(&mut tx, &run).await?;
            insert_step(&mut tx, &step).await?;
            insert_operation(&mut tx, &operation).await?;
            crate::postgres_repository::postgres_save_event_idempotent(&mut *tx, &event).await?;
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(())
        })
    }

    fn load_permission_operation(
        &self,
        permission_request_id: &str,
    ) -> DatabaseResult<Option<PermissionOperationRow>> {
        let pool = self.pool.pool().clone();
        let permission_request_id = permission_request_id.to_string();
        self.pool.run_db(async move {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {OPERATION_COLUMNS} FROM permission_operations
                 WHERE permission_request_id = $1"
            )))
            .bind(permission_request_id)
            .fetch_optional(&pool)
            .await
            .map_err(map_sqlx_error)?
            .as_ref()
            .map(map_operation_row)
            .transpose()
        })
    }

    fn decide_permission_operation(
        &self,
        permission_request_id: &str,
        decision: &str,
        decided_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<PermissionOperationRow> {
        if !matches!(decision, "allow" | "deny") {
            return Err(DatabaseError::ConstraintViolation(
                "permission decision must be allow or deny".to_string(),
            ));
        }
        let pool = self.pool.pool().clone();
        let permission_request_id = permission_request_id.to_string();
        let decision = decision.to_string();
        let decided_at = decided_at.to_string();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let row = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {OPERATION_COLUMNS} FROM permission_operations
                 WHERE permission_request_id = $1 FOR UPDATE"
            )))
            .bind(&permission_request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .ok_or_else(|| {
                DatabaseError::NotFound(format!(
                    "permission operation not found: {permission_request_id}"
                ))
            })?;
            let operation = map_operation_row(&row)?;
            if operation.state != PermissionOperationState::Pending {
                let stored_decision = sqlx::query_scalar::<_, String>(
                    "SELECT status FROM permissions WHERE permission_request_id = $1",
                )
                .bind(&permission_request_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                if stored_decision == decision {
                    tx.commit().await.map_err(map_sqlx_error)?;
                    return Ok(operation);
                }
                return Err(DatabaseError::ConstraintViolation(
                    "permission operation has a conflicting decision".to_string(),
                ));
            }
            sqlx::query(
                "UPDATE permissions SET status = $2, updated_at = $3
                 WHERE permission_request_id = $1 AND status = 'pending'",
            )
            .bind(&permission_request_id)
            .bind(&decision)
            .bind(&decided_at)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            if decision == "allow" {
                sqlx::query(
                    "UPDATE permission_operations SET state = 'claimable', updated_at = $2
                     WHERE permission_request_id = $1 AND state = 'pending'",
                )
                .bind(&permission_request_id)
                .bind(&decided_at)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
            } else {
                sqlx::query(
                    "UPDATE permission_operations SET state = 'cancelled', payload_ref = '',
                         payload_digest = '', encryption_key_id = NULL, updated_at = $2
                     WHERE permission_request_id = $1 AND state = 'pending'",
                )
                .bind(&permission_request_id)
                .bind(&decided_at)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                sqlx::query(
                    "UPDATE steps SET state = 'cancelled', finished_at = $2, updated_at = $2
                     WHERE step_id = $1 AND state = 'awaiting_permission'",
                )
                .bind(&operation.step_id)
                .bind(&decided_at)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                sqlx::query(
                    "UPDATE runs SET state = 'cancelled', finished_at = $2, updated_at = $2,
                         fencing_token = fencing_token + 1
                     WHERE run_id = $1 AND state = 'awaiting_permission'",
                )
                .bind(&operation.run_id)
                .bind(&decided_at)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                sqlx::query(
                    "UPDATE tasks SET state = 'cancelled', updated_at = $2
                     WHERE task_id = (SELECT task_id FROM runs WHERE run_id = $1)
                       AND state NOT IN ('completed', 'failed', 'cancelled')",
                )
                .bind(&operation.run_id)
                .bind(&decided_at)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
            }
            crate::postgres_repository::postgres_save_event_idempotent(&mut *tx, &event).await?;
            let updated = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {OPERATION_COLUMNS} FROM permission_operations
                 WHERE permission_request_id = $1"
            )))
            .bind(&permission_request_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            let updated = map_operation_row(&updated)?;
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(updated)
        })
    }

    fn claim_permission_operation(
        &self,
        worker_id: &str,
        now: &str,
        lease_expires_at: &str,
    ) -> DatabaseResult<Option<ClaimedPermissionOperation>> {
        let pool = self.pool.pool().clone();
        let worker_id = worker_id.to_string();
        let now = now.to_string();
        let lease_expires_at = lease_expires_at.to_string();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let operation = sqlx::query(sqlx::AssertSqlSafe(format!(
                "WITH candidate AS (
                    SELECT permission_request_id FROM permission_operations
                    WHERE expires_at > $1
                      AND (state = 'claimable'
                           OR (state = 'executing' AND lease_expires_at <= $1))
                    ORDER BY created_at, permission_request_id
                    FOR UPDATE SKIP LOCKED LIMIT 1
                 )
                 UPDATE permission_operations AS operation
                 SET state = 'executing', lease_owner = $2, lease_expires_at = $3,
                     fencing_token = operation.fencing_token + 1, updated_at = $1
                 FROM candidate
                 WHERE operation.permission_request_id = candidate.permission_request_id
                 RETURNING {}",
                OPERATION_COLUMNS
                    .split(',')
                    .map(|column| format!("operation.{}", column.trim()))
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
            .bind(&now)
            .bind(&worker_id)
            .bind(&lease_expires_at)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            let Some(operation) = operation else {
                tx.commit().await.map_err(map_sqlx_error)?;
                return Ok(None);
            };
            let operation = map_operation_row(&operation)?;
            let run_row = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {RUN_COLUMNS} FROM runs WHERE run_id = $1"
            )))
            .bind(&operation.run_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            let step_row = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {STEP_COLUMNS} FROM steps WHERE step_id = $1"
            )))
            .bind(&operation.step_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            let claim = ClaimedPermissionOperation {
                operation,
                run: map_run_row(&run_row)?,
                step: map_step_row(&step_row)?,
            };
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(Some(claim))
        })
    }

    fn expire_permission_operations(
        &self,
        now: &str,
        batch_size: i64,
    ) -> DatabaseResult<Vec<EventRow>> {
        if !(1..=200).contains(&batch_size) {
            return Err(DatabaseError::ConstraintViolation(
                "permission expiry batch_size must be between 1 and 200".to_string(),
            ));
        }
        let pool = self.pool.pool().clone();
        let now = now.to_string();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let rows = sqlx::query(
                "SELECT operation.permission_request_id, operation.run_id, operation.step_id,
                        runs.task_id, runs.session_id
                 FROM permission_operations AS operation
                 JOIN runs ON runs.run_id = operation.run_id
                 WHERE operation.expires_at <= $1
                   AND (operation.state IN ('pending','claimable')
                        OR (operation.state = 'executing'
                            AND operation.lease_expires_at <= $1))
                 ORDER BY operation.expires_at, operation.permission_request_id
                 FOR UPDATE OF operation SKIP LOCKED LIMIT $2",
            )
            .bind(&now)
            .bind(batch_size)
            .fetch_all(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            let mut events = Vec::with_capacity(rows.len());
            for row in rows {
                let permission_id: String = row
                    .try_get("permission_request_id")
                    .map_err(map_sqlx_error)?;
                let run_id: String = row.try_get("run_id").map_err(map_sqlx_error)?;
                let step_id: String = row.try_get("step_id").map_err(map_sqlx_error)?;
                let task_id: String = row.try_get("task_id").map_err(map_sqlx_error)?;
                let session_id: String = row.try_get("session_id").map_err(map_sqlx_error)?;
                let changed = sqlx::query(
                    "UPDATE permission_operations SET state = 'expired', payload_ref = '',
                         payload_digest = '', encryption_key_id = NULL, lease_owner = NULL,
                         lease_expires_at = NULL, fencing_token = fencing_token + 1,
                         updated_at = $2 WHERE permission_request_id = $1 AND expires_at <= $2
                       AND (state IN ('pending','claimable')
                            OR (state = 'executing' AND lease_expires_at <= $2))",
                )
                .bind(&permission_id)
                .bind(&now)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?
                .rows_affected();
                if changed != 1 {
                    continue;
                }
                sqlx::query(
                    "UPDATE permissions SET status = 'expired', updated_at = $2
                     WHERE permission_request_id = $1",
                )
                .bind(&permission_id)
                .bind(&now)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                sqlx::query(
                    "UPDATE steps SET state = 'cancelled', finished_at = $2, updated_at = $2
                     WHERE step_id = $1 AND state = 'awaiting_permission'",
                )
                .bind(&step_id)
                .bind(&now)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                sqlx::query(
                    "UPDATE runs SET state = 'cancelled', finished_at = $2, updated_at = $2,
                         fencing_token = fencing_token + 1
                     WHERE run_id = $1 AND state = 'awaiting_permission'",
                )
                .bind(&run_id)
                .bind(&now)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                sqlx::query(
                    "UPDATE tasks SET state = 'cancelled', updated_at = $2 WHERE task_id = $1
                       AND state NOT IN ('completed', 'failed', 'cancelled')",
                )
                .bind(&task_id)
                .bind(&now)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                let event = EventRow {
                    event_id: format!(
                        "event.permission.expired.{}",
                        &sdkwork_utils_rust::sha256_hash(permission_id.as_bytes())[..32]
                    ),
                    session_id: Some(session_id),
                    event_type: "permission.expired".to_string(),
                    severity: "warn".to_string(),
                    payload: Some(
                        serde_json::json!({ "permissionRequestId": permission_id }).to_string(),
                    ),
                    created_at: now.clone(),
                };
                crate::postgres_repository::postgres_save_event_idempotent(&mut *tx, &event)
                    .await?;
                events.push(event);
            }
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(events)
        })
    }

    fn renew_permission_operation_lease(
        &self,
        permission_request_id: &str,
        worker_id: &str,
        fencing_token: i64,
        now: &str,
        lease_expires_at: &str,
    ) -> DatabaseResult<bool> {
        let pool = self.pool.pool().clone();
        let permission_request_id = permission_request_id.to_string();
        let worker_id = worker_id.to_string();
        let now = now.to_string();
        let lease_expires_at = lease_expires_at.to_string();
        self.pool.run_db(async move {
            Ok(sqlx::query(
                "UPDATE permission_operations SET lease_expires_at = $5, updated_at = $4
                 WHERE permission_request_id = $1 AND state = 'executing'
                   AND lease_owner = $2 AND fencing_token = $3 AND expires_at > $4",
            )
            .bind(permission_request_id)
            .bind(worker_id)
            .bind(fencing_token)
            .bind(now)
            .bind(lease_expires_at)
            .execute(&pool)
            .await
            .map_err(map_sqlx_error)?
            .rows_affected()
                == 1)
        })
    }

    fn complete_permission_operation(
        &self,
        claim: &ClaimedPermissionOperation,
        result_json: &str,
        finished_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<()> {
        let (owner, fence) = ensure_claim(claim)?;
        let pool = self.pool.pool().clone();
        let claim = claim.clone();
        let result_json = result_json.to_string();
        let finished_at = finished_at.to_string();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let changed = sqlx::query(
                "UPDATE permission_operations SET state = 'completed', result_json = $4,
                     payload_ref = '', payload_digest = '', encryption_key_id = NULL,
                     lease_owner = NULL, lease_expires_at = NULL, updated_at = $5
                 WHERE permission_request_id = $1 AND state = 'executing'
                   AND lease_owner = $2 AND fencing_token = $3",
            )
            .bind(&claim.operation.permission_request_id)
            .bind(&owner)
            .bind(fence)
            .bind(&result_json)
            .bind(&finished_at)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .rows_affected();
            if changed != 1 {
                return Err(DatabaseError::ConstraintViolation(
                    "permission operation lease was lost".to_string(),
                ));
            }
            sqlx::query(
                "UPDATE steps SET state = 'completed', result_json = $2, finished_at = $3,
                     updated_at = $3 WHERE step_id = $1 AND state = 'awaiting_permission'",
            )
            .bind(&claim.step.step_id)
            .bind(&result_json)
            .bind(&finished_at)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            sqlx::query(
                "UPDATE runs SET state = 'completed', finished_at = $2, updated_at = $2
                 WHERE run_id = $1 AND state = 'awaiting_permission'",
            )
            .bind(&claim.run.run_id)
            .bind(&finished_at)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            sqlx::query(
                "UPDATE tasks SET state = 'completed', updated_at = $2 WHERE task_id = $1
                   AND state NOT IN ('completed', 'failed', 'cancelled')",
            )
            .bind(&claim.run.task_id)
            .bind(&finished_at)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            crate::postgres_repository::postgres_save_event_idempotent(&mut *tx, &event).await?;
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(())
        })
    }

    fn fail_permission_operation(
        &self,
        claim: &ClaimedPermissionOperation,
        error_kind: &str,
        error_code: Option<&str>,
        error_detail: &str,
        finished_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<()> {
        let (owner, fence) = ensure_claim(claim)?;
        let pool = self.pool.pool().clone();
        let claim = claim.clone();
        let error_kind = error_kind.to_string();
        let error_code = error_code.map(str::to_string);
        let error_detail = error_detail.to_string();
        let finished_at = finished_at.to_string();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let changed = sqlx::query(
                "UPDATE permission_operations SET state = 'failed', error_kind = $4,
                     error_code = $5, error_detail = $6, payload_ref = '', payload_digest = '',
                     encryption_key_id = NULL, lease_owner = NULL, lease_expires_at = NULL,
                     updated_at = $7 WHERE permission_request_id = $1 AND state = 'executing'
                     AND lease_owner = $2 AND fencing_token = $3",
            )
            .bind(&claim.operation.permission_request_id)
            .bind(&owner)
            .bind(fence)
            .bind(&error_kind)
            .bind(&error_code)
            .bind(&error_detail)
            .bind(&finished_at)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .rows_affected();
            if changed != 1 {
                return Err(DatabaseError::ConstraintViolation(
                    "permission operation lease was lost".to_string(),
                ));
            }
            sqlx::query(
                "UPDATE steps SET state = 'failed', error_kind = $2, error_code = $3,
                     error_detail = $4, finished_at = $5, updated_at = $5
                 WHERE step_id = $1 AND state = 'awaiting_permission'",
            )
            .bind(&claim.step.step_id)
            .bind(&error_kind)
            .bind(&error_code)
            .bind(&error_detail)
            .bind(&finished_at)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            sqlx::query(
                "UPDATE runs SET state = 'failed', error_kind = $2, error_code = $3,
                     error_detail = $4, finished_at = $5, updated_at = $5
                 WHERE run_id = $1 AND state = 'awaiting_permission'",
            )
            .bind(&claim.run.run_id)
            .bind(&error_kind)
            .bind(&error_code)
            .bind(&error_detail)
            .bind(&finished_at)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            sqlx::query(
                "UPDATE tasks SET state = 'failed', updated_at = $2 WHERE task_id = $1
                   AND state NOT IN ('completed', 'failed', 'cancelled')",
            )
            .bind(&claim.run.task_id)
            .bind(&finished_at)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            crate::postgres_repository::postgres_save_event_idempotent(&mut *tx, &event).await?;
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(())
        })
    }
}
