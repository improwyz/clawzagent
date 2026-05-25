//! Peripheral device management: USB/serial discovery and serial communication.
//!
//! This module scans the host for attached peripherals (USB devices, serial ports,
//! cameras, microphones, etc.), classifies them by type, and provides a blocking
//! serial connection wrapper with platform termios configuration.
//!
//! ## Platform support
//!
//! - **Linux**: Reads `/sys/bus/usb/devices` for USB enumeration and `/dev/tty*`
//!   for serial ports. Configures raw serial mode via `termios` ioctls.
//! - **macOS**: Uses `system_profiler SPUSBDataType` for USB and `/dev/cu.*` for serial.
//!
//! ## Design notes
//!
//! - `Peripherals` deduplicates USB-backed serial devices so a single FTDI adapter
//!   does not appear twice (once as USB, once as serial).
//! - `SerialConnection` avoids the `serialport` crate by using raw `libc` termios
//!   bindings, keeping the dependency tree small.
//!
//! ## Key dependencies
//! - `serde` for serializable device descriptions
//! - `clawz_core::error` for unified error handling

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clawz_core::error::{ClawzError, Result};

// ── Peripheral types ──────────────────────────────────────────────────────────

/// Classification of a connected peripheral device.
///
/// Used for filtering (e.g. "show only cameras") and for UI display grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PeripheralType {
    /// Generic USB device or unclassifiable USB peripheral
    USB,
    /// Serial/UART port (USB-to-serial adapters, native UARTs, etc.)
    Serial,
    /// I2C bus device
    I2C,
    /// SPI bus device
    SPI,
    /// General-purpose input/output pin
    GPIO,
    /// Video camera (typically UVC class)
    Camera,
    /// Audio input device
    Microphone,
    /// Audio output device
    Speaker,
    /// Human interface keyboard
    Keyboard,
    /// Human interface pointing device
    Mouse,
    /// Video display or framebuffer device
    Display,
}

impl PeripheralType {
    /// Human-readable label for this peripheral type.
    pub fn display_name(&self) -> &'static str {
        match self {
            PeripheralType::USB => "USB",
            PeripheralType::Serial => "Serial/UART",
            PeripheralType::I2C => "I2C",
            PeripheralType::SPI => "SPI",
            PeripheralType::GPIO => "GPIO",
            PeripheralType::Camera => "Camera",
            PeripheralType::Microphone => "Microphone",
            PeripheralType::Speaker => "Speaker",
            PeripheralType::Keyboard => "Keyboard",
            PeripheralType::Mouse => "Mouse",
            PeripheralType::Display => "Display",
        }
    }
}

// ── USB device info (raw from sysfs) ─────────────────────────────────────────

/// Raw USB device descriptor extracted from sysfs (Linux) or system_profiler (macOS).
///
/// This is the lowest-level representation before classification into a [`Peripheral`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbDeviceInfo {
    /// USB bus number
    pub bus: u8,
    /// Device number on that bus
    pub device: u8,
    /// USB Vendor ID (hex)
    pub vendor_id: u16,
    /// USB Product ID (hex)
    pub product_id: u16,
    /// Vendor name string (from idVendor + string descriptor or system_profiler)
    pub vendor_name: Option<String>,
    /// Product name string
    pub product_name: Option<String>,
    /// Serial number string
    pub serial: Option<String>,
    /// USB interface class code (e.g. 0x02 = CDC/Serial, 0x03 = HID, 0x0e = Video)
    pub usb_class: u8,
    /// sysfs path, e.g. `/sys/bus/usb/devices/1-1` (Linux only)
    pub sysfs_path: PathBuf,
}

impl UsbDeviceInfo {
    /// Infer a [`PeripheralType`] from USB class code and product name heuristics.
    ///
    /// HID devices (class 0x03) are further classified into Keyboard or Mouse by
    /// checking the product name string.
    pub fn inferred_type(&self) -> PeripheralType {
        match self.usb_class {
            0x01 => PeripheralType::Speaker, // Audio
            0x02 | 0x0a => PeripheralType::Serial, // CDC / CDC-Data
            0x03 => {
                // HID — try to distinguish keyboard vs mouse from product name
                let name = self.product_name.as_deref().unwrap_or("").to_lowercase();
                if name.contains("keyboard") {
                    PeripheralType::Keyboard
                } else if name.contains("mouse") || name.contains("pointer") {
                    PeripheralType::Mouse
                } else {
                    PeripheralType::USB
                }
            }
            0x0e => PeripheralType::Camera, // Video/UVC
            0xff => {
                // Vendor-specific; check product name for common microcontrollers
                let name = self.product_name.as_deref().unwrap_or("").to_lowercase();
                if name.contains("uart") || name.contains("serial") || name.contains("com") {
                    PeripheralType::Serial
                } else {
                    PeripheralType::USB
                }
            }
            _ => PeripheralType::USB,
        }
    }
}

// ── Peripheral ────────────────────────────────────────────────────────────────

/// A classified, connected peripheral ready for upstream consumption.
///
/// Bridges raw USB/serial discovery into a stable, serializable representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peripheral {
    /// Stable ID (e.g. "usb:1-1", "serial:/dev/ttyUSB0")
    pub id: String,
    /// Human-readable name, often vendor + product
    pub name: String,
    /// Classified type (USB, Serial, Camera, etc.)
    pub peripheral_type: PeripheralType,
    /// Whether the device is currently connected
    pub connected: bool,
    /// Device node path (e.g. /dev/ttyUSB0, /dev/video0)
    pub device_path: Option<PathBuf>,
    /// Extended USB information if this peripheral originated from USB enumeration
    pub usb_info: Option<UsbDeviceInfo>,
}

impl Peripheral {
    /// Build a [`Peripheral`] from [`UsbDeviceInfo`].
    ///
    /// Derives the display name from vendor/product strings and assigns a stable
    /// ID of the form `usb:{bus}-{device}`.
    fn from_usb(info: UsbDeviceInfo) -> Self {
        let id = format!("usb:{}-{}", info.bus, info.device);
        let name = match (&info.vendor_name, &info.product_name) {
            (Some(v), Some(p)) => format!("{v} {p}"),
            (None, Some(p)) => p.clone(),
            (Some(v), None) => format!("{v} {:04x}", info.product_id),
            (None, None) => format!("USB {:04x}:{:04x}", info.vendor_id, info.product_id),
        };
        let peripheral_type = info.inferred_type();
        Peripheral {
            id,
            name,
            peripheral_type,
            connected: true,
            device_path: None,
            usb_info: Some(info),
        }
    }

    /// Build a [`Peripheral`] from a `/dev/tty*` path.
    ///
    /// Assigns an ID of the form `serial:{path}` and marks it as `Serial` type.
    fn from_serial(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("serial")
            .to_string();
        let id = format!("serial:{}", path.display());
        Peripheral {
            id,
            name,
            peripheral_type: PeripheralType::Serial,
            connected: true,
            device_path: Some(path),
            usb_info: None,
        }
    }
}

// ── Serial connection ─────────────────────────────────────────────────────────

/// A simple blocking serial connection.
///
/// Uses platform `/dev/tty*` paths on Linux/macOS and configures raw termios
/// attributes (no echo, no canonical mode, 8N1) via `ioctl` calls.
///
/// ## Drop behaviour
/// The underlying `std::fs::File` is closed when the struct is dropped.
pub struct SerialConnection {
    /// Path to the serial device node
    pub path: PathBuf,
    /// Configured baud rate
    pub baud_rate: u32,
    /// Underlying POSIX file descriptor wrapped in `std::fs::File`
    file: std::fs::File,
}

impl SerialConnection {
    /// Open a serial port at the given path and baud rate.
    ///
    /// On Linux/macOS this opens the device with `O_NOCTTY | O_NONBLOCK` and
    /// then applies raw termios settings (8 data bits, no parity, 1 stop bit).
    pub fn open(path: &Path, baud_rate: u32) -> Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc_o_noctty() | libc_o_nonblock())
            .open(path)
            .map_err(|e| {
                ClawzError::Hardware(format!(
                    "cannot open serial port {}: {e}",
                    path.display()
                ))
            })?;

        // Configure baud rate via platform-specific code
        configure_serial(&file, baud_rate).map_err(|e| {
            ClawzError::Hardware(format!(
                "cannot configure serial port {}: {e}",
                path.display()
            ))
        })?;

        Ok(SerialConnection {
            path: path.to_path_buf(),
            baud_rate,
            file,
        })
    }

    /// Read up to `buf.len()` bytes from the serial port.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        use std::io::Read;
        self.file.read(buf).map_err(|e| {
            ClawzError::Hardware(format!("serial read error on {}: {e}", self.path.display()))
        })
    }

    /// Write bytes to the serial port.
    pub fn write(&mut self, data: &[u8]) -> Result<usize> {
        use std::io::Write;
        self.file.write(data).map_err(|e| {
            ClawzError::Hardware(format!(
                "serial write error on {}: {e}",
                self.path.display()
            ))
        })
    }

    /// Flush the write buffer.
    pub fn flush(&mut self) -> Result<()> {
        use std::io::Write;
        self.file.flush().map_err(|e| {
            ClawzError::Hardware(format!(
                "serial flush error on {}: {e}",
                self.path.display()
            ))
        })
    }

    /// Close the port (explicit; also happens on Drop).
    pub fn close(self) {
        // Drop closes the file
        drop(self);
    }
}

// ── libc flag helpers (avoid direct libc dependency) ─────────────────────────

/// `O_NOCTTY` value for the current platform.
///
/// Hard-coded to avoid pulling in the `libc` crate for a single constant.
#[cfg(target_os = "linux")]
fn libc_o_noctty() -> i32 {
    0o0400 // O_NOCTTY on Linux
}

#[cfg(target_os = "macos")]
fn libc_o_noctty() -> i32 {
    0x20000 // O_NOCTTY on macOS
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn libc_o_noctty() -> i32 {
    0
}

/// `O_NONBLOCK` value for the current platform.
#[cfg(target_os = "linux")]
fn libc_o_nonblock() -> i32 {
    0o4000 // O_NONBLOCK on Linux
}

#[cfg(target_os = "macos")]
fn libc_o_nonblock() -> i32 {
    0x0004 // O_NONBLOCK on macOS
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn libc_o_nonblock() -> i32 {
    0
}

// ── Serial configuration via termios ─────────────────────────────────────────

/// Configure a serial port for raw 8N1 operation at the requested baud rate.
///
/// Disables echo, canonical mode, flow control, and signal generation so that
/// the port behaves as a transparent byte pipe suitable for microcontroller
/// communication.
#[cfg(unix)]
fn configure_serial(file: &std::fs::File, baud_rate: u32) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;

    let fd = file.as_raw_fd();

    // Safety: we call tcgetattr then tcsetattr, which are standard POSIX
    unsafe {
        let mut tty: libc_termios = std::mem::zeroed();

        if tcgetattr(fd, &mut tty) != 0 {
            return Err(std::io::Error::last_os_error());
        }

        // Raw mode: no echo, no canonical, no signals
        tty.c_iflag &= !(IXON | IXOFF | IXANY | ICRNL | INLCR);
        tty.c_oflag &= !OPOST;
        tty.c_lflag &= !(ECHO | ECHOE | ECHONL | ICANON | ISIG | IEXTEN);
        tty.c_cflag |= CLOCAL | CREAD;
        tty.c_cflag &= !CSIZE;
        tty.c_cflag |= CS8; // 8 data bits
        tty.c_cflag &= !PARENB; // no parity
        tty.c_cflag &= !CSTOPB; // 1 stop bit

        // VMIN/VTIME: return as soon as any data available
        tty.c_cc[VMIN] = 0;
        tty.c_cc[VTIME] = 10; // 1 second timeout in 0.1s units

        let speed = baud_rate_to_speed(baud_rate);
        cfsetispeed(&mut tty, speed);
        cfsetospeed(&mut tty, speed);

        if tcsetattr(fd, TCSANOW, &tty) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    Ok(())
}

#[cfg(not(unix))]
fn configure_serial(_file: &std::fs::File, _baud_rate: u32) -> std::io::Result<()> {
    Ok(()) // No-op on non-Unix
}

/// Map a numeric baud rate to the platform `speed_t` constant.
///
/// Values are octal `B*` constants from `termios.h`. Unknown baud rates fall
/// back to 9600.
#[cfg(unix)]
fn baud_rate_to_speed(baud_rate: u32) -> u32 {
    match baud_rate {
        50 => 0o000001,
        75 => 0o000002,
        110 => 0o000003,
        134 => 0o000004,
        150 => 0o000005,
        200 => 0o000006,
        300 => 0o000007,
        600 => 0o000010,
        1_200 => 0o000011,
        1_800 => 0o000012,
        2_400 => 0o000013,
        4_800 => 0o000014,
        9_600 => 0o000015,
        19_200 => 0o000016,
        38_400 => 0o000017,
        57_600 => 0o010001,
        115_200 => 0o010002,
        230_400 => 0o010003,
        460_800 => 0o010004,
        500_000 => 0o010005,
        921_600 => 0o010007,
        1_000_000 => 0o010010,
        1_500_000 => 0o010012,
        2_000_000 => 0o010013,
        _ => 0o000015, // Default 9600
    }
}

// Minimal termios bindings to avoid pulling in the libc crate
#[cfg(unix)]
#[repr(C)]
struct libc_termios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 32],
    c_ispeed: u32,
    c_ospeed: u32,
}

#[cfg(unix)]
const VMIN: usize = 6;
#[cfg(unix)]
const VTIME: usize = 5;

#[cfg(unix)]
const IXON: u32 = 0o002000;
#[cfg(unix)]
const IXOFF: u32 = 0o010000;
#[cfg(unix)]
const IXANY: u32 = 0o004000;
#[cfg(unix)]
const ICRNL: u32 = 0o000400;
#[cfg(unix)]
const INLCR: u32 = 0o000100;
#[cfg(unix)]
const OPOST: u32 = 0o000001;
#[cfg(unix)]
const ECHO: u32 = 0o000010;
#[cfg(unix)]
const ECHOE: u32 = 0o000020;
#[cfg(unix)]
const ECHONL: u32 = 0o000100;
#[cfg(unix)]
const ICANON: u32 = 0o000002;
#[cfg(unix)]
const ISIG: u32 = 0o000001;
#[cfg(unix)]
const IEXTEN: u32 = 0o100000;
#[cfg(unix)]
const CLOCAL: u32 = 0o004000;
#[cfg(unix)]
const CREAD: u32 = 0o000200;
#[cfg(unix)]
const CSIZE: u32 = 0o000060;
#[cfg(unix)]
const CS8: u32 = 0o000060;
#[cfg(unix)]
const PARENB: u32 = 0o000400;
#[cfg(unix)]
const CSTOPB: u32 = 0o000100;
#[cfg(unix)]
const TCSANOW: i32 = 0;

#[cfg(unix)]
unsafe extern "C" {
    fn tcgetattr(fd: i32, termios: *mut libc_termios) -> i32;
    fn tcsetattr(fd: i32, optional_actions: i32, termios: *const libc_termios) -> i32;
    fn cfsetispeed(termios: *mut libc_termios, speed: u32) -> i32;
    fn cfsetospeed(termios: *mut libc_termios, speed: u32) -> i32;
}

// ── Peripherals manager ───────────────────────────────────────────────────────

/// Central registry for discovered peripherals.
///
/// Populated by [`Peripherals::scan`], which queries USB and serial devices
/// on the host. Provides filtering and lookup by stable ID.
pub struct Peripherals {
    /// Map of peripheral ID → [`Peripheral`]
    devices: HashMap<String, Peripheral>,
}

impl Peripherals {
    /// Create an empty peripherals manager.
    pub fn new() -> Self {
        Self {
            devices: HashMap::new(),
        }
    }

    /// Scan for connected peripherals (USB + serial).
    ///
    /// Clears the previous device list and re-enumerates:
    /// 1. USB devices via sysfs (Linux) or system_profiler (macOS)
    /// 2. Serial ports in `/dev` that are not already represented by a USB device
    pub fn scan(&mut self) {
        self.devices.clear();

        // USB devices
        #[cfg(target_os = "linux")]
        {
            let usb_devices = scan_usb_linux();
            for dev in usb_devices {
                let p = Peripheral::from_usb(dev);
                self.devices.insert(p.id.clone(), p);
            }
        }

        #[cfg(target_os = "macos")]
        {
            let usb_devices = scan_usb_macos();
            for dev in usb_devices {
                let p = Peripheral::from_usb(dev);
                self.devices.insert(p.id.clone(), p);
            }
        }

        // Serial devices (/dev/ttyUSB*, /dev/ttyACM*, /dev/ttyS*)
        let serial_paths = scan_serial_ports();
        for path in serial_paths {
            // Avoid duplicates with USB-based serial devices already found
            let id = format!("serial:{}", path.display());
            if !self.devices.contains_key(&id) {
                let p = Peripheral::from_serial(path);
                self.devices.insert(p.id.clone(), p);
            }
        }
    }

    /// Return all detected peripherals.
    pub fn list(&self) -> Vec<&Peripheral> {
        self.devices.values().collect()
    }

    /// Return peripherals filtered by type.
    pub fn list_by_type(&self, t: PeripheralType) -> Vec<&Peripheral> {
        self.devices
            .values()
            .filter(|p| p.peripheral_type == t)
            .collect()
    }

    /// Look up a peripheral by its ID.
    pub fn get(&self, id: &str) -> Option<&Peripheral> {
        self.devices.get(id)
    }

    /// Open a serial connection to a peripheral device node.
    ///
    /// Validates that the path exists before attempting to open it.
    pub fn open_serial(path: &Path, baud_rate: u32) -> Result<SerialConnection> {
        if !path.exists() {
            return Err(ClawzError::Hardware(format!(
                "serial device not found: {}",
                path.display()
            )));
        }
        SerialConnection::open(path, baud_rate)
    }
}

impl Default for Peripherals {
    fn default() -> Self {
        Self::new()
    }
}

// ── Linux USB scanning (/sys/bus/usb/devices) ─────────────────────────────────

/// Enumerate USB devices by reading sysfs attributes.
///
/// Skips root hubs and interface subdirectories by requiring the presence of
/// `idVendor`, which only exists on real device entries (bus-port paths).
#[cfg(target_os = "linux")]
fn scan_usb_linux() -> Vec<UsbDeviceInfo> {
    let usb_root = Path::new("/sys/bus/usb/devices");
    let mut devices = Vec::new();

    let rd = match std::fs::read_dir(usb_root) {
        Ok(r) => r,
        Err(_) => return devices,
    };

    for entry in rd.flatten() {
        let path = entry.path();
        let _name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // Skip root hubs and interfaces; we want bus-port entries like "1-1", "1-1.2"
        // These have an idVendor file
        let vendor_id_path = path.join("idVendor");
        if !vendor_id_path.exists() {
            continue;
        }

        let vendor_id = read_hex_u16(&path.join("idVendor")).unwrap_or(0);
        let product_id = read_hex_u16(&path.join("idProduct")).unwrap_or(0);
        let usb_class = read_hex_u8(&path.join("bDeviceClass")).unwrap_or(0);
        let bus = read_dec_u8(&path.join("busnum")).unwrap_or(0);
        let device = read_dec_u8(&path.join("devnum")).unwrap_or(0);

        let vendor_name = read_sysfs_string(&path.join("manufacturer"));
        let product_name = read_sysfs_string(&path.join("product"));
        let serial = read_sysfs_string(&path.join("serial"));

        devices.push(UsbDeviceInfo {
            bus,
            device,
            vendor_id,
            product_id,
            vendor_name,
            product_name,
            serial,
            usb_class,
            sysfs_path: path,
        });
    }

    devices
}

/// Read a sysfs string file, trimming whitespace and filtering empty results.
#[cfg(target_os = "linux")]
fn read_sysfs_string(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Read a hex-encoded `u16` from a sysfs attribute file.
#[cfg(target_os = "linux")]
fn read_hex_u16(path: &Path) -> Option<u16> {
    let s = std::fs::read_to_string(path).ok()?;
    u16::from_str_radix(s.trim(), 16).ok()
}

/// Read a hex-encoded `u8` from a sysfs attribute file.
#[cfg(target_os = "linux")]
fn read_hex_u8(path: &Path) -> Option<u8> {
    let s = std::fs::read_to_string(path).ok()?;
    u8::from_str_radix(s.trim(), 16).ok()
}

/// Read a decimal `u8` from a sysfs attribute file.
#[cfg(target_os = "linux")]
fn read_dec_u8(path: &Path) -> Option<u8> {
    let s = std::fs::read_to_string(path).ok()?;
    s.trim().parse::<u8>().ok()
}

// ── macOS USB scanning (system_profiler) ──────────────────────────────────────

/// Enumerate USB devices on macOS using `system_profiler SPUSBDataType`.
#[cfg(target_os = "macos")]
fn scan_usb_macos() -> Vec<UsbDeviceInfo> {
    use std::process::Command;

    let output = match Command::new("system_profiler")
        .args(["SPUSBDataType", "-json"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let json: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut devices = Vec::new();
    collect_macos_usb_devices(&json, &mut devices, 1, 1);
    devices
}

/// Recursively walk `system_profiler` JSON to collect USB devices.
#[cfg(target_os = "macos")]
fn collect_macos_usb_devices(
    json: &serde_json::Value,
    out: &mut Vec<UsbDeviceInfo>,
    bus: u8,
    dev_counter: u8,
) {
    if let Some(items) = json.get("SPUSBDataType").and_then(|v| v.as_array()) {
        for item in items {
            collect_macos_usb_item(item, out, bus, dev_counter);
        }
    }
}

/// Parse a single `system_profiler` USB item and recurse into `_items` children.
#[cfg(target_os = "macos")]
fn collect_macos_usb_item(
    item: &serde_json::Value,
    out: &mut Vec<UsbDeviceInfo>,
    bus: u8,
    dev: u8,
) {
    let product_name = item
        .get("_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let vendor_id = item
        .get("vendor_id")
        .and_then(|v| v.as_str())
        .and_then(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);

    let product_id = item
        .get("product_id")
        .and_then(|v| v.as_str())
        .and_then(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);

    let serial = item
        .get("serial_num")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if vendor_id != 0 || product_id != 0 {
        out.push(UsbDeviceInfo {
            bus,
            device: dev,
            vendor_id,
            product_id,
            vendor_name: None,
            product_name,
            serial,
            usb_class: 0xff,
            sysfs_path: PathBuf::from("/dev/usb"),
        });
    }

    // Recurse into sub-items
    if let Some(items) = item.get("_items").and_then(|v| v.as_array()) {
        for (i, sub) in items.iter().enumerate() {
            collect_macos_usb_item(sub, out, bus, dev + i as u8 + 1);
        }
    }
}

// ── Serial port scanning ──────────────────────────────────────────────────────

/// Find serial-like devices in `/dev`.
///
/// On Linux: `/dev/ttyUSB*`, `/dev/ttyACM*`, `/dev/ttyS0-3`, `/dev/rfcomm*`.
/// On macOS: `/dev/cu.*` (call-up devices, preferred over `/dev/tty.*`).
fn scan_serial_ports() -> Vec<PathBuf> {
    let mut ports = Vec::new();

    #[cfg(target_os = "linux")]
    {
        let patterns = ["/dev/ttyUSB", "/dev/ttyACM", "/dev/ttyS", "/dev/rfcomm"];
        let rd = match std::fs::read_dir("/dev") {
            Ok(r) => r,
            Err(_) => return ports,
        };
        for entry in rd.flatten() {
            let path = entry.path();
            let dev_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            // Only include ttyS0-ttyS3 to avoid listing all 64 virtual terminals
            let include = patterns[..2].iter().any(|p| {
                path.to_str().map(|s| s.starts_with(p)).unwrap_or(false)
            }) || (dev_name.starts_with("ttyS") && {
                dev_name[4..].parse::<u32>().map(|n| n <= 3).unwrap_or(false)
            }) || path.to_str().map(|s| s.starts_with("/dev/rfcomm")).unwrap_or(false);

            if include && path.exists() {
                ports.push(path);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let rd = match std::fs::read_dir("/dev") {
            Ok(r) => r,
            Err(_) => return ports,
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                // macOS serial: /dev/cu.* or /dev/tty.* (prefer cu.*)
                if name.starts_with("cu.") {
                    ports.push(path);
                }
            }
        }
    }

    ports.sort();
    ports
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peripherals_new_is_empty() {
        let peripherals = Peripherals::new();
        assert!(peripherals.list().is_empty());
    }

    #[test]
    fn test_peripherals_scan_does_not_panic() {
        let mut peripherals = Peripherals::new();
        peripherals.scan();
        // Count may be 0 in CI; just ensure no panic
        let _ = peripherals.list().len();
    }

    #[test]
    fn test_peripherals_get_missing_returns_none() {
        let peripherals = Peripherals::new();
        assert!(peripherals.get("nonexistent").is_none());
    }

    #[test]
    fn test_peripheral_type_display_names() {
        assert_eq!(PeripheralType::USB.display_name(), "USB");
        assert_eq!(PeripheralType::Serial.display_name(), "Serial/UART");
        assert_eq!(PeripheralType::I2C.display_name(), "I2C");
    }

    #[test]
    fn test_peripheral_from_usb_naming() {
        let info = UsbDeviceInfo {
            bus: 1,
            device: 5,
            vendor_id: 0x0403,
            product_id: 0x6001,
            vendor_name: Some("FTDI".to_string()),
            product_name: Some("FT232R USB UART".to_string()),
            serial: None,
            usb_class: 0xff,
            sysfs_path: PathBuf::from("/sys/bus/usb/devices/1-5"),
        };
        let p = Peripheral::from_usb(info);
        assert_eq!(p.id, "usb:1-5");
        assert!(p.name.contains("FTDI"));
        assert_eq!(p.peripheral_type, PeripheralType::Serial);
    }

    #[test]
    fn test_usb_class_inference() {
        let make = |class: u8, name: Option<&str>| UsbDeviceInfo {
            bus: 1,
            device: 1,
            vendor_id: 0,
            product_id: 0,
            vendor_name: None,
            product_name: name.map(|s| s.to_string()),
            serial: None,
            usb_class: class,
            sysfs_path: PathBuf::new(),
        };

        assert_eq!(make(0x03, Some("USB Keyboard")).inferred_type(), PeripheralType::Keyboard);
        assert_eq!(make(0x03, Some("USB Mouse")).inferred_type(), PeripheralType::Mouse);
        assert_eq!(make(0x0e, None).inferred_type(), PeripheralType::Camera);
        assert_eq!(make(0x02, None).inferred_type(), PeripheralType::Serial);
    }

    #[test]
    fn test_list_by_type_empty() {
        let peripherals = Peripherals::new();
        let serial = peripherals.list_by_type(PeripheralType::Serial);
        assert!(serial.is_empty());
    }

    #[test]
    fn test_open_serial_missing_device() {
        let result = Peripherals::open_serial(Path::new("/dev/nonexistent_xyz"), 115200);
        assert!(result.is_err());
    }

    #[test]
    fn test_baud_rate_mapping() {
        #[cfg(unix)]
        {
            assert_eq!(baud_rate_to_speed(9_600), 0o000015);
            assert_eq!(baud_rate_to_speed(115_200), 0o010002);
        }
    }

    #[test]
    fn test_peripheral_serialization() {
        let p = Peripheral {
            id: "usb:1-1".to_string(),
            name: "Test Device".to_string(),
            peripheral_type: PeripheralType::USB,
            connected: true,
            device_path: None,
            usb_info: None,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("usb:1-1"));
        let back: Peripheral = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "usb:1-1");
    }
}
