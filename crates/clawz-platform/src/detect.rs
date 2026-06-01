//! Platform detection

use crate::PlatformTier;

// The tier features (t0..t3) are not mutually exclusive in Cargo's feature
// model — `--all-features` (or any multi-tier combination) enables several at
// once. Select exactly one active definition via a priority order (higher tier
// wins) so the crate still compiles under `--all-features`, plus a fallback for
// when no tier feature is set at all.
#[cfg(feature = "t3")]
pub const fn detect_platform_auto() -> PlatformTier {
    PlatformTier::T3
}
#[cfg(all(feature = "t2", not(feature = "t3")))]
pub const fn detect_platform_auto() -> PlatformTier {
    PlatformTier::T2
}
#[cfg(all(feature = "t1", not(any(feature = "t2", feature = "t3"))))]
pub const fn detect_platform_auto() -> PlatformTier {
    PlatformTier::T1
}
#[cfg(all(
    feature = "t0",
    not(any(feature = "t1", feature = "t2", feature = "t3"))
))]
pub const fn detect_platform_auto() -> PlatformTier {
    PlatformTier::T0
}
#[cfg(not(any(feature = "t0", feature = "t1", feature = "t2", feature = "t3")))]
pub const fn detect_platform_auto() -> PlatformTier {
    PlatformTier::T1
}

/// Runtime detection for unknown platforms. Falls back to T1.
pub fn detect_platform_fallback() -> PlatformTier {
    if let Ok(tier) = std::env::var("CLAWZ_TIER") {
        match tier.as_str() {
            "T0" => return PlatformTier::T0,
            "T1" => return PlatformTier::T1,
            "T2" => return PlatformTier::T2,
            "T3" => return PlatformTier::T3,
            _ => {}
        }
    }
    if std::path::Path::new("/.dockerenv").exists() || std::env::var("DOCKER_CONTAINER").is_ok() {
        return PlatformTier::T2;
    }
    PlatformTier::T1
}
