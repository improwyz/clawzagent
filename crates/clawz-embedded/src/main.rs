//! ESP32-S3 entry point for clawz-embedded T0 runtime.
//!
//! This binary is only for host testing. Actual ESP32 targets need
//! esp-idf-sys and proper target triple (xtensa-esp32s3-noneelf).

#![cfg_attr(not(feature = "no_std"), allow(dead_code))]

fn main() {
    #[cfg(feature = "std")]
    {
        eprintln!("clawz-embedded: running in std mode (host test)");
        eprintln!("Note: For actual ESP32-S3, compile with no_std feature and xtensa-esp32s3-noneelf target");
    }

    #[cfg(not(feature = "std"))]
    {
        // On bare-metal, we would initialize embassy executor here
        // For now, just halt since we can't run without actual hardware
        loop {
            // halt
        }
    }
}
