use super::local_store::{self, LocalLicense};

#[derive(Debug, Clone)]
pub enum LicenseStatus {
    Valid {
        tier: String,
        days_remaining: Option<u32>,
    },
    Trial {
        days_remaining: u32,
    },
    GracePeriod {
        days_remaining: u32,
    },
    Expired,
    ClockTampered,
}

impl LicenseStatus {
    pub fn is_usable(&self) -> bool {
        matches!(
            self,
            LicenseStatus::Valid { .. }
                | LicenseStatus::Trial { .. }
                | LicenseStatus::GracePeriod { .. }
        )
    }
}

impl std::fmt::Display for LicenseStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LicenseStatus::Valid { tier, days_remaining } => {
                write!(f, "Licensed ({tier})")?;
                if let Some(d) = days_remaining {
                    write!(f, ", {d} days remaining")?;
                }
                Ok(())
            }
            LicenseStatus::Trial { days_remaining } => {
                write!(f, "Trial — {days_remaining} days remaining")
            }
            LicenseStatus::GracePeriod { days_remaining } => {
                write!(f, "License expired — {days_remaining} day grace period remaining")
            }
            LicenseStatus::Expired => write!(f, "License expired"),
            LicenseStatus::ClockTampered => {
                write!(f, "Clock manipulation detected — refusing to start")
            }
        }
    }
}

pub fn verify_or_trial() -> LicenseStatus {
    let mut license = match local_store::load() {
        Some(lic) => lic,
        None => {
            let lic = LocalLicense::new_trial();
            let _ = local_store::save(&lic);
            tracing::info!("ClawZ: Starting 30-day trial");
            return LicenseStatus::Trial { days_remaining: 30 };
        }
    };

    // Clock tamper check
    if license.is_clock_tampered() {
        return LicenseStatus::ClockTampered;
    }

    // Update usage tracking
    license.update_seen();
    let _ = local_store::save(&license);

    // Trial path
    if license.is_trial() {
        let remaining = license.trial_days_remaining();
        if remaining == 0 {
            return LicenseStatus::Expired;
        }
        return LicenseStatus::Trial {
            days_remaining: remaining,
        };
    }

    // Paid license path
    if license.is_expired() {
        if let Some(grace) = license.grace_days_remaining() {
            return LicenseStatus::GracePeriod {
                days_remaining: grace,
            };
        }
        return LicenseStatus::Expired;
    }

    let days_remaining = license.expires_at.map(|exp| {
        let secs = exp - chrono::Utc::now().timestamp();
        (secs / 86400).max(0) as u32
    });

    LicenseStatus::Valid {
        tier: license.tier.clone(),
        days_remaining,
    }
}

pub fn activate_key(key: &str) -> Result<LicenseStatus, String> {
    let mut license = local_store::load().unwrap_or_else(LocalLicense::new_trial);

    // In the future, this validates against clawz.net
    // For now, accept any non-empty key and set 1 year expiry
    if key.is_empty() {
        return Err("License key cannot be empty".to_string());
    }

    let now = chrono::Utc::now().timestamp();
    let one_year = 365 * 24 * 60 * 60;

    license.license_key = Some(key.to_string());
    license.activated_at = Some(now);
    license.expires_at = Some(now + one_year);
    license.tier = "pro".to_string();

    local_store::save(&license)?;

    tracing::info!("ClawZ: License activated — pro tier, 1 year");
    Ok(LicenseStatus::Valid {
        tier: "pro".to_string(),
        days_remaining: Some(365),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_creates_trial_on_first_run() {
        // Clean slate
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let path = std::path::PathBuf::from(&home)
            .join(".clawz")
            .join("license.json");
        let _ = std::fs::remove_file(&path);

        let status = verify_or_trial();
        assert!(status.is_usable());
        assert!(matches!(status, LicenseStatus::Trial { days_remaining: 30 }));
    }
}
