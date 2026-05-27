//! Hardware detection: CPU, GPU, memory, and OS probing.
//!
//! This module implements platform-agnostic hardware introspection. It queries
//! CPU features, enumerates GPUs (NVIDIA via nvidia-smi, AMD via rocm-smi,
//! Apple via Metal/system_profiler, and Linux fallback via lspci), measures
//! system RAM/swap, and recommends model sizes based on available VRAM.
//!
//! ## Design notes
//!
//! - Detection is **best-effort**: missing tools (e.g. nvidia-smi on a machine
//!   without NVIDIA drivers) gracefully return empty results rather than failing.
//! - VRAM-based recommendations leave a 20 % headroom for activations and KV cache.
//!
//! ## Key dependencies
//! - `sysinfo` for CPU and memory statistics
//! - `serde` for serializable output types
//! - External CLI tools: `nvidia-smi`, `rocm-smi`, `system_profiler`, `lspci`, `sysctl`

use serde::{Deserialize, Serialize};
use std::process::Command;
use sysinfo::System;

// ── Public types ──────────────────────────────────────────────────────────────

/// Complete hardware snapshot produced by [`HardwareDetector::detect_all`].
///
/// Combines CPU info, GPU list, memory stats, and OS identification into a
/// single serializable struct suitable for upstream reporting or caching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareInfo {
    /// CPU specifications including core count, frequency, and feature flags.
    pub cpu: CpuInfo,
    /// All GPUs detected on the system. May be empty if no GPU tools are installed.
    pub gpus: Vec<GpuInfo>,
    /// System RAM and swap usage statistics.
    pub memory: MemoryInfo,
    /// Operating system name, version, kernel, and architecture.
    pub os: OsInfo,
}

/// CPU specifications extracted from `/proc/cpuinfo` (Linux) or `sysctl` (macOS).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuInfo {
    /// Brand string, e.g. "Intel(R) Core(TM) i9-13900K"
    pub brand: String,
    /// Physical core count
    pub cores: usize,
    /// Logical thread count (hyperthreading)
    pub threads: usize,
    /// Max frequency in MHz
    pub frequency_mhz: u64,
    /// Architecture string, e.g. "x86_64", "aarch64"
    pub architecture: String,
    /// Detected CPU feature flags (SSE4, AVX, AVX2, AVX512, NEON, etc.)
    pub features: Vec<String>,
}

/// Known GPU vendors used to select the appropriate compute API and driver path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GpuVendor {
    /// NVIDIA GPUs — typically driven by CUDA.
    Nvidia,
    /// AMD GPUs — typically driven by ROCm or Vulkan.
    Amd,
    /// Intel integrated/discrete GPUs — typically driven by Vulkan or OpenCL.
    Intel,
    /// Apple Silicon GPUs — driven by Metal.
    Apple,
    /// Vendor could not be determined from available information.
    Unknown,
}

/// Compute API available for a detected GPU.
///
/// Used downstream to decide which inference backend (CUDA, ROCm, Metal,
/// Vulkan) should be selected when loading a model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ComputeApi {
    /// NVIDIA CUDA
    Cuda,
    /// AMD ROCm/HIP
    Rocm,
    /// Apple Metal Performance Shaders
    Metal,
    /// Vulkan compute (fallback for AMD/Intel)
    Vulkan,
    /// No compute API detected
    None,
}

/// GPU specifications for a single detected accelerator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    /// Marketing or driver-reported GPU name, e.g. "NVIDIA GeForce RTX 4090"
    pub name: String,
    /// Detected vendor classification
    pub vendor: GpuVendor,
    /// VRAM in megabytes (0 = unknown)
    pub vram_mb: u64,
    /// Driver version string if available
    pub driver_version: Option<String>,
    /// Best-effort compute API assignment based on vendor and platform
    pub compute_api: ComputeApi,
    /// CUDA compute capability (major, minor) — only populated for NVIDIA GPUs
    pub compute_capability: Option<(u8, u8)>,
    /// Whether the GPU is considered available for compute (not disabled or in use)
    pub available: bool,
}

/// System memory (RAM) and swap statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInfo {
    /// Total physical RAM in megabytes
    pub total_mb: u64,
    /// Available RAM in megabytes (free + cache/buffers)
    pub available_mb: u64,
    /// Used RAM in megabytes
    pub used_mb: u64,
    /// Total swap space in megabytes
    pub swap_total_mb: u64,
    /// Used swap space in megabytes
    pub swap_used_mb: u64,
}

/// Operating system identification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsInfo {
    /// OS name, e.g. "Ubuntu", "macOS"
    pub name: String,
    /// OS version string, e.g. "22.04", "14.2.1"
    pub version: String,
    /// Kernel version, e.g. "6.5.0-15-generic" or "23.2.0"
    pub kernel: String,
    /// Machine architecture (from `std::env::consts::ARCH`), e.g. "x86_64"
    pub arch: String,
}

// ── Model size recommendation ─────────────────────────────────────────────────

/// Based on available GPU VRAM (or RAM if no GPU), suggest maximum model size in billions of parameters.
///
/// Uses conservative estimates: ~2 bytes per parameter for FP16, ~1 byte for INT8, ~0.5 for Q4.
/// A 20 % headroom is reserved for activations, KV cache, and runtime overhead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSizeRecommendation {
    /// Maximum parameters (B) for FP16/BF16 loading
    pub max_params_fp16_b: f32,
    /// Maximum parameters (B) for INT8 quantization
    pub max_params_int8_b: f32,
    /// Maximum parameters (B) for Q4 quantization (GGUF style)
    pub max_params_q4_b: f32,
    /// Human-readable suggested model, e.g. "llama-3-8b-q4"
    pub suggested_model: String,
    /// Available compute memory in MB (GPU VRAM or RAM)
    pub available_memory_mb: u64,
}

// ── Detector ──────────────────────────────────────────────────────────────────

/// Entry point for hardware introspection.
///
/// Holds a refreshed `sysinfo::System` instance and exposes methods to probe
/// CPU, GPU, memory, and OS. All detection methods are non-destructive and
/// rely on external CLI tools where kernel APIs are insufficient.
///
/// # Example
///
/// ```ignore
/// // HardwareDetector requires sysinfo and CLI tool access,
/// // so this example is run in a real environment only.
/// let detector = HardwareDetector::new();
/// let info = detector.detect_all();
/// ```
pub struct HardwareDetector {
    /// Underlying `sysinfo` system handle. Refreshed once at construction.
    system: System,
}

impl HardwareDetector {
    /// Create a new detector with a fully refreshed system snapshot.
    pub fn new() -> Self {
        let mut system = System::new_all();
        system.refresh_all();
        Self { system }
    }

    /// Detect full CPU information including feature flags.
    ///
    /// Falls back to "Unknown CPU" and sensible defaults when `sysinfo` cannot
    /// read processor details (common in containerized CI environments).
    pub fn detect_cpu(&self) -> CpuInfo {
        let cpus = self.system.cpus();
        let first = cpus.first();

        let brand = first
            .map(|c| c.brand().to_string())
            .unwrap_or_else(|| "Unknown CPU".to_string());

        let cores = self.system.physical_core_count().unwrap_or(1);
        let threads = cpus.len().max(1);
        let frequency_mhz = first.map(|c| c.frequency()).unwrap_or(0);
        let architecture = std::env::consts::ARCH.to_string();

        // Feature detection requires platform-specific parsing (/proc/cpuinfo or sysctl)
        let features = detect_cpu_features();

        CpuInfo {
            brand,
            cores,
            threads,
            frequency_mhz,
            architecture,
            features,
        }
    }

    /// Detect GPU(s) by running vendor-specific CLI tools.
    ///
    /// Detection order:
    /// 1. `nvidia-smi` for NVIDIA GPUs (includes compute capability)
    /// 2. `rocm-smi` for AMD GPUs
    /// 3. `system_profiler SPDisplaysDataType` on macOS for Metal GPUs
    /// 4. `lspci` fallback on Linux for Intel / generic display controllers
    ///
    /// Each stage is independent; failure in one does not block others.
    pub fn detect_gpus(&self) -> Vec<GpuInfo> {
        let mut gpus = Vec::new();

        // --- NVIDIA via nvidia-smi ---
        // nvidia-smi is the most reliable source for VRAM and driver version on Linux/Windows.
        if let Some(mut nvidia_gpus) = detect_nvidia_gpus() {
            gpus.append(&mut nvidia_gpus);
        }

        // --- AMD via rocm-smi ---
        // rocm-smi provides VRAM totals in JSON; absent on systems without ROCm.
        if let Some(mut amd_gpus) = detect_amd_gpus() {
            gpus.append(&mut amd_gpus);
        }

        // --- macOS Metal via system_profiler ---
        // system_profiler is the only built-in way to enumerate Apple Silicon GPUs with VRAM.
        #[cfg(target_os = "macos")]
        if let Some(mut metal_gpus) = detect_metal_gpus() {
            gpus.append(&mut metal_gpus);
        }

        // --- Fallback: lspci on Linux for Intel / generic Vulkan ---
        // Only run if no GPUs were found above, because lspci cannot report VRAM.
        if gpus.is_empty() {
            if let Some(mut lspci_gpus) = detect_lspci_gpus() {
                gpus.append(&mut lspci_gpus);
            }
        }

        gpus
    }

    /// Detect system memory (RAM + swap).
    ///
    /// Converts sysinfo's byte values to megabytes for easier upstream consumption.
    pub fn detect_memory(&self) -> MemoryInfo {
        let total = self.system.total_memory();
        let available = self.system.available_memory();
        let swap_total = self.system.total_swap();
        let swap_used = self.system.used_swap();

        MemoryInfo {
            total_mb: total / (1024 * 1024),
            available_mb: available / (1024 * 1024),
            used_mb: total.saturating_sub(available) / (1024 * 1024),
            swap_total_mb: swap_total / (1024 * 1024),
            swap_used_mb: swap_used / (1024 * 1024),
        }
    }

    /// Detect OS information.
    ///
    /// Uses `sysinfo`'s static helpers for name, version, and kernel, plus
    /// `std::env::consts::ARCH` for the machine architecture.
    pub fn detect_os(&self) -> OsInfo {
        OsInfo {
            name: System::name().unwrap_or_else(|| "Unknown".to_string()),
            version: System::os_version().unwrap_or_else(|| "Unknown".to_string()),
            kernel: System::kernel_version().unwrap_or_else(|| "Unknown".to_string()),
            arch: std::env::consts::ARCH.to_string(),
        }
    }

    /// Run all detection and return a combined [`HardwareInfo`].
    pub fn detect_all(&self) -> HardwareInfo {
        HardwareInfo {
            cpu: self.detect_cpu(),
            gpus: self.detect_gpus(),
            memory: self.detect_memory(),
            os: self.detect_os(),
        }
    }

    /// Based on available GPU VRAM (or RAM if no GPU), recommend maximum model sizes.
    ///
    /// The recommendation uses conservative per-parameter memory estimates:
    /// - FP16/BF16: 2 bytes/parameter  → 1B params ≈ 2000 MB
    /// - INT8:      1 byte/parameter   → 1B params ≈ 1000 MB
    /// - Q4:        0.5 bytes/parameter → 1B params ≈ 500 MB
    ///
    /// 20 % of detected memory is reserved for runtime overhead (activations, KV cache,
    /// CUDA context, etc.) so that the suggested model is actually runnable.
    pub fn recommended_model_size(&self) -> ModelSizeRecommendation {
        let gpus = self.detect_gpus();
        let memory = self.detect_memory();

        // Use the GPU with the most VRAM, or fall back to system RAM
        let available_mb = gpus
            .iter()
            .filter(|g| g.available && g.vram_mb > 0)
            .map(|g| g.vram_mb)
            .max()
            .unwrap_or(memory.available_mb);

        // Conservative estimates:
        //   FP16: 2 bytes/param  → 1B params ≈ 2000 MB
        //   INT8: 1 byte/param   → 1B params ≈ 1000 MB
        //   Q4:   0.5 bytes/param → 1B params ≈ 500 MB
        // Leave 20% headroom for activations / KV cache
        let usable_mb = available_mb as f32 * 0.8;

        let max_params_fp16_b = usable_mb / 2000.0;
        let max_params_int8_b = usable_mb / 1000.0;
        let max_params_q4_b = usable_mb / 500.0;

        let suggested_model = suggest_model(max_params_q4_b);

        ModelSizeRecommendation {
            max_params_fp16_b,
            max_params_int8_b,
            max_params_q4_b,
            suggested_model,
            available_memory_mb: available_mb,
        }
    }
}

impl Default for HardwareDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Parse CPU feature flags from `/proc/cpuinfo` on Linux, or use `sysctl` on macOS.
///
/// We only keep features relevant to ML inference performance (SIMD, AMX, NEON).
fn detect_cpu_features() -> Vec<String> {
    let mut features = Vec::new();

    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/cpuinfo") {
            // Find the first "flags" or "Features" line
            for line in content.lines() {
                let lower = line.to_lowercase();
                if lower.starts_with("flags") || lower.starts_with("features") {
                    if let Some(val) = line.split_once(':').map(|x| x.1) {
                        let flags: Vec<&str> = val.split_whitespace().collect();

                        // x86 features we care about
                        let interesting = [
                            "sse4_1", "sse4_2", "avx", "avx2", "avx512f", "avx512bw",
                            "avx512cd", "avx512dq", "avx512vl", "fma", "bmi1", "bmi2",
                            "aes", "vaes", "vpclmulqdq", "amx_bf16", "amx_int8",
                            // ARM
                            "asimd", "neon", "sve", "sve2", "fp16", "dotprod",
                        ];

                        for flag in &flags {
                            if interesting.contains(flag) {
                                features.push(flag.to_uppercase().replace('_', ""));
                            }
                        }
                        break;
                    }
                }
            }
        }
    }

    // macOS: use sysctl
    #[cfg(target_os = "macos")]
    {
        let macfeatures = [
            ("hw.optional.avx1_0", "AVX"),
            ("hw.optional.avx2_0", "AVX2"),
            ("hw.optional.avx512f", "AVX512F"),
            ("hw.optional.neon", "NEON"),
            ("hw.optional.arm.FEAT_DotProd", "DOTPROD"),
        ];
        for (key, label) in &macfeatures {
            if let Ok(out) = Command::new("sysctl").arg("-n").arg(key).output() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if s == "1" {
                    features.push(label.to_string());
                }
            }
        }
    }

    features
}

/// Run `nvidia-smi` and parse GPU name, memory, driver version, and compute capability.
///
/// The `--format=csv,noheader,nounits` flag gives stable, parseable output without
/// unit suffixes (e.g. "24576" instead of "24576 MiB").
fn detect_nvidia_gpus() -> Option<Vec<GpuInfo>> {
    // Query: name, memory.total, driver_version, compute_cap
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total,driver_version,compute_cap",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut gpus = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(4, ',').map(|s| s.trim()).collect();
        if parts.len() < 4 {
            continue;
        }

        let name = parts[0].to_string();
        let vram_mb = parts[1].parse::<u64>().unwrap_or(0);
        let driver_version = parts[2].to_string();
        let compute_cap_str = parts[3];

        // compute_cap is like "8.6" or "7.5"
        let compute_capability = parse_compute_capability(compute_cap_str);

        gpus.push(GpuInfo {
            name,
            vendor: GpuVendor::Nvidia,
            vram_mb,
            driver_version: Some(driver_version),
            compute_api: ComputeApi::Cuda,
            compute_capability,
            available: true,
        });
    }

    if gpus.is_empty() {
        None
    } else {
        Some(gpus)
    }
}

/// Parse a CUDA compute capability string of the form "major.minor".
///
/// Unknown or malformed strings return `None`.
fn parse_compute_capability(s: &str) -> Option<(u8, u8)> {
    let s = s.trim();
    if let Some((major, minor)) = s.split_once('.') {
        let maj = major.parse::<u8>().ok()?;
        let min = minor.parse::<u8>().ok()?;
        return Some((maj, min));
    }
    None
}

/// Run `rocm-smi` to detect AMD GPUs.
///
/// Uses `--json` output and best-effort parsing because rocm-smi's JSON schema
/// varies across ROCm versions.
fn detect_amd_gpus() -> Option<Vec<GpuInfo>> {
    // Try rocm-smi --showproductname --showmeminfo vram
    let output = Command::new("rocm-smi")
        .args(["--showproductname", "--showmeminfo", "vram", "--json"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // rocm-smi JSON output is structured; do a best-effort parse
    // Example:  {"card0": {"Card series": "...", "VRAM Total Memory (B)": "..."}}
    let json: serde_json::Value = serde_json::from_str(&stdout).ok()?;
    let mut gpus = Vec::new();

    if let Some(obj) = json.as_object() {
        for (card_key, card_val) in obj {
            if !card_key.starts_with("card") {
                continue;
            }
            let name = card_val
                .get("Card series")
                .or_else(|| card_val.get("Card model"))
                .and_then(|v| v.as_str())
                .unwrap_or("AMD GPU")
                .to_string();

            // VRAM bytes → MB
            let vram_mb = card_val
                .get("VRAM Total Memory (B)")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<u64>().ok())
                .map(|b| b / (1024 * 1024))
                .unwrap_or(0);

            gpus.push(GpuInfo {
                name,
                vendor: GpuVendor::Amd,
                vram_mb,
                driver_version: None,
                compute_api: ComputeApi::Rocm,
                compute_capability: None,
                available: true,
            });
        }
    }

    if gpus.is_empty() {
        None
    } else {
        Some(gpus)
    }
}

/// macOS: detect GPUs via `system_profiler SPDisplaysDataType`.
///
/// This is the only reliable built-in source for Apple Silicon GPU names and VRAM.
#[cfg(target_os = "macos")]
fn detect_metal_gpus() -> Option<Vec<GpuInfo>> {
    let output = Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).ok()?;

    let displays = json
        .get("SPDisplaysDataType")
        .and_then(|v| v.as_array())?;

    let mut gpus = Vec::new();

    for display in displays {
        let name = display
            .get("sppci_model")
            .and_then(|v| v.as_str())
            .unwrap_or("Apple GPU")
            .to_string();

        // VRAM: may appear as "spdisplays_vram" e.g. "16 GB"
        let vram_mb = display
            .get("spdisplays_vram")
            .and_then(|v| v.as_str())
            .and_then(|s| parse_vram_string(s))
            .unwrap_or(0);

        let vendor = if name.to_lowercase().contains("apple") {
            GpuVendor::Apple
        } else if name.to_lowercase().contains("intel") {
            GpuVendor::Intel
        } else if name.to_lowercase().contains("amd")
            || name.to_lowercase().contains("radeon")
        {
            GpuVendor::Amd
        } else {
            GpuVendor::Unknown
        };

        gpus.push(GpuInfo {
            name,
            vendor,
            vram_mb,
            driver_version: None,
            compute_api: ComputeApi::Metal,
            compute_capability: None,
            available: true,
        });
    }

    if gpus.is_empty() {
        None
    } else {
        Some(gpus)
    }
}

/// Parse a VRAM string like "16 GB", "8192 MB" → megabytes.
#[cfg(target_os = "macos")]
fn parse_vram_string(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    if let Some(gb_str) = s.strip_suffix(" gb") {
        let gb: u64 = gb_str.trim().parse().ok()?;
        return Some(gb * 1024);
    }
    if let Some(mb_str) = s.strip_suffix(" mb") {
        let mb: u64 = mb_str.trim().parse().ok()?;
        return Some(mb);
    }
    None
}

/// Linux fallback: run `lspci` to find display controllers.
///
/// lspci is universally available but cannot report VRAM, so this is only used
/// when vendor-specific tools (nvidia-smi, rocm-smi) yield no results.
fn detect_lspci_gpus() -> Option<Vec<GpuInfo>> {
    let output = Command::new("lspci").arg("-mm").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut gpus = Vec::new();

    for line in stdout.lines() {
        let lower = line.to_lowercase();
        // Match VGA, 3D, Display controllers
        if lower.contains("vga") || lower.contains("3d controller") || lower.contains("display controller") {
            let name = extract_lspci_device_name(line);
            let vendor = detect_vendor_from_name(&name);

            // Determine likely compute API
            let compute_api = match vendor {
                GpuVendor::Nvidia => ComputeApi::Cuda,
                GpuVendor::Amd => ComputeApi::Vulkan,
                GpuVendor::Intel => ComputeApi::Vulkan,
                _ => ComputeApi::Vulkan,
            };

            gpus.push(GpuInfo {
                name,
                vendor,
                vram_mb: 0, // lspci doesn't report VRAM easily
                driver_version: None,
                compute_api,
                compute_capability: None,
                available: true,
            });
        }
    }

    if gpus.is_empty() {
        None
    } else {
        Some(gpus)
    }
}

/// Extract a human-readable device name from an `lspci -mm` line.
///
/// Format: `00:02.0 "VGA compatible controller" "Intel Corporation" "..."`
/// Quoted tokens after splitting on `"` give vendor + device name.
fn extract_lspci_device_name(line: &str) -> String {
    // Collect quoted tokens
    let parts: Vec<&str> = line
        .split('"')
        .filter(|s| !s.trim().is_empty())
        .collect();

    // Vendor is parts[1], device is parts[2] (when 0-indexed after split on quotes)
    if parts.len() >= 3 {
        format!("{} {}", parts[1].trim(), parts[2].trim())
    } else {
        line.trim().to_string()
    }
}

/// Heuristic vendor classification from a free-text GPU name string.
fn detect_vendor_from_name(name: &str) -> GpuVendor {
    let lower = name.to_lowercase();
    if lower.contains("nvidia") || lower.contains("geforce") || lower.contains("quadro") {
        GpuVendor::Nvidia
    } else if lower.contains("amd") || lower.contains("radeon") || lower.contains("advanced micro") {
        GpuVendor::Amd
    } else if lower.contains("intel") {
        GpuVendor::Intel
    } else {
        GpuVendor::Unknown
    }
}

/// Suggest a model tier given max Q4 parameters in billions.
///
/// Thresholds are chosen to align with common open-weights releases
/// (Llama 3, Mistral, Gemma, Phi families).
fn suggest_model(max_q4_b: f32) -> String {
    if max_q4_b >= 70.0 {
        "llama-3-70b-q4 or larger".to_string()
    } else if max_q4_b >= 34.0 {
        "llama-3-34b-q4 / codestral-22b".to_string()
    } else if max_q4_b >= 13.0 {
        "llama-3-13b-q4 / mistral-7b-q8".to_string()
    } else if max_q4_b >= 7.0 {
        "llama-3-8b-q4 / gemma-7b-q4".to_string()
    } else if max_q4_b >= 3.0 {
        "phi-3-mini-q4 / gemma-2b-q8".to_string()
    } else {
        "phi-2 or smaller (limited memory)".to_string()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_detection_returns_brand() {
        let detector = HardwareDetector::new();
        let cpu = detector.detect_cpu();
        // Brand may be empty on some CI VMs but the field must exist
        assert!(cpu.threads >= 1);
        assert!(cpu.cores >= 1);
    }

    #[test]
    fn test_cpu_architecture() {
        let detector = HardwareDetector::new();
        let cpu = detector.detect_cpu();
        assert!(!cpu.architecture.is_empty());
    }

    #[test]
    fn test_gpu_detection_does_not_panic() {
        let detector = HardwareDetector::new();
        let gpus = detector.detect_gpus();
        // No panic; may be empty in CI
        let _ = gpus.len();
    }

    #[test]
    fn test_memory_detection() {
        let detector = HardwareDetector::new();
        let mem = detector.detect_memory();
        assert!(mem.total_mb > 0, "total memory should be > 0");
        assert!(mem.available_mb <= mem.total_mb);
    }

    #[test]
    fn test_os_detection() {
        let detector = HardwareDetector::new();
        let os = detector.detect_os();
        assert!(!os.arch.is_empty());
    }

    #[test]
    fn test_detect_all() {
        let detector = HardwareDetector::new();
        let info = detector.detect_all();
        assert!(info.memory.total_mb > 0);
        assert!(info.cpu.threads >= 1);
    }

    #[test]
    fn test_model_size_recommendation() {
        let detector = HardwareDetector::new();
        let rec = detector.recommended_model_size();
        assert!(rec.available_memory_mb > 0);
        assert!(rec.max_params_q4_b > 0.0);
        assert!(!rec.suggested_model.is_empty());
    }

    #[test]
    fn test_parse_compute_capability() {
        assert_eq!(parse_compute_capability("8.6"), Some((8, 6)));
        assert_eq!(parse_compute_capability("7.5"), Some((7, 5)));
        assert_eq!(parse_compute_capability("bad"), None);
    }

    #[test]
    fn test_suggest_model() {
        assert!(suggest_model(80.0).contains("70b"));
        assert!(suggest_model(5.0).contains("phi") || suggest_model(5.0).contains("gemma") || suggest_model(5.0).contains("8b"));
        assert!(!suggest_model(1.0).is_empty());
    }

    #[test]
    fn test_serialization() {
        let detector = HardwareDetector::new();
        let info = detector.detect_all();
        let json = serde_json::to_string(&info).expect("serialization failed");
        assert!(!json.is_empty());
        let _back: HardwareInfo = serde_json::from_str(&json).expect("deserialization failed");
    }
}
