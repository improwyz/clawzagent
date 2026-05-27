//! Usage-based hybrid licensing system for ClawZ.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub mod driver;

pub use driver::{LicenseDriver, NoOpLicenseDriver};

// =============================================================================
// Error Types
// =============================================================================

#[derive(Debug, thiserror::Error)]
pub enum LicenseError {
    #[error("Account not found")]
    AccountNotFound,
    #[error("Subscription expired at {0}")]
    SubscriptionExpired(DateTime<Utc>),
    #[error("Quota exceeded for {resource:?}")]
    QuotaExceeded { resource: ResourceType },
    #[error("Feature '{feature}' not available on current plan")]
    FeatureNotAvailable { feature: String },
    #[error("Driver error: {0}")]
    DriverError(String),
    #[error("License revoked")]
    LicenseRevoked,
    #[error("Payment required — no active subscription")]
    NoActiveSubscription,
}

// =============================================================================
// Resource Types (Billing Meters)
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ResourceType {
    Messages = 0,
    AgentMinutes = 1,
    ToolInvocations = 2,
    ConcurrentAgents = 3,
    StorageGb = 4,
    MeshPeers = 5,
}

impl ResourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResourceType::Messages => "messages",
            ResourceType::AgentMinutes => "agent_minutes",
            ResourceType::ToolInvocations => "tool_invocations",
            ResourceType::ConcurrentAgents => "concurrent_agents",
            ResourceType::StorageGb => "storage_gb",
            ResourceType::MeshPeers => "mesh_peers",
        }
    }
}

// =============================================================================
// Usage Records
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaRemaining {
    pub resource: ResourceType,
    pub remaining: i64,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeteredUsage {
    pub resource: ResourceType,
    pub quantity: i64,
    pub timestamp: DateTime<Utc>,
    pub idempotency_key: String,
}

// =============================================================================
// Entitlement Scope
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntitlementScope {
    pub account_id: String,
    pub plan: String,
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub enabled_tiers: Vec<crate::PlatformTier>,
    #[serde(default)]
    pub enabled_features: Vec<String>,
    #[serde(default)]
    pub quotas: Vec<QuotaRemaining>,
}

impl EntitlementScope {
    pub fn can_spawn(&self, agents: usize) -> bool {
        self.quotas
            .iter()
            .find(|q| q.resource == ResourceType::ConcurrentAgents)
            .map(|q| q.remaining < 0 || q.remaining >= agents as i64)
            .unwrap_or(true)
    }

    pub fn has_feature(&self, feature: &str) -> bool {
        self.enabled_features.contains(&feature.to_string())
            || self.enabled_features.contains(&"*".to_string())
    }

    pub fn can_target(&self, tier: crate::PlatformTier) -> bool {
        self.enabled_tiers.contains(&tier) || self.enabled_tiers.is_empty()
    }

    pub fn quota(&self, resource: ResourceType) -> Option<i64> {
        self.quotas
            .iter()
            .find(|q| q.resource == resource)
            .map(|q| q.remaining)
    }

    pub fn is_active(&self) -> bool {
        self.expires_at.map(|exp| Utc::now() < exp).unwrap_or(true)
    }
}

// =============================================================================
// License Gate (Hard Gate)
// =============================================================================

pub struct LicenseGate {
    driver: Arc<dyn LicenseDriver<AccountId = String>>,
    account_id: String,
    scope: parking_lot::RwLock<Option<EntitlementScope>>,
}

impl LicenseGate {
    pub fn new(driver: Arc<dyn LicenseDriver<AccountId = String>>, account_id: String) -> Self {
        Self {
            driver,
            account_id,
            scope: parking_lot::RwLock::new(None),
        }
    }

    pub async fn verify(&self) -> Result<(), LicenseError> {
        let scope = self.driver.get_entitlements(&self.account_id).await?;
        *self.scope.write() = Some(scope);
        Ok(())
    }

    pub fn assert_export_allowed(&self) -> Result<(), LicenseError> {
        let scope = self.scope.read();
        let s = scope.as_ref().ok_or(LicenseError::LicenseRevoked)?;
        if let Some(expired) = s.expires_at {
            if Utc::now() > expired {
                return Err(LicenseError::SubscriptionExpired(expired));
            }
        }
        Ok(())
    }

    pub async fn refresh(&self) -> Result<(), LicenseError> {
        let new = self.driver.get_entitlements(&self.account_id).await?;
        *self.scope.write() = Some(new);
        Ok(())
    }

    pub fn scope(&self) -> parking_lot::RwLockReadGuard<'_, Option<EntitlementScope>> {
        self.scope.read()
    }
}

// =============================================================================
// Usage Tracker
// =============================================================================

pub struct UsageTracker {
    buckets: parking_lot::Mutex<std::collections::HashMap<ResourceType, i64>>,
    account_id: String,
    driver: Arc<dyn LicenseDriver<AccountId = String>>,
}

impl UsageTracker {
    pub fn new(account_id: String, driver: Arc<dyn LicenseDriver<AccountId = String>>) -> Self {
        Self {
            buckets: parking_lot::Mutex::new(std::collections::HashMap::new()),
            account_id,
            driver,
        }
    }

    pub fn record(&self, resource: ResourceType, _quantity: i64) {
        self.buckets.lock().entry(resource).or_insert(0);
    }

    pub async fn flush(&self) -> Result<(), LicenseError> {
        let metered: Vec<MeteredUsage> = {
            let mut b = self.buckets.lock();
            b.drain()
                .map(|(resource, quantity)| MeteredUsage {
                    resource,
                    quantity,
                    timestamp: Utc::now(),
                    idempotency_key: uuid::Uuid::new_v4().to_string(),
                })
                .collect()
        };
        if metered.is_empty() {
            return Ok(());
        }
        self.driver.record_usage(&self.account_id, &metered).await?;
        Ok(())
    }
}
