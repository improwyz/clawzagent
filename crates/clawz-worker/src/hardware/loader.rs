//! Model loading: file format detection, metadata extraction, and hardware validation.
//!
//! This module implements a lightweight model registry. It detects file formats
//! by magic bytes, reads GGUF headers to extract architecture and quantization,
//! estimates parameter counts from filenames, and validates whether the local
//! hardware (GPU VRAM or RAM) can accommodate a given model.
//!
//! ## Design notes
//!
//! - `ModelLoader` does **not** load weights into memory; it only registers
//!   models by reading headers and computing metadata.
//! - GGUF is the best-supported format because it embeds rich KV metadata.
//! - Other formats (SafeTensors, PyTorch, ONNX) rely on filename heuristics.
//!
//! ## Cross-module dependencies
//! - `HardwareInfo` from `detect.rs` is required for [`ModelLoader::validate_hardware_requirements`].
//! - `ClawzError` from `clawz_core` is used for all error propagation.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use clawz_core::error::{ClawzError, Result};

// Dependency: detect::HardwareInfo for memory-requirement validation
use super::detect::HardwareInfo;

// ── Model format ──────────────────────────────────────────────────────────────

/// Known model file formats.
///
/// Formats are detected primarily by magic bytes in the file header, with
/// extension fallback for ambiguous cases (e.g. SafeTensors has no unique magic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelFormat {
    /// GGUF — the modern llama.cpp format (magic: 0x46475547 = "GGUF")
    GGUF,
    /// SafeTensors — Hugging Face's safe serialisation format
    SafeTensors,
    /// PyTorch pickle format (.pt / .pth)
    PyTorch,
    /// ONNX ML exchange format
    ONNX,
    /// Legacy GGML format (before GGUF)
    GGML,
}

impl ModelFormat {
    /// Detect format from file extension.
    ///
    /// Used as a fallback when header magic detection is ambiguous.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "gguf" => Some(ModelFormat::GGUF),
            "safetensors" => Some(ModelFormat::SafeTensors),
            "pt" | "pth" => Some(ModelFormat::PyTorch),
            "onnx" => Some(ModelFormat::ONNX),
            "bin" | "ggml" => Some(ModelFormat::GGML),
            _ => None,
        }
    }

    /// Return a human-readable name.
    pub fn display_name(&self) -> &'static str {
        match self {
            ModelFormat::GGUF => "GGUF",
            ModelFormat::SafeTensors => "SafeTensors",
            ModelFormat::PyTorch => "PyTorch",
            ModelFormat::ONNX => "ONNX",
            ModelFormat::GGML => "GGML (legacy)",
        }
    }
}

// ── GGUF header types ─────────────────────────────────────────────────────────

/// Magic number for GGUF files: bytes "GGUF" in little-endian = 0x46475547
const GGUF_MAGIC: u32 = 0x46475547;
/// Magic number for GGML legacy files (first 4 bytes = 0x67676d6c = "ggml" or 0x67676a74)
const GGML_MAGIC: u32 = 0x67676d6c;
/// ONNX protobuf magic (first 2 bytes of a protobuf file vary; check extension + attempt parse)
#[allow(dead_code)]
const ONNX_MAGIC_BYTES: &[u8] = b"\x08\x07"; // common ONNX pb field

/// Metadata read from a GGUF file header.
///
/// GGUF stores key–value metadata (architecture, context length, parameter count,
/// quantization type, etc.) after a fixed-size header. This struct captures the
/// parsed KV map along with tensor and version counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GgufMetadata {
    /// GGUF specification version (1–3)
    pub version: u32,
    /// Number of tensors declared in the file
    pub tensor_count: u64,
    /// Number of key–value metadata entries
    pub kv_count: u64,
    /// Extracted key-value metadata as strings
    pub metadata: HashMap<String, String>,
}

/// Quantization type parsed from model metadata or filename.
///
/// Each variant carries an approximate bytes-per-parameter cost used to estimate
/// memory requirements.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(non_camel_case_types)]
pub enum Quantization {
    /// Full-precision 32-bit float (4 bytes/param)
    F32,
    /// Half-precision 16-bit float (2 bytes/param)
    F16,
    /// BFloat16 (2 bytes/param)
    BF16,
    /// 8-bit quantization (1 byte/param)
    Q8_0,
    /// 6-bit K-quant (≈0.75 bytes/param)
    Q6_K,
    /// 5-bit K-quant (≈0.625 bytes/param)
    Q5_K,
    /// 5-bit legacy quant (≈0.625 bytes/param)
    Q5_0,
    /// 4-bit K-quant (≈0.5 bytes/param)
    Q4_K,
    /// 4-bit legacy quant (≈0.5 bytes/param)
    Q4_0,
    /// 3-bit K-quant (≈0.375 bytes/param)
    Q3_K,
    /// 2-bit K-quant (≈0.25 bytes/param)
    Q2_K,
    /// Unrecognised quantization string
    Other(String),
}

impl Quantization {
    /// Approximate bytes per parameter.
    ///
    /// These are conservative averages; actual size depends on embedding tables
    /// and layer-specific quantization schemes.
    pub fn bytes_per_param(&self) -> f32 {
        match self {
            Quantization::F32 => 4.0,
            Quantization::F16 | Quantization::BF16 => 2.0,
            Quantization::Q8_0 => 1.0,
            Quantization::Q6_K => 0.75,
            Quantization::Q5_K | Quantization::Q5_0 => 0.625,
            Quantization::Q4_K | Quantization::Q4_0 => 0.5,
            Quantization::Q3_K => 0.375,
            Quantization::Q2_K => 0.25,
            Quantization::Other(_) => 1.0,
        }
    }

    /// Parse a quantization string (case-insensitive) into a [`Quantization`] variant.
    pub fn parse_quantization(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "F32" => Quantization::F32,
            "F16" => Quantization::F16,
            "BF16" => Quantization::BF16,
            "Q8_0" | "Q8" => Quantization::Q8_0,
            "Q6_K" | "Q6" => Quantization::Q6_K,
            "Q5_K_M" | "Q5_K" | "Q5" => Quantization::Q5_K,
            "Q5_0" => Quantization::Q5_0,
            "Q4_K_M" | "Q4_K" | "Q4" => Quantization::Q4_K,
            "Q4_0" => Quantization::Q4_0,
            "Q3_K_M" | "Q3_K" | "Q3" => Quantization::Q3_K,
            "Q2_K" | "Q2" => Quantization::Q2_K,
            other => Quantization::Other(other.to_string()),
        }
    }
}

// ── Model info ────────────────────────────────────────────────────────────────

/// Rich metadata about a model file.
///
/// Populated by [`ModelLoader::model_info`]. For GGUF files most fields are
/// extracted from the header; for other formats they are inferred from the
/// filename and file size.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Absolute or relative path to the model file
    pub path: PathBuf,
    /// Detected file format
    pub format: ModelFormat,
    /// File size on disk in bytes
    pub file_size_bytes: u64,
    /// File size in MB
    pub file_size_mb: u64,
    /// Approximate parameter count in billions (may be 0 if unknown)
    pub param_count_b: f32,
    /// Quantization type
    pub quantization: Option<Quantization>,
    /// Context length in tokens (if parseable)
    pub context_length: Option<u32>,
    /// Architecture name (llama, mistral, gemma, phi, etc.)
    pub architecture: Option<String>,
    /// GGUF-specific metadata (only for GGUF files)
    pub gguf_metadata: Option<GgufMetadata>,
    /// Estimated memory required to load model in MB
    pub estimated_memory_mb: u64,
}

/// A model that has been registered by the loader.
///
/// Stores a generated UUID, the parsed [`ModelInfo`], and an RFC 3339 timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedModel {
    /// Stable UUIDv4 assigned at registration time
    pub id: String,
    /// Parsed metadata for the model file
    pub info: ModelInfo,
    /// Registration timestamp in RFC 3339 format
    pub registered_at: String,
}

// ── Loader ────────────────────────────────────────────────────────────────────

/// Registry for discovered model files.
///
/// `ModelLoader` maintains an in-memory map of [`LoadedModel`] entries keyed
/// by UUID. It does not load tensor data into RAM; it only validates headers
/// and extracts metadata for downstream scheduling.
pub struct ModelLoader {
    /// Active registry of loaded models
    models: HashMap<String, LoadedModel>,
}

impl ModelLoader {
    /// Create an empty loader.
    pub fn new() -> Self {
        Self {
            models: HashMap::new(),
        }
    }

    /// Detect the format of a file by reading its header bytes.
    ///
    /// Reads the first 8 bytes and checks known magic signatures:
    /// - GGUF: `0x46475547`
    /// - GGML: `0x67676d6c`
    /// - PyTorch: pickle `0x80 0x02` or ZIP `0x50 0x4b`
    /// - ONNX: protobuf field start `0x08` (only when extension is `.onnx`)
    /// - SafeTensors: no unique magic; confirmed by extension
    ///
    /// Falls back to extension if header detection is ambiguous.
    pub fn detect_format(path: &Path) -> Result<ModelFormat> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        // Read first 8 bytes for magic number detection
        let mut file = std::fs::File::open(path)
            .map_err(|e| ClawzError::Hardware(format!("cannot open model file: {e}")))?;

        let mut header = [0u8; 8];
        let n = file.read(&mut header).unwrap_or(0);

        if n >= 4 {
            let magic = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);

            if magic == GGUF_MAGIC {
                return Ok(ModelFormat::GGUF);
            }
            if magic == GGML_MAGIC {
                return Ok(ModelFormat::GGML);
            }
            // PyTorch pickle starts with 0x80 0x02 or 0x50 0x4b (ZIP)
            if (header[0] == 0x80 && header[1] == 0x02) || (header[0] == 0x50 && header[1] == 0x4b)
            {
                return Ok(ModelFormat::PyTorch);
            }
            // ONNX protobuf: field 1 varint, very common first byte is 0x08
            if header[0] == 0x08 && ext == "onnx" {
                return Ok(ModelFormat::ONNX);
            }
            // SafeTensors starts with a little-endian u64 header length
            if ext == "safetensors" {
                return Ok(ModelFormat::SafeTensors);
            }
        }

        // Fall back to extension
        ModelFormat::from_extension(&ext).ok_or_else(|| {
            ClawzError::Hardware(format!(
                "cannot detect model format for: {}",
                path.display()
            ))
        })
    }

    /// Read model metadata without loading weights into memory.
    ///
    /// Validates the file header and extracts available metadata. For GGUF files
    /// this parses the full KV header; for other formats it uses filename heuristics.
    pub fn model_info(path: &Path) -> Result<ModelInfo> {
        let meta = std::fs::metadata(path)
            .map_err(|e| ClawzError::Hardware(format!("cannot stat model file: {e}")))?;

        let file_size_bytes = meta.len();
        let file_size_mb = file_size_bytes / (1024 * 1024);

        let format = Self::detect_format(path)?;

        let (gguf_metadata, param_count_b, quantization, context_length, architecture) =
            if format == ModelFormat::GGUF {
                let gguf = read_gguf_metadata(path)?;
                let params = extract_param_count_b(&gguf.metadata);
                let quant = extract_quantization(&gguf.metadata, path);
                let ctx = extract_context_length(&gguf.metadata);
                let arch = extract_architecture(&gguf.metadata);
                (Some(gguf), params, quant, ctx, arch)
            } else {
                // For other formats, estimate params from filename and file size
                let params = estimate_params_from_filename(path);
                let quant = guess_quantization_from_filename(path);
                (None, params, quant, None, None)
            };

        // Estimate memory needed: file size * 1.2 overhead (activations, etc.)
        let estimated_memory_mb = (file_size_mb as f64 * 1.2) as u64;

        Ok(ModelInfo {
            path: path.to_path_buf(),
            format,
            file_size_bytes,
            file_size_mb,
            param_count_b,
            quantization,
            context_length,
            architecture,
            gguf_metadata,
            estimated_memory_mb,
        })
    }

    /// Load (register) a model by path. Validates the file and reads metadata.
    /// Does NOT load model weights into memory.
    ///
    /// Returns a [`LoadedModel`] with a generated UUID that can be used to
    /// retrieve or unload the entry later.
    pub fn load(&mut self, path: PathBuf) -> Result<LoadedModel> {
        let info = Self::model_info(&path)?;
        let id = uuid::Uuid::new_v4().to_string();

        let model = LoadedModel {
            id: id.clone(),
            info,
            registered_at: chrono::Utc::now().to_rfc3339(),
        };

        self.models.insert(id, model.clone());
        Ok(model)
    }

    /// Scan a directory recursively for model files, returning [`ModelInfo`] for each.
    ///
    /// Skips files that fail header validation and logs a warning.
    pub fn list_models(directory: &Path) -> Result<Vec<ModelInfo>> {
        if !directory.exists() {
            return Err(ClawzError::Hardware(format!(
                "model directory does not exist: {}",
                directory.display()
            )));
        }

        let mut results = Vec::new();
        scan_dir_for_models(directory, &mut results);
        Ok(results)
    }

    /// Check whether the hardware can accommodate the model's memory requirements.
    ///
    /// Prefers GPU VRAM over system RAM. Returns a [`ValidationReport`] with
    /// headroom calculations and a human-readable recommendation string.
    ///
    // Dependency: detect::HardwareInfo for GPU VRAM and RAM totals
    pub fn validate_hardware_requirements(
        model_path: &Path,
        hardware: &HardwareInfo,
    ) -> Result<ValidationReport> {
        let info = Self::model_info(model_path)?;

        // Determine available compute memory
        let gpu_vram_mb: u64 = hardware
            .gpus
            .iter()
            .filter(|g| g.available)
            .map(|g| g.vram_mb)
            .sum();

        let ram_available_mb = hardware.memory.available_mb;

        // Prefer GPU VRAM, fall back to RAM
        let (compute_memory_mb, using_gpu) = if gpu_vram_mb > 0 {
            (gpu_vram_mb, true)
        } else {
            (ram_available_mb, false)
        };

        let fits = info.estimated_memory_mb <= compute_memory_mb;
        let headroom_mb = compute_memory_mb.saturating_sub(info.estimated_memory_mb);

        Ok(ValidationReport {
            model_path: model_path.to_path_buf(),
            model_size_mb: info.file_size_mb,
            estimated_memory_mb: info.estimated_memory_mb,
            available_memory_mb: compute_memory_mb,
            headroom_mb,
            fits,
            using_gpu,
            recommendation: if fits {
                format!(
                    "Model fits with {}MB headroom (using {})",
                    headroom_mb,
                    if using_gpu { "GPU VRAM" } else { "CPU RAM" }
                )
            } else {
                format!(
                    "Model requires {}MB but only {}MB available. Consider a smaller quantization.",
                    info.estimated_memory_mb, compute_memory_mb
                )
            },
        })
    }

    /// Get a registered model by ID.
    pub fn get(&self, id: &str) -> Option<&LoadedModel> {
        self.models.get(id)
    }

    /// Unregister a model and remove it from the in-memory registry.
    pub fn unload(&mut self, id: &str) -> Option<LoadedModel> {
        self.models.remove(id)
    }

    /// List all registered models.
    pub fn list_loaded(&self) -> Vec<&LoadedModel> {
        self.models.values().collect()
    }

    /// Always returns `true`; reserved for future readiness checks.
    pub fn is_ready(&self) -> bool {
        true
    }

    /// Check whether a file extension is recognized as a supported model format.
    pub fn supports(&self, format: &str) -> bool {
        ModelFormat::from_extension(format).is_some()
    }
}

impl Default for ModelLoader {
    fn default() -> Self {
        Self::new()
    }
}

// ── Hardware validation report ────────────────────────────────────────────────

/// Result of comparing a model's estimated memory needs against available hardware.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    /// Path to the model file that was validated
    pub model_path: PathBuf,
    /// On-disk size in MB
    pub model_size_mb: u64,
    /// Estimated runtime memory in MB (file size + 20 % overhead)
    pub estimated_memory_mb: u64,
    /// Available compute memory in MB (GPU VRAM or CPU RAM)
    pub available_memory_mb: u64,
    /// Difference between available and estimated memory
    pub headroom_mb: u64,
    /// `true` if the model fits within available memory
    pub fits: bool,
    /// `true` if GPU VRAM was used as the memory budget
    pub using_gpu: bool,
    /// Human-readable recommendation (fit / no-fit guidance)
    pub recommendation: String,
}

// ── GGUF parsing ──────────────────────────────────────────────────────────────

/// Read GGUF file header and extract key-value metadata.
///
/// GGUF v3 binary layout:
///   magic        : u32  (0x46475547)
///   version      : u32
///   tensor_count : u64
///   kv_count     : u64
///   kv pairs: key (string), value_type (u32), value
fn read_gguf_metadata(path: &Path) -> Result<GgufMetadata> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| ClawzError::Hardware(format!("cannot open GGUF file: {e}")))?;

    // Read magic
    let magic = read_u32_le(&mut file)?;
    if magic != GGUF_MAGIC {
        return Err(ClawzError::Hardware(format!(
            "not a GGUF file: bad magic 0x{magic:08x}"
        )));
    }

    let version = read_u32_le(&mut file)?;
    if !(1..=3).contains(&version) {
        // Accept versions 1-3; warn but continue
        log::warn!("GGUF version {} may not be fully supported", version);
    }

    let tensor_count = read_u64_le(&mut file)?;
    let kv_count = read_u64_le(&mut file)?;

    // Parse KV pairs (up to kv_count, but cap to avoid huge files hanging)
    let mut metadata = HashMap::new();
    let max_kvs = kv_count.min(512);

    for _ in 0..max_kvs {
        match read_gguf_kv(&mut file) {
            Ok((key, value)) => {
                metadata.insert(key, value);
            }
            Err(_) => break, // Stop on parse error; we have what we have
        }
    }

    Ok(GgufMetadata {
        version,
        tensor_count,
        kv_count,
        metadata,
    })
}

/// GGUF value type constants as defined by the GGUF specification.
const GGUF_TYPE_UINT8: u32 = 0;
const GGUF_TYPE_INT8: u32 = 1;
const GGUF_TYPE_UINT16: u32 = 2;
const GGUF_TYPE_INT16: u32 = 3;
const GGUF_TYPE_UINT32: u32 = 4;
const GGUF_TYPE_INT32: u32 = 5;
const GGUF_TYPE_FLOAT32: u32 = 6;
const GGUF_TYPE_BOOL: u32 = 7;
const GGUF_TYPE_STRING: u32 = 8;
const GGUF_TYPE_ARRAY: u32 = 9;
const GGUF_TYPE_UINT64: u32 = 10;
const GGUF_TYPE_INT64: u32 = 11;
const GGUF_TYPE_FLOAT64: u32 = 12;

/// Parse a single GGUF key–value pair from the file.
///
/// Strings are read as `(u64 len, bytes)`. Arrays are skipped after noting their
/// element count so that parsing can continue with the next KV.
fn read_gguf_kv(file: &mut std::fs::File) -> Result<(String, String)> {
    let key = read_gguf_string(file)?;
    let value_type = read_u32_le(file)?;

    let value = match value_type {
        GGUF_TYPE_UINT8 => {
            let mut b = [0u8; 1];
            file.read_exact(&mut b)
                .map_err(|e| ClawzError::Hardware(e.to_string()))?;
            b[0].to_string()
        }
        GGUF_TYPE_INT8 => {
            let mut b = [0u8; 1];
            file.read_exact(&mut b)
                .map_err(|e| ClawzError::Hardware(e.to_string()))?;
            (b[0] as i8).to_string()
        }
        GGUF_TYPE_UINT16 => read_u16_le(file)?.to_string(),
        GGUF_TYPE_INT16 => (read_u16_le(file)? as i16).to_string(),
        GGUF_TYPE_UINT32 => read_u32_le(file)?.to_string(),
        GGUF_TYPE_INT32 => (read_u32_le(file)? as i32).to_string(),
        GGUF_TYPE_FLOAT32 => {
            let bits = read_u32_le(file)?;
            f32::from_bits(bits).to_string()
        }
        GGUF_TYPE_BOOL => {
            let mut b = [0u8; 1];
            file.read_exact(&mut b)
                .map_err(|e| ClawzError::Hardware(e.to_string()))?;
            (b[0] != 0).to_string()
        }
        GGUF_TYPE_STRING => read_gguf_string(file)?,
        GGUF_TYPE_UINT64 => read_u64_le(file)?.to_string(),
        GGUF_TYPE_INT64 => (read_u64_le(file)? as i64).to_string(),
        GGUF_TYPE_FLOAT64 => {
            let bits = read_u64_le(file)?;
            f64::from_bits(bits).to_string()
        }
        GGUF_TYPE_ARRAY => {
            // Array: element_type (u32), count (u64), elements...
            let elem_type = read_u32_le(file)?;
            let count = read_u64_le(file)?;
            // Skip all elements; just note the count
            skip_gguf_array(file, elem_type, count)?;
            format!("[array:{}]", count)
        }
        _ => {
            return Err(ClawzError::Hardware(format!(
                "unknown GGUF value type: {value_type}"
            )));
        }
    };

    Ok((key, value))
}

/// Skip over a GGUF array without materialising its contents.
///
/// Bounded to 1024 elements to prevent pathological files from causing
/// unbounded seeks.
fn skip_gguf_array(file: &mut std::fs::File, elem_type: u32, count: u64) -> Result<()> {
    let max = count.min(1024); // Don't hang on pathological files
    for _ in 0..max {
        match elem_type {
            GGUF_TYPE_UINT8 | GGUF_TYPE_INT8 | GGUF_TYPE_BOOL => {
                file.seek(SeekFrom::Current(1))
                    .map_err(|e| ClawzError::Hardware(e.to_string()))?;
            }
            GGUF_TYPE_UINT16 | GGUF_TYPE_INT16 => {
                file.seek(SeekFrom::Current(2))
                    .map_err(|e| ClawzError::Hardware(e.to_string()))?;
            }
            GGUF_TYPE_UINT32 | GGUF_TYPE_INT32 | GGUF_TYPE_FLOAT32 => {
                file.seek(SeekFrom::Current(4))
                    .map_err(|e| ClawzError::Hardware(e.to_string()))?;
            }
            GGUF_TYPE_UINT64 | GGUF_TYPE_INT64 | GGUF_TYPE_FLOAT64 => {
                file.seek(SeekFrom::Current(8))
                    .map_err(|e| ClawzError::Hardware(e.to_string()))?;
            }
            GGUF_TYPE_STRING => {
                let _ = read_gguf_string(file)?;
            }
            _ => break,
        }
    }
    Ok(())
}

/// Read a GGUF string: `(u64 length, UTF-8 bytes)` with no null terminator.
fn read_gguf_string(file: &mut std::fs::File) -> Result<String> {
    let len = read_u64_le(file)? as usize;
    if len > 4096 {
        return Err(ClawzError::Hardware(format!("GGUF string too long: {len}")));
    }
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)
        .map_err(|e| ClawzError::Hardware(format!("GGUF string read error: {e}")))?;
    String::from_utf8(buf).map_err(|e| ClawzError::Hardware(format!("GGUF string not UTF-8: {e}")))
}

/// Read a little-endian `u16` from the file.
fn read_u16_le(file: &mut std::fs::File) -> Result<u16> {
    let mut b = [0u8; 2];
    file.read_exact(&mut b)
        .map_err(|e| ClawzError::Hardware(e.to_string()))?;
    Ok(u16::from_le_bytes(b))
}

/// Read a little-endian `u32` from the file.
fn read_u32_le(file: &mut std::fs::File) -> Result<u32> {
    let mut b = [0u8; 4];
    file.read_exact(&mut b)
        .map_err(|e| ClawzError::Hardware(e.to_string()))?;
    Ok(u32::from_le_bytes(b))
}

/// Read a little-endian `u64` from the file.
fn read_u64_le(file: &mut std::fs::File) -> Result<u64> {
    let mut b = [0u8; 8];
    file.read_exact(&mut b)
        .map_err(|e| ClawzError::Hardware(e.to_string()))?;
    Ok(u64::from_le_bytes(b))
}

// ── Metadata extraction helpers ───────────────────────────────────────────────

/// Extract parameter count in billions from GGUF metadata keys.
///
/// Tries multiple known key prefixes because different model families store
/// the count under different namespaces (llama, phi2, mistral, etc.).
fn extract_param_count_b(meta: &HashMap<String, String>) -> f32 {
    // Common GGUF metadata keys for parameter count
    for key in &[
        "general.parameter_count",
        "llama.parameter_count",
        "phi2.parameter_count",
        "mistral.parameter_count",
    ] {
        if let Some(val) = meta.get(*key) {
            if let Ok(n) = val.parse::<u64>() {
                return n as f32 / 1_000_000_000.0;
            }
        }
    }
    0.0
}

/// Extract quantization from GGUF metadata, falling back to filename heuristics.
fn extract_quantization(meta: &HashMap<String, String>, path: &Path) -> Option<Quantization> {
    // GGUF stores file type as integer; map common values
    if let Some(val) = meta.get("general.file_type") {
        let quant_str = gguf_file_type_to_quant_str(val.parse::<u32>().unwrap_or(0));
        return Some(Quantization::parse_quantization(quant_str));
    }
    // Fall back to filename
    guess_quantization_from_filename(path)
}

/// Map a GGUF `general.file_type` integer code to a quantization string.
///
/// These codes are defined by the llama.cpp GGUF specification.
fn gguf_file_type_to_quant_str(file_type: u32) -> &'static str {
    match file_type {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        6 => "Q5_0",
        7 => "Q5_1",
        8 => "Q8_0",
        10 => "Q2_K",
        11 => "Q3_K",
        12 => "Q4_K",
        13 => "Q5_K",
        14 => "Q6_K",
        15 => "Q8_K",
        16 => "IQ2_XXS",
        17 => "IQ2_XS",
        18 => "Q2_K_S",
        19 => "IQ3_XS",
        20 => "IQ3_XXS",
        _ => "F16",
    }
}

/// Extract context length in tokens from GGUF metadata.
fn extract_context_length(meta: &HashMap<String, String>) -> Option<u32> {
    for key in &[
        "llama.context_length",
        "phi2.context_length",
        "mistral.context_length",
        "gemma.context_length",
        "general.context_length",
    ] {
        if let Some(val) = meta.get(*key) {
            if let Ok(n) = val.parse::<u32>() {
                return Some(n);
            }
        }
    }
    None
}

/// Extract architecture string (e.g. "llama", "mistral") from GGUF metadata.
fn extract_architecture(meta: &HashMap<String, String>) -> Option<String> {
    meta.get("general.architecture").cloned()
}

/// Guess parameter count (B) from common filename patterns like "llama-3-8b", "mistral-7b".
fn estimate_params_from_filename(path: &Path) -> f32 {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Match patterns like "70b", "13b", "7b", "3b", "1.5b"
    let re_patterns = [
        ("70b", 70.0f32),
        ("65b", 65.0),
        ("34b", 34.0),
        ("30b", 30.0),
        ("13b", 13.0),
        ("8b", 8.0),
        ("7b", 7.0),
        ("3b", 3.0),
        ("2b", 2.0),
        ("1.5b", 1.5),
        ("1b", 1.0),
    ];

    for (pat, val) in &re_patterns {
        if name.contains(pat) {
            return *val;
        }
    }
    0.0
}

/// Guess quantization from filename patterns.
///
/// Searches for common quant suffixes (Q4_K_M, Q5_K_M, etc.) in the filename.
fn guess_quantization_from_filename(path: &Path) -> Option<Quantization> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_uppercase();

    let quants = [
        "Q4_K_M", "Q4_K_S", "Q4_K", "Q4_0", "Q4_1", "Q5_K_M", "Q5_K_S", "Q5_K", "Q5_0", "Q5_1",
        "Q6_K", "Q8_0", "Q8_K", "Q2_K", "Q3_K_M", "Q3_K_S", "Q3_K", "F16", "BF16", "F32",
    ];

    for q in &quants {
        if name.contains(q) {
            return Some(Quantization::parse_quantization(q));
        }
    }
    None
}

// ── Directory scanning ────────────────────────────────────────────────────────

/// Recursively scan a directory for model files and append [`ModelInfo`] results.
///
/// Silently skips directories that cannot be read.
fn scan_dir_for_models(dir: &Path, results: &mut Vec<ModelInfo>) {
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };

    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir_for_models(&path, results);
        } else if is_model_file(&path) {
            match ModelLoader::model_info(&path) {
                Ok(info) => results.push(info),
                Err(e) => {
                    log::warn!("skipping {}: {e}", path.display());
                }
            }
        }
    }
}

/// Check whether a path has a recognized model file extension.
fn is_model_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        ext.as_str(),
        "gguf" | "safetensors" | "pt" | "pth" | "onnx" | "bin" | "ggml"
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Helper: write a minimal GGUF v3 file with STRING KV pairs for testing.
    fn write_gguf_v3(path: &Path, kv_pairs: &[(&str, &str)]) {
        use std::io::Write;
        let mut f = std::fs::File::create(path).unwrap();

        // magic
        f.write_all(&GGUF_MAGIC.to_le_bytes()).unwrap();
        // version
        f.write_all(&3u32.to_le_bytes()).unwrap();
        // tensor_count
        f.write_all(&0u64.to_le_bytes()).unwrap();
        // kv_count
        f.write_all(&(kv_pairs.len() as u64).to_le_bytes()).unwrap();

        for (key, val) in kv_pairs {
            // key string
            f.write_all(&(key.len() as u64).to_le_bytes()).unwrap();
            f.write_all(key.as_bytes()).unwrap();
            // value type: STRING (8)
            f.write_all(&8u32.to_le_bytes()).unwrap();
            // value string
            f.write_all(&(val.len() as u64).to_le_bytes()).unwrap();
            f.write_all(val.as_bytes()).unwrap();
        }
    }

    #[test]
    fn test_loader_creation() {
        let loader = ModelLoader::new();
        assert!(loader.is_ready());
        assert!(loader.list_loaded().is_empty());
    }

    #[test]
    fn test_supported_formats() {
        assert!(ModelLoader::default().supports("gguf"));
        assert!(ModelLoader::default().supports("safetensors"));
        assert!(ModelLoader::default().supports("onnx"));
    }

    #[test]
    fn test_model_format_from_extension() {
        assert_eq!(ModelFormat::from_extension("gguf"), Some(ModelFormat::GGUF));
        assert_eq!(
            ModelFormat::from_extension("safetensors"),
            Some(ModelFormat::SafeTensors)
        );
        assert_eq!(
            ModelFormat::from_extension("pt"),
            Some(ModelFormat::PyTorch)
        );
        assert_eq!(ModelFormat::from_extension("onnx"), Some(ModelFormat::ONNX));
        assert_eq!(ModelFormat::from_extension("xyz"), None);
    }

    #[test]
    fn test_gguf_magic_detection() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.gguf");
        write_gguf_v3(&p, &[("general.architecture", "llama")]);
        let fmt = ModelLoader::detect_format(&p).unwrap();
        assert_eq!(fmt, ModelFormat::GGUF);
    }

    #[test]
    fn test_gguf_metadata_parse() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.gguf");
        write_gguf_v3(
            &p,
            &[
                ("general.architecture", "llama"),
                ("llama.context_length", "4096"),
            ],
        );
        let meta = read_gguf_metadata(&p).unwrap();
        assert_eq!(meta.version, 3);
        assert_eq!(meta.metadata.get("general.architecture").unwrap(), "llama");
        assert_eq!(meta.metadata.get("llama.context_length").unwrap(), "4096");
    }

    #[test]
    fn test_model_info_gguf() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("llama-3-8b-q4_k_m.gguf");
        write_gguf_v3(
            &p,
            &[
                ("general.architecture", "llama"),
                ("llama.context_length", "8192"),
                ("general.parameter_count", "8000000000"),
            ],
        );
        let info = ModelLoader::model_info(&p).unwrap();
        assert_eq!(info.format, ModelFormat::GGUF);
        assert!(info.architecture.as_deref() == Some("llama"));
        assert!((info.param_count_b - 8.0).abs() < 0.1);
        assert_eq!(info.context_length, Some(8192));
    }

    #[test]
    fn test_list_models_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let models = ModelLoader::list_models(dir.path()).unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn test_list_models_missing_dir() {
        let result = ModelLoader::list_models(Path::new("/nonexistent/path/xyz"));
        assert!(result.is_err());
    }

    #[test]
    fn test_estimate_params_from_filename() {
        assert_eq!(
            estimate_params_from_filename(Path::new("llama-3-70b-q4.gguf")),
            70.0
        );
        assert_eq!(
            estimate_params_from_filename(Path::new("mistral-7b-instruct.gguf")),
            7.0
        );
    }

    #[test]
    fn test_quantization_bytes_per_param() {
        assert_eq!(Quantization::F32.bytes_per_param(), 4.0);
        assert_eq!(Quantization::Q4_0.bytes_per_param(), 0.5);
        assert_eq!(Quantization::Q8_0.bytes_per_param(), 1.0);
    }

    #[test]
    fn test_load_and_unload() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("model-7b-q4.gguf");
        write_gguf_v3(&p, &[("general.architecture", "mistral")]);

        let mut loader = ModelLoader::new();
        let model = loader.load(p).unwrap();
        assert!(!model.id.is_empty());

        let retrieved = loader.get(&model.id).unwrap();
        assert_eq!(retrieved.id, model.id);

        let unloaded = loader.unload(&model.id).unwrap();
        assert_eq!(unloaded.id, model.id);
        assert!(loader.list_loaded().is_empty());
    }
}
