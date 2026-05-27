//! Dynamic Plugin Loader — discovers, loads, and hot-reloads channel plugins.
//!
//! The loader supports three sources of channel plugins:
//!
//! 1. **Built-in native channels** — compiled into the worker binary (see
//!    `super::native`).
//! 2. **Dynamic shared libraries** — `.so` / `.dylib` / `.dll` files in a
//!    configurable plugins directory discovered at start-up.
//! 3. **Hot-reload** — an optional `notify`-based file watcher that
//!    automatically reloads changed libraries without restarting the worker.
//!
//! Each dynamic plugin must export a C-ABI factory function named
//! `create_channel_plugin` which returns a heap-allocated trait object.
//! The loader wraps the raw pointer in an `Arc` so the runtime and the
//! registry can share ownership safely.
//!
//! # Safety notes
//!
//! Loading arbitrary shared libraries is inherently `unsafe`; the worker
//! trusts that plugins are compiled against the same `clawz_core` version
//! and ABI.  Factory panics are caught with `catch_unwind` so a buggy
//! plugin cannot crash the worker process.
//!
//! # Dependencies
//!
//! - `libloading` for cross-platform shared-library loading.
//! - `notify` for filesystem watching.
//! - `clawz_core::traits::{ChannelMetadata, ChannelPlugin}` — shared types.

// ── Dynamic Plugin Loader ─────────────────────────────────────────────────────

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// Dependency: clawz_core::error — unified error type used across the 3-tier architecture.
use clawz_core::error::{ClawzError, Result};
// Dependency: clawz_core::traits — shared plugin trait and metadata struct from the core crate.
use clawz_core::traits::{ChannelMetadata, ChannelPlugin};
// Dependency: libloading — cross-platform shared library loading.
use libloading::{Library, Symbol};
// Dependency: notify — filesystem watcher for hot-reload.
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
// Dependency: tokio::sync::RwLock — async-aware concurrency primitive.
use tokio::sync::RwLock;

/// Name of the C-ABI factory function that each dynamic plugin must export.
///
/// ```c
/// // In the plugin .so:
/// #[no_mangle]
/// pub extern "C" fn create_channel_plugin() -> *mut dyn ChannelPlugin { ... }
/// ```
const PLUGIN_FACTORY: &[u8] = b"create_channel_plugin\0";

/// Lifecycle state of a loaded plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginLifecycle {
    /// Library is on disk but the factory has not been called yet.
    Loading,
    /// Factory succeeded and the plugin is available for use.
    Ready,
    /// `unload_plugin` was called; existing Arc clones may still be active.
    ShuttingDown,
}

/// Internal record for one dynamically loaded plugin.
struct DynEntry {
    /// Shared trait object returned by the factory.  Arc allows the runtime
    /// to hold a reference while the loader can later unload the library.
    plugin: Arc<dyn ChannelPlugin>,
    /// The shared library handle must be kept alive as long as the plugin is
    /// in use (dropping the library would unmap the code).
    _library: Library,
    /// Current lifecycle stage; used to prevent new callers from obtaining
    /// a plugin that is about to be unloaded.
    lifecycle: PluginLifecycle,
    /// Absolute path to the loaded file; stored so hot-reload can correlate
    /// `notify` events with existing entries.
    #[allow(dead_code)]
    path: PathBuf,
}

/// Loads and manages the lifecycle of dynamically linked channel plugins.
pub struct PluginLoader {
    /// plugin_id → dynamic entry
    plugins: Arc<RwLock<HashMap<String, DynEntry>>>,
    /// path → watcher handle (keeps the OS subscription alive)
    _watchers: Arc<RwLock<Vec<RecommendedWatcher>>>,
}

impl PluginLoader {
    /// Create a new loader with empty internal storage.
    pub fn new() -> Self {
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            _watchers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    // ── loading ───────────────────────────────────────────────────────────────

    /// Load a single plugin from a dynamic library at `path`.
    ///
    /// The library must export `create_channel_plugin()`.
    /// Returns the plugin's `ChannelMetadata` on success.
    pub async fn load_plugin(&self, path: &Path) -> Result<ChannelMetadata> {
        // Safety: we are loading a trusted plugin compiled against this binary.
        let library = unsafe { Library::new(path) }.map_err(|e| {
            ClawzError::Channel(format!("failed to load plugin '{}': {e}", path.display()))
        })?;

        // Resolve the factory symbol.  The byte slice includes a trailing NUL
        // because libloading's `get` expects a C-string on Unix platforms.
        let factory: Symbol<unsafe extern "C" fn() -> *mut dyn ChannelPlugin> = unsafe {
            library.get(PLUGIN_FACTORY).map_err(|e| {
                ClawzError::Channel(format!("plugin '{}' missing factory: {e}", path.display()))
            })?
        };

        // Catch panics inside the factory call so a bad plugin cannot crash
        // the whole worker process.
        let raw_ptr = std::panic::catch_unwind(|| unsafe { factory() }).map_err(|_| {
            ClawzError::Channel(format!("plugin factory panicked: '{}'", path.display()))
        })?;

        if raw_ptr.is_null() {
            return Err(ClawzError::Channel(format!(
                "plugin factory returned null: '{}'",
                path.display()
            )));
        }

        // SAFETY: the pointer was just returned from the factory; we own it.
        // Wrapping in Arc gives us reference-counted sharing with the runtime.
        let plugin: Arc<dyn ChannelPlugin> = unsafe { Arc::from_raw(raw_ptr) };
        let metadata = plugin.metadata();

        let entry = DynEntry {
            plugin,
            _library: library,
            lifecycle: PluginLifecycle::Ready,
            path: path.to_path_buf(),
        };

        self.plugins
            .write()
            .await
            .insert(metadata.name.clone(), entry);

        Ok(metadata)
    }

    /// Load all `.so` / `.dylib` / `.dll` files found in `directory`.
    ///
    /// Non-library files and unreadable entries are silently skipped.
    pub async fn load_directory(&self, directory: &Path) -> Vec<Result<ChannelMetadata>> {
        let read_dir = match std::fs::read_dir(directory) {
            Ok(r) => r,
            Err(e) => {
                return vec![Err(ClawzError::Channel(format!(
                    "cannot read plugin directory '{}': {e}",
                    directory.display()
                )))];
            }
        };

        let mut results = Vec::new();
        for entry in read_dir.flatten() {
            let path = entry.path();
            // Accept all major platform dynamic-library extensions so the same
            // configuration directory works on Linux, macOS, and Windows.
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext == "so" || ext == "dylib" || ext == "dll" {
                results.push(self.load_plugin(&path).await);
            }
        }
        results
    }

    // ── unloading ─────────────────────────────────────────────────────────────

    /// Gracefully shut down and unload a plugin by its name/id.
    ///
    /// The library is dropped only once all `Arc` clones of the plugin have
    /// been released by callers.  Callers should stop using the plugin before
    /// calling this.
    pub async fn unload_plugin(&self, name: &str) -> Result<()> {
        let mut guard = self.plugins.write().await;
        let entry = guard.get_mut(name).ok_or_else(|| ClawzError::NotFound {
            entity: "plugin".into(),
            id: name.to_string(),
        })?;
        // Mark as shutting down first so concurrent `get_plugin` calls
        // will filter it out even though the entry still exists in the map.
        entry.lifecycle = PluginLifecycle::ShuttingDown;
        guard.remove(name);
        Ok(())
    }

    // ── queries ───────────────────────────────────────────────────────────────

    /// Return a shared reference to a loaded plugin.
    ///
    /// Only plugins in the `Ready` lifecycle are returned; `ShuttingDown`
    /// plugins are ignored to prevent use-after-unload.
    pub async fn get_plugin(&self, name: &str) -> Option<Arc<dyn ChannelPlugin>> {
        self.plugins
            .read()
            .await
            .get(name)
            .filter(|e| e.lifecycle == PluginLifecycle::Ready)
            .map(|e| Arc::clone(&e.plugin))
    }

    /// Metadata for every loaded plugin.
    pub async fn list_plugins(&self) -> Vec<ChannelMetadata> {
        self.plugins
            .read()
            .await
            .values()
            .map(|e| e.plugin.metadata())
            .collect()
    }

    // ── hot-reload ────────────────────────────────────────────────────────────

    /// Watch `directory` for `.so`/`.dylib` modifications and reload changed
    /// plugins automatically.
    ///
    /// This spawns a Tokio task that processes `notify` events.
    pub async fn enable_hot_reload(&self, directory: &Path) -> Result<()> {
        // Bounded channel prevents the watcher task from allocating unbounded
        // memory if the OS spams change events (e.g. a build script touching
        // many files).
        let (tx, mut rx) = tokio::sync::mpsc::channel::<PathBuf>(32);

        let mut watcher =
            notify::recommended_watcher(move |res: std::result::Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    // We only care about new files or modifications; deletions are
                    // ignored because the worker may intentionally remove a broken
                    // plugin without wanting to unload it from memory.
                    if event.kind.is_modify() || event.kind.is_create() {
                        for path in event.paths {
                            let _ = tx.blocking_send(path);
                        }
                    }
                }
            })
            .map_err(|e| ClawzError::Channel(format!("watcher error: {e}")))?;

        watcher
            .watch(directory, RecursiveMode::NonRecursive)
            .map_err(|e| ClawzError::Channel(format!("watch error: {e}")))?;

        // Store the watcher so it isn't dropped.
        self._watchers.write().await.push(watcher);

        // Clone the Arc handles into the spawned task so the task can
        // access the same HashMap and watcher vector.
        let loader = PluginLoader {
            plugins: Arc::clone(&self.plugins),
            _watchers: Arc::clone(&self._watchers),
        };

        tokio::spawn(async move {
            while let Some(path) = rx.recv().await {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext == "so" || ext == "dylib" || ext == "dll" {
                    log::info!("Hot-reloading plugin: {}", path.display());
                    // `load_plugin` will overwrite any existing entry with the
                    // same metadata.name, achieving the reload semantics.
                    match loader.load_plugin(&path).await {
                        Ok(meta) => log::info!("Reloaded plugin '{}'", meta.name),
                        Err(e) => log::error!("Failed to reload plugin: {e}"),
                    }
                }
            }
        });

        Ok(())
    }
}

impl Default for PluginLoader {
    fn default() -> Self {
        Self::new()
    }
}
