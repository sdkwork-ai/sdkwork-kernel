//! Background worker that polls typed provider health into [`BackendHealthMonitor`].
//!
//! Spawned during agent-server bootstrap so backend selection can degrade and
//! recover from stale provider health instead of relying on one-shot checks.

use sdkwork_agent_kernel::{
    AgentRuntime, BackendHealthMonitor, HealthMonitorConfig, SdkDriverHealth,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

/// Cooldown applied when a monitor tick bails out early or panics, so a
/// poisoned lock cannot turn the worker thread into a hot spin.
const BAILOUT_BACKOFF: Duration = Duration::from_secs(1);

/// Owns a [`BackendHealthMonitor`] and a background thread that refreshes driver health.
#[derive(Clone)]
pub struct BackendHealthWorker {
    monitor: Arc<RwLock<BackendHealthMonitor>>,
    stop: Arc<AtomicBool>,
    wake: Arc<(Mutex<()>, Condvar)>,
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl BackendHealthWorker {
    pub fn spawn(agent_runtime: Arc<AgentRuntime>, config: HealthMonitorConfig) -> Self {
        let mut monitor = BackendHealthMonitor::new(config.clone());
        for diagnostic in agent_runtime.diagnostics().provider_diagnostics {
            monitor.register_driver(&diagnostic.provider_id);
        }

        let monitor = Arc::new(RwLock::new(monitor));
        let stop = Arc::new(AtomicBool::new(false));
        let wake = Arc::new((Mutex::new(()), Condvar::new()));
        let monitor_for_thread = monitor.clone();
        let stop_for_thread = stop.clone();
        let wake_for_thread = wake.clone();
        let interval = config.check_interval;

        let thread = std::thread::spawn(move || {
            // The whole monitor loop is wrapped in catch_unwind so a panic in
            // one tick (a poisoned lock, a panicking provider health probe)
            // does not silently kill the worker; it logs and continues with
            // the next wakeup instead. When a tick bails out early (poisoned
            // wake lock), the backoff sleep keeps the outer loop from spinning.
            while !stop_for_thread.load(Ordering::Relaxed) {
                let tick_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    health_check_loop(
                        &stop_for_thread,
                        &wake_for_thread,
                        &monitor_for_thread,
                        &agent_runtime,
                        interval,
                    )
                }));
                match tick_result {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::warn!(
                            "backend health worker tick bailed out early; backing off"
                        );
                        std::thread::sleep(BAILOUT_BACKOFF);
                    }
                    Err(_) => {
                        tracing::warn!("backend health worker panicked; continuing on the next check");
                        std::thread::sleep(BAILOUT_BACKOFF);
                    }
                }
            }
        });

        Self {
            monitor,
            stop,
            wake,
            thread: Arc::new(Mutex::new(Some(thread))),
        }
    }

    pub fn spawn_default(agent_runtime: Arc<AgentRuntime>) -> Self {
        Self::spawn(agent_runtime, HealthMonitorConfig::default())
    }

    pub fn monitor(&self) -> Arc<RwLock<BackendHealthMonitor>> {
        self.monitor.clone()
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let (_wake_lock, wake_signal) = &*self.wake;
        wake_signal.notify_all();
        if let Ok(mut guard) = self.thread.lock() {
            if let Some(thread) = guard.take() {
                let _ = thread.join();
            }
        }
    }
}

impl std::fmt::Debug for BackendHealthWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let driver_count = self
            .monitor
            .read()
            .map(|monitor| monitor.registered_drivers().len())
            .unwrap_or(0);
        f.debug_struct("BackendHealthWorker")
            .field("registered_driver_count", &driver_count)
            .finish()
    }
}

/// One pass of the backend health monitor: wait for the next wakeup (or the
/// check interval) and refresh every registered driver's health. Returns
/// `true` when the tick waited on the wake condition normally, `false` when
/// it bailed out early (poisoned wake lock) so the caller can back off
/// instead of spinning.
fn health_check_loop(
    stop_for_thread: &AtomicBool,
    wake_for_thread: &Arc<(Mutex<()>, Condvar)>,
    monitor_for_thread: &Arc<RwLock<BackendHealthMonitor>>,
    agent_runtime: &Arc<AgentRuntime>,
    interval: std::time::Duration,
) -> bool {
    let (wake_lock, wake_signal) = &**wake_for_thread;
    let Ok(wake_guard) = wake_lock.lock() else {
        return false;
    };
    let Ok((wake_guard, _timeout)) = wake_signal.wait_timeout_while(wake_guard, interval, |_| {
        !stop_for_thread.load(Ordering::Relaxed)
    }) else {
        return false;
    };
    drop(wake_guard);
    if stop_for_thread.load(Ordering::Relaxed) {
        return true;
    }

    let Ok(mut guard) = monitor_for_thread.write() else {
        return false;
    };
    if !guard.should_check() {
        return true;
    }
    guard.mark_check();
    let diagnostics = agent_runtime.diagnostics();
    for diagnostic in diagnostics.provider_diagnostics {
        let Some(health) = diagnostic.health.as_ref() else {
            continue;
        };
        let driver_health = SdkDriverHealth::from_provider_health(health);
        let _ = guard.record_driver_health(&diagnostic.provider_id, driver_health);
    }
    true
}

impl Drop for BackendHealthWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdkwork_agent_kernel::RuntimeState;
    use std::time::Duration;

    #[test]
    fn worker_registers_providers_from_runtime_diagnostics() {
        let runtime =
            Arc::new(crate::runtime_bootstrap::bootstrap_agent_runtime().expect("bootstrap"));
        assert_ne!(runtime.state(), RuntimeState::Failed);

        let worker = BackendHealthWorker::spawn_default(runtime);
        std::thread::sleep(Duration::from_millis(50));

        let monitor = worker.monitor();
        let guard = monitor.read().expect("monitor lock");
        let registered = guard.registered_drivers();
        assert!(!registered.is_empty());
    }

    #[test]
    fn shutdown_wakes_sleeping_worker_without_waiting_for_check_interval() {
        let runtime =
            Arc::new(crate::runtime_bootstrap::bootstrap_agent_runtime().expect("bootstrap"));
        let worker = BackendHealthWorker::spawn(
            runtime,
            HealthMonitorConfig::default().with_check_interval(Duration::from_secs(5)),
        );
        std::thread::sleep(Duration::from_millis(100));

        let started = std::time::Instant::now();
        worker.shutdown();

        assert!(
            started.elapsed() < Duration::from_millis(500),
            "backend health worker shutdown must not block until the next check interval"
        );
    }
}
