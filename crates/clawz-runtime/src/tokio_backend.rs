//! Tokio-backed RuntimeBackend — for T2 (containerized) and T3 (server/desktop).

use async_trait::async_trait;
use std::fs::File;
use std::future::Future;
use std::io::{self, Read};
use std::time::{Duration, Instant};

#[async_trait]
impl crate::RuntimeBackend for crate::TokioBackend {
    async fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        tokio::spawn(task);
    }

    async fn spawn_blocking(&self, task: impl FnOnce() + Send + 'static) {
        let _ = tokio::task::spawn_blocking(task).await;
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    fn now(&self) -> Instant {
        tokio::time::Instant::now().into_std()
    }

    fn try_read_fd(&self, fd: i32, buf: &mut [u8]) -> Result<usize, io::Error> {
        use std::os::fd::FromRawFd;
        let mut f: File = unsafe { FromRawFd::from_raw_fd(fd) };
        f.read(buf)
    }
}

#[cfg(test)]
mod tests {
    use crate::TokioBackend;
    use crate::RuntimeBackend;
    use std::time::Duration;

    #[tokio::test]
    async fn test_spawn_and_sleep() {
        let backend = TokioBackend::new();
        let handled = std::sync::atomic::AtomicBool::new(false);
        let flag = handled.clone();
        backend.spawn(async move {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }).await;
        assert!(handled.load(std::sync::atomic::Ordering::SeqCst));
        backend.sleep(Duration::from_millis(1)).await;
    }

    #[tokio::test]
    async fn test_now() {
        let backend = TokioBackend::new();
        let t1 = backend.now();
        backend.sleep(Duration::from_millis(10)).await;
        let t2 = backend.now();
        assert!(t2 > t1);
    }
}