//! Mobile shell scaffold — shares the `web/` build with responsive layout.
//!
//! Build: `cd crates/clawz-tauri-mobile && cargo tauri android init` (first time),
//! then `cargo tauri android build` / `cargo tauri ios build`.

#![cfg_attr(mobile, no_main)]

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("clawz mobile shell");
}
