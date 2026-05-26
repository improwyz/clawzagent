//! Platform detection and tier classification for ClawZ.

mod detect;

pub use detect::{detect_platform_auto, detect_platform_fallback};

/// Platform tier — determines which features and backends are available.
/// Detected at compile time via Cargo feature flags, or at runtime via
/// `detect_platform_auto()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum PlatformTier {
    /// ESP32-S3, Arduino, or other bare-metal with no OS.
    /// No threading, no heap-allocated networking, no_std + embassy executor.
    T0 = 0,
    /// RISC-V SBC or bare-metal Linux (Raspberry Pi, BeagleBone, etc.)
    T1 = 1,
    /// Containerized deployment (Docker/Podman) on any architecture.
    T2 = 2,
    /// Full server or Tauri desktop/mobile.
    T3 = 3,
}

impl PlatformTier {
    pub fn name(&self) -> &'static str {
        match self {
            PlatformTier::T0 => "embedded (ESP32/bare-metal)",
            PlatformTier::T1 => "bare-metal SBC",
            PlatformTier::T2 => "containerized",
            PlatformTier::T3 => "server/desktop",
        }
    }

    pub fn binary_budget(&self) -> usize {
        match self {
            PlatformTier::T0 => 500 * 1024,
            PlatformTier::T1 => 5 * 1024 * 1024,
            PlatformTier::T2 => 15 * 1024 * 1024,
            PlatformTier::T3 => 30 * 1024 * 1024,
        }
    }

    pub fn ram_budget(&self) -> usize {
        match self {
            PlatformTier::T0 => 64 * 1024,
            PlatformTier::T1 => 20 * 1024 * 1024,
            PlatformTier::T2 => 64 * 1024 * 1024,
            PlatformTier::T3 => 256 * 1024 * 1024,
        }
    }

    pub fn supports_containers(&self) -> bool {
        matches!(self, PlatformTier::T2 | PlatformTier::T3)
    }

    pub fn is_server(&self) -> bool {
        matches!(self, PlatformTier::T2 | PlatformTier::T3)
    }
}

impl std::fmt::Display for PlatformTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::str::FromStr for PlatformTier {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "T0" => Ok(PlatformTier::T0),
            "T1" => Ok(PlatformTier::T1),
            "T2" => Ok(PlatformTier::T2),
            "T3" => Ok(PlatformTier::T3),
            _ => Err(format!("unknown platform tier: {}", s)),
        }
    }
}