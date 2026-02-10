use std::net::IpAddr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// Global configuration for pmxcfs
pub struct Config {
    /// Node name (hostname without domain)
    nodename: String,

    /// Node IP address
    node_ip: IpAddr,

    /// www-data group ID for file permissions
    www_data_gid: u32,

    /// Force local mode (no clustering)
    local_mode: bool,

    /// Cluster name (CPG group name)
    cluster_name: String,

    /// Debug level (0 = normal, 1+ = debug) - mutable at runtime
    debug_level: AtomicU8,
}

impl Clone for Config {
    fn clone(&self) -> Self {
        Self {
            nodename: self.nodename.clone(),
            node_ip: self.node_ip,
            www_data_gid: self.www_data_gid,
            local_mode: self.local_mode,
            cluster_name: self.cluster_name.clone(),
            debug_level: AtomicU8::new(self.debug_level.load(Ordering::Relaxed)),
        }
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("nodename", &self.nodename)
            .field("node_ip", &self.node_ip)
            .field("www_data_gid", &self.www_data_gid)
            .field("local_mode", &self.local_mode)
            .field("cluster_name", &self.cluster_name)
            .field("debug_level", &self.debug_level.load(Ordering::Relaxed))
            .finish()
    }
}

impl Config {
    /// Validate a hostname according to RFC 1123
    ///
    /// Hostname requirements:
    /// - Length: 1-253 characters
    /// - Labels (dot-separated parts): 1-63 characters each
    /// - Characters: alphanumeric and hyphens
    /// - Cannot start or end with hyphen
    /// - Case insensitive (lowercase preferred)
    fn validate_hostname(hostname: &str) -> Result<(), String> {
        if hostname.is_empty() {
            return Err("Hostname cannot be empty".to_string());
        }
        if hostname.len() > 253 {
            return Err(format!("Hostname too long: {} > 253 characters", hostname.len()));
        }

        for label in hostname.split('.') {
            if label.is_empty() {
                return Err("Hostname cannot have empty labels (consecutive dots)".to_string());
            }
            if label.len() > 63 {
                return Err(format!("Hostname label '{}' too long: {} > 63 characters", label, label.len()));
            }
            if label.starts_with('-') || label.ends_with('-') {
                return Err(format!("Hostname label '{}' cannot start or end with hyphen", label));
            }
            if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(format!("Hostname label '{}' contains invalid characters (only alphanumeric and hyphen allowed)", label));
            }
        }

        Ok(())
    }

    pub fn new(
        nodename: String,
        node_ip: IpAddr,
        www_data_gid: u32,
        debug: bool,
        local_mode: bool,
        cluster_name: String,
    ) -> Self {
        // Validate hostname (log warning but don't fail - matches C behavior)
        // The C implementation accepts any hostname from uname() without validation
        if let Err(e) = Self::validate_hostname(&nodename) {
            tracing::warn!("Invalid nodename '{}': {}", nodename, e);
        }

        let debug_level = if debug { 1 } else { 0 };
        Self {
            nodename,
            node_ip,
            www_data_gid,
            local_mode,
            cluster_name,
            debug_level: AtomicU8::new(debug_level),
        }
    }

    pub fn shared(
        nodename: String,
        node_ip: IpAddr,
        www_data_gid: u32,
        debug: bool,
        local_mode: bool,
        cluster_name: String,
    ) -> Arc<Self> {
        Arc::new(Self::new(nodename, node_ip, www_data_gid, debug, local_mode, cluster_name))
    }

    pub fn cluster_name(&self) -> &str {
        &self.cluster_name
    }

    pub fn nodename(&self) -> &str {
        &self.nodename
    }

    pub fn node_ip(&self) -> IpAddr {
        self.node_ip
    }

    pub fn www_data_gid(&self) -> u32 {
        self.www_data_gid
    }

    pub fn is_debug(&self) -> bool {
        self.debug_level() > 0
    }

    pub fn is_local_mode(&self) -> bool {
        self.local_mode
    }

    /// Get current debug level (0 = normal, 1+ = debug)
    pub fn debug_level(&self) -> u8 {
        self.debug_level.load(Ordering::Relaxed)
    }

    /// Set debug level (0 = normal, 1+ = debug)
    pub fn set_debug_level(&self, level: u8) {
        self.debug_level.store(level, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for Config struct
    //!
    //! This test module provides comprehensive coverage for:
    //! - Configuration creation and initialization
    //! - Getter methods for all configuration fields
    //! - Debug level mutation and thread safety
    //! - Concurrent access patterns (reads and writes)
    //! - Clone independence
    //! - Debug formatting
    //! - Edge cases (empty strings, long strings, special characters, unicode)
    //!
    //! ## Thread Safety
    //!
    //! The Config struct uses `AtomicU8` for debug_level to allow
    //! safe concurrent reads and writes. Tests verify:
    //! - 10 threads × 100 operations (concurrent modifications)
    //! - 20 threads × 1000 operations (concurrent reads)
    //!
    //! ## Edge Cases
    //!
    //! Tests cover various edge cases including:
    //! - Empty strings for node/cluster names
    //! - Long strings (1000+ characters)
    //! - Special characters in strings
    //! - Unicode support (emoji, non-ASCII characters)

    use super::*;
    use std::thread;

    // ===== Basic Construction Tests =====

    #[test]
    fn test_config_creation() {
        let config = Config::new(
            "node1".to_string(),
            "192.168.1.10".parse().unwrap(),
            33,
            false,
            false,
            "pmxcfs".to_string(),
        );

        assert_eq!(config.nodename(), "node1");
        assert_eq!(config.node_ip(), "192.168.1.10".parse::<IpAddr>().unwrap());
        assert_eq!(config.www_data_gid(), 33);
        assert!(!config.is_debug());
        assert!(!config.is_local_mode());
        assert_eq!(config.cluster_name(), "pmxcfs");
        assert_eq!(
            config.debug_level(),
            0,
            "Debug level should be 0 when debug is false"
        );
    }

    #[test]
    fn test_config_creation_with_debug() {
        let config = Config::new(
            "node2".to_string(),
            "10.0.0.5".parse().unwrap(),
            1000,
            true,
            false,
            "test-cluster".to_string(),
        );

        assert!(config.is_debug());
        assert_eq!(
            config.debug_level(),
            1,
            "Debug level should be 1 when debug is true"
        );
    }

    #[test]
    fn test_config_creation_local_mode() {
        let config = Config::new(
            "localhost".to_string(),
            "127.0.0.1".parse().unwrap(),
            33,
            false,
            true,
            "local".to_string(),
        );

        assert!(config.is_local_mode());
        assert!(!config.is_debug());
    }

    // ===== Getter Tests =====

    #[test]
    fn test_all_getters() {
        let config = Config::new(
            "testnode".to_string(),
            "172.16.0.1".parse().unwrap(),
            999,
            true,
            true,
            "my-cluster".to_string(),
        );

        // Test all getter methods
        assert_eq!(config.nodename(), "testnode");
        assert_eq!(config.node_ip(), "172.16.0.1".parse::<IpAddr>().unwrap());
        assert_eq!(config.www_data_gid(), 999);
        assert!(config.is_debug());
        assert!(config.is_local_mode());
        assert_eq!(config.cluster_name(), "my-cluster");
        assert_eq!(config.debug_level(), 1);
    }

    // ===== Debug Level Mutation Tests =====

    #[test]
    fn test_debug_level_mutation() {
        let config = Config::new(
            "node1".to_string(),
            "192.168.1.1".parse().unwrap(),
            33,
            false,
            false,
            "pmxcfs".to_string(),
        );

        assert_eq!(config.debug_level(), 0);

        config.set_debug_level(1);
        assert_eq!(config.debug_level(), 1);

        config.set_debug_level(5);
        assert_eq!(config.debug_level(), 5);

        config.set_debug_level(0);
        assert_eq!(config.debug_level(), 0);
    }

    #[test]
    fn test_debug_level_max_value() {
        let config = Config::new(
            "node1".to_string(),
            "192.168.1.1".parse().unwrap(),
            33,
            false,
            false,
            "pmxcfs".to_string(),
        );

        config.set_debug_level(255);
        assert_eq!(config.debug_level(), 255);

        config.set_debug_level(0);
        assert_eq!(config.debug_level(), 0);
    }

    // ===== Thread Safety Tests =====

    #[test]
    fn test_debug_level_thread_safety() {
        let config = Config::shared(
            "node1".to_string(),
            "192.168.1.1".parse().unwrap(),
            33,
            false,
            false,
            "pmxcfs".to_string(),
        );

        let config_clone = Arc::clone(&config);

        // Spawn multiple threads that concurrently modify debug level
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let cfg = Arc::clone(&config);
                thread::spawn(move || {
                    for _ in 0..100 {
                        cfg.set_debug_level(i);
                        let _ = cfg.debug_level();
                    }
                })
            })
            .collect();

        // All threads should complete without panicking
        for handle in handles {
            handle.join().unwrap();
        }

        // Final value should be one of the values set by threads
        let final_level = config_clone.debug_level();
        assert!(
            final_level < 10,
            "Debug level should be < 10, got {final_level}"
        );
    }

    #[test]
    fn test_concurrent_reads() {
        let config = Config::shared(
            "node1".to_string(),
            "192.168.1.1".parse().unwrap(),
            33,
            true,
            false,
            "pmxcfs".to_string(),
        );

        // Spawn multiple threads that concurrently read config
        let handles: Vec<_> = (0..20)
            .map(|_| {
                let cfg = Arc::clone(&config);
                thread::spawn(move || {
                    for _ in 0..1000 {
                        assert_eq!(cfg.nodename(), "node1");
                        assert_eq!(cfg.node_ip(), "192.168.1.1".parse::<IpAddr>().unwrap());
                        assert_eq!(cfg.www_data_gid(), 33);
                        assert!(cfg.is_debug());
                        assert!(!cfg.is_local_mode());
                        assert_eq!(cfg.cluster_name(), "pmxcfs");
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }
    }

    // ===== Clone Tests =====

    #[test]
    fn test_config_clone() {
        let config1 = Config::new(
            "node1".to_string(),
            "192.168.1.1".parse().unwrap(),
            33,
            true,
            false,
            "pmxcfs".to_string(),
        );

        config1.set_debug_level(5);

        let config2 = config1.clone();

        // Cloned config should have same values
        assert_eq!(config2.nodename(), config1.nodename());
        assert_eq!(config2.node_ip(), config1.node_ip());
        assert_eq!(config2.www_data_gid(), config1.www_data_gid());
        assert_eq!(config2.is_debug(), config1.is_debug());
        assert_eq!(config2.is_local_mode(), config1.is_local_mode());
        assert_eq!(config2.cluster_name(), config1.cluster_name());
        assert_eq!(config2.debug_level(), 5);

        // Modifying one should not affect the other
        config2.set_debug_level(10);
        assert_eq!(config1.debug_level(), 5);
        assert_eq!(config2.debug_level(), 10);
    }

    // ===== Debug Formatting Tests =====

    #[test]
    fn test_debug_format() {
        let config = Config::new(
            "node1".to_string(),
            "192.168.1.1".parse().unwrap(),
            33,
            true,
            false,
            "pmxcfs".to_string(),
        );

        let debug_str = format!("{config:?}");

        // Check that debug output contains all fields
        assert!(debug_str.contains("Config"));
        assert!(debug_str.contains("nodename"));
        assert!(debug_str.contains("node1"));
        assert!(debug_str.contains("node_ip"));
        assert!(debug_str.contains("192.168.1.1"));
        assert!(debug_str.contains("www_data_gid"));
        assert!(debug_str.contains("33"));
        assert!(debug_str.contains("local_mode"));
        assert!(debug_str.contains("false"));
        assert!(debug_str.contains("cluster_name"));
        assert!(debug_str.contains("pmxcfs"));
        assert!(debug_str.contains("debug_level"));
    }

    // ===== Edge Cases and Boundary Tests =====

    #[test]
    fn test_empty_strings() {
        let config = Config::new(
            String::new(),
            "127.0.0.1".parse().unwrap(),
            0,
            false,
            false,
            String::new(),
        );

        assert_eq!(config.nodename(), "");
        assert_eq!(config.node_ip(), "127.0.0.1".parse::<IpAddr>().unwrap());
        assert_eq!(config.cluster_name(), "");
        assert_eq!(config.www_data_gid(), 0);
    }

    #[test]
    fn test_long_strings() {
        let long_name = "a".repeat(1000);
        let long_cluster = "cluster-".to_string() + &"x".repeat(500);

        let config = Config::new(
            long_name.clone(),
            "192.168.1.1".parse().unwrap(),
            u32::MAX,
            true,
            true,
            long_cluster.clone(),
        );

        assert_eq!(config.nodename(), long_name);
        assert_eq!(config.node_ip(), "192.168.1.1".parse::<IpAddr>().unwrap());
        assert_eq!(config.cluster_name(), long_cluster);
        assert_eq!(config.www_data_gid(), u32::MAX);
    }

    #[test]
    fn test_special_characters_in_strings() {
        let config = Config::new(
            "node-1_test.local".to_string(),
            "192.168.1.10".parse().unwrap(),
            33,
            false,
            false,
            "my-cluster_v2.0".to_string(),
        );

        assert_eq!(config.nodename(), "node-1_test.local");
        assert_eq!(config.node_ip(), "192.168.1.10".parse::<IpAddr>().unwrap());
        assert_eq!(config.cluster_name(), "my-cluster_v2.0");
    }

    #[test]
    fn test_unicode_in_strings() {
        let config = Config::new(
            "ノード1".to_string(),
            "::1".parse().unwrap(),
            33,
            false,
            false,
            "集群".to_string(),
        );

        assert_eq!(config.nodename(), "ノード1");
        assert_eq!(config.node_ip(), "::1".parse::<IpAddr>().unwrap());
        assert_eq!(config.cluster_name(), "集群");
    }
}
