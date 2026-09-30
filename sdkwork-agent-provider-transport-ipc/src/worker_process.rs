use crate::transport::{SharedJsonRpcTransport, StdioJsonRpcSession, TransportError};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};

/// Upper bound for worker stderr retained for crash diagnostics. The reader
/// keeps draining past this bound (discarding excess) so a chatty child can
/// never fill the pipe and deadlock.
pub const WORKER_STDERR_TAIL_BYTES: usize = 64 * 1024;

/// Bounded tail of the worker's stderr, filled by a dedicated drain thread.
pub type StderrTail = Arc<Mutex<Vec<u8>>>;

/// Keeps a stdio JSON-RPC worker process alive for cancellation and respawn.
pub struct SpawnedWorker {
    session: Arc<StdioJsonRpcSession>,
    child: Mutex<Option<Child>>,
    stderr_tail: StderrTail,
}

impl SpawnedWorker {
    pub fn spawn(mut command: Command) -> Result<Self, TransportError> {
        // Own process group on Unix so cancellation can terminate provider
        // grandchildren (CLIs, gateways) the worker spawned, not just the
        // direct child. Windows lacks a std process-group equivalent; kill()
        // terminates the direct child and descendants exit on stdin EOF.
        #[cfg(unix)]
        command.process_group(0);
        let (session, child, stderr_tail) = StdioJsonRpcSession::spawn(command)?;
        Ok(Self {
            session: Arc::new(session),
            child: Mutex::new(Some(child)),
            stderr_tail,
        })
    }

    pub fn transport(&self) -> SharedJsonRpcTransport {
        SharedJsonRpcTransport::new(self.session.clone())
    }

    pub fn is_running(&self) -> bool {
        let Ok(mut guard) = self.child.lock() else {
            return false;
        };
        let Some(child) = guard.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) => true,
            Err(_) => false,
        }
    }

    pub fn is_reusable(&self) -> bool {
        self.session.is_reusable() && self.is_running()
    }

    /// Most recent worker stderr, bounded to [`WORKER_STDERR_TAIL_BYTES`].
    /// Included in fail-closed errors so a worker that dies at startup is
    /// diagnosable in production.
    pub fn stderr_tail(&self) -> String {
        let guard = self
            .stderr_tail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        String::from_utf8_lossy(&guard).trim().to_string()
    }

    fn append_stderr_context(message: &str, stderr_tail: &StderrTail) -> String {
        let guard = stderr_tail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_empty() {
            return message.to_string();
        }
        format!(
            "{message}: worker stderr (tail): {}",
            String::from_utf8_lossy(&guard).trim()
        )
    }

    /// Terminates this worker and its process group (Unix). Request-scoped
    /// cancellation is implemented by leasing one worker to one active
    /// request.
    pub fn cancel_inflight(&self) -> Result<(), TransportError> {
        let Ok(mut guard) = self.child.lock() else {
            return Err(TransportError::new("worker child lock failed"));
        };
        if let Some(mut child) = guard.take() {
            #[cfg(unix)]
            let child_id = child.id();
            match child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => {}
                Err(error) => {
                    *guard = Some(child);
                    return Err(TransportError::new(format!(
                        "worker status check failed: {error}"
                    )));
                }
            }
            // Kill the whole process group first so worker-spawned provider
            // children cannot outlive the cancellation. A negative pid
            // targets the group spawned with `process_group(0)`; failure
            // (group already reaped) falls through to the direct kill.
            #[cfg(unix)]
            {
                // SAFETY: `kill(2)` with a negative pid signals the process
                // group; pids from `Child::id()` are always positive, so the
                // negation never targets every process (`-1`) or the caller's
                // group (`0`).
                let _ = unsafe { libc::kill(-(child_id as libc::pid_t), libc::SIGKILL) };
            }
            if let Err(error) = child.kill() {
                *guard = Some(child);
                return Err(TransportError::new(Self::append_stderr_context(
                    &format!("worker termination failed: {error}"),
                    &self.stderr_tail,
                )));
            }
            child.wait().map_err(|error| {
                TransportError::new(Self::append_stderr_context(
                    &format!("worker wait failed: {error}"),
                    &self.stderr_tail,
                ))
            })?;
        }
        Ok(())
    }
}

impl Drop for SpawnedWorker {
    fn drop(&mut self) {
        let _ = self.cancel_inflight();
    }
}
