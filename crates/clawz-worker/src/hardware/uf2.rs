//! UF2 firmware flashing: parsing, validation, and bootloader detection.
//!
//! UF2 (USB Flashing Format) is a Microsoft-specified file format used by
//! Raspberry Pi Pico, Adafruit boards, and many other microcontrollers to
//! receive firmware over USB mass storage. Each UF2 file consists of fixed-
//! size 512-byte blocks containing payload data, target addresses, and magic
//! numbers.
//!
//! This module provides:
//! - Low-level block parsing (`parse_uf2`, `Uf2Block`)
//! - Metadata extraction (`FirmwareFlasher::parse_uf2` → `FirmwareInfo`)
//! - Bootloader volume auto-detection (`FirmwareFlasher::detect_bootloader`)
//! - Flashing by file copy (`FirmwareFlasher::flash`)
//! - Post-flash verification (`FirmwareFlasher::verify_flash`)
//!
//! ## Design notes
//!
//! - Most UF2 bootloaders mount as FAT12/FAT16 mass-storage devices. Flashing
//!   is performed by simply copying the `.uf2` file to the mount point.
//! - After a successful copy the bootloader automatically unmounts, re-flashes
//!   the internal flash, and re-enumerates as the application USB device.
//!
//! ## Key dependencies
//! - `clawz_core::error` for unified error propagation

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use clawz_core::error::{ClawzError, Result};

// ── UF2 binary format constants ───────────────────────────────────────────────

/// First magic number in every UF2 block: "UF2\n" in little-endian
const UF2_MAGIC_START0: u32 = 0x0A324655;
/// Second magic number in every UF2 block
const UF2_MAGIC_START1: u32 = 0x9E5D5157;
/// End magic number (last 4 bytes of block)
const UF2_MAGIC_END: u32 = 0x0AB16F30;

/// Fixed UF2 block size in bytes
const UF2_BLOCK_SIZE: usize = 512;

/// Maximum payload bytes per UF2 block (defined by spec)
const UF2_DATA_SIZE: usize = 256;

/// UF2 flags
const UF2_FLAG_NOT_MAIN_FLASH: u32 = 0x00000001;
const UF2_FLAG_FILE_CONTAINER: u32 = 0x00001000;
const UF2_FLAG_FAMILYID_PRESENT: u32 = 0x00002000;
const UF2_FLAG_MD5_CHECKSUM: u32 = 0x00004000;
const UF2_FLAG_EXTENSION_TAGS: u32 = 0x00008000;

// ── Known family IDs ──────────────────────────────────────────────────────────

/// Maps a UF2 family ID to a human-readable target chip name.
///
/// Family IDs are 32-bit identifiers that tell the bootloader which
/// microcontroller family the firmware is intended for.
fn family_id_to_name(id: u32) -> Option<&'static str> {
    match id {
        0xe48bff56 => Some("RP2040 (Raspberry Pi Pico)"),
        0xe48bff57 => Some("RP2350-ARM-S (Raspberry Pi Pico 2)"),
        0xe48bff59 => Some("RP2350-RISCV (Raspberry Pi Pico 2 RISC-V)"),
        0x68ed2b88 => Some("nRF52840"),
        0x2b88d29c => Some("nRF52833"),
        0x6f51571a => Some("nRF52832"),
        0xada52840 => Some("SAMD21"),
        0x55114460 => Some("SAMD51"),
        0x9af03e33 => Some("STM32F1"),
        0x5ee21072 => Some("STM32F4"),
        0x53b80f00 => Some("STM32F7"),
        0x6db66082 => Some("STM32L4"),
        0x0dc417df => Some("STM32G0B1"),
        0x4b684d71 => Some("STM32WB55"),
        0x1dc11454 => Some("ESP32"),
        0xbfdd4eee => Some("ESP32-S2"),
        0xc47e5767 => Some("ESP32-S3"),
        0x6921571a => Some("ESP32-C3"),
        _ => None,
    }
}

// ── UF2 block ─────────────────────────────────────────────────────────────────

/// A single parsed UF2 block (512 bytes).
///
/// Represents the raw on-disk layout after validation. Payload may be up to
/// 256 bytes; trailing bytes in `data` beyond `payload_size` are padding.
#[derive(Debug, Clone)]
pub struct Uf2Block {
    /// Block flags bitmask (see `UF2_FLAG_*` constants)
    pub flags: u32,
    /// Target flash address for this block's data
    pub target_addr: u32,
    /// Number of valid bytes in `data`
    pub payload_size: u32,
    /// Block sequence number (0-based)
    pub block_no: u32,
    /// Total number of blocks in the file
    pub num_blocks: u32,
    /// Family ID or file size (depending on flags)
    pub file_size_or_family_id: u32,
    /// Payload data (up to 256 bytes)
    pub data: [u8; UF2_DATA_SIZE],
}

impl Uf2Block {
    /// Returns true if the `FAMILYID_PRESENT` flag is set.
    pub fn has_family_id(&self) -> bool {
        self.flags & UF2_FLAG_FAMILYID_PRESENT != 0
    }

    /// Returns true if this block targets main flash (not a file container or other region).
    pub fn is_main_flash(&self) -> bool {
        self.flags & UF2_FLAG_NOT_MAIN_FLASH == 0
    }
}

// ── Parsed UF2 file ───────────────────────────────────────────────────────────

/// Metadata extracted from a UF2 file.
///
/// Used for pre-flash validation and UI display (target chip, version, size).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirmwareInfo {
    /// Filename (from the source path)
    pub name: String,
    /// File size in bytes
    pub size_bytes: u64,
    /// Number of UF2 blocks
    pub block_count: u32,
    /// Target address range (start, end) for main-flash blocks
    pub address_range: Option<(u32, u32)>,
    /// Family ID if present in any block
    pub family_id: Option<u32>,
    /// Human-readable target chip name
    pub target: String,
    /// Version string (from embedded metadata, if any)
    pub version: String,
}

// ── FirmwareFlasher ───────────────────────────────────────────────────────────

/// High-level interface for UF2 firmware operations.
///
/// Parses UF2 files, auto-detects bootloader volumes, performs the flash copy,
/// and verifies that the device re-enumerated after flashing.
pub struct FirmwareFlasher;

impl FirmwareFlasher {
    /// Create a new flasher instance.
    pub fn new() -> Self {
        Self
    }

    /// Parse and validate a UF2 file, returning metadata without flashing.
    ///
    /// Validates all magic numbers, checks block count consistency, and extracts
    /// the target address range and family ID for downstream display.
    pub fn parse_uf2(&self, path: &Path) -> Result<FirmwareInfo> {
        let data = std::fs::read(path)
            .map_err(|e| ClawzError::Hardware(format!("cannot read UF2 file: {e}")))?;

        let blocks = parse_uf2_blocks(&data)?;

        let block_count = blocks.len() as u32;
        let size_bytes = data.len() as u64;

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("firmware.uf2")
            .to_string();

        // Determine address range from all main-flash blocks
        let flash_blocks: Vec<&Uf2Block> = blocks.iter().filter(|b| b.is_main_flash()).collect();

        let address_range = if flash_blocks.is_empty() {
            None
        } else {
            let start = flash_blocks.iter().map(|b| b.target_addr).min().unwrap();
            let last = flash_blocks.iter().max_by_key(|b| b.target_addr).unwrap();
            let end = last.target_addr + last.payload_size;
            Some((start, end))
        };

        // Extract family ID from first block that has one
        let family_id = blocks
            .iter()
            .find(|b| b.has_family_id())
            .map(|b| b.file_size_or_family_id);

        let target = family_id
            .and_then(|id| family_id_to_name(id))
            .unwrap_or("Unknown target")
            .to_string();

        Ok(FirmwareInfo {
            name,
            size_bytes,
            block_count,
            address_range,
            family_id,
            target,
            version: extract_version_from_name(path),
        })
    }

    /// Find a mounted UF2 bootloader volume on the system.
    ///
    /// Returns the mount path if found (e.g. `/media/user/RPI-RP2`).
    ///
    /// On Linux this parses `/proc/mounts` for FAT volumes with known bootloader
    /// names or an `INFO_UF2.TXT` marker file. On macOS it scans `/Volumes`.
    pub fn detect_bootloader(&self) -> Option<PathBuf> {
        #[cfg(target_os = "linux")]
        {
            detect_bootloader_linux()
        }

        #[cfg(target_os = "macos")]
        {
            detect_bootloader_macos()
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            None
        }
    }

    /// Flash a UF2 file to a bootloader volume by copying it.
    ///
    /// Most UF2 bootloaders (RP2040, SAMD, etc.) mount as a mass storage device.
    /// Writing the `.uf2` file to the mount point triggers flashing.
    pub async fn flash(&self, uf2_path: &Path, device_path: &str) -> Result<()> {
        // Validate the UF2 file first
        let info = self.parse_uf2(uf2_path)?;

        let device = Path::new(device_path);

        // Determine destination: if device_path is a directory (mount point), copy file into it
        let destination = if device.is_dir() {
            device.join(
                uf2_path
                    .file_name()
                    .ok_or_else(|| ClawzError::Hardware("UF2 path has no filename".to_string()))?,
            )
        } else {
            device.to_path_buf()
        };

        log::info!(
            "Flashing {} ({} blocks, target: {}) to {}",
            info.name,
            info.block_count,
            info.target,
            destination.display()
        );

        // Copy file — on UF2 mass-storage bootloaders this triggers the flash
        std::fs::copy(uf2_path, &destination).map_err(|e| {
            ClawzError::Hardware(format!(
                "flash copy failed from {} to {}: {e}",
                uf2_path.display(),
                destination.display()
            ))
        })?;

        log::info!("Flash copy complete: {}", destination.display());
        Ok(())
    }

    /// Verify that a flash operation completed by checking the device re-enumerated.
    ///
    /// After a successful flash, the bootloader volume disappears and the
    /// device re-enumerates as its normal application USB device.
    pub fn verify_flash(&self, device_path: &str) -> Result<FlashVerification> {
        let path = Path::new(device_path);

        // If the bootloader mount point no longer exists → successful flash
        if path.is_dir() {
            // Bootloader still mounted → flash may not have completed
            // Check if any UF2 file exists (some bootloaders keep it briefly)
            let has_uf2 = std::fs::read_dir(path)
                .ok()
                .map(|rd| {
                    rd.flatten().any(|e| {
                        e.path()
                            .extension()
                            .and_then(|x| x.to_str())
                            .map(|x| x.eq_ignore_ascii_case("uf2"))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false);

            if has_uf2 {
                return Ok(FlashVerification {
                    success: false,
                    device_path: device_path.to_string(),
                    message: "Bootloader still mounted with UF2 file present; flash may be in progress".to_string(),
                });
            }

            return Ok(FlashVerification {
                success: false,
                device_path: device_path.to_string(),
                message: "Bootloader still mounted; flash not yet triggered".to_string(),
            });
        }

        // Mount point gone → device re-enumerated → success
        Ok(FlashVerification {
            success: true,
            device_path: device_path.to_string(),
            message: "Bootloader volume unmounted; device appears to have re-enumerated successfully".to_string(),
        })
    }
}

impl Default for FirmwareFlasher {
    fn default() -> Self {
        Self::new()
    }
}

// ── Verification result ───────────────────────────────────────────────────────

/// Outcome of a post-flash verification check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlashVerification {
    /// `true` if the bootloader volume disappeared (indicating successful re-flash)
    pub success: bool,
    /// Path that was checked
    pub device_path: String,
    /// Human-readable status message
    pub message: String,
}

// ── UF2 parsing ───────────────────────────────────────────────────────────────

/// Parse raw bytes into a list of validated UF2 blocks.
///
/// Convenience wrapper around [`parse_uf2_blocks`].
pub fn parse_uf2(data: &[u8]) -> Result<Vec<Uf2Block>> {
    parse_uf2_blocks(data)
}

/// Low-level UF2 parser.
///
/// Validates:
/// - File size is a multiple of 512 bytes
/// - Every block has correct start and end magic numbers
/// - Block count is consistent across all headers
/// - Block sequence numbers are contiguous (0..n)
fn parse_uf2_blocks(data: &[u8]) -> Result<Vec<Uf2Block>> {
    if data.len() % UF2_BLOCK_SIZE != 0 {
        return Err(ClawzError::Hardware(format!(
            "UF2 file size {} is not a multiple of 512 bytes",
            data.len()
        )));
    }

    if data.is_empty() {
        return Err(ClawzError::Hardware("UF2 file is empty".to_string()));
    }

    let block_count = data.len() / UF2_BLOCK_SIZE;
    let mut blocks = Vec::with_capacity(block_count);

    for i in 0..block_count {
        let offset = i * UF2_BLOCK_SIZE;
        let block_data = &data[offset..offset + UF2_BLOCK_SIZE];
        let block = parse_single_block(block_data, i)?;
        blocks.push(block);
    }

    // Cross-validate: all blocks should agree on num_blocks
    if let Some(first) = blocks.first() {
        let expected = first.num_blocks;
        if expected as usize != block_count {
            return Err(ClawzError::Hardware(format!(
                "UF2 header says {} blocks but file has {} blocks",
                expected, block_count
            )));
        }
        for (idx, block) in blocks.iter().enumerate() {
            if block.num_blocks != expected {
                return Err(ClawzError::Hardware(format!(
                    "Block {idx} has inconsistent num_blocks: {} vs expected {expected}",
                    block.num_blocks
                )));
            }
            if block.block_no != idx as u32 {
                return Err(ClawzError::Hardware(format!(
                    "Block {idx} has wrong block_no: {}",
                    block.block_no
                )));
            }
        }
    }

    Ok(blocks)
}

/// Parse and validate a single 512-byte UF2 block.
fn parse_single_block(data: &[u8], index: usize) -> Result<Uf2Block> {
    if data.len() < UF2_BLOCK_SIZE {
        return Err(ClawzError::Hardware(format!(
            "Block {index} is too short: {} bytes",
            data.len()
        )));
    }

    let magic0 = read_u32_le_at(data, 0);
    let magic1 = read_u32_le_at(data, 4);

    if magic0 != UF2_MAGIC_START0 || magic1 != UF2_MAGIC_START1 {
        return Err(ClawzError::Hardware(format!(
            "Block {index} has bad magic: 0x{magic0:08x} 0x{magic1:08x} (expected 0x{UF2_MAGIC_START0:08x} 0x{UF2_MAGIC_START1:08x})"
        )));
    }

    let end_magic = read_u32_le_at(data, 508);
    if end_magic != UF2_MAGIC_END {
        return Err(ClawzError::Hardware(format!(
            "Block {index} has bad end magic: 0x{end_magic:08x}"
        )));
    }

    let flags = read_u32_le_at(data, 8);
    let target_addr = read_u32_le_at(data, 12);
    let payload_size = read_u32_le_at(data, 16);
    let block_no = read_u32_le_at(data, 20);
    let num_blocks = read_u32_le_at(data, 24);
    let file_size_or_family_id = read_u32_le_at(data, 28);

    if payload_size as usize > UF2_DATA_SIZE {
        return Err(ClawzError::Hardware(format!(
            "Block {index} payload_size {payload_size} exceeds max {UF2_DATA_SIZE}"
        )));
    }

    let mut payload = [0u8; UF2_DATA_SIZE];
    payload.copy_from_slice(&data[32..32 + UF2_DATA_SIZE]);

    Ok(Uf2Block {
        flags,
        target_addr,
        payload_size,
        block_no,
        num_blocks,
        file_size_or_family_id,
        data: payload,
    })
}

/// Read a little-endian `u32` from `data` at `offset`.
fn read_u32_le_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

// ── Bootloader detection ──────────────────────────────────────────────────────

/// Linux: detect a UF2 bootloader by scanning `/proc/mounts`.
///
/// Looks for FAT filesystems whose mount point or volume label matches known
/// bootloader names (e.g. `RPI-RP2`, `CIRCUITPY`). Also checks for the presence
/// of `INFO_UF2.TXT`, which every UF2 bootloader exposes.
#[cfg(target_os = "linux")]
fn detect_bootloader_linux() -> Option<PathBuf> {
    // Parse /proc/mounts to find FAT volumes with known bootloader names
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;

    let known_names = [
        "RPI-RP2",    // Raspberry Pi Pico (RP2040)
        "RP2350",     // Raspberry Pi Pico 2
        "CIRCUITPY",  // CircuitPython
        "ARDUINO",    // Arduino UF2 bootloader
        "FTHRS2BOOT", // Feather M0
        "FTHR840BOOT",
        "ITSYBOOT",
        "METROBOOT",
        "GEMMABOOT",
        "TRINKETBOOT",
        "PYBFLASH",   // MicroPython
    ];

    for line in mounts.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let mount_point = parts[1];
        let fs_type = parts[2];

        // UF2 bootloaders always mount as FAT (vfat / fat)
        if !fs_type.contains("fat") {
            continue;
        }

        // Check if mount point or volume label matches
        let mp_upper = mount_point.to_uppercase();
        if known_names.iter().any(|name| mp_upper.contains(name)) {
            return Some(PathBuf::from(mount_point));
        }

        // Also check if there's an INFO_UF2.TXT file (present on all UF2 bootloaders)
        let info_txt = Path::new(mount_point).join("INFO_UF2.TXT");
        if info_txt.exists() {
            return Some(PathBuf::from(mount_point));
        }
    }

    None
}

/// macOS: detect a UF2 bootloader by scanning `/Volumes`.
#[cfg(target_os = "macos")]
fn detect_bootloader_macos() -> Option<PathBuf> {
    use std::process::Command;

    // On macOS, UF2 bootloaders appear in /Volumes
    let rd = std::fs::read_dir("/Volumes").ok()?;

    let known_names = ["RPI-RP2", "RP2350", "CIRCUITPY", "ARDUINO", "PYBFLASH"];

    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry
            .file_name()
            .to_string_lossy()
            .to_uppercase();

        if known_names.iter().any(|n| name.contains(n)) {
            return Some(path);
        }

        // Check for INFO_UF2.TXT
        if path.join("INFO_UF2.TXT").exists() {
            return Some(path);
        }
    }

    None
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Try to extract a version string from the filename, e.g. "firmware_v1.2.3.uf2" → "v1.2.3"
fn extract_version_from_name(path: &Path) -> String {
    let name = path
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    // Look for patterns like "_v1.2.3", "-v1.2", "_1.2.3"
    for part in name.split(|c: char| c == '_' || c == '-') {
        if part.starts_with('v') || part.starts_with('V') {
            let rest = &part[1..];
            if rest.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                return part.to_string();
            }
        }
        // plain version number like "1.2.3"
        if part.contains('.') && part.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return part.to_string();
        }
    }

    "unknown".to_string()
}

/// Build a valid UF2 block for testing purposes.
///
/// Constructs the full 512-byte block with magic numbers, flags, payload, and
/// end magic. Payload is zero-padded to 256 bytes if shorter.
pub fn build_uf2_block(
    block_no: u32,
    num_blocks: u32,
    target_addr: u32,
    family_id: Option<u32>,
    data: &[u8],
) -> [u8; UF2_BLOCK_SIZE] {
    let mut block = [0u8; UF2_BLOCK_SIZE];

    let payload_size = data.len().min(UF2_DATA_SIZE) as u32;
    let flags: u32 = if family_id.is_some() {
        UF2_FLAG_FAMILYID_PRESENT
    } else {
        0
    };
    let family_or_file = family_id.unwrap_or(0);

    write_u32_le(&mut block, 0, UF2_MAGIC_START0);
    write_u32_le(&mut block, 4, UF2_MAGIC_START1);
    write_u32_le(&mut block, 8, flags);
    write_u32_le(&mut block, 12, target_addr);
    write_u32_le(&mut block, 16, payload_size);
    write_u32_le(&mut block, 20, block_no);
    write_u32_le(&mut block, 24, num_blocks);
    write_u32_le(&mut block, 28, family_or_file);

    let copy_len = data.len().min(UF2_DATA_SIZE);
    block[32..32 + copy_len].copy_from_slice(&data[..copy_len]);

    write_u32_le(&mut block, 508, UF2_MAGIC_END);

    block
}

/// Write a little-endian `u32` into `buf` at `offset`.
fn write_u32_le(buf: &mut [u8], offset: usize, value: u32) {
    let bytes = value.to_le_bytes();
    buf[offset..offset + 4].copy_from_slice(&bytes);
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_uf2(num_blocks: u32, family_id: Option<u32>) -> Vec<u8> {
        let mut data = Vec::new();
        for i in 0..num_blocks {
            let payload = vec![0xAA_u8; 256];
            let block = build_uf2_block(
                i,
                num_blocks,
                0x10000000 + i * 256,
                family_id,
                &payload,
            );
            data.extend_from_slice(&block);
        }
        data
    }

    #[test]
    fn test_parse_valid_uf2() {
        let data = make_test_uf2(4, Some(0xe48bff56));
        let blocks = parse_uf2(&data).unwrap();
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[0].block_no, 0);
        assert_eq!(blocks[3].block_no, 3);
    }

    #[test]
    fn test_parse_uf2_magic_validation() {
        let mut data = make_test_uf2(1, None);
        // Corrupt the first magic
        data[0] = 0xFF;
        let result = parse_uf2(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("bad magic"));
    }

    #[test]
    fn test_parse_uf2_end_magic_validation() {
        let mut data = make_test_uf2(1, None);
        // Corrupt end magic
        data[508] = 0xFF;
        let result = parse_uf2(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("end magic"));
    }

    #[test]
    fn test_parse_uf2_wrong_size() {
        // Not a multiple of 512
        let data = vec![0u8; 600];
        let result = parse_uf2(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("multiple of 512"));
    }

    #[test]
    fn test_parse_uf2_empty() {
        let result = parse_uf2(&[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_firmware_info_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("firmware_v1.2.3.uf2");
        let data = make_test_uf2(10, Some(0xe48bff56)); // RP2040
        std::fs::write(&path, &data).unwrap();

        let flasher = FirmwareFlasher::new();
        let info = flasher.parse_uf2(&path).unwrap();

        assert_eq!(info.block_count, 10);
        assert_eq!(info.size_bytes, 5120);
        assert!(info.target.contains("RP2040"));
        assert!(info.address_range.is_some());
        let (start, end) = info.address_range.unwrap();
        assert_eq!(start, 0x10000000);
        assert!(end > start);
        assert_eq!(info.version, "v1.2.3");
    }

    #[test]
    fn test_firmware_info_unknown_family() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.uf2");
        let data = make_test_uf2(2, None);
        std::fs::write(&path, &data).unwrap();

        let flasher = FirmwareFlasher::new();
        let info = flasher.parse_uf2(&path).unwrap();
        assert_eq!(info.family_id, None);
        assert!(info.target.contains("Unknown"));
    }

    #[test]
    fn test_parse_uf2_block_count_mismatch() {
        // Create a file with 3 blocks but header says 4
        let mut data = make_test_uf2(4, None);
        // Truncate to 3 blocks
        data.truncate(3 * UF2_BLOCK_SIZE);
        let result = parse_uf2(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_family_id_names() {
        assert!(family_id_to_name(0xe48bff56).unwrap().contains("RP2040"));
        assert!(family_id_to_name(0x68ed2b88).unwrap().contains("nRF52840"));
        assert!(family_id_to_name(0x1dc11454).unwrap().contains("ESP32"));
        assert!(family_id_to_name(0x00000000).is_none());
    }

    #[test]
    fn test_uf2_flag_has_family_id() {
        let block_with_family = build_uf2_block(0, 1, 0, Some(0xe48bff56), &[]);
        let parsed = parse_single_block(&block_with_family, 0).unwrap();
        assert!(parsed.has_family_id());
        assert_eq!(parsed.file_size_or_family_id, 0xe48bff56);
    }

    #[test]
    fn test_uf2_flag_no_family_id() {
        let block_no_family = build_uf2_block(0, 1, 0, None, &[]);
        let parsed = parse_single_block(&block_no_family, 0).unwrap();
        assert!(!parsed.has_family_id());
    }

    #[test]
    fn test_detect_bootloader_no_panic() {
        let flasher = FirmwareFlasher::new();
        let _ = flasher.detect_bootloader(); // may return None in CI
    }

    #[test]
    fn test_verify_flash_nonexistent_path() {
        let flasher = FirmwareFlasher::new();
        let result = flasher.verify_flash("/tmp/nonexistent_bootloader_xyz");
        // Non-existent path → device re-enumerated → success
        assert!(result.is_ok());
        assert!(result.unwrap().success);
    }

    #[test]
    fn test_extract_version_from_name() {
        assert_eq!(
            extract_version_from_name(Path::new("firmware_v1.2.3.uf2")),
            "v1.2.3"
        );
        assert_eq!(
            extract_version_from_name(Path::new("pico-2.0.0.uf2")),
            "2.0.0"
        );
        assert_eq!(
            extract_version_from_name(Path::new("firmware.uf2")),
            "unknown"
        );
    }

    #[test]
    fn test_build_uf2_round_trip() {
        let payload = b"Hello, Pico!";
        let block = build_uf2_block(0, 1, 0x20000000, Some(0xe48bff56), payload);
        let parsed = parse_single_block(&block, 0).unwrap();
        assert_eq!(parsed.target_addr, 0x20000000);
        assert_eq!(parsed.payload_size, payload.len() as u32);
        assert_eq!(&parsed.data[..payload.len()], payload);
        assert!(parsed.has_family_id());
        assert_eq!(parsed.file_size_or_family_id, 0xe48bff56);
    }
}
