//! Agent-runtime schema initialization authority.
//!
//! Initialization state: the authoritative PostgreSQL DDL is a single
//! idempotent baseline (`agent_runtime.postgres.sql`, v2-v5 evolution layers
//! folded in); no versioned migrations exist for PostgreSQL. The SQLite
//! client-local store keeps its own versioned migration path.

use crate::error::{DatabaseError, DatabaseResult};
#[cfg(feature = "postgres-sync")]
use crate::types::CURRENT_SCHEMA_VERSION;
use sdkwork_utils_rust::crypto::sha256_hash;

#[cfg(feature = "sqlite")]
pub const SQLITE_MIGRATION_SQL: &str = include_str!("../migrations/agent_runtime.sqlite.sql");
#[cfg(feature = "sqlite")]
const SQLITE_PAGINATION_MIGRATION_SQL: &str =
    include_str!("../migrations/agent_runtime.sqlite.v3.sql");
#[cfg(feature = "sqlite")]
const SQLITE_RETENTION_MIGRATION_SQL: &str =
    include_str!("../migrations/agent_runtime.sqlite.v4.sql");
#[cfg(feature = "sqlite")]
const SQLITE_EXECUTION_MIGRATION_SQL: &str =
    include_str!("../migrations/agent_runtime.sqlite.v5.sql");

#[cfg(any(feature = "postgres-sync", test))]
pub const POSTGRES_MIGRATION_SQL: &str = include_str!("../migrations/agent_runtime.postgres.sql");

#[cfg(feature = "sqlite")]
const SQLITE_LEGACY_REPAIR_CHECKSUM_SOURCE: &str =
    "agent-runtime-sqlite-v2:columns+orphan-recovery+foreign-key-table-rebuild:1";
#[cfg(feature = "sqlite")]
const SQLITE_HISTORY_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS agent_runtime_schema_migration_history (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    checksum TEXT NOT NULL,
    applied_at TEXT NOT NULL
);
"#;

#[cfg(feature = "postgres-sync")]
const POSTGRES_MIGRATION_LOCK_KEY: i64 = 0x5344_4B57_4B45_524E;

#[derive(Clone, Copy)]
struct SqlMigration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

#[cfg(feature = "sqlite")]
const SQLITE_SQL_MIGRATIONS: &[SqlMigration] = &[
    SqlMigration {
        version: 1,
        name: "create_runtime_schema",
        sql: SQLITE_MIGRATION_SQL,
    },
    SqlMigration {
        version: 3,
        name: "add_stable_pagination_indexes",
        sql: SQLITE_PAGINATION_MIGRATION_SQL,
    },
    SqlMigration {
        version: 4,
        name: "add_runtime_retention_indexes",
        sql: SQLITE_RETENTION_MIGRATION_SQL,
    },
    SqlMigration {
        version: 5,
        name: "add_durable_runtime_execution",
        sql: SQLITE_EXECUTION_MIGRATION_SQL,
    },
];

/// Apply all SQLite migrations inside one immediate transaction.
///
/// `BEGIN IMMEDIATE` serializes concurrent process startup before migration
/// history is inspected, so duplicate startup cannot race DDL or history rows.
#[cfg(feature = "sqlite")]
pub fn apply_sqlite_connection(conn: &rusqlite::Connection) -> DatabaseResult<()> {
    use rusqlite::{Transaction, TransactionBehavior};

    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(|error| {
        DatabaseError::Transaction(format!("failed to begin SQLite migration: {error}"))
    })?;
    tx.execute_batch(SQLITE_HISTORY_TABLE_SQL)
        .map_err(|error| migration_error("create SQLite migration history", error))?;

    apply_sqlite_sql_migration(&tx, SQLITE_SQL_MIGRATIONS[0])?;
    apply_sqlite_legacy_repair(&tx)?;
    apply_sqlite_sql_migration(&tx, SQLITE_SQL_MIGRATIONS[1])?;
    apply_sqlite_sql_migration(&tx, SQLITE_SQL_MIGRATIONS[2])?;
    apply_sqlite_sql_migration(&tx, SQLITE_SQL_MIGRATIONS[3])?;
    validate_sqlite_schema(&tx)?;

    tx.commit().map_err(|error| {
        DatabaseError::Transaction(format!("failed to commit SQLite migration: {error}"))
    })
}

#[cfg(feature = "sqlite")]
fn apply_sqlite_sql_migration(
    tx: &rusqlite::Transaction<'_>,
    migration: SqlMigration,
) -> DatabaseResult<()> {
    let checksum = migration_checksum(migration.sql);
    if sqlite_migration_is_current(tx, migration.version, &checksum)? {
        return Ok(());
    }

    tx.execute_batch(migration.sql)
        .map_err(|error| migration_error(migration.name, error))?;
    record_sqlite_migration(tx, migration.version, migration.name, &checksum)
}

#[cfg(feature = "sqlite")]
fn apply_sqlite_legacy_repair(tx: &rusqlite::Transaction<'_>) -> DatabaseResult<()> {
    const VERSION: i64 = 2;
    const NAME: &str = "repair_legacy_runtime_schema";

    let checksum = migration_checksum(SQLITE_LEGACY_REPAIR_CHECKSUM_SOURCE);
    if sqlite_migration_is_current(tx, VERSION, &checksum)? {
        return Ok(());
    }

    for (table, column, definition) in [
        ("sessions", "provider_id", "TEXT"),
        ("sessions", "bridge_id", "TEXT"),
        ("sessions", "owner_tenant_id", "TEXT"),
        ("sessions", "owner_user_ref", "TEXT"),
        ("messages", "metadata_json", "TEXT"),
        ("tasks", "updated_at", "TEXT"),
        ("events", "payload", "TEXT"),
        ("permissions", "owner_tenant_id", "TEXT"),
        ("permissions", "owner_user_ref", "TEXT"),
        ("permissions", "updated_at", "TEXT"),
    ] {
        ensure_sqlite_column(tx, table, column, definition)?;
    }

    recover_sqlite_orphan_sessions(tx)?;
    rebuild_sqlite_child_table_if_needed(tx, "messages", SQLITE_MESSAGES_TABLE_SQL)?;
    rebuild_sqlite_child_table_if_needed(tx, "tasks", SQLITE_TASKS_TABLE_SQL)?;
    rebuild_sqlite_child_table_if_needed(tx, "events", SQLITE_EVENTS_TABLE_SQL)?;
    rebuild_sqlite_child_table_if_needed(tx, "permissions", SQLITE_PERMISSIONS_TABLE_SQL)?;

    record_sqlite_migration(tx, VERSION, NAME, &checksum)
}

#[cfg(feature = "sqlite")]
fn sqlite_migration_is_current(
    tx: &rusqlite::Transaction<'_>,
    version: i64,
    expected_checksum: &str,
) -> DatabaseResult<bool> {
    use rusqlite::OptionalExtension;

    let applied = tx
        .query_row(
            "SELECT checksum FROM agent_runtime_schema_migration_history WHERE version = ?1",
            [version],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| migration_error("read SQLite migration history", error))?;

    match applied {
        Some(checksum) if checksum == expected_checksum => Ok(true),
        Some(checksum) => Err(DatabaseError::Migration(format!(
            "SQLite migration {version} checksum mismatch: applied={checksum}, expected={expected_checksum}"
        ))),
        None => Ok(false),
    }
}

#[cfg(feature = "sqlite")]
fn record_sqlite_migration(
    tx: &rusqlite::Transaction<'_>,
    version: i64,
    name: &str,
    checksum: &str,
) -> DatabaseResult<()> {
    tx.execute(
        "INSERT INTO agent_runtime_schema_migration_history \
         (version, name, checksum, applied_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            version,
            name,
            checksum,
            crate::types::runtime_now_timestamp()
        ],
    )
    .map_err(|error| migration_error("record SQLite migration history", error))?;
    Ok(())
}

#[cfg(feature = "sqlite")]
fn ensure_sqlite_column(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
    definition: &str,
) -> DatabaseResult<()> {
    if sqlite_column_exists(tx, table, column)? {
        return Ok(());
    }
    let sql = format!(
        "ALTER TABLE {} ADD COLUMN {} {}",
        quote_sqlite_identifier(table),
        quote_sqlite_identifier(column),
        definition
    );
    tx.execute(&sql, [])
        .map_err(|error| migration_error("add SQLite legacy column", error))?;
    Ok(())
}

#[cfg(feature = "sqlite")]
fn sqlite_column_exists(
    conn: &rusqlite::Connection,
    table: &str,
    column: &str,
) -> DatabaseResult<bool> {
    let sql = format!("PRAGMA table_info({})", quote_sqlite_identifier(table));
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| migration_error("inspect SQLite columns", error))?;
    let mut rows = statement
        .query([])
        .map_err(|error| migration_error("inspect SQLite columns", error))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| migration_error("inspect SQLite columns", error))?
    {
        let existing: String = row
            .get(1)
            .map_err(|error| migration_error("read SQLite column metadata", error))?;
        if existing == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(feature = "sqlite")]
fn recover_sqlite_orphan_sessions(tx: &rusqlite::Transaction<'_>) -> DatabaseResult<()> {
    tx.execute_batch(
        r#"
        INSERT OR IGNORE INTO sessions (
            session_id, agent_id, kind, source, state, message_count,
            created_at, updated_at, metadata_json
        )
        SELECT
            child_rows.session_id,
            'agent.runtime.migration',
            'main',
            'migration',
            'orphaned',
            0,
            MIN(child_rows.created_at),
            MIN(child_rows.created_at),
            '{"migrationReason":"recovered orphan child rows before foreign-key repair"}'
        FROM (
            SELECT session_id, created_at FROM messages
            UNION ALL
            SELECT session_id, created_at FROM tasks
            UNION ALL
            SELECT session_id, created_at FROM events WHERE session_id IS NOT NULL
            UNION ALL
            SELECT session_id, created_at FROM permissions WHERE session_id IS NOT NULL
        ) AS child_rows
        LEFT JOIN sessions ON sessions.session_id = child_rows.session_id
        WHERE sessions.session_id IS NULL
        GROUP BY child_rows.session_id;
        "#,
    )
    .map_err(|error| migration_error("recover SQLite orphan sessions", error))
}

#[cfg(feature = "sqlite")]
fn rebuild_sqlite_child_table_if_needed(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    create_sql: &str,
) -> DatabaseResult<()> {
    if sqlite_has_session_cascade_foreign_key(tx, table)? {
        return Ok(());
    }
    ensure_sqlite_rebuild_is_non_destructive(tx, table)?;

    let legacy_table = format!("__sdkwork_{table}_legacy_rebuild");
    tx.execute(
        &format!(
            "ALTER TABLE {} RENAME TO {}",
            quote_sqlite_identifier(table),
            quote_sqlite_identifier(&legacy_table)
        ),
        [],
    )
    .map_err(|error| migration_error("rename SQLite legacy child table", error))?;
    tx.execute_batch(create_sql)
        .map_err(|error| migration_error("create repaired SQLite child table", error))?;
    tx.execute(&sqlite_child_copy_sql(table, &legacy_table), [])
        .map_err(|error| migration_error("copy SQLite legacy child rows", error))?;
    tx.execute(
        &format!("DROP TABLE {}", quote_sqlite_identifier(&legacy_table)),
        [],
    )
    .map_err(|error| migration_error("remove SQLite rebuilt child table", error))?;
    Ok(())
}

#[cfg(feature = "sqlite")]
fn ensure_sqlite_rebuild_is_non_destructive(
    conn: &rusqlite::Connection,
    table: &str,
) -> DatabaseResult<()> {
    let allowed_columns: &[&str] = match table {
        "messages" => &[
            "message_id",
            "session_id",
            "role",
            "content",
            "created_at",
            "metadata_json",
        ],
        "tasks" => &[
            "task_id",
            "session_id",
            "instruction",
            "state",
            "created_at",
            "updated_at",
        ],
        "events" => &[
            "event_id",
            "session_id",
            "event_type",
            "severity",
            "payload",
            "created_at",
        ],
        "permissions" => &[
            "permission_request_id",
            "session_id",
            "category",
            "resource",
            "side_effect_level",
            "reason",
            "status",
            "owner_tenant_id",
            "owner_user_ref",
            "created_at",
            "updated_at",
        ],
        _ => unreachable!("only canonical runtime child tables are rebuilt"),
    };
    let sql = format!("PRAGMA table_info({})", quote_sqlite_identifier(table));
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| migration_error("inspect SQLite rebuild columns", error))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| migration_error("inspect SQLite rebuild columns", error))?;
    for column in columns {
        let column =
            column.map_err(|error| migration_error("read SQLite rebuild column", error))?;
        if !allowed_columns.contains(&column.as_str()) {
            return Err(DatabaseError::Migration(format!(
                "SQLite migration cannot safely rebuild {table}: unknown column {column} requires an explicit expand/contract migration"
            )));
        }
    }

    let trigger_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND tbl_name = ?1",
            [table],
            |row| row.get(0),
        )
        .map_err(|error| migration_error("inspect SQLite rebuild triggers", error))?;
    if trigger_count > 0 {
        return Err(DatabaseError::Migration(format!(
            "SQLite migration cannot safely rebuild {table}: custom triggers require an explicit expand/contract migration"
        )));
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
fn sqlite_child_copy_sql(table: &str, legacy_table: &str) -> String {
    let columns = match table {
        "messages" => "message_id, session_id, role, content, created_at, metadata_json",
        "tasks" => "task_id, session_id, instruction, state, created_at, updated_at",
        "events" => "event_id, session_id, event_type, severity, payload, created_at",
        "permissions" => {
            "permission_request_id, session_id, category, resource, side_effect_level, reason, \
             status, owner_tenant_id, owner_user_ref, created_at, updated_at"
        }
        _ => unreachable!("only canonical runtime child tables are rebuilt"),
    };
    format!(
        "INSERT INTO {} ({columns}) SELECT {columns} FROM {}",
        quote_sqlite_identifier(table),
        quote_sqlite_identifier(legacy_table)
    )
}

#[cfg(feature = "sqlite")]
fn sqlite_has_session_cascade_foreign_key(
    conn: &rusqlite::Connection,
    table: &str,
) -> DatabaseResult<bool> {
    sqlite_has_foreign_key(
        conn,
        table,
        "session_id",
        "sessions",
        "session_id",
        "CASCADE",
    )
}

#[cfg(feature = "sqlite")]
fn sqlite_has_foreign_key(
    conn: &rusqlite::Connection,
    table: &str,
    source: &str,
    expected_target_table: &str,
    target: &str,
    on_delete_action: &str,
) -> DatabaseResult<bool> {
    let sql = format!(
        "PRAGMA foreign_key_list({})",
        quote_sqlite_identifier(table)
    );
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| migration_error("inspect SQLite foreign keys", error))?;
    let mut rows = statement
        .query([])
        .map_err(|error| migration_error("inspect SQLite foreign keys", error))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| migration_error("inspect SQLite foreign keys", error))?
    {
        let target_table: String = row
            .get(2)
            .map_err(|error| migration_error("read SQLite foreign-key target", error))?;
        let source_column: String = row
            .get(3)
            .map_err(|error| migration_error("read SQLite foreign-key source", error))?;
        let target_column: String = row
            .get(4)
            .map_err(|error| migration_error("read SQLite foreign-key column", error))?;
        let on_delete: String = row
            .get(6)
            .map_err(|error| migration_error("read SQLite foreign-key action", error))?;
        if target_table == expected_target_table
            && source_column == source
            && target_column == target
            && on_delete.eq_ignore_ascii_case(on_delete_action)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(feature = "sqlite")]
pub(crate) fn validate_sqlite_schema(conn: &rusqlite::Connection) -> DatabaseResult<()> {
    for column in [
        "provider_id",
        "bridge_id",
        "owner_tenant_id",
        "owner_user_ref",
    ] {
        if !sqlite_column_exists(conn, "sessions", column)? {
            return Err(DatabaseError::Migration(format!(
                "SQLite schema drift: sessions.{column} is missing"
            )));
        }
    }
    for table in ["messages", "tasks", "events", "permissions"] {
        if !sqlite_has_session_cascade_foreign_key(conn, table)? {
            return Err(DatabaseError::Migration(format!(
                "SQLite schema drift: {table}.session_id cascade foreign key is missing"
            )));
        }
    }
    for (table, required_columns) in [
        (
            "runs",
            &["run_id", "task_id", "session_id", "state", "fencing_token"][..],
        ),
        ("steps", &["step_id", "run_id", "action_kind", "state"][..]),
        (
            "permission_operations",
            &[
                "permission_request_id",
                "run_id",
                "step_id",
                "payload_kind",
                "payload_ref",
                "fencing_token",
            ][..],
        ),
    ] {
        for column in required_columns {
            if !sqlite_column_exists(conn, table, column)? {
                return Err(DatabaseError::Migration(format!(
                    "SQLite schema drift: {table}.{column} is missing"
                )));
            }
        }
    }
    for (table, source, target_table, target) in [
        ("runs", "task_id", "tasks", "task_id"),
        ("runs", "session_id", "sessions", "session_id"),
        ("steps", "run_id", "runs", "run_id"),
        (
            "permission_operations",
            "permission_request_id",
            "permissions",
            "permission_request_id",
        ),
        ("permission_operations", "run_id", "runs", "run_id"),
        ("permission_operations", "step_id", "steps", "step_id"),
    ] {
        if !sqlite_has_foreign_key(conn, table, source, target_table, target, "CASCADE")? {
            return Err(DatabaseError::Migration(format!(
                "SQLite schema drift: {table}.{source} cascade foreign key is missing"
            )));
        }
    }
    for index in [
        "idx_messages_session_created_at_message_id",
        "idx_tasks_session_created_at_task_id",
        "idx_events_session_created_at_event_id",
        "idx_events_created_at_event_id",
        "idx_sessions_retention_state_time_id",
        "idx_messages_retention_created_id",
        "idx_tasks_retention_state_time_id",
        "idx_permissions_retention_time_id",
        "idx_runs_claim",
        "idx_runs_task_created",
        "idx_runs_session_created",
        "idx_runs_lease_expiry",
        "idx_runs_retention_state_time_id",
        "idx_steps_run_sequence",
        "idx_steps_retention_state_time_id",
        "idx_permission_operations_claim",
        "idx_permission_operations_run_step",
        "idx_permission_operations_retention_state_time_id",
    ] {
        let present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                [index],
                |row| row.get(0),
            )
            .map_err(|error| migration_error("validate SQLite index", error))?;
        if present != 1 {
            return Err(DatabaseError::Migration(format!(
                "SQLite schema drift: index {index} is missing"
            )));
        }
    }
    let mut foreign_key_check = conn
        .prepare("PRAGMA foreign_key_check")
        .map_err(|error| migration_error("prepare SQLite foreign-key check", error))?;
    let mut violations = foreign_key_check
        .query([])
        .map_err(|error| migration_error("run SQLite foreign-key check", error))?;
    if violations
        .next()
        .map_err(|error| migration_error("read SQLite foreign-key check", error))?
        .is_some()
    {
        return Err(DatabaseError::Migration(
            "SQLite schema migration left foreign-key violations".to_string(),
        ));
    }
    Ok(())
}

#[cfg(feature = "postgres-sync")]
const POSTGRES_HISTORY_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS agent_runtime_schema_migration_history (
    version BIGINT PRIMARY KEY,
    name TEXT NOT NULL,
    checksum TEXT NOT NULL,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
"#;

/// The PostgreSQL store has no versioned migrations: the authoritative DDL is
/// one idempotent baseline recorded as a single history row at
/// `CURRENT_SCHEMA_VERSION`. `schema_status` treats that one row — not a row
/// per migration version — as the applied-state invariant for this backend.
#[cfg(feature = "postgres-sync")]
const POSTGRES_BASELINE_MIGRATION_NAME: &str = "postgres_runtime_baseline";

#[cfg(feature = "postgres-sync")]
pub async fn apply_postgres_pool(pool: &sqlx::PgPool) -> DatabaseResult<()> {
    let mut tx = pool.begin().await.map_err(postgres_migration_error)?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(POSTGRES_MIGRATION_LOCK_KEY)
        .execute(&mut *tx)
        .await
        .map_err(postgres_migration_error)?;

    // Initialization state: the full authoritative DDL is one idempotent
    // baseline; no versioned migrations exist for PostgreSQL.
    sqlx::raw_sql(POSTGRES_HISTORY_TABLE_SQL)
        .execute(&mut *tx)
        .await
        .map_err(|error| postgres_migration_error(error))?;
    sqlx::raw_sql(POSTGRES_MIGRATION_SQL)
        .execute(&mut *tx)
        .await
        .map_err(postgres_migration_error)?;
    record_postgres_baseline(&mut tx).await?;

    validate_postgres_schema(&mut tx).await?;
    tx.commit().await.map_err(postgres_migration_error)
}

#[cfg(feature = "postgres-sync")]
async fn record_postgres_baseline(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> DatabaseResult<()> {
    let checksum = migration_checksum(POSTGRES_MIGRATION_SQL);
    sqlx::query(
        "INSERT INTO agent_runtime_schema_migration_history (version, name, checksum) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (version) DO NOTHING",
    )
    .bind(CURRENT_SCHEMA_VERSION)
    .bind(POSTGRES_BASELINE_MIGRATION_NAME)
    .bind(checksum)
    .execute(&mut **tx)
    .await
    .map_err(postgres_migration_error)?;
    Ok(())
}

#[cfg(feature = "postgres-sync")]
pub(crate) async fn validate_postgres_schema(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> DatabaseResult<()> {
    for table in ["runs", "steps", "permission_operations"] {
        let present = sqlx::query_scalar::<_, bool>("SELECT to_regclass($1) IS NOT NULL")
            .bind(table)
            .fetch_one(&mut **tx)
            .await
            .map_err(postgres_migration_error)?;
        if !present {
            return Err(DatabaseError::Migration(format!(
                "PostgreSQL schema drift: table {table} is missing"
            )));
        }
    }
    for index in [
        "idx_messages_session_created_at_message_id",
        "idx_tasks_session_created_at_task_id",
        "idx_events_session_created_at_event_id",
        "idx_events_created_at_event_id",
        "idx_sessions_retention_state_time_id",
        "idx_messages_retention_created_id",
        "idx_tasks_retention_state_time_id",
        "idx_permissions_retention_time_id",
        "idx_runs_claim",
        "idx_runs_task_created",
        "idx_runs_session_created",
        "idx_runs_lease_expiry",
        "idx_runs_retention_state_time_id",
        "idx_steps_run_sequence",
        "idx_steps_retention_state_time_id",
        "idx_permission_operations_claim",
        "idx_permission_operations_run_step",
        "idx_permission_operations_retention_state_time_id",
    ] {
        let present = sqlx::query_scalar::<_, bool>("SELECT to_regclass($1) IS NOT NULL")
            .bind(index)
            .fetch_one(&mut **tx)
            .await
            .map_err(postgres_migration_error)?;
        if !present {
            return Err(DatabaseError::Migration(format!(
                "PostgreSQL schema drift: index {index} is missing"
            )));
        }
    }
    let cascade_foreign_keys = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) \
         FROM pg_constraint \
         WHERE conrelid IN ('messages'::regclass, 'tasks'::regclass, \
                            'events'::regclass, 'permissions'::regclass) \
           AND contype = 'f' \
           AND confrelid = 'sessions'::regclass \
           AND confdeltype = 'c'",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(postgres_migration_error)?;
    if cascade_foreign_keys < 4 {
        return Err(DatabaseError::Migration(
            "PostgreSQL schema drift: runtime child cascade foreign keys are incomplete".into(),
        ));
    }
    let execution_cascade_foreign_keys = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)
         FROM pg_constraint
         WHERE conrelid IN ('runs'::regclass, 'steps'::regclass,
                            'permission_operations'::regclass)
           AND contype = 'f'
           AND confdeltype = 'c'",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(postgres_migration_error)?;
    if execution_cascade_foreign_keys < 6 {
        return Err(DatabaseError::Migration(
            "PostgreSQL schema drift: execution cascade foreign keys are incomplete".into(),
        ));
    }
    Ok(())
}

fn migration_checksum(source: &str) -> String {
    sha256_hash(source.as_bytes())
}

#[cfg(feature = "sqlite")]
fn quote_sqlite_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

#[cfg(feature = "sqlite")]
fn migration_error(context: &str, error: rusqlite::Error) -> DatabaseError {
    DatabaseError::Migration(format!("{context}: {error}"))
}

#[cfg(feature = "postgres-sync")]
fn postgres_migration_error(error: sqlx::Error) -> DatabaseError {
    DatabaseError::Migration(error.to_string())
}

#[cfg(feature = "sqlite")]
const SQLITE_MESSAGES_TABLE_SQL: &str = r#"
CREATE TABLE messages (
    message_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    metadata_json TEXT,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);
"#;

#[cfg(feature = "sqlite")]
const SQLITE_TASKS_TABLE_SQL: &str = r#"
CREATE TABLE tasks (
    task_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    instruction TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'created',
    created_at TEXT NOT NULL,
    updated_at TEXT,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);
"#;

#[cfg(feature = "sqlite")]
const SQLITE_EVENTS_TABLE_SQL: &str = r#"
CREATE TABLE events (
    event_id TEXT PRIMARY KEY,
    session_id TEXT,
    event_type TEXT NOT NULL,
    severity TEXT NOT NULL,
    payload TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);
"#;

#[cfg(feature = "sqlite")]
const SQLITE_PERMISSIONS_TABLE_SQL: &str = r#"
CREATE TABLE permissions (
    permission_request_id TEXT PRIMARY KEY,
    session_id TEXT,
    category TEXT NOT NULL,
    resource TEXT NOT NULL,
    side_effect_level TEXT NOT NULL,
    reason TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    owner_tenant_id TEXT,
    owner_user_ref TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_sql_is_non_destructive_and_contains_required_schema() {
        for sql in [POSTGRES_MIGRATION_SQL] {
            assert!(sql.contains("CREATE TABLE IF NOT EXISTS sessions"));
            assert!(sql.contains("ON DELETE CASCADE"));
            assert!(!sql.to_ascii_uppercase().contains("DROP TABLE"));
        }
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_migration_sql_is_non_destructive_and_contains_required_schema() {
        for sql in [SQLITE_MIGRATION_SQL] {
            assert!(sql.contains("CREATE TABLE IF NOT EXISTS sessions"));
            assert!(sql.contains("ON DELETE CASCADE"));
            assert!(!sql.to_ascii_uppercase().contains("DROP TABLE"));
        }
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn sqlite_migration_is_versioned_and_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().expect("memory db");
        conn.execute_batch("PRAGMA foreign_keys=ON")
            .expect("foreign keys");

        apply_sqlite_connection(&conn).expect("first migration");
        apply_sqlite_connection(&conn).expect("repeat migration");

        let applied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_runtime_schema_migration_history",
                [],
                |row| row.get(0),
            )
            .expect("history count");
        assert_eq!(applied, 5);
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn sqlite_migration_repairs_legacy_columns_foreign_keys_and_orphans() {
        let conn = rusqlite::Connection::open_in_memory().expect("memory db");
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys=ON;
            CREATE TABLE sessions (
                session_id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                kind TEXT NOT NULL DEFAULT 'main',
                source TEXT NOT NULL DEFAULT 'api',
                state TEXT NOT NULL DEFAULT 'created',
                title TEXT,
                model TEXT,
                cwd TEXT,
                token_usage_json TEXT,
                message_count INTEGER DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT,
                metadata_json TEXT
            );
            CREATE TABLE messages (
                message_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE agents (agent_id TEXT PRIMARY KEY, payload TEXT);
            INSERT INTO agents VALUES ('legacy.agent', 'preserve');
            INSERT INTO messages VALUES (
                'msg.orphan', 'session.orphan', 'user', 'preserve me',
                '2026-01-01T00:00:00Z'
            );
            "#,
        )
        .expect("legacy schema");

        apply_sqlite_connection(&conn).expect("legacy migration");

        for column in [
            "provider_id",
            "bridge_id",
            "owner_tenant_id",
            "owner_user_ref",
        ] {
            assert!(
                sqlite_column_exists(&conn, "sessions", column).expect("column probe"),
                "sessions.{column} should be repaired"
            );
        }
        assert!(sqlite_has_session_cascade_foreign_key(&conn, "messages").expect("fk probe"));
        let recovered_state: String = conn
            .query_row(
                "SELECT state FROM sessions WHERE session_id = 'session.orphan'",
                [],
                |row| row.get(0),
            )
            .expect("recovered session");
        assert_eq!(recovered_state, "orphaned");
        let preserved_agents: i64 = conn
            .query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))
            .expect("legacy agents preserved");
        assert_eq!(preserved_agents, 1);
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn sqlite_migration_rejects_checksum_drift() {
        let conn = rusqlite::Connection::open_in_memory().expect("memory db");
        conn.execute_batch("PRAGMA foreign_keys=ON")
            .expect("foreign keys");
        apply_sqlite_connection(&conn).expect("migration");
        conn.execute(
            "UPDATE agent_runtime_schema_migration_history SET checksum = 'drift' WHERE version = 1",
            [],
        )
        .expect("inject drift");

        let error = apply_sqlite_connection(&conn).expect_err("checksum drift must fail");
        assert!(matches!(error, DatabaseError::Migration(_)));
    }
}
