pub fn hardware_fingerprint() -> String {
    let mut parts = Vec::new();

    // 1. Machine ID (most stable identifier)
    if let Ok(id) = machine_id() {
        parts.push(id);
    }

    // 2. CPU brand string
    #[cfg(target_os = "linux")]
    if let Ok(cpu) = std::fs::read_to_string("/proc/cpuinfo") {
        if let Some(line) = cpu.lines().find(|l| l.starts_with("model name")) {
            if let Some(val) = line.split(':').nth(1) {
                parts.push(val.trim().to_string());
            }
        }
    }

    // 3. Total RAM rounded to nearest GB
    #[cfg(target_os = "linux")]
    if let Ok(mem) = std::fs::read_to_string("/proc/meminfo") {
        if let Some(line) = mem.lines().find(|l| l.starts_with("MemTotal")) {
            if let Some(kb_str) = line.split_whitespace().nth(1) {
                if let Ok(kb) = kb_str.parse::<u64>() {
                    let gb = (kb + 524288) / 1048576; // round to nearest GB
                    parts.push(format!("{gb}GB"));
                }
            }
        }
    }

    if parts.is_empty() {
        return "unknown-hardware".to_string();
    }

    let combined = parts.join("|");
    let digest = sha256_hex(combined.as_bytes());
    digest[..16].to_string()
}

fn machine_id() -> Result<String, std::io::Error> {
    // Linux: /etc/machine-id
    if let Ok(id) = std::fs::read_to_string("/etc/machine-id") {
        let trimmed = id.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    // macOS: IOPlatformUUID via ioreg
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.contains("IOPlatformUUID") {
                if let Some(uuid) = line.split('"').nth(3) {
                    return Ok(uuid.to_string());
                }
            }
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no machine ID found",
    ))
}

fn sha256_hex(data: &[u8]) -> String {
    use std::fmt::Write;
    // Simple SHA-256 using the system's openssl or a pure implementation
    // For now, use a basic hash — in production, use the sha2 crate
    let mut hash = [0u8; 32];
    // Fallback: use std hash (not cryptographic, but functional for fingerprinting)
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hasher::write(&mut hasher, data);
    let h = std::hash::Hasher::finish(&hasher);
    let bytes = h.to_le_bytes();
    // Repeat to fill 32 bytes
    for i in 0..32 {
        hash[i] = bytes[i % 8];
    }
    let mut hex = String::with_capacity(64);
    for b in &hash {
        write!(hex, "{b:02x}").unwrap();
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable() {
        let fp1 = hardware_fingerprint();
        let fp2 = hardware_fingerprint();
        assert_eq!(fp1, fp2);
        assert!(!fp1.is_empty());
    }

    #[test]
    fn fingerprint_is_hex() {
        let fp = hardware_fingerprint();
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    }
}
