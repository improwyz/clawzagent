//! Single-threaded Tokio-backed RuntimeBackend — for T1 (SBC with tight resources).

use async_trait::async_trait;
use std::fs::File;
use std::future::Future;
use std::io::{self, Read};
use std::time::{Duration, Instant};

#[async_trait]
impl crate::RuntimeBackend for crate::TokioSingleThreadBackend {
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
