//! A latched host stop request survives startup and listener replacement.
use crate::Result;
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone)]
pub struct Lifecycle {
    stop: watch::Sender<bool>,
    ready: Arc<dyn Fn() -> Result<()> + Send + Sync>,
    console: bool,
}

impl Lifecycle {
    pub fn foreground() -> Self {
        let mut lifecycle = Self::controlled(|| Ok(()));
        lifecycle.console = true;
        lifecycle
    }

    pub fn controlled(ready: impl Fn() -> Result<()> + Send + Sync + 'static) -> Self {
        Self {
            stop: watch::channel(false).0,
            ready: Arc::new(ready),
            console: false,
        }
    }

    pub fn request_stop(&self) {
        self.stop.send_replace(true);
    }

    pub fn stopping(&self) -> bool {
        *self.stop.borrow()
    }

    pub(crate) fn ready(&self) -> Result<()> {
        (self.ready)()
    }

    pub(crate) async fn stopped(&self) -> std::io::Result<()> {
        let mut receiver = self.stop.subscribe();
        let requested = async {
            receiver
                .wait_for(|stop| *stop)
                .await
                .map(|_| ())
                .map_err(std::io::Error::other)
        };
        if self.console {
            tokio::select! {
                result = requested => result,
                result = super::server::shutdown_signal() => result,
            }
        } else {
            requested.await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn lifecycle_stop_is_latched_before_wait_and_across_clones() -> Result<()> {
        let lifecycle = Lifecycle::controlled(|| Ok(()));
        lifecycle.request_stop();
        lifecycle.request_stop();
        assert!(lifecycle.stopping());
        for host in [lifecycle.clone(), lifecycle] {
            tokio::time::timeout(Duration::from_secs(1), host.stopped())
                .await
                .map_err(|_| crate::Error::Internal("latched stop was lost"))??;
        }
        Ok(())
    }

    #[tokio::test]
    async fn lifecycle_wakes_a_waiter_and_reports_readiness_errors() -> Result<()> {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let lifecycle = Lifecycle::controlled(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(crate::Error::Internal("injected readiness failure"))
        });
        assert!(lifecycle.ready().is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let waiting = lifecycle.clone();
        let task = tokio::spawn(async move { waiting.stopped().await });
        tokio::task::yield_now().await;
        lifecycle.request_stop();
        task.await
            .map_err(|_| crate::Error::Internal("stop waiter failed"))??;
        Ok(())
    }
}
