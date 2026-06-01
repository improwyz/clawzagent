use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::hwid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalLicense {
    pub install_ts: i64,
    pub last_seen_ts: i64,
    pub days_used: u32,
    pub license_key: Option<String>,
    pub hw_fingerprint: String,
    pub activated_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub tier: String,
}

impl LocalLicense {
    pub fn new_trial() -> Self {
        let now = Utc::now().timestamp();
        Self {
            install_ts: now,
            last_seen_ts: now,
            days_used: 0,
            license_key: None,
            hw_fingerprint: hwid::hardware_fingerprint(),
            activated_at: None,
            expires_at: None,
            tier: "trial".to_string(),
        }
    }

    pub fn is_trial(&self) -> bool {
        self.license_key.is_none()
    }

    pub fn trial_days_remaining(&self) -> u32 {
        30u32.saturating_sub(self.days_used)
    }

    pub fn is_expired(&self) -> bool {
        if self.is_trial() {
            return self.days_used >= 30;
        }
        if let Some(exp) = self.expires_at {
            return Utc::now().timestamp() > exp;
        }
        false
    }

    pub fn grace_days_remaining(&self) -> Option<u32> {
        if self.is_trial() || !self.is_expired() {
            return None;
        }
        if let Some(exp) = self.expires_at {
            let days_past = ((Utc::now().timestamp() - exp) / 86400) as u32;
            if days_past < 7 {
                return Some(7 - days_past);
            }
        }
        None
    }

    pub fn is_clock_tampered(&self) -> bool {
        Utc::now().timestamp() < self.last_seen_ts - 300 // 5 min tolerance
    }

    pub fn update_seen(&mut self) {
        let now = Utc::now().timestamp();
        let today = Utc::now().date_naive();
        let last_date = DateTime::from_timestamp(self.last_seen_ts, 0)
            .map(|dt| dt.date_naive())
            .unwrap_or(today);

        if today > last_date {
            self.days_used += 1;
        }
        self.last_seen_ts = now;
    }
}

fn license_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".clawz").join("license.json")
}

pub fn load() -> Option<LocalLicense> {
    let path = license_path();
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn save(license: &LocalLicense) -> Result<(), String> {
    let path = license_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(license).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("write: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_trial_has_30_days() {
        let lic = LocalLicense::new_trial();
        assert!(lic.is_trial());
        assert_eq!(lic.trial_days_remaining(), 30);
        assert!(!lic.is_expired());
    }

    #[test]
    fn expired_trial() {
        let mut lic = LocalLicense::new_trial();
        lic.days_used = 30;
        assert!(lic.is_expired());
        assert_eq!(lic.trial_days_remaining(), 0);
    }
}
