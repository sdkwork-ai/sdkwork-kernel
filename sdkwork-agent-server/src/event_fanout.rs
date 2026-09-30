//! Cross-pod event fan-out wakeup transport.
//!
//! The durable event store (PostgreSQL) stays the single source of truth for
//! SSE delivery: every subscriber keeps its bounded durable poll as the
//! correctness backstop. This component adds a best-effort **wakeup signal**
//! over Redis pub/sub so an SSE stream on pod B learns about an event
//! persisted on pod A within milliseconds instead of waiting out the poll
//! backoff (up to 30 s). Wakeups carry no payload — a wakeup only triggers an
//! immediate durable poll — so a lost or duplicated wakeup can never corrupt
//! delivery; it only changes latency.
//!
//! Degradation is graceful: when no Redis URL is configured the fan-out is a
//! no-op and behavior matches the previous single-process bus; when the
//! Redis connection drops, the local broadcast goes quiet and subscribers
//! fall back to the plain poll schedule.

use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::StreamExt;

/// Redis pub/sub channel carrying wakeup signals.
const FANOUT_CHANNEL: &str = "sdkwork:agent-server:event-wakeup";
/// Bound for queued publish notifications; overflow drops wakeups (the
/// durable poll backstop covers latency for dropped signals).
const NOTIFY_QUEUE_CAPACITY: usize = 4096;
/// Local fan-out capacity mirroring the in-process event bus.
const WAKEUP_BROADCAST_CAPACITY: usize = 1024;

#[derive(Clone, Debug)]
pub struct EventFanout {
    inner: Option<Arc<FanoutInner>>,
}

#[derive(Debug)]
struct FanoutInner {
    notify_tx: mpsc::Sender<()>,
    wakeup_tx: broadcast::Sender<()>,
}

impl EventFanout {
    /// A disabled fan-out: `notify` and `subscribe` are no-ops.
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    /// Connect to Redis and start the publish-forwarding and subscribe
    /// fan-out tasks. Returns an error when the initial connection fails so
    /// callers can fail closed in coordination modes that require it.
    pub async fn connect(redis_url: &str) -> anyhow::Result<Self> {
        let client = redis::Client::open(redis_url.to_string())?;
        let publish_connection = client
            .get_connection_manager()
            .await
            .map_err(|error| anyhow::anyhow!("event fanout connect failed: {error}"))?;
        let mut pubsub = client
            .get_async_pubsub()
            .await
            .map_err(|error| anyhow::anyhow!("event fanout subscribe failed: {error}"))?;
        pubsub
            .subscribe(FANOUT_CHANNEL)
            .await
            .map_err(|error| {
                anyhow::anyhow!("event fanout channel subscribe failed: {error}")
            })?;

        let (notify_tx, mut notify_rx) = mpsc::channel::<()>(NOTIFY_QUEUE_CAPACITY);
        let (wakeup_tx, _) = broadcast::channel::<()>(WAKEUP_BROADCAST_CAPACITY);

        // Forward queued notifications to Redis. `()` payloads keep the
        // signal payload-free; every subscriber polls its own durable store.
        let publish_task_connection = publish_connection.clone();
        tokio::spawn(async move {
            let mut connection = publish_task_connection;
            while notify_rx.recv().await.is_some() {
                let result: Result<(), redis::RedisError> = redis::cmd("PUBLISH")
                    .arg(FANOUT_CHANNEL)
                    .arg("wakeup")
                    .query_async(&mut connection)
                    .await;
                if let Err(error) = result {
                    tracing::warn!(error = %error, "event fanout publish failed; durable poll remains the backstop");
                }
            }
        });

        // Fan Redis wakeups out to every local SSE subscriber.
        let wakeup_tx_task = wakeup_tx.clone();
        tokio::spawn(async move {
            let mut stream = pubsub.on_message();
            while let Some(_message) = stream.next().await {
                // A dropped send only means no local subscriber cares.
                let _ = wakeup_tx_task.send(());
            }
            tracing::warn!("event fanout subscription closed; durable poll remains the backstop");
        });

        Ok(Self {
            inner: Some(Arc::new(FanoutInner {
                notify_tx,
                wakeup_tx,
            })),
        })
    }

    /// Whether a remote transport is wired.
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Signal that a durable event was persisted. Non-blocking and
    /// best-effort: a full queue drops the wakeup, and the durable poll
    /// backstop bounds the added latency.
    pub fn notify(&self) {
        if let Some(inner) = &self.inner {
            let _ = inner.notify_tx.try_send(());
        }
    }

    /// Wait for the next remote wakeup. Returns `None` when the transport is
    /// disabled or its subscription has closed (callers fall back to the
    /// plain poll schedule).
    pub async fn wakeup(&self) -> Option<()> {
        let inner = self.inner.as_ref()?;
        let mut receiver = inner.wakeup_tx.subscribe();
        receiver.recv().await.ok()
    }

    /// Subscribe once for repeated wakeups. `None` when the transport is
    /// disabled; a closed receiver means the remote subscription ended and
    /// callers should fall back to the plain poll schedule.
    pub fn subscribe_wakeup(&self) -> Option<broadcast::Receiver<()>> {
        let inner = self.inner.as_ref()?;
        Some(inner.wakeup_tx.subscribe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_fanout_is_a_noop() {
        let fanout = EventFanout::disabled();
        assert!(!fanout.is_enabled());
        fanout.notify();
        // wakeup() resolves immediately to None when disabled.
        futures_executor_block_on_disabled(&fanout);
    }

    fn futures_executor_block_on_disabled(fanout: &EventFanout) {
        // A disabled fan-out's wakeup future must resolve without awaiting
        // Redis; poll it once with a no-op waker via a trivial runtime.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime");
        let resolved = runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_millis(50), fanout.wakeup())
                .await
                .is_ok()
        });
        assert!(resolved, "disabled fanout wakeup must resolve immediately");
    }
}
