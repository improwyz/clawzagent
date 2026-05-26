//! Platform detection

use super::PlatformTier;

#[cfg(feature = "t0")]
pub const fn detect_platform_auto() -> PlatformTier { PlatformTier::T0 }
#[cfg(feature = "t1")]
pub const fn detect_platform_auto() -> PlatformTier { PlatformTier::T1 }
#[cfg(feature = "t2")]
pub const fn detect_platform_auto() -> PlatformTier { PlatformTier::T2 }
#[cfg(feature = "t3")]
pub const fn detect_platform_auto() -> PlatformTier { PlatformTier::T3 }

/// Runtime detection for unknown platforms. Falls back to T3.
pub fn detect_platform_fallback() -> PlatformTier {
    if let Ok(tier) = std::env::var("CLAWZ_TIER") {
        if let Ok(t) = tier.parse() {
            return t;
        }
    }
    if std::path::Path::new("/.dockerenv").exists()
        || std::env::var("DOCKER_CONTAINER").is_ok()
    {
        return PlatformTier::T2;
    }
    PlatformTier::T1
}