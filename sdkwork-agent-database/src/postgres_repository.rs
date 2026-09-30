use sqlx::Row;

use crate::error::{DatabaseError, DatabaseResult};
use crate::pagination::{resolve_history_limit, resolve_list_limit, resolve_list_offset};
use crate::postgres::PostgresDatabase;
use crate::traits::*;
use crate::types::*;

fn map_session_row(row: &sqlx::postgres::PgRow) -> DatabaseResult<SessionRow> {
    Ok(SessionRow {
        session_id: row.try_get("session_id").map_err(map_sqlx_error)?,
        agent_id: row.try_get("agent_id").map_err(map_sqlx_error)?,
        kind: row.try_get("kind").map_err(map_sqlx_error)?,
        source: row.try_get("source").map_err(map_sqlx_error)?,
        state: row.try_get("state").map_err(map_sqlx_error)?,
        title: row.try_get("title").map_err(map_sqlx_error)?,
        model: row.try_get("model").map_err(map_sqlx_error)?,
        cwd: row.try_get("cwd").map_err(map_sqlx_error)?,
        provider_id: row.try_get("provider_id").map_err(map_sqlx_error)?,
        bridge_id: row.try_get("bridge_id").map_err(map_sqlx_error)?,
        token_usage_json: row.try_get("token_usage_json").map_err(map_sqlx_error)?,
        message_count: row.try_get("message_count").map_err(map_sqlx_error)?,
        owner_tenant_id: row.try_get("owner_tenant_id").map_err(map_sqlx_error)?,
        owner_user_ref: row.try_get("owner_user_ref").map_err(map_sqlx_error)?,
        created_at: row.try_get("created_at").map_err(map_sqlx_error)?,
        updated_at: row.try_get("updated_at").map_err(map_sqlx_error)?,
        metadata_json: row.try_get("metadata_json").map_err(map_sqlx_error)?,
    })
}

pub(crate) fn map_message_row(row: &sqlx::postgres::PgRow) -> DatabaseResult<MessageRow> {
    Ok(MessageRow {
        message_id: row.try_get("message_id").map_err(map_sqlx_error)?,
        session_id: row.try_get("session_id").map_err(map_sqlx_error)?,
        role: row.try_get("role").map_err(map_sqlx_error)?,
        content: row.try_get("content").map_err(map_sqlx_error)?,
        created_at: row.try_get("created_at").map_err(map_sqlx_error)?,
        metadata_json: row.try_get("metadata_json").map_err(map_sqlx_error)?,
    })
}

pub(crate) fn map_task_row(row: &sqlx::postgres::PgRow) -> DatabaseResult<TaskRow> {
    Ok(TaskRow {
        task_id: row.try_get("task_id").map_err(map_sqlx_error)?,
        session_id: row.try_get("session_id").map_err(map_sqlx_error)?,
        instruction: row.try_get("instruction").map_err(map_sqlx_error)?,
        state: row.try_get("state").map_err(map_sqlx_error)?,
        created_at: row.try_get("created_at").map_err(map_sqlx_error)?,
        updated_at: row.try_get("updated_at").map_err(map_sqlx_error)?,
    })
}

fn map_event_row(row: &sqlx::postgres::PgRow) -> DatabaseResult<EventRow> {
    Ok(EventRow {
        event_id: row.try_get("event_id").map_err(map_sqlx_error)?,
        session_id: row.try_get("session_id").map_err(map_sqlx_error)?,
        event_type: row.try_get("event_type").map_err(map_sqlx_error)?,
        severity: row.try_get("severity").map_err(map_sqlx_error)?,
        payload: row.try_get("payload").map_err(map_sqlx_error)?,
        created_at: row.try_get("created_at").map_err(map_sqlx_error)?,
    })
}

pub(crate) async fn postgres_save_event_idempotent<'e, E>(
    executor: E,
    event: &EventRow,
) -> DatabaseResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let accepted = sqlx::query_scalar::<_, bool>(
        "WITH inserted AS (
            INSERT INTO events (
                event_id, session_id, event_type, severity, payload, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (event_id) DO NOTHING
            RETURNING event_id
        )
        SELECT EXISTS(SELECT 1 FROM inserted)
            OR EXISTS(
                SELECT 1 FROM events
                WHERE event_id = $1
                  AND session_id IS NOT DISTINCT FROM $2
                  AND event_type = $3
                  AND severity = $4
                  AND payload IS NOT DISTINCT FROM $5
                  AND created_at = $6
            )",
    )
    .bind(&event.event_id)
    .bind(&event.session_id)
    .bind(&event.event_type)
    .bind(&event.severity)
    .bind(&event.payload)
    .bind(&event.created_at)
    .fetch_one(executor)
    .await
    .map_err(map_sqlx_error)?;
    if !accepted {
        return Err(DatabaseError::ConstraintViolation(format!(
            "event {} already exists with different identity or payload",
            event.event_id
        )));
    }
    Ok(())
}

async fn postgres_validate_event_retry_if_present<'e, E>(
    executor: E,
    event: &EventRow,
) -> DatabaseResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let accepted = sqlx::query_scalar::<_, bool>(
        "SELECT NOT EXISTS(SELECT 1 FROM events WHERE event_id = $1)
            OR EXISTS(
                SELECT 1 FROM events
                WHERE event_id = $1
                  AND session_id IS NOT DISTINCT FROM $2
                  AND event_type = $3
                  AND severity = $4
                  AND payload IS NOT DISTINCT FROM $5
                  AND created_at = $6
            )",
    )
    .bind(&event.event_id)
    .bind(&event.session_id)
    .bind(&event.event_type)
    .bind(&event.severity)
    .bind(&event.payload)
    .bind(&event.created_at)
    .fetch_one(executor)
    .await
    .map_err(map_sqlx_error)?;
    if !accepted {
        return Err(DatabaseError::ConstraintViolation(format!(
            "event {} already exists with different identity or payload",
            event.event_id
        )));
    }
    Ok(())
}

fn map_sqlx_error(error: sqlx::Error) -> DatabaseError {
    DatabaseError::Query(error.to_string())
}

impl RuntimeMaintenance for PostgresDatabase {
    fn purge_expired(&self, cutoff: &str, batch_size: i64) -> DatabaseResult<RuntimePurgeCounts> {
        if !(1..=10_000).contains(&batch_size) {
            return Err(DatabaseError::Query(
                "runtime purge batch_size must be between 1 and 10000".to_string(),
            ));
        }
        let pool = self.pool.pool().clone();
        let cutoff = cutoff.to_owned();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let mut counts = RuntimePurgeCounts::default();

            let session_ids = sqlx::query_scalar::<_, String>(
                "SELECT session_id FROM sessions
                 WHERE COALESCE(updated_at, created_at) < $1
                   AND lower(state) IN (
                     'closed','completed','complete','failed','cancelled','canceled',
                     'terminated','expired','orphaned','rejected','denied','approved','allow','deny'
                   )
                 ORDER BY COALESCE(updated_at, created_at), session_id
                 FOR UPDATE SKIP LOCKED LIMIT $2",
            )
            .bind(&cutoff)
            .bind(batch_size)
            .fetch_all(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            if !session_ids.is_empty() {
                for (table, count) in [
                    ("messages", &mut counts.messages),
                    ("tasks", &mut counts.tasks),
                    ("events", &mut counts.events),
                    ("permissions", &mut counts.permissions),
                ] {
                    let sql = format!("SELECT COUNT(*) FROM {table} WHERE session_id = ANY($1)");
                    let value = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql.clone()))
                        .bind(&session_ids)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(map_sqlx_error)?;
                    *count = (*count).saturating_add(value.max(0) as u64);
                }
                for (sql, count) in [
                    (
                        "SELECT COUNT(*) FROM runs WHERE session_id = ANY($1)",
                        &mut counts.runs,
                    ),
                    (
                        "SELECT COUNT(*) FROM steps WHERE run_id IN
                         (SELECT run_id FROM runs WHERE session_id = ANY($1))",
                        &mut counts.steps,
                    ),
                    (
                        "SELECT COUNT(*) FROM permission_operations WHERE run_id IN
                         (SELECT run_id FROM runs WHERE session_id = ANY($1))",
                        &mut counts.permission_operations,
                    ),
                ] {
                    let value = sqlx::query_scalar::<_, i64>(sql)
                        .bind(&session_ids)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(map_sqlx_error)?;
                    *count = count.saturating_add(value.max(0) as u64);
                }
                let result = sqlx::query("DELETE FROM sessions WHERE session_id = ANY($1)")
                    .bind(&session_ids)
                    .execute(&mut *tx)
                    .await
                    .map_err(map_sqlx_error)?;
                counts.sessions = result.rows_affected();
            }

            let message_rows = sqlx::query(
                "SELECT message_id, session_id FROM messages
                 WHERE created_at < $1 ORDER BY created_at, message_id
                 FOR UPDATE SKIP LOCKED LIMIT $2",
            )
            .bind(&cutoff)
            .bind(batch_size)
            .fetch_all(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            if !message_rows.is_empty() {
                let message_ids = message_rows
                    .iter()
                    .map(|row| {
                        row.try_get::<String, _>("message_id")
                            .map_err(map_sqlx_error)
                    })
                    .collect::<DatabaseResult<Vec<_>>>()?;
                let mut affected_sessions = message_rows
                    .iter()
                    .map(|row| {
                        row.try_get::<String, _>("session_id")
                            .map_err(map_sqlx_error)
                    })
                    .collect::<DatabaseResult<Vec<_>>>()?;
                affected_sessions.sort_unstable();
                affected_sessions.dedup();
                let result = sqlx::query("DELETE FROM messages WHERE message_id = ANY($1)")
                    .bind(&message_ids)
                    .execute(&mut *tx)
                    .await
                    .map_err(map_sqlx_error)?;
                counts.messages = counts.messages.saturating_add(result.rows_affected());
                sqlx::query(
                    "UPDATE sessions SET message_count = remaining.count
                     FROM (
                       SELECT sessions.session_id, COUNT(messages.message_id)::BIGINT AS count
                       FROM sessions
                       LEFT JOIN messages ON messages.session_id = sessions.session_id
                       WHERE sessions.session_id = ANY($1)
                       GROUP BY sessions.session_id
                     ) AS remaining
                     WHERE sessions.session_id = remaining.session_id",
                )
                .bind(&affected_sessions)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
            }

            let task_ids = sqlx::query_scalar::<_, String>(
                "SELECT task_id FROM tasks
                 WHERE COALESCE(updated_at, created_at) < $1
                   AND lower(state) IN (
                     'closed','completed','complete','failed','cancelled','canceled',
                     'terminated','expired','orphaned','rejected','denied','approved','allow','deny'
                   )
                 ORDER BY COALESCE(updated_at, created_at), task_id
                 FOR UPDATE SKIP LOCKED LIMIT $2",
            )
            .bind(&cutoff)
            .bind(batch_size)
            .fetch_all(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            if !task_ids.is_empty() {
                for (sql, count) in [
                    (
                        "SELECT COUNT(*) FROM runs WHERE task_id = ANY($1)",
                        &mut counts.runs,
                    ),
                    (
                        "SELECT COUNT(*) FROM steps WHERE run_id IN
                         (SELECT run_id FROM runs WHERE task_id = ANY($1))",
                        &mut counts.steps,
                    ),
                    (
                        "SELECT COUNT(*) FROM permission_operations WHERE run_id IN
                         (SELECT run_id FROM runs WHERE task_id = ANY($1))",
                        &mut counts.permission_operations,
                    ),
                ] {
                    let value = sqlx::query_scalar::<_, i64>(sql)
                        .bind(&task_ids)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(map_sqlx_error)?;
                    *count = count.saturating_add(value.max(0) as u64);
                }
                let result = sqlx::query("DELETE FROM tasks WHERE task_id = ANY($1)")
                    .bind(&task_ids)
                    .execute(&mut *tx)
                    .await
                    .map_err(map_sqlx_error)?;
                counts.tasks = counts.tasks.saturating_add(result.rows_affected());
            }

            let event_ids = sqlx::query_scalar::<_, String>(
                "SELECT event_id FROM events WHERE created_at < $1
                 ORDER BY created_at, event_id FOR UPDATE SKIP LOCKED LIMIT $2",
            )
            .bind(&cutoff)
            .bind(batch_size)
            .fetch_all(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            if !event_ids.is_empty() {
                let result = sqlx::query("DELETE FROM events WHERE event_id = ANY($1)")
                    .bind(&event_ids)
                    .execute(&mut *tx)
                    .await
                    .map_err(map_sqlx_error)?;
                counts.events = counts.events.saturating_add(result.rows_affected());
            }

            let operation_ids = sqlx::query_scalar::<_, String>(
                "SELECT permission_request_id FROM permission_operations
                 WHERE updated_at < $1
                   AND state IN ('completed','failed','expired','cancelled')
                 ORDER BY updated_at, permission_request_id
                 FOR UPDATE SKIP LOCKED LIMIT $2",
            )
            .bind(&cutoff)
            .bind(batch_size)
            .fetch_all(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            if !operation_ids.is_empty() {
                let result = sqlx::query(
                    "DELETE FROM permission_operations WHERE permission_request_id = ANY($1)",
                )
                .bind(&operation_ids)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                counts.permission_operations = counts
                    .permission_operations
                    .saturating_add(result.rows_affected());
            }

            let run_ids = sqlx::query_scalar::<_, String>(
                "SELECT run_id FROM runs
                 WHERE COALESCE(finished_at, updated_at) < $1
                   AND state IN ('completed','failed','cancelled')
                 ORDER BY COALESCE(finished_at, updated_at), run_id
                 FOR UPDATE SKIP LOCKED LIMIT $2",
            )
            .bind(&cutoff)
            .bind(batch_size)
            .fetch_all(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            if !run_ids.is_empty() {
                for (table, count) in [
                    ("steps", &mut counts.steps),
                    ("permission_operations", &mut counts.permission_operations),
                ] {
                    let value = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                        "SELECT COUNT(*) FROM {table} WHERE run_id = ANY($1)"
                    )))
                    .bind(&run_ids)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(map_sqlx_error)?;
                    *count = count.saturating_add(value.max(0) as u64);
                }
                let result = sqlx::query("DELETE FROM runs WHERE run_id = ANY($1)")
                    .bind(&run_ids)
                    .execute(&mut *tx)
                    .await
                    .map_err(map_sqlx_error)?;
                counts.runs = counts.runs.saturating_add(result.rows_affected());
            }

            let permission_ids = sqlx::query_scalar::<_, String>(
                "SELECT permission_request_id FROM permissions
                 WHERE COALESCE(updated_at, created_at) < $1
                   AND lower(status) IN (
                     'closed','completed','complete','failed','cancelled','canceled',
                     'terminated','expired','orphaned','rejected','denied','approved','allow','deny'
                   )
                 ORDER BY COALESCE(updated_at, created_at), permission_request_id
                 FOR UPDATE SKIP LOCKED LIMIT $2",
            )
            .bind(&cutoff)
            .bind(batch_size)
            .fetch_all(&mut *tx)
            .await
            .map_err(map_sqlx_error)?;
            if !permission_ids.is_empty() {
                let result =
                    sqlx::query("DELETE FROM permissions WHERE permission_request_id = ANY($1)")
                        .bind(&permission_ids)
                        .execute(&mut *tx)
                        .await
                        .map_err(map_sqlx_error)?;
                counts.permissions = counts.permissions.saturating_add(result.rows_affected());
            }

            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(counts)
        })
    }

    fn schema_status(&self) -> DatabaseResult<RuntimeSchemaStatus> {
        let pool = self.pool.pool().clone();
        self.pool.run_db(async move {
            let (version, count) = sqlx::query_as::<_, (i64, i64)>(
                "SELECT COALESCE(MAX(version), 0), COUNT(*)
                 FROM agent_runtime_schema_migration_history",
            )
            .fetch_one(&pool)
            .await
            .map_err(map_sqlx_error)?;
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let structural = crate::schema_migrations::validate_postgres_schema(&mut tx)
                .await
                .is_ok();
            tx.rollback().await.map_err(map_sqlx_error)?;
            // The PostgreSQL store records the authoritative idempotent
            // baseline as exactly one history row at CURRENT_SCHEMA_VERSION
            // (see apply_postgres_pool); SQLite instead records one row per
            // versioned migration. Structural drift is validated separately.
            Ok(RuntimeSchemaStatus {
                version,
                expected_version: CURRENT_SCHEMA_VERSION,
                drift_free: structural
                    && version == CURRENT_SCHEMA_VERSION
                    && count == 1,
            })
        })
    }

    fn run_maintenance(&self) -> DatabaseResult<()> {
        // PostgreSQL autovacuum owns dead-row reclamation. Application workers
        // must not issue blocking VACUUM commands on every retention pass.
        Ok(())
    }
}

impl SessionRepository for PostgresDatabase {
    fn save_session(&self, session: &SessionRow) -> DatabaseResult<()> {
        let pool = self.pool.pool().clone();
        let session = session.clone();
        self.pool.run_db(async move {
            let result = sqlx::query(
                "INSERT INTO sessions (
                    session_id, agent_id, kind, source, state, title, model, cwd,
                    provider_id, bridge_id, token_usage_json, message_count,
                    owner_tenant_id, owner_user_ref,
                    created_at, updated_at, metadata_json
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
                ON CONFLICT (session_id) DO UPDATE SET
                    agent_id = EXCLUDED.agent_id,
                    kind = EXCLUDED.kind,
                    source = EXCLUDED.source,
                    state = EXCLUDED.state,
                    title = EXCLUDED.title,
                    model = EXCLUDED.model,
                    cwd = EXCLUDED.cwd,
                    provider_id = EXCLUDED.provider_id,
                    bridge_id = EXCLUDED.bridge_id,
                    token_usage_json = EXCLUDED.token_usage_json,
                    updated_at = EXCLUDED.updated_at,
                    metadata_json = EXCLUDED.metadata_json
                WHERE sessions.provider_id IS NOT DISTINCT FROM EXCLUDED.provider_id
                  AND (
                    LOWER(sessions.state) NOT IN ('closed', 'failed', 'archived')
                    OR LOWER(EXCLUDED.state) IN ('closed', 'failed', 'archived')
                  )",
            )
            .bind(&session.session_id)
            .bind(&session.agent_id)
            .bind(&session.kind)
            .bind(&session.source)
            .bind(&session.state)
            .bind(&session.title)
            .bind(&session.model)
            .bind(&session.cwd)
            .bind(&session.provider_id)
            .bind(&session.bridge_id)
            .bind(&session.token_usage_json)
            .bind(session.message_count)
            .bind(&session.owner_tenant_id)
            .bind(&session.owner_user_ref)
            .bind(&session.created_at)
            .bind(&session.updated_at)
            .bind(&session.metadata_json)
            .execute(&pool)
            .await?;
            if result.rows_affected() == 0 {
                return Err(DatabaseError::ConstraintViolation(format!(
                    "session {} update conflicts with provider ownership or terminal lifecycle",
                    session.session_id
                )));
            }
            Ok(())
        })
    }

    fn load_session(&self, session_id: &str) -> DatabaseResult<Option<SessionRow>> {
        let pool = self.pool.pool().clone();
        let session_id = session_id.to_owned();
        self.pool.run_db(async move {
            let row = sqlx::query(
                "SELECT session_id, agent_id, kind, source, state, title, model, cwd,
                        provider_id, bridge_id, token_usage_json, message_count,
                        owner_tenant_id, owner_user_ref,
                        created_at, updated_at, metadata_json
                 FROM sessions WHERE session_id = $1",
            )
            .bind(&session_id)
            .fetch_optional(&pool)
            .await?;
            row.map(|row| map_session_row(&row)).transpose()
        })
    }

    fn list_sessions(&self, query: &SessionQuery) -> DatabaseResult<Vec<SessionRow>> {
        let pool = self.pool.pool().clone();
        let query = query.clone();
        self.pool.run_db(async move {
            let mut builder = sqlx::QueryBuilder::new(
                "SELECT session_id, agent_id, kind, source, state, title, model, cwd,
                        provider_id, bridge_id, token_usage_json, message_count,
                        owner_tenant_id, owner_user_ref,
                        created_at, updated_at, metadata_json
                 FROM sessions WHERE 1 = 1",
            );
            if let Some(agent_id) = query.agent_id.as_deref() {
                builder.push(" AND agent_id = ");
                builder.push_bind(agent_id);
            }
            if let Some(state) = query.state.as_deref() {
                builder.push(" AND state = ");
                builder.push_bind(state);
            }
            if let Some(kind) = query.kind.as_deref() {
                builder.push(" AND kind = ");
                builder.push_bind(kind);
            }
            if let Some(provider_id) = query.provider_id.as_deref() {
                builder.push(" AND provider_id = ");
                builder.push_bind(provider_id);
            }
            if let Some(bridge_id) = query.bridge_id.as_deref() {
                builder.push(" AND bridge_id = ");
                builder.push_bind(bridge_id);
            }
            if let Some(owner_tenant_id) = query.owner_tenant_id.as_deref() {
                builder.push(" AND owner_tenant_id = ");
                builder.push_bind(owner_tenant_id);
            }
            if let Some(owner_user_ref) = query.owner_user_ref.as_deref() {
                builder.push(" AND owner_user_ref = ");
                builder.push_bind(owner_user_ref);
            }
            if let Some(after_session_id) = query
                .after_session_id
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                if let Some(after_sort_at) = query
                    .after_session_sort_at
                    .as_deref()
                    .filter(|value| !value.is_empty())
                {
                    builder.push(
                        " AND (
                            COALESCE(updated_at, created_at) < ",
                    );
                    builder.push_bind(after_sort_at);
                    builder.push(
                        " OR (
                            COALESCE(updated_at, created_at) = ",
                    );
                    builder.push_bind(after_sort_at);
                    builder.push(" AND session_id < ");
                    builder.push_bind(after_session_id);
                    builder.push(" ) )");
                } else {
                    builder.push(
                        " AND EXISTS (
                            SELECT 1 FROM sessions AS session_cursor
                            WHERE session_cursor.session_id = ",
                    );
                    builder.push_bind(after_session_id);
                    builder.push(
                        " AND (
                            COALESCE(sessions.updated_at, sessions.created_at)
                              < COALESCE(session_cursor.updated_at, session_cursor.created_at)
                            OR (
                              COALESCE(sessions.updated_at, sessions.created_at)
                                = COALESCE(session_cursor.updated_at, session_cursor.created_at)
                              AND sessions.session_id < session_cursor.session_id
                            )
                          )
                        )",
                    );
                }
            }
            builder.push(" ORDER BY COALESCE(updated_at, created_at) DESC, session_id DESC");
            let limit = resolve_list_limit(query.limit);
            let offset = if query.after_session_id.is_some() {
                0
            } else {
                resolve_list_offset(query.offset)
            };
            builder.push(" LIMIT ");
            builder.push_bind(limit);
            builder.push(" OFFSET ");
            builder.push_bind(offset);
            let rows = builder.build().fetch_all(&pool).await?;
            rows.iter().map(map_session_row).collect()
        })
    }

    fn update_session(&self, session: &SessionRow) -> DatabaseResult<()> {
        let pool = self.pool.pool().clone();
        let session = session.clone();
        self.pool.run_db(async move {
            let result = sqlx::query(
                "UPDATE sessions SET
                    agent_id = $2, kind = $3, source = $4, state = $5,
                    title = $6, model = $7, cwd = $8, provider_id = $9,
                    bridge_id = $10, token_usage_json = $11, updated_at = $12,
                    metadata_json = $13
                 WHERE session_id = $1
                   AND provider_id IS NOT DISTINCT FROM $9
                   AND (
                     LOWER(state) NOT IN ('closed', 'failed', 'archived')
                     OR LOWER($5) IN ('closed', 'failed', 'archived')
                   )",
            )
            .bind(&session.session_id)
            .bind(&session.agent_id)
            .bind(&session.kind)
            .bind(&session.source)
            .bind(&session.state)
            .bind(&session.title)
            .bind(&session.model)
            .bind(&session.cwd)
            .bind(&session.provider_id)
            .bind(&session.bridge_id)
            .bind(&session.token_usage_json)
            .bind(&session.updated_at)
            .bind(&session.metadata_json)
            .execute(&pool)
            .await?;
            if result.rows_affected() != 1 {
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id = $1)",
                )
                .bind(&session.session_id)
                .fetch_one(&pool)
                .await?;
                return if exists {
                    Err(DatabaseError::ConstraintViolation(format!(
                        "session {} update conflicts with provider ownership or terminal lifecycle",
                        session.session_id
                    )))
                } else {
                    Err(DatabaseError::NotFound(format!(
                        "session not found: {}",
                        session.session_id
                    )))
                };
            }
            Ok(())
        })
    }

    fn delete_session(&self, session_id: &str) -> DatabaseResult<()> {
        let pool = self.pool.pool().clone();
        let session_id = session_id.to_owned();
        self.pool.run_db(async move {
            sqlx::query("DELETE FROM sessions WHERE session_id = $1")
                .bind(&session_id)
                .execute(&pool)
                .await?;
            Ok(())
        })
    }

    fn delete_session_cascade(&self, session_id: &str) -> DatabaseResult<()> {
        let pool = self.pool.pool().clone();
        let session_id = session_id.to_owned();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            sqlx::query("DELETE FROM events WHERE session_id = $1")
                .bind(&session_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM messages WHERE session_id = $1")
                .bind(&session_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM tasks WHERE session_id = $1")
                .bind(&session_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM permissions WHERE session_id = $1")
                .bind(&session_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM sessions WHERE session_id = $1")
                .bind(&session_id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(())
        })
    }

    fn increment_session_message_count(&self, session_id: &str) -> DatabaseResult<i64> {
        let pool = self.pool.pool().clone();
        let session_id = session_id.to_owned();
        let updated_at = crate::types::runtime_now_timestamp();
        self.pool.run_db(async move {
            let count = sqlx::query_scalar::<_, i64>(
                "UPDATE sessions SET message_count = message_count + 1, updated_at = $2 \
                 WHERE session_id = $1
                   AND LOWER(state) NOT IN ('closed', 'failed', 'archived')
                   AND message_count < 9223372036854775807
                 RETURNING message_count",
            )
            .bind(&session_id)
            .bind(&updated_at)
            .fetch_optional(&pool)
            .await?;
            if let Some(count) = count {
                return Ok(count);
            }
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id = $1)")
                    .bind(&session_id)
                    .fetch_one(&pool)
                    .await?;
            if exists {
                Err(DatabaseError::ConstraintViolation(format!(
                    "session {session_id} is terminal or its message count is exhausted"
                )))
            } else {
                Err(DatabaseError::NotFound(format!(
                    "session not found: {session_id}"
                )))
            }
        })
    }
}

impl MessageRepository for PostgresDatabase {
    fn save_message(&self, message: &MessageRow) -> DatabaseResult<()> {
        let pool = self.pool.pool().clone();
        let message = message.clone();
        self.pool.run_db(async move {
            let insert_result = sqlx::query(
                "INSERT INTO messages (
                    message_id, session_id, role, content, created_at, metadata_json
                ) VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (message_id) DO NOTHING",
            )
            .bind(&message.message_id)
            .bind(&message.session_id)
            .bind(&message.role)
            .bind(&message.content)
            .bind(&message.created_at)
            .bind(&message.metadata_json)
            .execute(&pool)
            .await?;
            if insert_result.rows_affected() == 0 {
                let existing = sqlx::query(
                    "SELECT message_id, session_id, role, content, created_at, metadata_json
                     FROM messages WHERE message_id = $1",
                )
                .bind(&message.message_id)
                .fetch_one(&pool)
                .await
                .map_err(map_sqlx_error)?;
                let existing = map_message_row(&existing)?;
                crate::message_identity::ensure_message_retry_matches(&existing, &message)?;
            }
            Ok(())
        })
    }

    fn load_messages(
        &self,
        session_id: &str,
        query: &MessageQuery,
    ) -> DatabaseResult<Vec<MessageRow>> {
        let pool = self.pool.pool().clone();
        let session_id = session_id.to_owned();
        let query = query.clone();
        self.pool.run_db(async move {
            let mut builder = sqlx::QueryBuilder::new(
                "SELECT message_id, session_id, role, content, created_at, metadata_json
                 FROM messages WHERE session_id = ",
            );
            builder.push_bind(&session_id);
            if let Some(after_message_id) = query
                .after_message_id
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                if let Some(after_created_at) = query
                    .after_message_created_at
                    .as_deref()
                    .filter(|value| !value.is_empty())
                {
                    builder.push(" AND (created_at > ");
                    builder.push_bind(after_created_at);
                    builder.push(" OR (created_at = ");
                    builder.push_bind(after_created_at);
                    builder.push(" AND message_id > ");
                    builder.push_bind(after_message_id);
                    builder.push(" ) )");
                } else {
                    builder.push(
                        " AND EXISTS (
                            SELECT 1 FROM messages AS message_cursor
                            WHERE message_cursor.message_id = ",
                    );
                    builder.push_bind(after_message_id);
                    builder.push(
                        " AND message_cursor.session_id = messages.session_id
                          AND (
                            messages.created_at > message_cursor.created_at
                            OR (
                              messages.created_at = message_cursor.created_at
                              AND messages.message_id > message_cursor.message_id
                            )
                          )
                        )",
                    );
                }
            }
            builder.push(" ORDER BY created_at ASC, message_id ASC");
            let limit = resolve_list_limit(query.limit);
            let offset = if query.after_message_id.is_some() {
                0
            } else {
                resolve_list_offset(query.offset)
            };
            builder.push(" LIMIT ");
            builder.push_bind(limit);
            builder.push(" OFFSET ");
            builder.push_bind(offset);
            let rows = builder.build().fetch_all(&pool).await?;
            rows.iter().map(map_message_row).collect()
        })
    }

    fn load_recent_messages(
        &self,
        session_id: &str,
        limit: i64,
    ) -> DatabaseResult<Vec<MessageRow>> {
        let limit = resolve_history_limit(limit)?;
        let pool = self.pool.pool().clone();
        let session_id = session_id.to_owned();
        self.pool.run_db(async move {
            let rows = sqlx::query(
                "SELECT message_id, session_id, role, content, created_at, metadata_json
                 FROM messages
                 WHERE session_id = $1
                 ORDER BY created_at DESC, message_id DESC
                 LIMIT $2",
            )
            .bind(&session_id)
            .bind(limit)
            .fetch_all(&pool)
            .await?;
            let mut messages: Vec<MessageRow> = rows
                .iter()
                .map(map_message_row)
                .collect::<DatabaseResult<_>>()?;
            messages.reverse();
            Ok(messages)
        })
    }

    fn message_count(&self, session_id: &str) -> DatabaseResult<i64> {
        let pool = self.pool.pool().clone();
        let session_id = session_id.to_owned();
        self.pool.run_db(async move {
            let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages WHERE session_id = $1")
                .bind(&session_id)
                .fetch_one(&pool)
                .await?;
            Ok(row.0)
        })
    }

    fn delete_messages(&self, session_id: &str) -> DatabaseResult<()> {
        let pool = self.pool.pool().clone();
        let session_id = session_id.to_owned();
        self.pool.run_db(async move {
            sqlx::query("DELETE FROM messages WHERE session_id = $1")
                .bind(&session_id)
                .execute(&pool)
                .await?;
            Ok(())
        })
    }
}

impl TaskRepository for PostgresDatabase {
    fn save_task(&self, task: &TaskRow) -> DatabaseResult<()> {
        let pool = self.pool.pool().clone();
        let task = task.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let _session_state = sqlx::query_scalar::<_, String>(
                "SELECT state FROM sessions WHERE session_id = $1 FOR UPDATE",
            )
            .bind(&task.session_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                DatabaseError::NotFound(format!("session not found: {}", task.session_id))
            })?;
            let result = sqlx::query(
                "INSERT INTO tasks (
                    task_id, session_id, instruction, state, created_at, updated_at
                ) VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (task_id) DO UPDATE SET
                    session_id = EXCLUDED.session_id,
                    instruction = EXCLUDED.instruction,
                    state = EXCLUDED.state,
                    created_at = EXCLUDED.created_at,
                    updated_at = EXCLUDED.updated_at
                WHERE tasks.session_id = EXCLUDED.session_id
                  AND (
                    LOWER(tasks.state) NOT IN ('completed', 'failed', 'cancelled', 'canceled')
                    OR LOWER(tasks.state) = LOWER(EXCLUDED.state)
                  )",
            )
            .bind(&task.task_id)
            .bind(&task.session_id)
            .bind(&task.instruction)
            .bind(&task.state)
            .bind(&task.created_at)
            .bind(&task.updated_at)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                return Err(DatabaseError::ConstraintViolation(format!(
                    "task {} update conflicts with session ownership or terminal lifecycle",
                    task.task_id
                )));
            }
            tx.commit().await?;
            Ok(())
        })
    }

    fn load_task(&self, task_id: &str) -> DatabaseResult<Option<TaskRow>> {
        let pool = self.pool.pool().clone();
        let task_id = task_id.to_owned();
        self.pool.run_db(async move {
            let row = sqlx::query(
                "SELECT task_id, session_id, instruction, state, created_at, updated_at
                 FROM tasks WHERE task_id = $1",
            )
            .bind(&task_id)
            .fetch_optional(&pool)
            .await?;
            row.map(|row| map_task_row(&row)).transpose()
        })
    }

    fn load_tasks(&self, session_id: &str, query: &TaskQuery) -> DatabaseResult<Vec<TaskRow>> {
        let pool = self.pool.pool().clone();
        let session_id = session_id.to_owned();
        let query = query.clone();
        self.pool.run_db(async move {
            let mut builder = sqlx::QueryBuilder::new(
                "SELECT task_id, session_id, instruction, state, created_at, updated_at
                 FROM tasks WHERE session_id = ",
            );
            builder.push_bind(&session_id);
            if let Some(after_task_id) = query
                .after_task_id
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                if let Some(after_created_at) = query
                    .after_task_created_at
                    .as_deref()
                    .filter(|value| !value.is_empty())
                {
                    builder.push(" AND (created_at > ");
                    builder.push_bind(after_created_at);
                    builder.push(" OR (created_at = ");
                    builder.push_bind(after_created_at);
                    builder.push(" AND task_id > ");
                    builder.push_bind(after_task_id);
                    builder.push(" ) )");
                } else {
                    builder.push(
                        " AND EXISTS (
                            SELECT 1 FROM tasks AS task_cursor
                            WHERE task_cursor.task_id = ",
                    );
                    builder.push_bind(after_task_id);
                    builder.push(
                        " AND task_cursor.session_id = tasks.session_id
                          AND (
                            tasks.created_at > task_cursor.created_at
                            OR (
                              tasks.created_at = task_cursor.created_at
                              AND tasks.task_id > task_cursor.task_id
                            )
                          )
                        )",
                    );
                }
            }
            builder.push(" ORDER BY created_at ASC, task_id ASC");
            let limit = resolve_list_limit(query.limit);
            let offset = if query.after_task_id.is_some() {
                0
            } else {
                resolve_list_offset(query.offset)
            };
            builder.push(" LIMIT ");
            builder.push_bind(limit);
            builder.push(" OFFSET ");
            builder.push_bind(offset);
            let rows = builder.build().fetch_all(&pool).await?;
            rows.iter().map(map_task_row).collect()
        })
    }

    fn update_task(&self, task: &TaskRow) -> DatabaseResult<()> {
        self.save_task(task)
    }

    fn delete_task(&self, task_id: &str) -> DatabaseResult<()> {
        let pool = self.pool.pool().clone();
        let task_id = task_id.to_owned();
        self.pool.run_db(async move {
            sqlx::query("DELETE FROM tasks WHERE task_id = $1")
                .bind(&task_id)
                .execute(&pool)
                .await?;
            Ok(())
        })
    }
}

impl EventRepository for PostgresDatabase {
    fn save_event(&self, event: &EventRow) -> DatabaseResult<()> {
        let pool = self.pool.pool().clone();
        let event = event.clone();
        self.pool
            .run_db(async move { postgres_save_event_idempotent(&pool, &event).await })
    }

    fn load_events(&self, session_id: &str, query: &EventQuery) -> DatabaseResult<Vec<EventRow>> {
        let pool = self.pool.pool().clone();
        let session_id = session_id.to_owned();
        let query = query.clone();
        self.pool.run_db(async move {
            let mut builder = sqlx::QueryBuilder::new(
                "SELECT event_id, session_id, event_type, severity, payload, created_at
                 FROM events WHERE session_id = ",
            );
            builder.push_bind(&session_id);
            if let Some(event_type) = query.event_type.as_deref() {
                builder.push(" AND event_type = ");
                builder.push_bind(event_type);
            }
            if let Some(severity) = query.severity.as_deref() {
                builder.push(" AND severity = ");
                builder.push_bind(severity);
            }
            if query.owner_tenant_id.is_some() || query.owner_user_ref.is_some() {
                builder.push(
                    " AND events.session_id IS NOT NULL
                      AND EXISTS (
                          SELECT 1 FROM sessions AS event_session
                          WHERE event_session.session_id = events.session_id",
                );
                if let Some(owner_tenant_id) = query.owner_tenant_id.as_deref() {
                    builder.push(" AND event_session.owner_tenant_id = ");
                    builder.push_bind(owner_tenant_id);
                }
                if let Some(owner_user_ref) = query.owner_user_ref.as_deref() {
                    builder.push(" AND event_session.owner_user_ref = ");
                    builder.push_bind(owner_user_ref);
                }
                builder.push(")");
            }
            if let Some(after_event_id) = query
                .after_event_id
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                builder.push(
                    " AND EXISTS (
                        SELECT 1 FROM events AS event_cursor
                        WHERE event_cursor.event_id = ",
                );
                builder.push_bind(after_event_id);
                builder.push(
                    " AND event_cursor.session_id = events.session_id
                      AND (
                        events.created_at > event_cursor.created_at
                        OR (
                          events.created_at = event_cursor.created_at
                          AND events.event_id > event_cursor.event_id
                        )
                      )
                    )",
                );
            }
            builder.push(" ORDER BY created_at ASC, event_id ASC");
            let limit = resolve_list_limit(query.limit);
            let offset = if query.after_event_id.is_some() {
                0
            } else {
                resolve_list_offset(query.offset)
            };
            builder.push(" LIMIT ");
            builder.push_bind(limit);
            builder.push(" OFFSET ");
            builder.push_bind(offset);
            let rows = builder.build().fetch_all(&pool).await?;
            rows.iter().map(map_event_row).collect()
        })
    }

    fn list_recent_events(&self, query: &EventQuery) -> DatabaseResult<Vec<EventRow>> {
        let pool = self.pool.pool().clone();
        let query = query.clone();
        self.pool.run_db(async move {
            let mut builder = sqlx::QueryBuilder::new(
                "SELECT event_id, session_id, event_type, severity, payload, created_at
                 FROM events WHERE 1 = 1",
            );
            if let Some(event_type) = query.event_type.as_deref() {
                builder.push(" AND event_type = ");
                builder.push_bind(event_type);
            }
            if let Some(severity) = query.severity.as_deref() {
                builder.push(" AND severity = ");
                builder.push_bind(severity);
            }
            if query.owner_tenant_id.is_some() || query.owner_user_ref.is_some() {
                builder.push(
                    " AND events.session_id IS NOT NULL
                      AND EXISTS (
                          SELECT 1 FROM sessions AS event_session
                          WHERE event_session.session_id = events.session_id",
                );
                if let Some(owner_tenant_id) = query.owner_tenant_id.as_deref() {
                    builder.push(" AND event_session.owner_tenant_id = ");
                    builder.push_bind(owner_tenant_id);
                }
                if let Some(owner_user_ref) = query.owner_user_ref.as_deref() {
                    builder.push(" AND event_session.owner_user_ref = ");
                    builder.push_bind(owner_user_ref);
                }
                builder.push(")");
            }
            builder.push(" ORDER BY created_at DESC, event_id DESC");
            let limit = resolve_list_limit(query.limit);
            let offset = if query.after_event_id.is_some() {
                0
            } else {
                resolve_list_offset(query.offset)
            };
            builder.push(" LIMIT ");
            builder.push_bind(limit);
            builder.push(" OFFSET ");
            builder.push_bind(offset);
            let rows = builder.build().fetch_all(&pool).await?;
            rows.iter().map(map_event_row).collect()
        })
    }

    fn delete_events(&self, session_id: &str) -> DatabaseResult<()> {
        let pool = self.pool.pool().clone();
        let session_id = session_id.to_owned();
        self.pool.run_db(async move {
            sqlx::query("DELETE FROM events WHERE session_id = $1")
                .bind(&session_id)
                .execute(&pool)
                .await?;
            Ok(())
        })
    }
}

fn map_permission_row(row: &sqlx::postgres::PgRow) -> DatabaseResult<PermissionRow> {
    Ok(PermissionRow {
        permission_request_id: row
            .try_get("permission_request_id")
            .map_err(map_sqlx_error)?,
        session_id: row.try_get("session_id").map_err(map_sqlx_error)?,
        category: row.try_get("category").map_err(map_sqlx_error)?,
        resource: row.try_get("resource").map_err(map_sqlx_error)?,
        side_effect_level: row.try_get("side_effect_level").map_err(map_sqlx_error)?,
        reason: row.try_get("reason").map_err(map_sqlx_error)?,
        status: row.try_get("status").map_err(map_sqlx_error)?,
        owner_tenant_id: row.try_get("owner_tenant_id").map_err(map_sqlx_error)?,
        owner_user_ref: row.try_get("owner_user_ref").map_err(map_sqlx_error)?,
        created_at: row.try_get("created_at").map_err(map_sqlx_error)?,
        updated_at: row.try_get("updated_at").map_err(map_sqlx_error)?,
    })
}

impl PermissionRepository for PostgresDatabase {
    fn create_permission_if_absent(&self, permission: &PermissionRow) -> DatabaseResult<bool> {
        let pool = self.pool.pool().clone();
        let permission = permission.clone();
        self.pool.run_db(async move {
            let result = sqlx::query(
                "INSERT INTO permissions (
                    permission_request_id, session_id, category, resource,
                    side_effect_level, reason, status, owner_tenant_id,
                    owner_user_ref, created_at, updated_at
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                 ON CONFLICT (permission_request_id) DO NOTHING",
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
            .execute(&pool)
            .await?;
            Ok(result.rows_affected() == 1)
        })
    }

    fn save_permission(&self, permission: &PermissionRow) -> DatabaseResult<()> {
        let pool = self.pool.pool().clone();
        let permission = permission.clone();
        self.pool.run_db(async move {
            sqlx::query(
                "INSERT INTO permissions (
                    permission_request_id, session_id, category, resource,
                    side_effect_level, reason, status, owner_tenant_id,
                    owner_user_ref, created_at, updated_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                ON CONFLICT (permission_request_id) DO UPDATE SET
                    session_id = EXCLUDED.session_id,
                    category = EXCLUDED.category,
                    resource = EXCLUDED.resource,
                    side_effect_level = EXCLUDED.side_effect_level,
                    reason = EXCLUDED.reason,
                    status = EXCLUDED.status,
                    owner_tenant_id = EXCLUDED.owner_tenant_id,
                    owner_user_ref = EXCLUDED.owner_user_ref,
                    created_at = EXCLUDED.created_at,
                    updated_at = EXCLUDED.updated_at",
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
            .execute(&pool)
            .await?;
            Ok(())
        })
    }

    fn load_permission(
        &self,
        permission_request_id: &str,
    ) -> DatabaseResult<Option<PermissionRow>> {
        let pool = self.pool.pool().clone();
        let permission_request_id = permission_request_id.to_owned();
        self.pool.run_db(async move {
            let row = sqlx::query(
                "SELECT permission_request_id, session_id, category, resource,
                 side_effect_level, reason, status, owner_tenant_id,
                 owner_user_ref, created_at, updated_at
                 FROM permissions WHERE permission_request_id = $1",
            )
            .bind(&permission_request_id)
            .fetch_optional(&pool)
            .await?;
            row.map(|row| map_permission_row(&row)).transpose()
        })
    }

    fn list_permissions(&self, query: &PermissionQuery) -> DatabaseResult<Vec<PermissionRow>> {
        let pool = self.pool.pool().clone();
        let query = query.clone();
        self.pool.run_db(async move {
            let mut builder = sqlx::QueryBuilder::new(
                "SELECT permission_request_id, session_id, category, resource,
                 side_effect_level, reason, status, owner_tenant_id,
                 owner_user_ref, created_at, updated_at
                 FROM permissions WHERE 1 = 1",
            );
            if let Some(ref status) = query.status {
                builder.push(" AND status = ");
                builder.push_bind(status);
            }
            if let Some(ref owner_tenant_id) = query.owner_tenant_id {
                builder.push(" AND owner_tenant_id = ");
                builder.push_bind(owner_tenant_id);
            }
            if let Some(ref owner_user_ref) = query.owner_user_ref {
                builder.push(" AND owner_user_ref = ");
                builder.push_bind(owner_user_ref);
            }
            builder.push(" ORDER BY created_at DESC, permission_request_id DESC");
            let limit = resolve_list_limit(query.limit);
            let offset = resolve_list_offset(query.offset);
            builder.push(" LIMIT ");
            builder.push_bind(limit);
            builder.push(" OFFSET ");
            builder.push_bind(offset);
            let rows = builder.build().fetch_all(&pool).await?;
            rows.iter().map(map_permission_row).collect()
        })
    }

    fn update_permission_status(
        &self,
        permission_request_id: &str,
        status: &str,
    ) -> DatabaseResult<()> {
        if !matches!(status, "allow" | "deny") {
            return Err(DatabaseError::ConstraintViolation(
                "permission status must be allow or deny".to_string(),
            ));
        }
        let pool = self.pool.pool().clone();
        let permission_request_id = permission_request_id.to_owned();
        let status = status.to_owned();
        self.pool.run_db(async move {
            let now = crate::types::runtime_now_timestamp();
            let result = sqlx::query(
                "UPDATE permissions SET status = $1, updated_at = $2
                 WHERE permission_request_id = $3 AND (status = 'pending' OR status = $1)",
            )
            .bind(&status)
            .bind(&now)
            .bind(&permission_request_id)
            .execute(&pool)
            .await?;
            if result.rows_affected() == 0 {
                return Err(DatabaseError::ConstraintViolation(
                    "permission request state conflict or not found".to_string(),
                ));
            }
            Ok(())
        })
    }
}

impl RuntimeSessionWrites for PostgresDatabase {
    fn save_session_with_event(
        &self,
        session: &SessionRow,
        event: &EventRow,
    ) -> DatabaseResult<()> {
        crate::event_identity::ensure_event_session(event, &session.session_id, "session write")?;
        let pool = self.pool.pool().clone();
        let session = session.clone();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let result = sqlx::query(
                "INSERT INTO sessions (
                    session_id, agent_id, kind, source, state, title, model, cwd,
                    provider_id, bridge_id, token_usage_json, message_count,
                    owner_tenant_id, owner_user_ref,
                    created_at, updated_at, metadata_json
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
                ON CONFLICT (session_id) DO UPDATE SET
                    agent_id = EXCLUDED.agent_id,
                    kind = EXCLUDED.kind,
                    source = EXCLUDED.source,
                    state = EXCLUDED.state,
                    title = EXCLUDED.title,
                    model = EXCLUDED.model,
                    cwd = EXCLUDED.cwd,
                    provider_id = EXCLUDED.provider_id,
                    bridge_id = EXCLUDED.bridge_id,
                    token_usage_json = EXCLUDED.token_usage_json,
                    updated_at = EXCLUDED.updated_at,
                    metadata_json = EXCLUDED.metadata_json
                WHERE sessions.provider_id IS NOT DISTINCT FROM EXCLUDED.provider_id
                  AND (
                    LOWER(sessions.state) NOT IN ('closed', 'failed', 'archived')
                    OR LOWER(EXCLUDED.state) IN ('closed', 'failed', 'archived')
                  )",
            )
            .bind(&session.session_id)
            .bind(&session.agent_id)
            .bind(&session.kind)
            .bind(&session.source)
            .bind(&session.state)
            .bind(&session.title)
            .bind(&session.model)
            .bind(&session.cwd)
            .bind(&session.provider_id)
            .bind(&session.bridge_id)
            .bind(&session.token_usage_json)
            .bind(session.message_count)
            .bind(&session.owner_tenant_id)
            .bind(&session.owner_user_ref)
            .bind(&session.created_at)
            .bind(&session.updated_at)
            .bind(&session.metadata_json)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                return Err(DatabaseError::ConstraintViolation(format!(
                    "session {} update conflicts with provider ownership or terminal lifecycle",
                    session.session_id
                )));
            }
            postgres_save_event_idempotent(&mut *tx, &event).await?;
            tx.commit().await?;
            Ok(())
        })
    }

    fn save_session_with_event_if_newer(
        &self,
        session: &SessionRow,
        event: &EventRow,
    ) -> DatabaseResult<bool> {
        crate::event_identity::ensure_event_session(
            event,
            &session.session_id,
            "provider session synchronization",
        )?;
        let pool = self.pool.pool().clone();
        let session = session.clone();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let applied = sqlx::query(
                "INSERT INTO sessions (
                    session_id, agent_id, kind, source, state, title, model, cwd,
                    provider_id, bridge_id, token_usage_json, message_count,
                    owner_tenant_id, owner_user_ref,
                    created_at, updated_at, metadata_json
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
                ON CONFLICT (session_id) DO UPDATE SET
                    agent_id = EXCLUDED.agent_id,
                    kind = EXCLUDED.kind,
                    source = EXCLUDED.source,
                    state = EXCLUDED.state,
                    title = EXCLUDED.title,
                    model = EXCLUDED.model,
                    cwd = EXCLUDED.cwd,
                    provider_id = EXCLUDED.provider_id,
                    bridge_id = EXCLUDED.bridge_id,
                    token_usage_json = EXCLUDED.token_usage_json,
                    message_count = EXCLUDED.message_count,
                    owner_tenant_id = EXCLUDED.owner_tenant_id,
                    owner_user_ref = EXCLUDED.owner_user_ref,
                    updated_at = EXCLUDED.updated_at,
                    metadata_json = EXCLUDED.metadata_json
                WHERE EXCLUDED.updated_at IS NOT NULL
                  AND (
                      sessions.provider_id IS NULL
                      OR EXCLUDED.provider_id IS NULL
                      OR sessions.provider_id = EXCLUDED.provider_id
                  )
                  AND (
                      LOWER(sessions.state) NOT IN ('closed', 'failed', 'archived')
                      OR LOWER(EXCLUDED.state) IN ('closed', 'failed', 'archived')
                  )
                  AND (
                      sessions.updated_at IS NULL
                      OR EXCLUDED.updated_at::timestamptz >= sessions.updated_at::timestamptz
                  )",
            )
            .bind(&session.session_id)
            .bind(&session.agent_id)
            .bind(&session.kind)
            .bind(&session.source)
            .bind(&session.state)
            .bind(&session.title)
            .bind(&session.model)
            .bind(&session.cwd)
            .bind(&session.provider_id)
            .bind(&session.bridge_id)
            .bind(&session.token_usage_json)
            .bind(session.message_count)
            .bind(&session.owner_tenant_id)
            .bind(&session.owner_user_ref)
            .bind(&session.created_at)
            .bind(&session.updated_at)
            .bind(&session.metadata_json)
            .execute(&mut *tx)
            .await?
            .rows_affected()
                > 0;
            if applied {
                postgres_save_event_idempotent(&mut *tx, &event).await?;
            }
            tx.commit().await?;
            Ok(applied)
        })
    }

    fn append_message_with_event(
        &self,
        message: &MessageRow,
        event: &EventRow,
    ) -> DatabaseResult<i64> {
        crate::event_identity::ensure_event_session(event, &message.session_id, "message append")?;
        let pool = self.pool.pool().clone();
        let message = message.clone();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let (session_state, current_count) = sqlx::query_as::<_, (String, i64)>(
                "SELECT state, message_count FROM sessions WHERE session_id = $1 FOR UPDATE",
            )
            .bind(&message.session_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                DatabaseError::NotFound(format!("session not found: {}", message.session_id))
            })?;
            if crate::types::session_state_is_terminal(&session_state) {
                let existing = sqlx::query(
                    "SELECT message_id, session_id, role, content, created_at, metadata_json
                     FROM messages WHERE message_id = $1",
                )
                .bind(&message.message_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                if let Some(existing) = existing {
                    let existing = map_message_row(&existing)?;
                    crate::message_identity::ensure_message_retry_matches(&existing, &message)?;
                    postgres_validate_event_retry_if_present(&mut *tx, &event).await?;
                    tx.commit().await?;
                    return Ok(current_count);
                }
                return Err(DatabaseError::ConstraintViolation(format!(
                    "session {} is terminal ({session_state})",
                    message.session_id
                )));
            }
            let insert_result = sqlx::query(
                "INSERT INTO messages (
                    message_id, session_id, role, content, created_at, metadata_json
                ) VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (message_id) DO NOTHING",
            )
            .bind(&message.message_id)
            .bind(&message.session_id)
            .bind(&message.role)
            .bind(&message.content)
            .bind(&message.created_at)
            .bind(&message.metadata_json)
            .execute(&mut *tx)
            .await?;
            let count = if insert_result.rows_affected() > 0 {
                let updated_at = crate::types::runtime_now_timestamp();
                let next_count = current_count.checked_add(1).ok_or_else(|| {
                    DatabaseError::ConstraintViolation("session message count overflow".to_string())
                })?;
                let count = sqlx::query_scalar::<_, i64>(
                    "UPDATE sessions SET message_count = $2, updated_at = $3 \
                     WHERE session_id = $1 RETURNING message_count",
                )
                .bind(&message.session_id)
                .bind(next_count)
                .bind(&updated_at)
                .fetch_optional(&mut *tx)
                .await?;
                count.ok_or_else(|| {
                    DatabaseError::NotFound(format!("session not found: {}", message.session_id))
                })?
            } else {
                let existing = sqlx::query(
                    "SELECT message_id, session_id, role, content, created_at, metadata_json
                     FROM messages WHERE message_id = $1",
                )
                .bind(&message.message_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                let existing = map_message_row(&existing)?;
                crate::message_identity::ensure_message_retry_matches(&existing, &message)?;
                postgres_validate_event_retry_if_present(&mut *tx, &event).await?;
                let count = sqlx::query_scalar::<_, i64>(
                    "SELECT message_count FROM sessions WHERE session_id = $1",
                )
                .bind(&message.session_id)
                .fetch_optional(&mut *tx)
                .await?;
                count.ok_or_else(|| {
                    DatabaseError::NotFound(format!("session not found: {}", message.session_id))
                })?
            };
            if insert_result.rows_affected() > 0 {
                postgres_save_event_idempotent(&mut *tx, &event).await?;
            }
            tx.commit().await?;
            Ok(count)
        })
    }

    fn append_message_turn_with_events(
        &self,
        turn_messages: &[MessageRow],
        turn_events: &[EventRow],
    ) -> DatabaseResult<i64> {
        let session_id =
            crate::message_identity::validate_message_turn(turn_messages, turn_events)?.to_owned();
        let messages = turn_messages.to_vec();
        let events = turn_events.to_vec();
        let pool = self.pool.pool().clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let (session_state, current_count) = sqlx::query_as::<_, (String, i64)>(
                "SELECT state, message_count FROM sessions WHERE session_id = $1 FOR UPDATE",
            )
            .bind(&session_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_sqlx_error)?
            .ok_or_else(|| DatabaseError::NotFound(format!("session not found: {session_id}")))?;
            if !session_state.eq_ignore_ascii_case("active") {
                for message in &messages {
                    let existing = sqlx::query(
                        "SELECT message_id, session_id, role, content, created_at, metadata_json
                         FROM messages WHERE message_id = $1",
                    )
                    .bind(&message.message_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(map_sqlx_error)?
                    .ok_or_else(|| {
                        DatabaseError::ConstraintViolation(format!(
                            "session {session_id} is not active"
                        ))
                    })?;
                    let existing = map_message_row(&existing)?;
                    crate::message_identity::ensure_message_retry_matches(&existing, message)?;
                }
                for event in &events {
                    postgres_validate_event_retry_if_present(&mut *tx, event).await?;
                }
                tx.commit().await.map_err(map_sqlx_error)?;
                return Ok(current_count);
            }

            let mut inserted_count = 0_i64;
            for message in &messages {
                let result = sqlx::query(
                    "INSERT INTO messages (
                        message_id, session_id, role, content, created_at, metadata_json
                    ) VALUES ($1, $2, $3, $4, $5, $6)
                    ON CONFLICT (message_id) DO NOTHING",
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
                if result.rows_affected() > 0 {
                    inserted_count = inserted_count.checked_add(1).ok_or_else(|| {
                        DatabaseError::ConstraintViolation("message turn size overflow".to_string())
                    })?;
                } else {
                    let existing = sqlx::query(
                        "SELECT message_id, session_id, role, content, created_at, metadata_json
                         FROM messages WHERE message_id = $1",
                    )
                    .bind(&message.message_id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(map_sqlx_error)?;
                    let existing = map_message_row(&existing)?;
                    crate::message_identity::ensure_message_retry_matches(&existing, message)?;
                }
            }
            let turn_size = i64::try_from(messages.len()).map_err(|_| {
                DatabaseError::ConstraintViolation("message turn size overflow".to_string())
            })?;
            if inserted_count > 0 && inserted_count != turn_size {
                return Err(DatabaseError::ConstraintViolation(
                    "a completed message turn cannot be partially replayed".to_string(),
                ));
            }

            let count = if inserted_count > 0 {
                let updated_at = crate::types::runtime_now_timestamp();
                let next_count = current_count.checked_add(inserted_count).ok_or_else(|| {
                    DatabaseError::ConstraintViolation("session message count overflow".to_string())
                })?;
                let count = sqlx::query_scalar::<_, i64>(
                    "UPDATE sessions
                     SET message_count = $2, updated_at = $3
                     WHERE session_id = $1 RETURNING message_count",
                )
                .bind(&session_id)
                .bind(next_count)
                .bind(&updated_at)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_sqlx_error)?;
                for event in &events {
                    postgres_save_event_idempotent(&mut *tx, event).await?;
                }
                count
            } else {
                for event in &events {
                    postgres_validate_event_retry_if_present(&mut *tx, event).await?;
                }
                current_count
            };

            tx.commit().await.map_err(map_sqlx_error)?;
            Ok(count)
        })
    }

    fn delete_messages_and_reset_count(
        &self,
        session_id: &str,
        updated_at: &str,
    ) -> DatabaseResult<()> {
        let pool = self.pool.pool().clone();
        let session_id = session_id.to_owned();
        let updated_at = updated_at.to_owned();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            // Lock the session row first so a concurrent append (which takes
            // the same row lock and increments message_count) cannot commit
            // between the DELETE and the count reset; otherwise the session
            // would carry messages with message_count = 0 forever.
            sqlx::query("SELECT 1 FROM sessions WHERE session_id = $1 FOR UPDATE")
                .bind(&session_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_sqlx_error)?
                .ok_or_else(|| {
                    DatabaseError::NotFound(format!("session not found: {session_id}"))
                })?;
            sqlx::query("DELETE FROM messages WHERE session_id = $1")
                .bind(&session_id)
                .execute(&mut *tx)
                .await?;
            let result = sqlx::query(
                "UPDATE sessions SET message_count = 0, updated_at = $2 WHERE session_id = $1",
            )
            .bind(&session_id)
            .bind(&updated_at)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                return Err(DatabaseError::NotFound(format!(
                    "session not found: {session_id}"
                )));
            }
            tx.commit().await?;
            Ok(())
        })
    }

    fn delete_messages_and_reset_count_with_event(
        &self,
        session_id: &str,
        updated_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<()> {
        crate::event_identity::ensure_event_session(event, session_id, "message deletion")?;
        let pool = self.pool.pool().clone();
        let session_id = session_id.to_owned();
        let updated_at = updated_at.to_owned();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            // Lock the session row first so a concurrent append (which takes
            // the same row lock and increments message_count) cannot commit
            // between the DELETE and the count reset; otherwise the session
            // would carry messages with message_count = 0 forever.
            sqlx::query("SELECT 1 FROM sessions WHERE session_id = $1 FOR UPDATE")
                .bind(&session_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_sqlx_error)?
                .ok_or_else(|| {
                    DatabaseError::NotFound(format!("session not found: {session_id}"))
                })?;
            sqlx::query("DELETE FROM messages WHERE session_id = $1")
                .bind(&session_id)
                .execute(&mut *tx)
                .await?;
            let result = sqlx::query(
                "UPDATE sessions SET message_count = 0, updated_at = $2 WHERE session_id = $1",
            )
            .bind(&session_id)
            .bind(&updated_at)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                return Err(DatabaseError::NotFound(format!(
                    "session not found: {session_id}"
                )));
            }
            postgres_save_event_idempotent(&mut *tx, &event).await?;
            tx.commit().await?;
            Ok(())
        })
    }

    fn save_task_with_event(&self, task: &TaskRow, event: &EventRow) -> DatabaseResult<()> {
        crate::event_identity::ensure_event_session(event, &task.session_id, "task write")?;
        let pool = self.pool.pool().clone();
        let task = task.clone();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let session_state = sqlx::query_scalar::<_, String>(
                "SELECT state FROM sessions WHERE session_id = $1 FOR UPDATE",
            )
            .bind(&task.session_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                DatabaseError::NotFound(format!("session not found: {}", task.session_id))
            })?;
            if crate::types::session_state_is_terminal(&session_state) {
                return Err(DatabaseError::ConstraintViolation(format!(
                    "cannot create or update task {} for terminal session {}",
                    task.task_id, task.session_id
                )));
            }
            let result = sqlx::query(
                "INSERT INTO tasks (
                    task_id, session_id, instruction, state, created_at, updated_at
                ) VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (task_id) DO UPDATE SET
                    session_id = EXCLUDED.session_id,
                    instruction = EXCLUDED.instruction,
                    state = EXCLUDED.state,
                    created_at = EXCLUDED.created_at,
                    updated_at = EXCLUDED.updated_at
                WHERE tasks.session_id = EXCLUDED.session_id
                  AND (
                    LOWER(tasks.state) NOT IN ('completed', 'failed', 'cancelled', 'canceled')
                    OR LOWER(tasks.state) = LOWER(EXCLUDED.state)
                  )",
            )
            .bind(&task.task_id)
            .bind(&task.session_id)
            .bind(&task.instruction)
            .bind(&task.state)
            .bind(&task.created_at)
            .bind(&task.updated_at)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                return Err(DatabaseError::ConstraintViolation(format!(
                    "task {} update conflicts with session ownership or terminal lifecycle",
                    task.task_id
                )));
            }
            postgres_save_event_idempotent(&mut *tx, &event).await?;
            tx.commit().await?;
            Ok(())
        })
    }

    fn cancel_task_with_event(
        &self,
        task_id: &str,
        updated_at: &str,
        event: &EventRow,
    ) -> DatabaseResult<(TaskRow, bool)> {
        let pool = self.pool.pool().clone();
        let task_id = task_id.to_owned();
        let updated_at = updated_at.to_owned();
        let event = event.clone();
        self.pool.run_db(async move {
            let mut tx = pool.begin().await.map_err(map_sqlx_error)?;
            let row = sqlx::query(
                "SELECT task_id, session_id, instruction, state, created_at, updated_at
                 FROM tasks WHERE task_id = $1 FOR UPDATE",
            )
            .bind(&task_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| DatabaseError::NotFound(format!("task not found: {task_id}")))?;
            let mut task = map_task_row(&row)?;
            crate::event_identity::ensure_event_session(
                &event,
                &task.session_id,
                "task cancellation",
            )?;
            postgres_validate_event_retry_if_present(&mut *tx, &event).await?;
            let normalized_state = task.state.to_ascii_lowercase();
            if matches!(normalized_state.as_str(), "cancelled" | "canceled") {
                tx.commit().await?;
                return Ok((task, false));
            }
            if !matches!(normalized_state.as_str(), "created" | "pending" | "running") {
                return Err(DatabaseError::ConstraintViolation(format!(
                    "task {task_id} is not active"
                )));
            }
            let result = sqlx::query(
                "UPDATE tasks SET state = 'cancelled', updated_at = $2
                 WHERE task_id = $1 AND LOWER(state) IN ('created', 'pending', 'running')",
            )
            .bind(&task_id)
            .bind(&updated_at)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() != 1 {
                return Err(DatabaseError::ConstraintViolation(format!(
                    "task {task_id} state changed concurrently"
                )));
            }
            postgres_save_event_idempotent(&mut *tx, &event).await?;
            tx.commit().await?;
            task.state = "cancelled".to_string();
            task.updated_at = Some(updated_at);
            Ok((task, true))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session(id: &str) -> SessionRow {
        SessionRow {
            session_id: id.to_string(),
            agent_id: "agent.1".to_string(),
            kind: "main".to_string(),
            source: "api".to_string(),
            state: "active".to_string(),
            title: Some("Test".to_string()),
            model: Some("gpt-4".to_string()),
            cwd: None,
            provider_id: Some("codex".to_string()),
            bridge_id: Some("bridge.codex".to_string()),
            token_usage_json: None,
            message_count: 0,
            owner_tenant_id: None,
            owner_user_ref: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: Some("2026-01-02T00:00:00Z".to_string()),
            metadata_json: None,
        }
    }

    fn open_postgres_or_skip() -> Option<PostgresDatabase> {
        if !sdkwork_database_config::workspace_database::workspace_postgres_env_is_configured() {
            return None;
        }
        let config = sdkwork_database_config::DatabaseConfig::from_env("agent_runtime").ok()?;
        if config.engine != sdkwork_database_config::DatabaseEngine::Postgres {
            return None;
        }
        let schema =
            sdkwork_database_config::workspace_database::resolve_workspace_postgres_schema()
                .ok()?;
        if schema != "sdkwork_ai_test" && !schema.starts_with("sdkwork_ai_test_") {
            return None;
        }
        PostgresDatabase::connect_migrated(&config.url).ok()
    }

    #[test]
    fn postgres_session_repository_roundtrip_when_uri_configured() {
        let Some(db) = open_postgres_or_skip() else {
            return;
        };
        db.save_session(&sample_session("session.pg.1"))
            .expect("saved");
        let loaded = db
            .load_session("session.pg.1")
            .expect("loaded")
            .expect("found");
        assert_eq!(loaded.agent_id, "agent.1");
        assert_eq!(loaded.provider_id.as_deref(), Some("codex"));
        let _ = db.delete_session("session.pg.1");
    }
}
