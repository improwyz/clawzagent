//! LicenseDriver trait and NoOp implementation.

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEvent {
    pub event_type: String,
    pub account_id: String,
    pub payload: serde_json::Value,
}

#[async_trait]
pub trait LicenseDriver: Send + Sync {
    type AccountId: Send + Sync + std::fmt::Display;

    async fn get_entitlements(
        &self,
        account: &Self::AccountId,
    ) -> Result<EntitlementScope, LicenseError>;

    async fn check_quota(
        &self,
        account: &Self::AccountId,
        resource: ResourceType,
    ) -> Result<QuotaRemaining, LicenseError>;

    async fn record_usage(
        &self,
        account: &Self::AccountId,
        metered: &[MeteredUsage],
    ) -> Result<(), LicenseError>;

    async fn activate_subscription(
        &self,
        account: &Self::AccountId,
        plan: String,
    ) -> Result<(), LicenseError>;

    async fn deactivate_subscription(&self, account: &Self::AccountId) -> Result<(), LicenseError>;

    async fn validate_webhook_signature(&self, payload: &[u8], signature: &[u8]) -> bool;
}

pub struct NoOpLicenseDriver;

impl NoOpLicenseDriver {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoOpLicenseDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LicenseDriver for NoOpLicenseDriver {
    type AccountId = String;

    async fn get_entitlements(
        &self,
        _account: &Self::AccountId,
    ) -> Result<EntitlementScope, LicenseError> {
        Ok(EntitlementScope {
            account_id: "self-hosted".to_string(),
            plan: "unlimited".to_string(),
            expires_at: None,
            quotas: vec![
                QuotaRemaining {
                    resource: ResourceType::Messages,
                    remaining: -1,
                    resets_at: None,
                },
                QuotaRemaining {
                    resource: ResourceType::AgentMinutes,
                    remaining: -1,
                    resets_at: None,
                },
                QuotaRemaining {
                    resource: ResourceType::ToolInvocations,
                    remaining: -1,
                    resets_at: None,
                },
                QuotaRemaining {
                    resource: ResourceType::ConcurrentAgents,
                    remaining: -1,
                    resets_at: None,
                },
                QuotaRemaining {
                    resource: ResourceType::StorageGb,
                    remaining: -1,
                    resets_at: None,
                },
                QuotaRemaining {
                    resource: ResourceType::MeshPeers,
                    remaining: -1,
                    resets_at: None,
                },
            ],
            enabled_tiers: vec![
                crate::PlatformTier::T0,
                crate::PlatformTier::T1,
                crate::PlatformTier::T2,
                crate::PlatformTier::T3,
            ],
            enabled_features: vec!["*".to_string()],
        })
    }

    async fn check_quota(
        &self,
        _account: &Self::AccountId,
        resource: ResourceType,
    ) -> Result<QuotaRemaining, LicenseError> {
        Ok(QuotaRemaining {
            resource,
            remaining: -1,
            resets_at: None,
        })
    }

    async fn record_usage(
        &self,
        _account: &Self::AccountId,
        _metered: &[MeteredUsage],
    ) -> Result<(), LicenseError> {
        Ok(())
    }

    async fn activate_subscription(
        &self,
        _account: &Self::AccountId,
        _plan: String,
    ) -> Result<(), LicenseError> {
        Ok(())
    }

    async fn deactivate_subscription(
        &self,
        _account: &Self::AccountId,
    ) -> Result<(), LicenseError> {
        Ok(())
    }

    async fn validate_webhook_signature(&self, _payload: &[u8], _signature: &[u8]) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_driver_returns_unlimited() {
        let driver = NoOpLicenseDriver::new();
        let scope = driver
            .get_entitlements(&"any-account".to_string())
            .await
            .unwrap();
        assert!(scope.is_active());
        assert!(scope.can_spawn(999));
        assert!(scope.has_feature("any-feature"));
    }
}
