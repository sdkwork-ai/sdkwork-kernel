use std::future::IntoFuture;
use std::sync::Arc;
use tracing::{info, warn};

use sdkwork_agent_server::{
    api::internal_runtime, app, config::ServerConfig, health,
    permission_operation_worker::PermissionOperationWorker, persistence::PersistenceState,
    preflight, runtime_cleanup_worker::RuntimeCleanupWorker, shutdown,
    task_execution_worker::TaskExecutionWorker,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = ServerConfig::from_env()?;
    let config = Arc::new(config);

    init_logging(config.as_ref())?;

    info!("SDKWork Agent Server starting...");
    info!("Version: {}", env!("CARGO_PKG_VERSION"));
    info!("Bind address: {}", config.bind_addr());
    info!(
        "Runtime database engine: {}",
        config.runtime_database_engine
    );
    info!("Runtime database: PostgreSQL (SDKWORK_DATABASE_*)");
    info!("Environment: {}", config.environment);
    info!("Ingress auth mode: {}", config.ingress_auth_mode);

    let preflight_result = preflight::validate(config.as_ref());
    preflight::print_results(&preflight_result);

    if !preflight_result.passed {
        anyhow::bail!("Preflight checks failed");
    }

    let health_state = Arc::new(health::HealthState::new());
    let event_fanout = match config.effective_event_fanout_redis_url() {
        Some(redis_url) => match sdkwork_agent_server::event_fanout::EventFanout::connect(redis_url).await {
            Ok(fanout) => {
                info!("Cross-pod event fan-out connected (coordination mode: {})", config.coordination_mode);
                fanout
            }
            Err(error) => {
                if config.is_cluster_coordination() {
                    return Err(anyhow::anyhow!(
                        "cluster coordination requires a working event fan-out Redis connection: {error}"
                    ));
                }
                tracing::warn!(error = %error, "event fanout connect failed; SSE falls back to durable polling");
                sdkwork_agent_server::event_fanout::EventFanout::disabled()
            }
        },
        None => sdkwork_agent_server::event_fanout::EventFanout::disabled(),
    };
    let persistence = Arc::new(
        PersistenceState::open_from_config_async(config.as_ref())
            .await?
            .with_event_fanout(event_fanout),
    );
    let runtime_state = Arc::new(
        internal_runtime::InternalRuntimeApiState::new_async(persistence.clone(), config.clone())
            .await
            .map_err(|error| anyhow::anyhow!("agent runtime bootstrap failed: {error}"))?,
    );

    let app = app::build_app_async(
        config.clone(),
        health_state,
        persistence.clone(),
        runtime_state.clone(),
    )
    .await?;

    let bind_addr = config.bind_addr();
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;

    info!("Server listening on {}", bind_addr);
    info!("Internal runtime API: /internal/v3/api/intelligence/runtime/*");

    // Graceful shutdown: on signal, axum stops accepting new connections and
    // drains in-flight requests. `force_close_timer()` caps total drain time
    // from the signal — it must not start at process boot.
    let (shutdown_tx, mut graceful_rx) = tokio::sync::watch::channel(false);
    let mut deadline_rx = shutdown_tx.subscribe();
    let cleanup_worker =
        RuntimeCleanupWorker::spawn(persistence.clone(), config.clone(), shutdown_tx.subscribe());
    let task_worker = TaskExecutionWorker::spawn(
        runtime_state.clone(),
        config.clone(),
        shutdown_tx.subscribe(),
    );
    let permission_worker = PermissionOperationWorker::spawn(
        runtime_state.clone(),
        config.clone(),
        shutdown_tx.subscribe(),
    );
    let signal_shutdown_tx = shutdown_tx.clone();
    tokio::spawn(async move {
        shutdown::shutdown_signal().await;
        let _ = signal_shutdown_tx.send(true);
    });

    let graceful_trigger = async move {
        if !*graceful_rx.borrow() {
            let _ = graceful_rx.changed().await;
        }
    };
    let hard_deadline = async move {
        if !*deadline_rx.borrow() {
            let _ = deadline_rx.changed().await;
        }
        shutdown::force_close_timer().await;
    };

    let server = axum::serve(listener, app)
        .with_graceful_shutdown(graceful_trigger)
        .into_future();
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result?,
        _ = hard_deadline => {
            warn!("shutdown grace period elapsed; force-closing remaining connections");
        }
    }
    let _ = shutdown_tx.send(true);
    cleanup_worker.join().await;
    task_worker.join().await;
    permission_worker.join().await;

    info!("Server shutdown complete");

    Ok(())
}

fn init_logging(config: &ServerConfig) -> anyhow::Result<()> {
    sdkwork_agent_server::observability::init_tracing(config)
}
