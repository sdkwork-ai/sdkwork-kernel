use sdkwork_agent_database::{
    ClaimedPermissionOperation, ClaimedRun, EventRow, MessageRow, PermissionOperationRepository,
    PermissionOperationRow, PermissionRepository, PermissionRow, PostgresDatabase,
    RunControlAction, RunRow, RuntimeExecutionRepository, RuntimeMaintenance, RuntimePurgeCounts,
    RuntimeSchemaStatus, SessionRow, StepRow, TaskRow,
};
#[cfg(feature = "sqlite")]
use sdkwork_agent_database::SqliteDatabase;

use sdkwork_agent_session::{MessageConfig, SessionConfig, SessionQuery, UnifiedSessionManager};
use std::sync::{Arc, RwLock, Weak};
use std::time::Duration;
use tokio::sync::{Semaphore, TryAcquireError};

use crate::config::ServerConfig;
use crate::event_bus::SessionEventBus;

#[cfg(feature = "sqlite")]
type AppSessionManagerSqlite = UnifiedSessionManager<SqliteDatabase>;

type AppSessionManagerPostgres = UnifiedSessionManager<PostgresDatabase>;

#[derive(Clone)]
enum ManagerInner {
    #[cfg(feature = "sqlite")]
    Sqlite(Arc<AppSessionManagerSqlite>),
    Postgres(Arc<AppSessionManagerPostgres>),
}

/// Database handle for permission persistence operations.
#[derive(Clone)]
enum PermissionDb {
    #[cfg(feature = "sqlite")]
    Sqlite(SqliteDatabase),
    Postgres(Arc<PostgresDatabase>),
}

#[derive(Clone)]
enum MaintenanceDb {
    #[cfg(feature = "sqlite")]
    Sqlite(SqliteDatabase),
    Postgres(Arc<PostgresDatabase>),
}

#[derive(Clone)]
enum ExecutionDb {
    #[cfg(feature = "sqlite")]
    Sqlite(SqliteDatabase),
    Postgres(Arc<PostgresDatabase>),
}

impl RuntimeExecutionRepository for ExecutionDb {
    fn create_task_execution(
        &self,
        task: &TaskRow,
        run: &RunRow,
        step: &StepRow,
        event: &EventRow,
    ) -> Result<(), sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.create_task_execution(task, run, step, event),
            Self::Postgres(db) => db.create_task_execution(task, run, step, event),
        }
    }

    fn load_run(
        &self,
        run_id: &str,
    ) -> Result<Option<RunRow>, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.load_run(run_id),
            Self::Postgres(db) => db.load_run(run_id),
        }
    }

    fn load_steps(
        &self,
        run_id: &str,
    ) -> Result<Vec<StepRow>, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.load_steps(run_id),
            Self::Postgres(db) => db.load_steps(run_id),
        }
    }

    fn next_task_attempt(
        &self,
        task_id: &str,
    ) -> Result<i64, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.next_task_attempt(task_id),
            Self::Postgres(db) => db.next_task_attempt(task_id),
        }
    }

    fn claim_ready_run(
        &self,
        worker_id: &str,
        now: &str,
        lease_expires_at: &str,
    ) -> Result<Option<ClaimedRun>, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.claim_ready_run(worker_id, now, lease_expires_at),
            Self::Postgres(db) => db.claim_ready_run(worker_id, now, lease_expires_at),
        }
    }

    fn renew_run_lease(
        &self,
        run_id: &str,
        worker_id: &str,
        fencing_token: i64,
        now: &str,
        lease_expires_at: &str,
    ) -> Result<bool, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => {
                db.renew_run_lease(run_id, worker_id, fencing_token, now, lease_expires_at)
            }
            Self::Postgres(db) => {
                db.renew_run_lease(run_id, worker_id, fencing_token, now, lease_expires_at)
            }
        }
    }

    fn start_claimed_run(
        &self,
        claim: &ClaimedRun,
        started_at: &str,
        event: &EventRow,
    ) -> Result<(), sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.start_claimed_run(claim, started_at, event),
            Self::Postgres(db) => db.start_claimed_run(claim, started_at, event),
        }
    }

    fn complete_claimed_run(
        &self,
        claim: &ClaimedRun,
        result_json: Option<&str>,
        finished_at: &str,
        event: &EventRow,
    ) -> Result<(), sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.complete_claimed_run(claim, result_json, finished_at, event),
            Self::Postgres(db) => db.complete_claimed_run(claim, result_json, finished_at, event),
        }
    }

    fn complete_claimed_run_with_messages(
        &self,
        claim: &ClaimedRun,
        messages: &[MessageRow],
        result_json: Option<&str>,
        finished_at: &str,
        event: &EventRow,
    ) -> Result<(), sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.complete_claimed_run_with_messages(
                claim,
                messages,
                result_json,
                finished_at,
                event,
            ),
            Self::Postgres(db) => db.complete_claimed_run_with_messages(
                claim,
                messages,
                result_json,
                finished_at,
                event,
            ),
        }
    }

    fn fail_claimed_run(
        &self,
        claim: &ClaimedRun,
        error_kind: &str,
        error_code: Option<&str>,
        error_detail: &str,
        finished_at: &str,
        event: &EventRow,
    ) -> Result<(), sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.fail_claimed_run(
                claim,
                error_kind,
                error_code,
                error_detail,
                finished_at,
                event,
            ),
            Self::Postgres(db) => db.fail_claimed_run(
                claim,
                error_kind,
                error_code,
                error_detail,
                finished_at,
                event,
            ),
        }
    }

    fn schedule_run_retry(
        &self,
        claim: &ClaimedRun,
        error_kind: &str,
        error_code: Option<&str>,
        error_detail: &str,
        next_attempt_at: &str,
        event: &EventRow,
    ) -> Result<(), sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.schedule_run_retry(
                claim,
                error_kind,
                error_code,
                error_detail,
                next_attempt_at,
                event,
            ),
            Self::Postgres(db) => db.schedule_run_retry(
                claim,
                error_kind,
                error_code,
                error_detail,
                next_attempt_at,
                event,
            ),
        }
    }

    fn request_task_cancellation(
        &self,
        task_id: &str,
        requested_at: &str,
        event: &EventRow,
    ) -> Result<(TaskRow, bool), sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.request_task_cancellation(task_id, requested_at, event),
            Self::Postgres(db) => db.request_task_cancellation(task_id, requested_at, event),
        }
    }

    fn retry_task_execution(
        &self,
        task_id: &str,
        run: &RunRow,
        step: &StepRow,
        event: &EventRow,
    ) -> Result<TaskRow, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.retry_task_execution(task_id, run, step, event),
            Self::Postgres(db) => db.retry_task_execution(task_id, run, step, event),
        }
    }

    fn control_run(
        &self,
        run_id: &str,
        action: RunControlAction,
        changed_at: &str,
        event: &EventRow,
    ) -> Result<RunRow, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.control_run(run_id, action, changed_at, event),
            Self::Postgres(db) => db.control_run(run_id, action, changed_at, event),
        }
    }
}

impl PermissionOperationRepository for ExecutionDb {
    fn create_permission_execution(
        &self,
        permission: &PermissionRow,
        task: &TaskRow,
        run: &RunRow,
        step: &StepRow,
        operation: &PermissionOperationRow,
        event: &EventRow,
    ) -> Result<(), sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => {
                db.create_permission_execution(permission, task, run, step, operation, event)
            }
            Self::Postgres(db) => {
                db.create_permission_execution(permission, task, run, step, operation, event)
            }
        }
    }

    fn load_permission_operation(
        &self,
        permission_request_id: &str,
    ) -> Result<Option<PermissionOperationRow>, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.load_permission_operation(permission_request_id),
            Self::Postgres(db) => db.load_permission_operation(permission_request_id),
        }
    }

    fn decide_permission_operation(
        &self,
        permission_request_id: &str,
        decision: &str,
        decided_at: &str,
        event: &EventRow,
    ) -> Result<PermissionOperationRow, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => {
                db.decide_permission_operation(permission_request_id, decision, decided_at, event)
            }
            Self::Postgres(db) => {
                db.decide_permission_operation(permission_request_id, decision, decided_at, event)
            }
        }
    }

    fn claim_permission_operation(
        &self,
        worker_id: &str,
        now: &str,
        lease_expires_at: &str,
    ) -> Result<Option<ClaimedPermissionOperation>, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.claim_permission_operation(worker_id, now, lease_expires_at),
            Self::Postgres(db) => db.claim_permission_operation(worker_id, now, lease_expires_at),
        }
    }

    fn renew_permission_operation_lease(
        &self,
        permission_request_id: &str,
        worker_id: &str,
        fencing_token: i64,
        now: &str,
        lease_expires_at: &str,
    ) -> Result<bool, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.renew_permission_operation_lease(
                permission_request_id,
                worker_id,
                fencing_token,
                now,
                lease_expires_at,
            ),
            Self::Postgres(db) => db.renew_permission_operation_lease(
                permission_request_id,
                worker_id,
                fencing_token,
                now,
                lease_expires_at,
            ),
        }
    }

    fn expire_permission_operations(
        &self,
        now: &str,
        batch_size: i64,
    ) -> Result<Vec<EventRow>, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.expire_permission_operations(now, batch_size),
            Self::Postgres(db) => db.expire_permission_operations(now, batch_size),
        }
    }

    fn complete_permission_operation(
        &self,
        claim: &ClaimedPermissionOperation,
        result_json: &str,
        finished_at: &str,
        event: &EventRow,
    ) -> Result<(), sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => {
                db.complete_permission_operation(claim, result_json, finished_at, event)
            }
            Self::Postgres(db) => {
                db.complete_permission_operation(claim, result_json, finished_at, event)
            }
        }
    }

    fn fail_permission_operation(
        &self,
        claim: &ClaimedPermissionOperation,
        error_kind: &str,
        error_code: Option<&str>,
        error_detail: &str,
        finished_at: &str,
        event: &EventRow,
    ) -> Result<(), sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.fail_permission_operation(
                claim,
                error_kind,
                error_code,
                error_detail,
                finished_at,
                event,
            ),
            Self::Postgres(db) => db.fail_permission_operation(
                claim,
                error_kind,
                error_code,
                error_detail,
                finished_at,
                event,
            ),
        }
    }
}

impl RuntimeMaintenance for MaintenanceDb {
    fn purge_expired(
        &self,
        cutoff: &str,
        batch_size: i64,
    ) -> Result<RuntimePurgeCounts, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            MaintenanceDb::Sqlite(db) => db.purge_expired(cutoff, batch_size),
            MaintenanceDb::Postgres(db) => db.purge_expired(cutoff, batch_size),
        }
    }

    fn schema_status(&self) -> Result<RuntimeSchemaStatus, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            MaintenanceDb::Sqlite(db) => db.schema_status(),
            MaintenanceDb::Postgres(db) => db.schema_status(),
        }
    }

    fn run_maintenance(&self) -> Result<(), sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            MaintenanceDb::Sqlite(db) => db.run_maintenance(),
            MaintenanceDb::Postgres(db) => db.run_maintenance(),
        }
    }
}

impl PermissionRepository for PermissionDb {
    fn create_permission_if_absent(
        &self,
        permission: &PermissionRow,
    ) -> Result<bool, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            PermissionDb::Sqlite(db) => db.create_permission_if_absent(permission),
            PermissionDb::Postgres(db) => db.create_permission_if_absent(permission),
        }
    }

    fn save_permission(
        &self,
        permission: &PermissionRow,
    ) -> Result<(), sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            PermissionDb::Sqlite(db) => db.save_permission(permission),
            PermissionDb::Postgres(db) => db.save_permission(permission),
        }
    }

    fn load_permission(
        &self,
        permission_request_id: &str,
    ) -> Result<Option<PermissionRow>, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            PermissionDb::Sqlite(db) => db.load_permission(permission_request_id),
            PermissionDb::Postgres(db) => db.load_permission(permission_request_id),
        }
    }

    fn list_permissions(
        &self,
        query: &sdkwork_agent_database::PermissionQuery,
    ) -> Result<Vec<PermissionRow>, sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            PermissionDb::Sqlite(db) => db.list_permissions(query),
            PermissionDb::Postgres(db) => db.list_permissions(query),
        }
    }

    fn update_permission_status(
        &self,
        permission_request_id: &str,
        status: &str,
    ) -> Result<(), sdkwork_agent_database::DatabaseError> {
        match self {
            #[cfg(feature = "sqlite")]
            PermissionDb::Sqlite(db) => db.update_permission_status(permission_request_id, status),
            PermissionDb::Postgres(db) => {
                db.update_permission_status(permission_request_id, status)
            }
        }
    }
}

macro_rules! with_manager {
    ($self:expr, |$mgr:ident| $body:expr) => {{
        match &$self.manager {
            #[cfg(feature = "sqlite")]
            ManagerInner::Sqlite(inner) => {
                let $mgr = &**inner;
                $body
            }
            ManagerInner::Postgres(inner) => {
                let $mgr = &**inner;
                $body
            }
        }
    }};
}

/// Shared session persistence for PostgreSQL-backed server handlers.
/// SQLite variants exist only for isolated in-memory test fixtures.
#[derive(Clone)]
pub struct PersistenceState {
    manager: ManagerInner,
    permission_db: PermissionDb,
    maintenance_db: MaintenanceDb,
    execution_db: ExecutionDb,
    event_bus: SessionEventBus,
    event_fanout: crate::event_fanout::EventFanout,
    backend: PersistenceBackend,
    admission: Arc<PersistenceAdmission>,
}

#[derive(Debug)]
struct PersistenceAdmission {
    active: Arc<Semaphore>,
    waiting: Arc<Semaphore>,
    timeout: Duration,
    metrics: RwLock<Option<Weak<crate::metrics::MetricsRegistry>>>,
}

struct PersistenceAdmissionLease {
    _permit: tokio::sync::OwnedSemaphorePermit,
    _active_guard: Option<crate::metrics::PersistenceAdmissionActiveGuard>,
}

impl PersistenceAdmission {
    fn from_config(config: &ServerConfig) -> Self {
        Self {
            active: Arc::new(Semaphore::new(config.persistence_max_concurrency)),
            waiting: Arc::new(Semaphore::new(config.persistence_max_waiters)),
            timeout: Duration::from_millis(config.persistence_admission_timeout_ms),
            metrics: RwLock::new(None),
        }
    }

    fn attach_metrics(&self, metrics: &Arc<crate::metrics::MetricsRegistry>) {
        let mut attached = self
            .metrics
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *attached = Some(Arc::downgrade(metrics));
    }

    fn metrics(&self) -> Option<Arc<crate::metrics::MetricsRegistry>> {
        self.metrics
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
    }

    async fn acquire(&self) -> Result<PersistenceAdmissionLease, String> {
        use crate::metrics::ProviderAdmissionRejection;

        let metrics = self.metrics();
        let wait_guard = metrics
            .as_ref()
            .map(crate::metrics::MetricsRegistry::begin_persistence_admission_wait);
        match self.active.clone().try_acquire_owned() {
            Ok(permit) => Ok(PersistenceAdmissionLease {
                _permit: permit,
                _active_guard: wait_guard.map(|guard| guard.acquired()),
            }),
            Err(TryAcquireError::Closed) => {
                if let Some(metrics) = &metrics {
                    metrics
                        .record_persistence_admission_rejection(ProviderAdmissionRejection::Closed);
                }
                Err("persistence admission closed".to_string())
            }
            Err(TryAcquireError::NoPermits) => {
                let waiting = self.waiting.clone().try_acquire_owned().map_err(|error| {
                    let (message, reason) = match error {
                        TryAcquireError::Closed => (
                            "persistence admission closed",
                            ProviderAdmissionRejection::Closed,
                        ),
                        TryAcquireError::NoPermits => (
                            "persistence admission queue full",
                            ProviderAdmissionRejection::QueueFull,
                        ),
                    };
                    if let Some(metrics) = &metrics {
                        metrics.record_persistence_admission_rejection(reason);
                    }
                    message.to_string()
                })?;
                let acquired =
                    tokio::time::timeout(self.timeout, self.active.clone().acquire_owned()).await;
                drop(waiting);
                match acquired {
                    Ok(Ok(permit)) => Ok(PersistenceAdmissionLease {
                        _permit: permit,
                        _active_guard: wait_guard.map(|guard| guard.acquired()),
                    }),
                    Ok(Err(_)) => {
                        if let Some(metrics) = &metrics {
                            metrics.record_persistence_admission_rejection(
                                ProviderAdmissionRejection::Closed,
                            );
                        }
                        Err("persistence admission closed".to_string())
                    }
                    Err(_) => {
                        if let Some(metrics) = &metrics {
                            metrics.record_persistence_admission_rejection(
                                ProviderAdmissionRejection::Timeout,
                            );
                        }
                        Err("persistence admission timeout".to_string())
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceBackend {
    #[cfg(feature = "sqlite")]
    Sqlite,
    Postgres,
}

impl PersistenceState {
    pub async fn open_from_config_async(config: &ServerConfig) -> anyhow::Result<Self> {
        if !config.uses_postgres_runtime_database() {
            anyhow::bail!("sdkwork-agent-server authoritative persistence requires PostgreSQL");
        }
        let state = Self::open_postgres_with_event_bus_async(SessionEventBus::new()).await?;
        Ok(state.with_admission_config(config))
    }

    pub fn open_from_config(config: &ServerConfig) -> anyhow::Result<Self> {
        if !config.uses_postgres_runtime_database() {
            anyhow::bail!("sdkwork-agent-server authoritative persistence requires PostgreSQL");
        }
        let state = Self::open_postgres_with_event_bus(SessionEventBus::new())?;
        Ok(state.with_admission_config(config))
    }

    pub fn open_postgres() -> anyhow::Result<Self> {
        Self::open_postgres_with_event_bus(SessionEventBus::new())
    }

    pub async fn open_postgres_with_event_bus_async(
        event_bus: SessionEventBus,
    ) -> anyhow::Result<Self> {
        let db = PostgresDatabase::connect_from_sdkwork_env_async("agent_runtime").await?;
        Ok(Self::from_postgres_database(db, event_bus))
    }

    pub fn open_postgres_with_event_bus(event_bus: SessionEventBus) -> anyhow::Result<Self> {
        let db = PostgresDatabase::connect_from_sdkwork_env("agent_runtime")?;
        Ok(Self::from_postgres_database(db, event_bus))
    }

    #[cfg(feature = "sqlite")]
    pub fn memory() -> anyhow::Result<Self> {
        Self::memory_with_event_bus(SessionEventBus::new())
    }

    #[cfg(feature = "sqlite")]
    pub fn memory_with_event_bus(event_bus: SessionEventBus) -> anyhow::Result<Self> {
        let db = SqliteDatabase::memory_migrated()?;
        Ok(Self::from_sqlite_database(db, event_bus))
    }

    pub fn backend(&self) -> PersistenceBackend {
        self.backend
    }

    pub fn persistence_backend_label(&self) -> &'static str {
        match self.backend {
            #[cfg(feature = "sqlite")]
            PersistenceBackend::Sqlite => "sqlite",
            PersistenceBackend::Postgres => "postgres",
        }
    }

    #[cfg(feature = "sqlite")]
    fn from_sqlite_database(db: SqliteDatabase, event_bus: SessionEventBus) -> Self {
        let permission_db = db.clone();
        let execution_db = db.clone();
        let mut manager = UnifiedSessionManager::new(db.clone());
        let bus = event_bus.clone();
        manager.set_event_listener(Arc::new(move |event| bus.publish(event)));
        Self {
            manager: ManagerInner::Sqlite(Arc::new(manager)),
            permission_db: PermissionDb::Sqlite(permission_db),
            maintenance_db: MaintenanceDb::Sqlite(db),
            execution_db: ExecutionDb::Sqlite(execution_db),
            event_bus,
            event_fanout: crate::event_fanout::EventFanout::disabled(),
            backend: PersistenceBackend::Sqlite,
            admission: Arc::new(PersistenceAdmission::from_config(&ServerConfig::default())),
        }
    }

    fn from_postgres_database(db: PostgresDatabase, event_bus: SessionEventBus) -> Self {
        let mut manager = UnifiedSessionManager::new(db.clone());
        let shared_db = Arc::new(db);
        let bus = event_bus.clone();
        manager.set_event_listener(Arc::new(move |event| bus.publish(event)));
        Self {
            manager: ManagerInner::Postgres(Arc::new(manager)),
            permission_db: PermissionDb::Postgres(shared_db.clone()),
            maintenance_db: MaintenanceDb::Postgres(shared_db.clone()),
            execution_db: ExecutionDb::Postgres(shared_db),
            event_bus,
            event_fanout: crate::event_fanout::EventFanout::disabled(),
            backend: PersistenceBackend::Postgres,
            admission: Arc::new(PersistenceAdmission::from_config(&ServerConfig::default())),
        }
    }

    fn with_admission_config(mut self, config: &ServerConfig) -> Self {
        self.admission = Arc::new(PersistenceAdmission::from_config(config));
        self
    }

    /// Wire a cross-pod event fan-out transport (disabled by default).
    pub fn with_event_fanout(mut self, event_fanout: crate::event_fanout::EventFanout) -> Self {
        self.event_fanout = event_fanout;
        self
    }

    /// Access the wired fan-out so SSE subscribers can await remote wakeups.
    pub fn event_fanout(&self) -> &crate::event_fanout::EventFanout {
        &self.event_fanout
    }

    /// Local broadcast plus best-effort remote wakeup for the durable store.
    fn publish_event(&self, event: &EventRow) {
        self.event_bus.publish(event.clone());
        self.event_fanout.notify();
    }

    pub fn attach_metrics(&self, metrics: &Arc<crate::metrics::MetricsRegistry>) {
        self.admission.attach_metrics(metrics);
    }

    pub fn event_bus(&self) -> &SessionEventBus {
        &self.event_bus
    }

    pub fn create_session(&self, config: SessionConfig) -> Result<SessionRow, String> {
        with_manager!(self, |manager| manager.create_session(config))
    }

    pub fn get_session(&self, session_id: &str) -> Result<SessionRow, String> {
        with_manager!(self, |manager| manager.get_session(session_id))
    }

    pub fn list_sessions(&self, query: SessionQuery) -> Result<Vec<SessionRow>, String> {
        with_manager!(self, |manager| manager.list_sessions(query))
    }

    pub fn close_session(&self, session_id: &str) -> Result<SessionRow, String> {
        with_manager!(self, |manager| manager.close_session(session_id))
    }

    pub fn delete_session(&self, session_id: &str) -> Result<(), String> {
        with_manager!(self, |manager| manager.delete_session(session_id))
    }

    pub fn send_message(
        &self,
        session_id: &str,
        config: MessageConfig,
    ) -> Result<MessageRow, String> {
        with_manager!(self, |manager| manager.send_message(session_id, config))
    }

    pub fn append_completed_turn(
        &self,
        session_id: &str,
        user_content: String,
        assistant_content: Option<String>,
    ) -> Result<(MessageRow, Option<MessageRow>), String> {
        with_manager!(self, |manager| manager.append_completed_turn(
            session_id,
            user_content,
            assistant_content
        ))
    }

    pub fn emit_session_event(
        &self,
        session_id: &str,
        event_type: &str,
        severity: &str,
        payload: Option<&str>,
    ) -> Result<(), String> {
        with_manager!(self, |manager| {
            manager.emit_session_event(session_id, event_type, severity, payload)
        })
    }

    pub fn list_messages(
        &self,
        session_id: &str,
        query: sdkwork_agent_database::MessageQuery,
    ) -> Result<Vec<MessageRow>, String> {
        with_manager!(self, |manager| manager.list_messages(session_id, query))
    }

    pub fn get_messages(
        &self,
        session_id: &str,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Vec<MessageRow>, String> {
        with_manager!(self, |manager| manager
            .get_messages(session_id, limit, offset))
    }

    pub fn load_recent_messages(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<MessageRow>, String> {
        with_manager!(self, |manager| manager
            .load_recent_messages(session_id, limit))
    }

    pub fn message_count(&self, session_id: &str) -> Result<i64, String> {
        with_manager!(self, |manager| manager.message_count(session_id))
    }

    pub fn delete_messages(&self, session_id: &str) -> Result<(), String> {
        with_manager!(self, |manager| manager.delete_messages(session_id))
    }

    pub fn health(&self) -> Result<bool, String> {
        with_manager!(self, |manager| manager.health())
    }

    pub fn schema_status(&self) -> Result<RuntimeSchemaStatus, String> {
        self.maintenance_db
            .schema_status()
            .map_err(|error| format!("failed to inspect runtime schema: {error}"))
    }

    pub fn readiness(&self) -> Result<(), String> {
        if !self.health()? {
            return Err("runtime persistence health probe returned false".to_string());
        }
        let status = self.schema_status()?;
        if !status.drift_free {
            return Err(format!(
                "runtime persistence schema is not current (version={}, expected={})",
                status.version, status.expected_version
            ));
        }
        Ok(())
    }

    pub fn purge_expired(
        &self,
        cutoff: &str,
        batch_size: i64,
    ) -> Result<RuntimePurgeCounts, String> {
        self.maintenance_db
            .purge_expired(cutoff, batch_size)
            .map_err(|error| format!("failed to purge expired runtime state: {error}"))
    }

    pub fn run_maintenance(&self) -> Result<(), String> {
        self.maintenance_db
            .run_maintenance()
            .map_err(|error| format!("failed to run runtime database maintenance: {error}"))
    }

    pub fn create_task(&self, session_id: &str, instruction: &str) -> Result<TaskRow, String> {
        with_manager!(self, |manager| manager.create_task(session_id, instruction))
    }

    pub fn create_task_execution(
        &self,
        task: &TaskRow,
        run: &RunRow,
        step: &StepRow,
        event: &EventRow,
    ) -> Result<(), String> {
        self.execution_db
            .create_task_execution(task, run, step, event)
            .map_err(|error| format!("failed to create task execution: {error}"))?;
        self.publish_event(&event);
        Ok(())
    }

    pub fn load_run(&self, run_id: &str) -> Result<Option<RunRow>, String> {
        self.execution_db
            .load_run(run_id)
            .map_err(|error| format!("failed to load run: {error}"))
    }

    pub fn load_steps(&self, run_id: &str) -> Result<Vec<StepRow>, String> {
        self.execution_db
            .load_steps(run_id)
            .map_err(|error| format!("failed to load run steps: {error}"))
    }

    pub fn next_task_attempt(&self, task_id: &str) -> Result<i64, String> {
        self.execution_db
            .next_task_attempt(task_id)
            .map_err(|error| format!("failed to resolve next task attempt: {error}"))
    }

    pub fn claim_ready_run(
        &self,
        worker_id: &str,
        now: &str,
        lease_expires_at: &str,
    ) -> Result<Option<ClaimedRun>, String> {
        self.execution_db
            .claim_ready_run(worker_id, now, lease_expires_at)
            .map_err(|error| format!("failed to claim ready run: {error}"))
    }

    pub fn renew_run_lease(
        &self,
        run_id: &str,
        worker_id: &str,
        fencing_token: i64,
        now: &str,
        lease_expires_at: &str,
    ) -> Result<bool, String> {
        self.execution_db
            .renew_run_lease(run_id, worker_id, fencing_token, now, lease_expires_at)
            .map_err(|error| format!("failed to renew run lease: {error}"))
    }

    pub fn complete_claimed_run(
        &self,
        claim: &ClaimedRun,
        result_json: Option<&str>,
        finished_at: &str,
        event: &EventRow,
    ) -> Result<(), String> {
        self.execution_db
            .complete_claimed_run(claim, result_json, finished_at, event)
            .map_err(|error| format!("failed to complete claimed run: {error}"))?;
        self.publish_event(&event);
        Ok(())
    }

    pub fn start_claimed_run(
        &self,
        claim: &ClaimedRun,
        started_at: &str,
        event: &EventRow,
    ) -> Result<(), String> {
        self.execution_db
            .start_claimed_run(claim, started_at, event)
            .map_err(|error| format!("failed to start claimed run: {error}"))?;
        self.publish_event(&event);
        Ok(())
    }

    pub fn complete_claimed_run_with_messages(
        &self,
        claim: &ClaimedRun,
        messages: &[MessageRow],
        result_json: Option<&str>,
        finished_at: &str,
        event: &EventRow,
    ) -> Result<(), String> {
        self.execution_db
            .complete_claimed_run_with_messages(claim, messages, result_json, finished_at, event)
            .map_err(|error| format!("failed to complete claimed run with messages: {error}"))?;
        self.publish_event(&event);
        Ok(())
    }

    pub fn fail_claimed_run(
        &self,
        claim: &ClaimedRun,
        error_kind: &str,
        error_code: Option<&str>,
        error_detail: &str,
        finished_at: &str,
        event: &EventRow,
    ) -> Result<(), String> {
        self.execution_db
            .fail_claimed_run(
                claim,
                error_kind,
                error_code,
                error_detail,
                finished_at,
                event,
            )
            .map_err(|error| format!("failed to fail claimed run: {error}"))?;
        self.publish_event(&event);
        Ok(())
    }

    pub fn schedule_run_retry(
        &self,
        claim: &ClaimedRun,
        error_kind: &str,
        error_code: Option<&str>,
        error_detail: &str,
        next_attempt_at: &str,
        event: &EventRow,
    ) -> Result<(), String> {
        self.execution_db
            .schedule_run_retry(
                claim,
                error_kind,
                error_code,
                error_detail,
                next_attempt_at,
                event,
            )
            .map_err(|error| format!("failed to schedule run retry: {error}"))?;
        self.publish_event(&event);
        Ok(())
    }

    pub fn request_task_cancellation(
        &self,
        task_id: &str,
        requested_at: &str,
        event: &EventRow,
    ) -> Result<(TaskRow, bool), String> {
        let result = self
            .execution_db
            .request_task_cancellation(task_id, requested_at, event)
            .map_err(|error| format!("failed to request task cancellation: {error}"))?;
        if result.1 {
            self.publish_event(&event);
        }
        Ok(result)
    }

    pub fn retry_task_execution(
        &self,
        task_id: &str,
        run: &RunRow,
        step: &StepRow,
        event: &EventRow,
    ) -> Result<TaskRow, String> {
        let task = self
            .execution_db
            .retry_task_execution(task_id, run, step, event)
            .map_err(|error| format!("failed to retry task execution: {error}"))?;
        self.publish_event(&event);
        Ok(task)
    }

    pub fn control_run(
        &self,
        run_id: &str,
        action: RunControlAction,
        changed_at: &str,
        event: &EventRow,
    ) -> Result<RunRow, String> {
        let run = self
            .execution_db
            .control_run(run_id, action, changed_at, event)
            .map_err(|error| format!("failed to control run: {error}"))?;
        self.publish_event(&event);
        Ok(run)
    }

    pub fn create_permission_execution(
        &self,
        permission: &PermissionRow,
        task: &TaskRow,
        run: &RunRow,
        step: &StepRow,
        operation: &PermissionOperationRow,
        event: &EventRow,
    ) -> Result<(), String> {
        self.execution_db
            .create_permission_execution(permission, task, run, step, operation, event)
            .map_err(|error| format!("failed to create permission execution: {error}"))?;
        self.publish_event(&event);
        Ok(())
    }

    pub fn load_permission_operation(
        &self,
        permission_request_id: &str,
    ) -> Result<Option<PermissionOperationRow>, String> {
        self.execution_db
            .load_permission_operation(permission_request_id)
            .map_err(|error| format!("failed to load permission operation: {error}"))
    }

    pub fn decide_permission_operation(
        &self,
        permission_request_id: &str,
        decision: &str,
        decided_at: &str,
        event: &EventRow,
    ) -> Result<PermissionOperationRow, String> {
        let operation = self
            .execution_db
            .decide_permission_operation(permission_request_id, decision, decided_at, event)
            .map_err(|error| format!("failed to decide permission operation: {error}"))?;
        self.publish_event(&event);
        Ok(operation)
    }

    pub fn claim_permission_operation(
        &self,
        worker_id: &str,
        now: &str,
        lease_expires_at: &str,
    ) -> Result<Option<ClaimedPermissionOperation>, String> {
        self.execution_db
            .claim_permission_operation(worker_id, now, lease_expires_at)
            .map_err(|error| format!("failed to claim permission operation: {error}"))
    }

    pub fn renew_permission_operation_lease(
        &self,
        permission_request_id: &str,
        worker_id: &str,
        fencing_token: i64,
        now: &str,
        lease_expires_at: &str,
    ) -> Result<bool, String> {
        self.execution_db
            .renew_permission_operation_lease(
                permission_request_id,
                worker_id,
                fencing_token,
                now,
                lease_expires_at,
            )
            .map_err(|error| format!("failed to renew permission operation lease: {error}"))
    }

    pub fn expire_permission_operations(
        &self,
        now: &str,
        batch_size: i64,
    ) -> Result<usize, String> {
        let events = self
            .execution_db
            .expire_permission_operations(now, batch_size)
            .map_err(|error| format!("failed to expire permission operations: {error}"))?;
        let count = events.len();
        for event in events {
            self.publish_event(&event);
        }
        Ok(count)
    }

    pub fn complete_permission_operation(
        &self,
        claim: &ClaimedPermissionOperation,
        result_json: &str,
        finished_at: &str,
        event: &EventRow,
    ) -> Result<(), String> {
        self.execution_db
            .complete_permission_operation(claim, result_json, finished_at, event)
            .map_err(|error| format!("failed to complete permission operation: {error}"))?;
        self.publish_event(&event);
        Ok(())
    }

    pub fn fail_permission_operation(
        &self,
        claim: &ClaimedPermissionOperation,
        error_kind: &str,
        error_code: Option<&str>,
        error_detail: &str,
        finished_at: &str,
        event: &EventRow,
    ) -> Result<(), String> {
        self.execution_db
            .fail_permission_operation(
                claim,
                error_kind,
                error_code,
                error_detail,
                finished_at,
                event,
            )
            .map_err(|error| format!("failed to fail permission operation: {error}"))?;
        self.publish_event(&event);
        Ok(())
    }

    pub fn get_task(&self, task_id: &str) -> Result<TaskRow, String> {
        with_manager!(self, |manager| manager.get_task(task_id))
    }

    pub fn list_tasks(
        &self,
        session_id: &str,
        query: sdkwork_agent_database::TaskQuery,
    ) -> Result<Vec<TaskRow>, String> {
        with_manager!(self, |manager| manager.list_tasks(session_id, query))
    }

    pub fn cancel_task(&self, task_id: &str) -> Result<TaskRow, String> {
        with_manager!(self, |manager| manager.cancel_task(task_id))
    }

    pub fn load_session_events(
        &self,
        session_id: &str,
        limit: Option<i64>,
        after_event_id: Option<&str>,
    ) -> Result<Vec<EventRow>, String> {
        with_manager!(self, |manager| {
            manager.load_session_events(session_id, limit, after_event_id)
        })
    }

    pub fn list_recent_events(
        &self,
        query: sdkwork_agent_database::EventQuery,
    ) -> Result<Vec<EventRow>, String> {
        with_manager!(self, |manager| manager.list_recent_events(query))
    }

    // -- Permission persistence --

    pub fn create_permission_if_absent(&self, permission: &PermissionRow) -> Result<bool, String> {
        self.permission_db
            .create_permission_if_absent(permission)
            .map_err(|error| format!("failed to create permission: {error}"))
    }

    pub fn save_permission(&self, permission: &PermissionRow) -> Result<(), String> {
        self.permission_db
            .save_permission(permission)
            .map_err(|error| format!("failed to save permission: {error}"))
    }

    pub fn load_permission(
        &self,
        permission_request_id: &str,
    ) -> Result<Option<PermissionRow>, String> {
        self.permission_db
            .load_permission(permission_request_id)
            .map_err(|error| format!("failed to load permission: {error}"))
    }

    pub fn list_permissions(
        &self,
        query: sdkwork_agent_database::PermissionQuery,
    ) -> Result<Vec<PermissionRow>, String> {
        self.permission_db
            .list_permissions(&query)
            .map_err(|error| format!("failed to list permissions: {error}"))
    }

    pub fn update_permission_status(
        &self,
        permission_request_id: &str,
        status: &str,
    ) -> Result<(), String> {
        self.permission_db
            .update_permission_status(permission_request_id, status)
            .map_err(|error| format!("failed to update permission: {error}"))
    }

    /// Run a blocking persistence operation off the async runtime thread pool.
    pub async fn run<F, T>(&self, operation: F) -> Result<T, String>
    where
        F: FnOnce(&PersistenceState) -> Result<T, String> + Send + 'static,
        T: Send + 'static,
    {
        let permit = self.admission.acquire().await?;
        let state = self.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation(&state)
        })
        .await
        .map_err(|error| format!("persistence worker failed: {error}"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    #[cfg(feature = "sqlite")]
    fn create_session_publishes_event() {
        let persistence = PersistenceState::memory().expect("persistence");
        let mut receiver = persistence.event_bus().subscribe();
        let row = persistence
            .create_session(SessionConfig::new("agent.1"))
            .expect("session");
        let event = receiver.try_recv().expect("published event");
        assert_eq!(event.session_id.as_deref(), Some(row.session_id.as_str()));
        assert_eq!(event.event_type, "session.created");
        assert_eq!(persistence.backend(), PersistenceBackend::Sqlite);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[cfg(feature = "sqlite")]
    async fn persistence_admission_rejects_when_execution_and_wait_queue_are_full() {
        let config = ServerConfig {
            persistence_max_concurrency: 1,
            persistence_max_waiters: 0,
            persistence_admission_timeout_ms: 100,
            ..Default::default()
        };
        let persistence = PersistenceState::memory()
            .expect("persistence")
            .with_admission_config(&config);
        let metrics = crate::metrics::MetricsRegistry::from_config(&config);
        persistence.attach_metrics(&metrics);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first = {
            let persistence = persistence.clone();
            tokio::spawn(async move {
                persistence
                    .run(move |_| {
                        started_tx.send(()).expect("started");
                        release_rx.recv().expect("released");
                        Ok(())
                    })
                    .await
            })
        };
        tokio::task::spawn_blocking(move || started_rx.recv_timeout(Duration::from_secs(1)))
            .await
            .expect("start waiter")
            .expect("first operation started");

        let error = persistence
            .run(|_| Ok(()))
            .await
            .expect_err("full admission must reject");
        assert_eq!(error, "persistence admission queue full");
        let rendered = metrics.render_prometheus(
            true,
            &crate::metrics::OperationalProfile::from_runtime("sqlite", false),
        );
        assert!(rendered.contains(
            "sdkwork_kernel_persistence_admission_rejected_total{service=\"sdkwork-agent-server\",environment=\"development\",deployment_profile=\"standalone\",runtime_target=\"server\",reason=\"queue_full\"} 1"
        ));

        release_tx.send(()).expect("release first operation");
        first.await.expect("first join").expect("first operation");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[cfg(feature = "sqlite")]
    async fn persistence_admission_times_out_a_bounded_waiter() {
        let config = ServerConfig {
            persistence_max_concurrency: 1,
            persistence_max_waiters: 1,
            persistence_admission_timeout_ms: 20,
            ..Default::default()
        };
        let persistence = PersistenceState::memory()
            .expect("persistence")
            .with_admission_config(&config);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first = {
            let persistence = persistence.clone();
            tokio::spawn(async move {
                persistence
                    .run(move |_| {
                        started_tx.send(()).expect("started");
                        release_rx.recv().expect("released");
                        Ok(())
                    })
                    .await
            })
        };
        tokio::task::spawn_blocking(move || started_rx.recv_timeout(Duration::from_secs(1)))
            .await
            .expect("start waiter")
            .expect("first operation started");

        let error = persistence
            .run(|_| Ok(()))
            .await
            .expect_err("waiter must time out");
        assert_eq!(error, "persistence admission timeout");

        release_tx.send(()).expect("release first operation");
        first.await.expect("first join").expect("first operation");
    }
}
