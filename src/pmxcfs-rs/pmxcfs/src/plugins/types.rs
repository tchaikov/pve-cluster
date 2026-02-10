/// Core plugin types and trait definitions
use anyhow::Result;

/// Plugin trait for special file handlers
///
/// Note: We can't use `const NAME: &'static str` as an associated constant because
/// it would make the trait not object-safe (dyn Plugin wouldn't work). Instead,
/// each implementation provides the name via the name() method.
pub trait Plugin: Send + Sync {
    /// Get plugin name
    fn name(&self) -> &str;

    /// Read content from this plugin
    fn read(&self) -> Result<Vec<u8>>;

    /// Write content to this plugin (if supported)
    fn write(&self, _data: &[u8]) -> Result<()> {
        Err(anyhow::anyhow!("Write not supported for this plugin"))
    }

    /// Get file mode
    fn mode(&self) -> u32;

    /// Check if this is a symbolic link
    fn is_symlink(&self) -> bool {
        false
    }
}

/// Link plugin - symbolic links
pub struct LinkPlugin {
    name: &'static str,
    target: String,
}

impl LinkPlugin {
    pub fn new(name: &'static str, target: impl Into<String>) -> Self {
        Self {
            name,
            target: target.into(),
        }
    }
}

impl Plugin for LinkPlugin {
    fn name(&self) -> &str {
        self.name
    }

    fn read(&self) -> Result<Vec<u8>> {
        Ok(self.target.as_bytes().to_vec())
    }

    fn mode(&self) -> u32 {
        0o777 // Symbolic links
    }

    fn is_symlink(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== LinkPlugin Tests =====

    #[test]
    fn test_link_plugin_creation() {
        let plugin = LinkPlugin::new("testlink", "/target/path");
        assert_eq!(plugin.name(), "testlink");
        assert!(plugin.is_symlink());
    }

    #[test]
    fn test_link_plugin_read_target() {
        let target = "/path/to/target";
        let plugin = LinkPlugin::new("mylink", target);

        let result = plugin.read().unwrap();
        assert_eq!(result, target.as_bytes());
    }

    #[test]
    fn test_link_plugin_mode() {
        let plugin = LinkPlugin::new("link", "/target");
        assert_eq!(
            plugin.mode(),
            0o777,
            "Symbolic links should have mode 0o777"
        );
    }

    #[test]
    fn test_link_plugin_write_not_supported() {
        let plugin = LinkPlugin::new("readonly", "/target");
        let result = plugin.write(b"test data");

        assert!(result.is_err(), "LinkPlugin should not support write");
        assert!(result.unwrap_err().to_string().contains("not supported"));
    }

    #[test]
    fn test_link_plugin_with_unicode_target() {
        let target = "/path/with/üñïçödé/target";
        let plugin = LinkPlugin::new("unicode", target);

        let result = plugin.read().unwrap();
        assert_eq!(String::from_utf8(result).unwrap(), target);
    }
}
