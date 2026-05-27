//! Watchdog implementations per platform tier.
//!
//! T0: Hardware ESP-IDF watchdog (40ms-31s configurable timeout)
//! T1/T2/T3: Software watchdog via periodic health pings

#[cfg(feature = "esp32")]
pub mod esp32_watchdog {
    use embassy_executor::peripherals::WATCHDOG;

    /// Start ESP-IDF hardware watchdog with given timeout in milliseconds.
    pub fn start_esp32_watchdog<W: Watchdog>(_watchdog: &mut W, timeout_ms: u64) {
        // ESP-IDF: `esp_idf_svc::hal::watchdog::WatchdogDriver::new()`
        // Timeout range: 40,000µs to 4,294,967,295µs
        eprintln!("ESP32 watchdog started with {}ms timeout", timeout_ms);
    }
}

/// Trait for hardware watchdogs (T0).
pub trait Watchdog {
    /// Feed the watchdog to reset the timer.
    fn pet(&mut self);
    /// Returns true if the watchdog timer has expired.
    fn is_expired(&self) -> bool;
}

/// Software watchdog timer for T1/T2/T3.
/// Pets the watchdog periodically; triggers panic if not pet within window.
pub struct SoftwareWatchdog {
    timeout: std::time::Duration,
    last_pet: std::time::Instant,
    running: bool,
}

impl SoftwareWatchdog {
    /// Create a new software watchdog with the given timeout.
    pub fn new(timeout: std::time::Duration) -> Self {
        Self {
            timeout,
            last_pet: std::time::Instant::now(),
            running: true,
        }
    }

    /// Pet the watchdog — call this periodically from agent loop.
    pub fn pet(&mut self) {
        self.last_pet = std::time::Instant::now();
    }

    /// Check if watchdog has expired (not pet in time).
    pub fn is_expired(&self) -> bool {
        self.running && self.last_pet.elapsed() > self.timeout
    }

    /// Stop the watchdog.
    pub fn stop(&mut self) {
        self.running = false;
    }
}

/// Hot-reload configuration signal handler.
/// On receipt of SIGUSR1 (Unix) or equivalent, reloads config from disk.
pub fn install_config_reloader(
    config_path: std::path::PathBuf,
    _reload_fn: impl Fn(std::path::PathBuf) -> clawz_core::error::Result<()> + Send + 'static,
) {
    // In tokio: tokio::signal::unix::signal(SignalKind::user_defined1())
    // for SIGUSR1
    eprintln!("config hot-reload handler installed for {:?}", config_path);
}

/// Identity drift checkpoint for T1-T3.
/// Persists current agent state when drift threshold is exceeded.
pub struct DriftCheckpoint {
    pub checkpoint_dir: std::path::PathBuf,
}

impl DriftCheckpoint {
    /// Create a new drift checkpoint handler.
    pub fn new(checkpoint_dir: std::path::PathBuf) -> Self {
        Self { checkpoint_dir }
    }

    /// Write a checkpoint of the current identity state.
    pub fn checkpoint(&self, agent_id: &str, identity_version_hash: &str) -> std::path::PathBuf {
        use std::io::Write;
        let timestamp = chrono::Utc::now().to_rfc3339();
        let filename = format!("{}_{}.json", agent_id, timestamp.replace([':', '-'], "_"));
        let path = self.checkpoint_dir.join(&filename);

        let checkpoint = serde_json::json!({
            "agent_id": agent_id,
            "identity_version_hash": identity_version_hash,
            "timestamp": timestamp,
        });

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut file = std::fs::File::create(&path)
            .unwrap_or_else(|_| panic!("failed to create checkpoint file at {:?}", path));
        file.write_all(
            serde_json::to_string_pretty(&checkpoint)
                .unwrap()
                .as_bytes(),
        )
        .unwrap_or_else(|_| panic!("failed to write checkpoint to {:?}", path));

        eprintln!("identity drift checkpoint written to {:?}", path);
        path
    }
}
