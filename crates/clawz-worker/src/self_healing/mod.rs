//! Self-healing supervisor patterns for ClawZ agent runtime.
//!
//! Patterns implemented per tier:
//! - T0: Hardware watchdog (ESP-IDF) via embassy watchdog task
//! - T1: Software watchdog + file-based state checkpoint
//! - T2: Container restart policy + periodic health checks
//! - T3: Process supervisor + SQLite WAL snapshots
//!
//! Wire into agent scheduler via `run_with_supervisor()` wrapper.

pub mod watchdog;

use clawz_core::error::Result;

/// Supervisor config for the current platform tier.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Enable circuit breaker (default: true)
    pub circuit_breaker_enabled: bool,
    /// Max restart attempts before giving up
    pub max_restart_attempts: usize,
    /// Restart cooldown in seconds
    pub restart_cooldown_secs: u64,
    /// Enable identity drift checkpoint (default: true)
    pub drift_checkpoint_enabled: bool,
    /// Drift threshold that triggers checkpoint (0.0-1.0)
    pub drift_threshold: f64,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            circuit_breaker_enabled: true,
            max_restart_attempts: 3,
            restart_cooldown_secs: 5,
            drift_checkpoint_enabled: true,
            drift_threshold: 0.75,
        }
    }
}

/// Trait for schedulers that support circuit breaker integration.
#[allow(async_fn_in_trait)]
pub trait CircuitBreakerScheduler {
    /// Returns true if the circuit should be opened (too many failures).
    fn should_open_circuit(&self) -> bool;
    /// Record a successful tick.
    fn record_success(&mut self);
    /// Record a failed tick.
    fn record_failure(&mut self);
    /// Attempt recovery after circuit opens.
    async fn recover(&mut self) -> Result<()>;
    /// Run one scheduler tick.
    async fn tick(&mut self) -> Result<()>;
}

/// Run the agent scheduler with self-healing supervision.
pub async fn run_with_supervisor<S: CircuitBreakerScheduler>(
    mut scheduler: S,
    config: SupervisorConfig,
) -> Result<()> {
    let mut restart_attempts = 0;

    loop {
        match scheduler.tick().await {
            Ok(()) => {
                restart_attempts = 0;
                scheduler.record_success();
            }
            Err(e) => {
                scheduler.record_failure();

                if scheduler.should_open_circuit() {
                    eprintln!("circuit breaker opened for {:?}", e);

                    if restart_attempts < config.max_restart_attempts {
                        restart_attempts += 1;
                        tokio::time::sleep(std::time::Duration::from_secs(
                            config.restart_cooldown_secs,
                        ))
                        .await;

                        scheduler.recover().await?;
                    } else {
                        return Err(clawz_core::error::ClawzError::Orchestration(format!(
                            "max restart attempts ({}) exceeded",
                            config.max_restart_attempts
                        )));
                    }
                }
            }
        }
    }
}
