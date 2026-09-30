use crate::error::{DatabaseError, DatabaseResult};
use crate::sqlite::SqliteDatabase;
use crate::sqlite_execution_repository::{
    insert_run, insert_step, insert_task, map_run_row, map_step_row, RUN_COLUMNS, STEP_COLUMNS,
};
use crate::traits::PermissionOperationRepository;
use crate::types::{
    ClaimedPermissionOperation, EventRow, PermissionOperationRow, PermissionOperationState,
    PermissionPayloadKind, PermissionRow, RunRow, RunState, StepRow, StepState, TaskRow,
};
use rusqlite::{params, OptionalExtension, Row, Transaction, TransactionBehavior};
use std::str::FromStr;

const OPERATION_COLUMNS: &str = "permission_request_id, run_id, step_id, tool_call_id,
    provider_id, descriptor_revision, policy_revision, payload_kind, payload_ref,
    payload_digest, encryption_key_id, state, expires_at, lease_owner,
    lease_expires_at, fencing_token, result_json, error_kind, error_code,
    error_detail, created_at, updated_at";

fn map_operation_row(row: &Row<'_>) -> rusqlite::Result<PermissionOperationRow> {
    let payload_kind: String = row.get("payload_kind")?;
    let state: String = row.get("state")?;
    let invalid = |column, value: String, error: String| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{error}; value={value}"),
            )),
        )
    };
    Ok(PermissionOperationRow {
        permission_request_id: row.get("permission_request_id")?,
        run_id: row.get("run_id")?,
        step_id: row.get("step_id")?,
        tool_call_id: row.get("tool_call_id")?,
        provider_id: row.get("provider_id")?,
        descriptor_revision: row.get("descriptor_revision")?,
        policy_revision: row.get("policy_revision")?,
        payload_kind: PermissionPayloadKind::from_str(&payload_kind)
            .map_err(|error| invalid(7, payload_kind, error))?,
        payload_ref: row.get("payload_ref")?,
        payload_digest: row.get("payload_digest")?,
        encryption_key_id: row.get("encryption_key_id")?,
        state: PermissionOperationState::from_str(&state)
            .map_err(|error| invalid(11, state, error))?,
        expires_at: row.get("expires_at")?,
        lease_owner: row.get("lease_owner")?,
        lease_expires_at: row.get("lease_expires_at")?,
        fencing_token: row.get("fencing_token")?,
        result_json: row.get("result_json")?,
        error_kind: row.get("error_kind")?,
        error_code: row.get("error_code")?,
        error_detail: row.get("error_detail")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn insert_permission(tx: &Transaction<'_>, permission: &PermissionRow) -> DatabaseResult<()> {
    tx.execute(
        "INSERT INTO permissions (
            permission_request_id, session_id, category, resource, side_effect_level,
            reason, status, owner_tenant_id, owner_user_ref, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            permission.permission_request_id,
            permission.session_id,
            permission.category,
            permission.resource,
            permission.side_effect_level,
            permission.reason,
            permission.status,
            permission.owner_tenant_id,
            permission.owner_user_ref,
            permission.created_at,
            permission.updated_at,
        ],
    )?;
    Ok(())
}

fn insert_operation(
    tx: &Transaction<'_>,
    operation: &PermissionOperationRow,
) -> DatabaseResult<()> {
    tx.execute(
        "INSERT INTO permission_operations (
            permission_request_id, run_id, step_id, tool_call_id, provider_id,
            descriptor_revision, policy_revision, payload_kind, payload_ref,
            payload_digest, encryption_key_id, state, expires_at, lease_owner,
            lease_expires_at, fencing_token, result_json, error_kind, error_code,
            error_detail, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                   ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
        params![
            operation.permission_request_id,
            operation.run_id,
            operation.step_id,
            operation.tool_call_id,
            operation.provider_id,
            operation.descriptor_revision,
            operation.policy_revision,
            operation.payload_kind.as_str(),
            operation.payload_ref,
            operation.payload_digest,
            operation.encryption_key_id,
            operation.state.as_str(),
            operation.expires_at,
            operation.lease_owner,
            operation.lease_expires_at,
            operation.fencing_token,
            operation.result_json,
            operation.error_kind,
            operation.error_code,
            operation.error_detail,
            operation.created_at,
            operation.updated_at,
        ],
    )?;
    Ok(())
}

fn load_claim(
    tx: &Transaction<'_>,
    permission_request_id: &str,
) -> DatabaseResult<ClaimedPermissionOperation> {
    let operation = tx.query_row(
        &format!(
            "SELECT {OPERATION_COLUMNS} FROM permission_operations
             WHERE permission_request_id = ?1"
        ),
        [permission_request_id],
        map_operation_row,
    )?;
    let run = tx.query_row(
        &format!("SELECT {RUN_COLUMNS} FROM runs WHERE run_id = ?1"),
        [&operation.run_id],
        map_run_row,
    )?;
    let step = tx.query_row(
        &format!("SELECT {STEP_COLUMNS} FROM steps WHERE step_id = ?1"),
        [&operation.step_id],
        map_step_row,
    )?;
    Ok(ClaimedPermissionOperation {
        operation,
        run,
        step,
    })
}

fn ensure_claim(claim: &ClaimedPermissionOperation) -> DatabaseResult<(&str, i64)> {
    let owner = claim.operation.lease_owner.as_deref().ok_or_else(|| {
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

impl PermissionOperationRepository for SqliteDatabase {
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
        let mut conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_task(&tx, task)?;
        insert_permission(&tx, permission)?;
        insert_run(&tx, run)?;
        insert_step(&tx, step)?;
        insert_operation(&tx, operation)?;
        crate::sqlite_repository::sqlite_save_event_idempotent(&tx, event)?;
        tx.commit()?;
        Ok(())
    }

    fn load_permission_operation(
        &self,
        permission_request_id: &str,
    ) -> DatabaseResult<Option<PermissionOperationRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        conn.query_row(
            &format!(
                "SELECT {OPERATION_COLUMNS} FROM permission_operations
                 WHERE permission_request_id = ?1"
            ),
            [permission_request_id],
            map_operation_row,
        )
        .optional()
        .map_err(Into::into)
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
        let mut conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let operation = tx
            .query_row(
                &format!(
                    "SELECT {OPERATION_COLUMNS} FROM permission_operations
                     WHERE permission_request_id = ?1"
                ),
                [permission_request_id],
                map_operation_row,
            )
            .optional()?
            .ok_or_else(|| {
                DatabaseError::NotFound(format!(
                    "permission operation not found: {permission_request_id}"
                ))
            })?;
        if operation.state != PermissionOperationState::Pending {
            let stored_decision: String = tx.query_row(
                "SELECT status FROM permissions WHERE permission_request_id = ?1",
                [permission_request_id],
                |row| row.get(0),
            )?;
            if stored_decision == decision {
                tx.commit()?;
                return Ok(operation);
            }
            return Err(DatabaseError::ConstraintViolation(
                "permission operation has a conflicting decision".to_string(),
            ));
        }
        tx.execute(
            "UPDATE permissions SET status = ?2, updated_at = ?3
             WHERE permission_request_id = ?1 AND status = 'pending'",
            params![permission_request_id, decision, decided_at],
        )?;
        if decision == "allow" {
            tx.execute(
                "UPDATE permission_operations
                 SET state = 'claimable', updated_at = ?2
                 WHERE permission_request_id = ?1 AND state = 'pending'",
                params![permission_request_id, decided_at],
            )?;
        } else {
            tx.execute(
                "UPDATE permission_operations
                 SET state = 'cancelled', payload_ref = '', payload_digest = '',
                     encryption_key_id = NULL, updated_at = ?2
                 WHERE permission_request_id = ?1 AND state = 'pending'",
                params![permission_request_id, decided_at],
            )?;
            tx.execute(
                "UPDATE steps SET state = 'cancelled', finished_at = ?2, updated_at = ?2
                 WHERE step_id = ?1 AND state = 'awaiting_permission'",
                params![operation.step_id, decided_at],
            )?;
            tx.execute(
                "UPDATE runs SET state = 'cancelled', finished_at = ?2, updated_at = ?2,
                     fencing_token = fencing_token + 1
                 WHERE run_id = ?1 AND state = 'awaiting_permission'",
                params![operation.run_id, decided_at],
            )?;
            tx.execute(
                "UPDATE tasks SET state = 'cancelled', updated_at = ?2
                 WHERE task_id = (SELECT task_id FROM runs WHERE run_id = ?1)
                   AND state NOT IN ('completed', 'failed', 'cancelled')",
                params![operation.run_id, decided_at],
            )?;
        }
        crate::sqlite_repository::sqlite_save_event_idempotent(&tx, event)?;
        let updated = tx.query_row(
            &format!(
                "SELECT {OPERATION_COLUMNS} FROM permission_operations
                 WHERE permission_request_id = ?1"
            ),
            [permission_request_id],
            map_operation_row,
        )?;
        tx.commit()?;
        Ok(updated)
    }

    fn claim_permission_operation(
        &self,
        worker_id: &str,
        now: &str,
        lease_expires_at: &str,
    ) -> DatabaseResult<Option<ClaimedPermissionOperation>> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let permission_request_id = tx
            .query_row(
                "SELECT permission_request_id FROM permission_operations
                 WHERE expires_at > ?1
                   AND (state = 'claimable'
                        OR (state = 'executing' AND lease_expires_at <= ?1))
                 ORDER BY created_at, permission_request_id LIMIT 1",
                [now],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(permission_request_id) = permission_request_id else {
            tx.commit()?;
            return Ok(None);
        };
        let changed = tx.execute(
            "UPDATE permission_operations
             SET state = 'executing', lease_owner = ?2, lease_expires_at = ?3,
                 fencing_token = fencing_token + 1, updated_at = ?1
             WHERE permission_request_id = ?4 AND expires_at > ?1
               AND (state = 'claimable'
                    OR (state = 'executing' AND lease_expires_at <= ?1))",
            params![now, worker_id, lease_expires_at, permission_request_id],
        )?;
        if changed != 1 {
            tx.commit()?;
            return Ok(None);
        }
        let claim = load_claim(&tx, &permission_request_id)?;
        tx.commit()?;
        Ok(Some(claim))
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
        let mut conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rows: Vec<(String, String, String, String, String)> = {
            let mut statement = tx.prepare(
                "SELECT operation.permission_request_id, operation.run_id, operation.step_id,
                        runs.task_id, runs.session_id
                 FROM permission_operations AS operation
                 JOIN runs ON runs.run_id = operation.run_id
                 WHERE operation.expires_at <= ?1
                   AND (operation.state IN ('pending','claimable')
                        OR (operation.state = 'executing'
                            AND operation.lease_expires_at <= ?1))
                 ORDER BY operation.expires_at, operation.permission_request_id LIMIT ?2",
            )?;
            let selected = statement.query_map(params![now, batch_size], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?;
            selected.collect::<Result<Vec<_>, _>>()?
        };
        let mut events = Vec::with_capacity(rows.len());
        for (permission_id, run_id, step_id, task_id, session_id) in rows {
            let changed = tx.execute(
                "UPDATE permission_operations SET state = 'expired', payload_ref = '',
                     payload_digest = '', encryption_key_id = NULL, lease_owner = NULL,
                     lease_expires_at = NULL, fencing_token = fencing_token + 1, updated_at = ?2
                 WHERE permission_request_id = ?1 AND expires_at <= ?2
                   AND (state IN ('pending','claimable')
                        OR (state = 'executing' AND lease_expires_at <= ?2))",
                params![permission_id, now],
            )?;
            if changed != 1 {
                continue;
            }
            tx.execute(
                "UPDATE permissions SET status = 'expired', updated_at = ?2
                 WHERE permission_request_id = ?1",
                params![permission_id, now],
            )?;
            tx.execute(
                "UPDATE steps SET state = 'cancelled', finished_at = ?2, updated_at = ?2
                 WHERE step_id = ?1 AND state = 'awaiting_permission'",
                params![step_id, now],
            )?;
            tx.execute(
                "UPDATE runs SET state = 'cancelled', finished_at = ?2, updated_at = ?2,
                     fencing_token = fencing_token + 1
                 WHERE run_id = ?1 AND state = 'awaiting_permission'",
                params![run_id, now],
            )?;
            tx.execute(
                "UPDATE tasks SET state = 'cancelled', updated_at = ?2 WHERE task_id = ?1
                   AND state NOT IN ('completed', 'failed', 'cancelled')",
                params![task_id, now],
            )?;
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
                created_at: now.to_string(),
            };
            crate::sqlite_repository::sqlite_save_event_idempotent(&tx, &event)?;
            events.push(event);
        }
        tx.commit()?;
        Ok(events)
    }

    fn renew_permission_operation_lease(
        &self,
        permission_request_id: &str,
        worker_id: &str,
        fencing_token: i64,
        now: &str,
        lease_expires_at: &str,
    ) -> DatabaseResult<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        Ok(conn.execute(
            "UPDATE permission_operations SET lease_expires_at = ?5, updated_at = ?4
             WHERE permission_request_id = ?1 AND state = 'executing'
               AND lease_owner = ?2 AND fencing_token = ?3 AND expires_at > ?4",
            params![
                permission_request_id,
                worker_id,
                fencing_token,
                now,
                lease_expires_at
            ],
        )? == 1)
    }

    fn complete_permission_operation(
        &self,
        claim: &ClaimedPermissionOperation,
        result_json: &str,
        finished_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<()> {
        let (owner, fence) = ensure_claim(claim)?;
        let mut conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE permission_operations SET state = 'completed', result_json = ?4,
                 payload_ref = '', payload_digest = '', encryption_key_id = NULL,
                 lease_owner = NULL, lease_expires_at = NULL, updated_at = ?5
             WHERE permission_request_id = ?1 AND state = 'executing'
               AND lease_owner = ?2 AND fencing_token = ?3",
            params![
                claim.operation.permission_request_id,
                owner,
                fence,
                result_json,
                finished_at
            ],
        )?;
        if changed != 1 {
            return Err(DatabaseError::ConstraintViolation(
                "permission operation lease was lost".to_string(),
            ));
        }
        tx.execute(
            "UPDATE steps SET state = 'completed', result_json = ?2, finished_at = ?3,
                 updated_at = ?3 WHERE step_id = ?1 AND state = 'awaiting_permission'",
            params![claim.step.step_id, result_json, finished_at],
        )?;
        tx.execute(
            "UPDATE runs SET state = 'completed', finished_at = ?2, updated_at = ?2
             WHERE run_id = ?1 AND state = 'awaiting_permission'",
            params![claim.run.run_id, finished_at],
        )?;
        tx.execute(
            "UPDATE tasks SET state = 'completed', updated_at = ?2 WHERE task_id = ?1
               AND state NOT IN ('completed', 'failed', 'cancelled')",
            params![claim.run.task_id, finished_at],
        )?;
        crate::sqlite_repository::sqlite_save_event_idempotent(&tx, event)?;
        tx.commit()?;
        Ok(())
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
        let mut conn = self
            .conn
            .lock()
            .map_err(|error| DatabaseError::Internal(format!("failed to acquire lock: {error}")))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE permission_operations SET state = 'failed', error_kind = ?4,
                 error_code = ?5, error_detail = ?6, payload_ref = '', payload_digest = '',
                 encryption_key_id = NULL, lease_owner = NULL, lease_expires_at = NULL,
                 updated_at = ?7 WHERE permission_request_id = ?1 AND state = 'executing'
                 AND lease_owner = ?2 AND fencing_token = ?3",
            params![
                claim.operation.permission_request_id,
                owner,
                fence,
                error_kind,
                error_code,
                error_detail,
                finished_at
            ],
        )?;
        if changed != 1 {
            return Err(DatabaseError::ConstraintViolation(
                "permission operation lease was lost".to_string(),
            ));
        }
        tx.execute(
            "UPDATE steps SET state = 'failed', error_kind = ?2, error_code = ?3,
                 error_detail = ?4, finished_at = ?5, updated_at = ?5
             WHERE step_id = ?1 AND state = 'awaiting_permission'",
            params![
                claim.step.step_id,
                error_kind,
                error_code,
                error_detail,
                finished_at
            ],
        )?;
        tx.execute(
            "UPDATE runs SET state = 'failed', error_kind = ?2, error_code = ?3,
                 error_detail = ?4, finished_at = ?5, updated_at = ?5
             WHERE run_id = ?1 AND state = 'awaiting_permission'",
            params![
                claim.run.run_id,
                error_kind,
                error_code,
                error_detail,
                finished_at
            ],
        )?;
        tx.execute(
            "UPDATE tasks SET state = 'failed', updated_at = ?2 WHERE task_id = ?1
               AND state NOT IN ('completed', 'failed', 'cancelled')",
            params![claim.run.task_id, finished_at],
        )?;
        crate::sqlite_repository::sqlite_save_event_idempotent(&tx, event)?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::{RuntimeExecutionRepository, SessionRepository, TaskRepository};
    use crate::types::{runtime_now_timestamp, ActionKind, SessionRow};

    fn database() -> SqliteDatabase {
        let database = SqliteDatabase::memory_migrated().expect("migrated database");
        database
            .save_session(&SessionRow {
                session_id: "session.permission".into(),
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
            })
            .expect("session");
        database
    }

    fn execution(
        suffix: &str,
    ) -> (
        PermissionRow,
        TaskRow,
        RunRow,
        StepRow,
        PermissionOperationRow,
        EventRow,
    ) {
        let now = "2026-07-17T00:00:00.000000000Z".to_string();
        let permission_id = format!("permission.{suffix}");
        let task = TaskRow {
            task_id: format!("task.permission.{suffix}"),
            session_id: "session.permission".into(),
            instruction: "execute approved tool".into(),
            state: "accepted".into(),
            created_at: now.clone(),
            updated_at: Some(now.clone()),
        };
        let run = RunRow {
            run_id: format!("run.permission.{suffix}"),
            task_id: task.task_id.clone(),
            session_id: task.session_id.clone(),
            attempt: 1,
            state: RunState::AwaitingPermission,
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
            step_id: format!("step.permission.{suffix}"),
            run_id: run.run_id.clone(),
            sequence_no: 0,
            action_kind: ActionKind::ToolCall,
            state: StepState::AwaitingPermission,
            provider_id: Some("provider.tool".into()),
            descriptor_revision: Some("1.0.0".into()),
            policy_revision: Some("1.0.0".into()),
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
        let permission = PermissionRow {
            permission_request_id: permission_id.clone(),
            session_id: Some(task.session_id.clone()),
            category: "tool.invoke".into(),
            resource: "tool.protected".into(),
            side_effect_level: "side_effectful".into(),
            reason: "approval required".into(),
            status: "pending".into(),
            owner_tenant_id: Some("tenant.test".into()),
            owner_user_ref: Some("user.test".into()),
            created_at: now.clone(),
            updated_at: None,
        };
        let operation = PermissionOperationRow {
            permission_request_id: permission_id,
            run_id: run.run_id.clone(),
            step_id: step.step_id.clone(),
            tool_call_id: format!("tool-call.{suffix}"),
            provider_id: "provider.tool".into(),
            descriptor_revision: "1.0.0".into(),
            policy_revision: "1.0.0".into(),
            payload_kind: PermissionPayloadKind::Ciphertext,
            payload_ref: "encrypted-payload".into(),
            payload_digest: "digest".into(),
            encryption_key_id: Some("key.v1".into()),
            state: PermissionOperationState::Pending,
            expires_at: "2026-07-17T01:00:00.000000000Z".into(),
            lease_owner: None,
            lease_expires_at: None,
            fencing_token: 0,
            result_json: None,
            error_kind: None,
            error_code: None,
            error_detail: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        let event = EventRow {
            event_id: format!("event.permission.{suffix}"),
            session_id: Some(task.session_id.clone()),
            event_type: "permission.requested".into(),
            severity: "warn".into(),
            payload: None,
            created_at: now,
        };
        (permission, task, run, step, operation, event)
    }

    #[test]
    fn allowed_operation_is_fenced_completed_and_crypto_erased() {
        let database = database();
        let (permission, task, run, step, operation, event) = execution("allow");
        database
            .create_permission_execution(&permission, &task, &run, &step, &operation, &event)
            .expect("permission execution created");
        assert!(database
            .claim_ready_run(
                "model-worker",
                "2026-07-17T00:00:01.000000000Z",
                "2026-07-17T00:01:01.000000000Z"
            )
            .expect("model claim")
            .is_none());
        let allowed_event = EventRow {
            event_id: "event.permission.allowed".into(),
            event_type: "permission.allowed".into(),
            created_at: "2026-07-17T00:00:02.000000000Z".into(),
            ..event.clone()
        };
        database
            .decide_permission_operation(
                &permission.permission_request_id,
                "allow",
                "2026-07-17T00:00:02.000000000Z",
                &allowed_event,
            )
            .expect("allowed");
        assert_eq!(
            database
                .decide_permission_operation(
                    &permission.permission_request_id,
                    "allow",
                    "2026-07-17T00:00:03.000000000Z",
                    &allowed_event,
                )
                .expect("same decision is idempotent")
                .state,
            PermissionOperationState::Claimable
        );
        assert!(matches!(
            database.decide_permission_operation(
                &permission.permission_request_id,
                "deny",
                "2026-07-17T00:00:03.000000000Z",
                &allowed_event,
            ),
            Err(DatabaseError::ConstraintViolation(_))
        ));
        let claim = database
            .claim_permission_operation(
                "permission-worker",
                "2026-07-17T00:00:03.000000000Z",
                "2026-07-17T00:01:03.000000000Z",
            )
            .expect("claim")
            .expect("claimable");
        assert_eq!(claim.operation.fencing_token, 1);
        database
            .complete_permission_operation(
                &claim,
                r#"{"output":"done"}"#,
                "2026-07-17T00:00:04.000000000Z",
                &EventRow {
                    event_id: "event.permission.completed".into(),
                    event_type: "permission.operation.completed".into(),
                    created_at: "2026-07-17T00:00:04.000000000Z".into(),
                    ..event
                },
            )
            .expect("completed");
        let stored = database
            .load_permission_operation(&permission.permission_request_id)
            .expect("load")
            .expect("operation");
        assert_eq!(stored.state, PermissionOperationState::Completed);
        assert!(stored.payload_ref.is_empty());
        assert!(stored.payload_digest.is_empty());
        assert!(stored.encryption_key_id.is_none());
        assert_eq!(
            database
                .load_task(&task.task_id)
                .expect("task")
                .expect("present")
                .state,
            "completed"
        );
    }

    #[test]
    fn denied_operation_atomically_terminates_linked_execution() {
        let database = database();
        let (permission, task, run, step, operation, event) = execution("deny");
        database
            .create_permission_execution(&permission, &task, &run, &step, &operation, &event)
            .expect("permission execution created");
        let denied = database
            .decide_permission_operation(
                &permission.permission_request_id,
                "deny",
                "2026-07-17T00:00:02.000000000Z",
                &EventRow {
                    event_id: "event.permission.denied".into(),
                    event_type: "permission.denied".into(),
                    created_at: "2026-07-17T00:00:02.000000000Z".into(),
                    ..event
                },
            )
            .expect("denied");
        assert_eq!(denied.state, PermissionOperationState::Cancelled);
        assert_eq!(
            database
                .load_run(&run.run_id)
                .expect("run")
                .expect("present")
                .state,
            RunState::Cancelled
        );
        assert_eq!(
            database
                .load_task(&task.task_id)
                .expect("task")
                .expect("present")
                .state,
            "cancelled"
        );
    }

    #[test]
    fn expiry_sweep_is_bounded_atomic_and_crypto_erases_payload() {
        let database = database();
        let (permission, task, run, step, mut operation, event) = execution("expired");
        operation.expires_at = "2026-07-17T00:00:01.000000000Z".into();
        database
            .create_permission_execution(&permission, &task, &run, &step, &operation, &event)
            .expect("permission execution created");
        let events = database
            .expire_permission_operations("2026-07-17T00:00:02.000000000Z", 1)
            .expect("expired");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "permission.expired");
        let stored = database
            .load_permission_operation(&permission.permission_request_id)
            .expect("load")
            .expect("operation");
        assert_eq!(stored.state, PermissionOperationState::Expired);
        assert!(stored.payload_ref.is_empty());
        assert_eq!(
            database
                .load_task(&task.task_id)
                .expect("task")
                .expect("present")
                .state,
            "cancelled"
        );
        assert!(database
            .expire_permission_operations("2026-07-17T00:00:03.000000000Z", 1)
            .expect("idempotent expiry")
            .is_empty());
    }
}
