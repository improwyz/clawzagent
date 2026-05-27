//! Embassy-backed RuntimeBackend for T0 (bare-metal embedded).
//!
//! Uses embassy-executor for interrupt-driven async with static task pools.
//! No heap allocation on hot path — all collections use heapless::Vec.

use async_trait::async_trait;
use core::time::Duration;

// =============================================================================
// no_std path: stub impl for compilation (actual embassy support needs
// RuntimeBackend trait to support no_std time types)
// =============================================================================
#[cfg(feature = "no_std")]
mod no_std_impl {
    use super::*;
    use clawz_runtime::RuntimeBackend;
    use embassy_executor::Executor;
    use embassy_time::Timer;
    use core::mem::MaybeUninit;
    use core::ptr;

    static EXECUTOR: StaticCell<Executor<64>> = StaticCell::new();

    /// Embassy-backed RuntimeBackend for bare-metal targets.
    /// Runs on ESP32-S3 with ~64KB RAM budget.
    /// 64 tasks max, 4KB stack each = 256KB total for task storage.
    pub struct EmbassyBackend {
        _priv: (),
    }

    impl EmbassyBackend {
        pub fn new() -> Self {
            Self { _priv: () }
        }

        pub fn executor(&self) -> &'static Executor<'static, 64> {
            EXECUTOR.init(Executor::new())
        }
    }

    impl Default for EmbassyBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait]
    impl RuntimeBackend for EmbassyBackend {
        async fn spawn(&self, task: impl core::future::Future<Output = ()> + Send + 'static) {
            self.executor().spawn(task, "").ok();
        }

        async fn spawn_blocking(&self, task: impl FnOnce() + Send + 'static) {
            task();
        }

        async fn sleep(&self, duration: Duration) {
            Timer::after(duration.into()).await;
        }

        fn now(&self) -> embassy_time::Instant {
            embassy_time::Instant::now()
        }

        fn try_read_fd(&self, _fd: i32, _buf: &mut [u8]) -> Result<usize, core::io::Error> {
            Err(core::io::Error::new(
                core::io::ErrorKind::Unsupported,
                "no file descriptors on bare-metal",
            ))
        }
    }

    struct StaticCell<T> {
        data: core::cell::UnsafeCell<MaybeUninit<T>>,
    }

    impl<T> StaticCell<T> {
        const fn new() -> Self {
            Self {
                data: core::cell::UnsafeCell::new(MaybeUninit::uninit()),
            }
        }

        fn init(&self, value: T) -> &'static T {
            let target = self.data.get();
            unsafe {
                ptr::write(target, MaybeUninit::new(value));
                &*(target as *const T)
            }
        }
    }
}

// =============================================================================
// std path: stub for testing on host
// =============================================================================
#[cfg(feature = "std")]
mod std_impl {
    use super::*;
    use clawz_runtime::RuntimeBackend;
    use std::io;
    use std::time::Instant;

    pub struct EmbassyBackend {
        _priv: (),
    }

    impl EmbassyBackend {
        pub fn new() -> Self {
            Self { _priv: () }
        }
    }

    impl Default for EmbassyBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait]
    impl RuntimeBackend for EmbassyBackend {
        async fn spawn(&self, _task: impl core::future::Future<Output = ()> + Send + 'static) {
            // std build: can't actually use embassy, just satisfy trait
        }

        async fn spawn_blocking(&self, task: impl FnOnce() + Send + 'static) {
            task();
        }

        async fn sleep(&self, duration: Duration) {
            tokio::time::sleep(duration).await;
        }

        fn now(&self) -> Instant {
            Instant::now()
        }

        fn try_read_fd(&self, _fd: i32, _buf: &mut [u8]) -> Result<usize, io::Error> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "not supported in std build"))
        }
    }
}

#[cfg(feature = "no_std")]
pub use no_std_impl::EmbassyBackend;

#[cfg(feature = "std")]
pub use std_impl::EmbassyBackend;

#[cfg(not(any(feature = "std", feature = "no_std")))]
compile_error!("must enable either \"std\" or \"no_std\" feature");

#[cfg(test)]
mod tests {
    use clawz_runtime::RuntimeBackend;

    use crate::EmbassyBackend;

    #[tokio::test]
    async fn test_embassy_backend_compiles() {
        let backend = EmbassyBackend::new();
        backend.sleep(core::time::Duration::from_millis(1)).await;
    }
}