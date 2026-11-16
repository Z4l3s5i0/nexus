use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Default)]
pub struct ImportGate {
    active: AtomicUsize,
    idle: Notify,
}

pub struct ImportGuard {
    gate: Arc<ImportGate>,
}

impl ImportGate {
    pub fn enter(self: &Arc<Self>) -> ImportGuard {
        self.active.fetch_add(1, Ordering::AcqRel);
        ImportGuard { gate: Arc::clone(self) }
    }

    pub async fn wait_idle(&self) {
        loop {
            let notified = self.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.is_busy() {
                return;
            }
            notified.await;
        }
    }

    pub fn is_busy(&self) -> bool {
        self.active.load(Ordering::Acquire) > 0
    }
}

impl Drop for ImportGuard {
    fn drop(&mut self) {
        if self.gate.active.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.gate.idle.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ImportGate;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn wait_idle_returns_when_gate_is_idle() {
        let gate = Arc::new(ImportGate::default());

        tokio::time::timeout(Duration::from_millis(100), gate.wait_idle())
            .await
            .expect("an idle gate should not block");
    }

    #[tokio::test]
    async fn wait_idle_blocks_and_wakes_when_last_guard_drops() {
        let gate = Arc::new(ImportGate::default());
        let guard = gate.enter();
        assert!(gate.is_busy());

        let waiter_gate = gate.clone();
        let waiter = tokio::spawn(async move { waiter_gate.wait_idle().await });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        drop(guard);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter should wake after import completes")
            .expect("wait task should not panic");
        assert!(!gate.is_busy());
    }
}