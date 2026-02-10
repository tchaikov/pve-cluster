/// .debug Plugin - Debug Level Control
///
/// This plugin provides read/write access to debug settings, matching the C implementation.
/// Format: "0\n" or "1\n" (debug level as text)
///
/// When written, this actually changes the tracing filter level at runtime,
/// matching the C implementation's behavior where cfs.debug controls cfs_debug() macro output.
use anyhow::Result;
use pmxcfs_config::Config;
use std::sync::Arc;

use super::Plugin;

/// Debug plugin - provides debug level control
pub struct DebugPlugin {
    config: Arc<Config>,
}

impl DebugPlugin {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }

    /// Generate debug setting content (read operation)
    fn generate_content(&self) -> String {
        let level = self.config.debug_level();
        format!("{level}\n")
    }

    /// Handle debug plugin write operation
    ///
    /// This changes the tracing filter level at runtime to match C implementation behavior.
    /// In C, writing to .debug sets cfs.debug which controls cfs_debug() macro output.
    fn handle_write(&self, data: &str) -> Result<()> {
        let level: u8 = data
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid debug level: must be a number"))?;

        // Update debug level in config
        self.config.set_debug_level(level);

        // Actually change the tracing filter level at runtime
        // This matches C implementation where cfs.debug controls logging
        if let Err(e) = crate::logging::set_debug_level(level) {
            tracing::error!("Failed to update log level: {}", e);
            // Don't fail - just log error. The level is still stored.
        }

        if level > 0 {
            tracing::info!("Debug mode enabled (level {})", level);
            tracing::debug!("Debug logging is now active");
        } else {
            tracing::info!("Debug mode disabled");
        }

        Ok(())
    }
}

impl Plugin for DebugPlugin {
    fn name(&self) -> &str {
        ".debug"
    }

    fn read(&self) -> anyhow::Result<Vec<u8>> {
        Ok(self.generate_content().into_bytes())
    }

    fn write(&self, data: &[u8]) -> Result<()> {
        let text = std::str::from_utf8(data)?;
        self.handle_write(text)
    }

    fn mode(&self) -> u32 {
        0o640
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debug_read() {
        let config = Arc::new(Config::new(
            "test".to_string(),
            "127.0.0.1".parse().unwrap(),
            33,
            false,
            false,
            "pmxcfs".to_string(),
        ));
        let plugin = DebugPlugin::new(config);
        let result = plugin.generate_content();
        assert_eq!(result, "0\n");
    }

    #[test]
    fn test_debug_write() {
        let config = Arc::new(Config::new(
            "test".to_string(),
            "127.0.0.1".parse().unwrap(),
            33,
            false,
            false,
            "pmxcfs".to_string(),
        ));

        let plugin = DebugPlugin::new(config.clone());
        let result = plugin.handle_write("1");
        // Note: This will fail to actually change the log level if the reload handle
        // hasn't been initialized (which is expected in unit tests without full setup).
        // The function should still succeed - it just warns about not being able to reload.
        assert!(result.is_ok());

        // Verify the stored level changed
        assert_eq!(config.debug_level(), 1);

        // Test setting it back to 0
        let result = plugin.handle_write("0");
        assert!(result.is_ok());
        assert_eq!(config.debug_level(), 0);
    }

    #[test]
    fn test_invalid_debug_level() {
        let config = Arc::new(Config::new(
            "test".to_string(),
            "127.0.0.1".parse().unwrap(),
            33,
            false,
            false,
            "pmxcfs".to_string(),
        ));

        let plugin = DebugPlugin::new(config.clone());

        let result = plugin.handle_write("invalid");
        assert!(result.is_err());

        let result = plugin.handle_write("");
        assert!(result.is_err());
    }
}
