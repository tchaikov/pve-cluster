/// Runtime-adjustable logging infrastructure
///
/// This module provides the ability to change tracing filter levels at runtime,
/// matching the C implementation's behavior where the .debug plugin can dynamically
/// enable/disable debug logging.
use anyhow::Result;
use parking_lot::Mutex;
use std::sync::OnceLock;
use tracing_subscriber::{EnvFilter, reload};

/// Type alias for the reload handle
type ReloadHandle = reload::Handle<EnvFilter, tracing_subscriber::Registry>;

/// Global reload handle for runtime log level adjustment
static LOG_RELOAD_HANDLE: OnceLock<Mutex<ReloadHandle>> = OnceLock::new();

/// Initialize the reload handle (called once during logging setup)
pub fn set_reload_handle(handle: ReloadHandle) -> Result<()> {
    LOG_RELOAD_HANDLE
        .set(Mutex::new(handle))
        .map_err(|_| anyhow::anyhow!("Failed to set log reload handle - already initialized"))
}

/// Set debug level at runtime (called by .debug plugin)
///
/// This changes the tracing filter to either "debug" (level > 0) or "info" (level == 0),
/// matching the C implementation where writing to .debug affects cfs_debug() output.
pub fn set_debug_level(level: u8) -> Result<()> {
    let filter = if level > 0 {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    if let Some(handle) = LOG_RELOAD_HANDLE.get() {
        handle
            .lock()
            .reload(filter)
            .map_err(|e| anyhow::anyhow!("Failed to reload log filter: {e}"))?;
        Ok(())
    } else {
        Err(anyhow::anyhow!("Log reload handle not initialized"))
    }
}
