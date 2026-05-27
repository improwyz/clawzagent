//! Hardware abstraction module for the clawz-worker execution layer.
//!
//! This module provides hardware introspection, model loading, peripheral
//! device management, and UF2 firmware flashing capabilities. It sits in the
//! middle tier of the 3-tier architecture:
//!
//! - `gateway` (API layer) → `worker` (execution layer) → `core` (shared types/traits)
//!
//! Workers execute via the runtime module and rely on this hardware module to
//! discover available compute resources, load ML models onto appropriate devices,
//! manage attached peripherals, and flash firmware to microcontrollers.
//!
//! ## Submodules
//!
//! - `detect` — CPU/GPU/memory detection and VRAM-based model size recommendations
//! - `loader` — Model file format detection, metadata extraction, and registration
//! - `peripherals` — USB/serial/peripheral scanning and serial communication
//! - `uf2` — UF2 firmware parsing and flashing for embedded devices

pub mod detect;
pub mod loader;
pub mod peripherals;
pub mod uf2;

// Re-export the most commonly used types so callers can `use hardware::*`
// instead of drilling into submodules.

// ── detect re-exports ─────────────────────────────────────────────────────────

/// Compute API available on a GPU (CUDA, ROCm, Metal, Vulkan, or none).
pub use detect::ComputeApi;
/// CPU specifications: brand, core count, architecture, and feature flags.
pub use detect::CpuInfo;
/// GPU specifications: name, vendor, VRAM, driver, and compute capability.
pub use detect::GpuInfo;
/// Known GPU vendors: NVIDIA, AMD, Intel, Apple, or unknown.
pub use detect::GpuVendor;
/// Primary entry point for hardware detection. Probes CPU, GPU, memory, and OS.
pub use detect::HardwareDetector;
/// Aggregated hardware snapshot returned by [`HardwareDetector::detect_all`].
pub use detect::HardwareInfo;
/// System memory statistics: total, available, used, and swap.
pub use detect::MemoryInfo;
/// VRAM-based recommendation for maximum model size per quantization level.
pub use detect::ModelSizeRecommendation;
/// Operating system identification: name, version, kernel, and architecture.
pub use detect::OsInfo;

// ── loader re-exports ─────────────────────────────────────────────────────────

/// Metadata extracted from a GGUF file header (version, tensor count, KV pairs).
pub use loader::GgufMetadata;
/// A model that has been registered by the loader (ID + metadata + timestamp).
pub use loader::LoadedModel;
/// Known model file formats: GGUF, SafeTensors, PyTorch, ONNX, legacy GGML.
pub use loader::ModelFormat;
/// Rich metadata about a model file: size, parameters, quantization, architecture.
pub use loader::ModelInfo;
/// Entry point for model discovery, validation, and registration.
pub use loader::ModelLoader;
/// Quantization scheme and its approximate bytes-per-parameter cost.
pub use loader::Quantization;
/// Report from [`ModelLoader::validate_hardware_requirements`] comparing model
/// memory needs against available GPU VRAM or system RAM.
pub use loader::ValidationReport;

// ── peripherals re-exports ────────────────────────────────────────────────────

/// Single detected peripheral device with ID, type, connection state, and paths.
pub use peripherals::Peripheral;
/// Classification of a peripheral: USB, Serial, I2C, SPI, GPIO, Camera, etc.
pub use peripherals::PeripheralType;
/// Manager that scans, stores, and looks up connected peripherals.
pub use peripherals::Peripherals;
/// Blocking serial port connection with platform termios configuration.
pub use peripherals::SerialConnection;
/// Raw USB device information scraped from sysfs or system_profiler.
pub use peripherals::UsbDeviceInfo;

// ── uf2 re-exports ────────────────────────────────────────────────────────────

/// High-level interface for parsing and flashing UF2 firmware files.
pub use uf2::FirmwareFlasher;
/// Metadata extracted from a parsed UF2 file: target chip, block count, version.
pub use uf2::FirmwareInfo;
/// Result of verifying whether a flash operation completed successfully.
pub use uf2::FlashVerification;
/// A single 512-byte UF2 block parsed from a firmware image.
pub use uf2::Uf2Block;
/// Construct a valid UF2 block for testing or custom firmware generation.
pub use uf2::build_uf2_block;
/// Low-level parser: convert raw bytes into a validated list of [`Uf2Block`]s.
pub use uf2::parse_uf2;
