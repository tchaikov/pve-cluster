/// .rrd Plugin - RRD (Round-Robin Database) Metrics
///
/// This plugin provides system metrics in text format matching C implementation:
/// ```text
/// pve2-node/nodename:timestamp:uptime:loadavg:maxcpu:cpu:iowait:memtotal:memused:...
/// pve2.3-vm/100:timestamp:status:uptime:...
/// ```
///
/// The format is compatible with the C implementation which uses rrd_update
/// to write data to RRD files on disk.
///
/// Data aging: Entries older than 5 minutes are automatically removed.
use pmxcfs_status::Status;
use std::sync::Arc;

use super::Plugin;

/// RRD plugin - provides system metrics
pub struct RrdPlugin {
    status: Arc<Status>,
}

impl RrdPlugin {
    pub fn new(status: Arc<Status>) -> Self {
        Self { status }
    }

    /// Generate RRD content (C-compatible text format)
    fn generate_content(&self) -> String {
        // Get RRD dump in text format from status module
        // Format: "key:data\n" for each entry
        // The status module handles data aging (removes entries >5 minutes old)
        self.status.get_rrd_dump()
    }
}

impl Plugin for RrdPlugin {
    fn name(&self) -> &str {
        ".rrd"
    }

    fn read(&self) -> anyhow::Result<Vec<u8>> {
        Ok(self.generate_content().into_bytes())
    }

    fn mode(&self) -> u32 {
        0o440
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rrd_empty() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

        let plugin = RrdPlugin::new(status);
        let result = plugin.generate_content();
        // Empty RRD data should return just NUL terminator (C compatibility)
        assert_eq!(result, "\0");
    }

    #[tokio::test]
    async fn test_rrd_with_data() {
        let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

        // Add some RRD data with proper schema
        // Note: RRD file creation will fail (no rrdcached in tests), but in-memory storage works
        // Node RRD (pve2 format): timestamp + 12 values
        // (loadavg, maxcpu, cpu, iowait, memtotal, memused, swaptotal, swapused, roottotal, rootused, netin, netout)
        let _ = status.set_rrd_data(
            "pve2-node/testnode".to_string(),
            "1234567890:0.5:4:1.2:0.25:8000000000:4000000000:2000000000:100000000:10000000000:5000000000:1000000:500000".to_string(),
        ).await; // May fail if rrdcached not running, but in-memory storage succeeds

        // VM RRD (pve2.3 format): timestamp + 10 values
        // (maxcpu, cpu, maxmem, mem, maxdisk, disk, netin, netout, diskread, diskwrite)
        let _ = status
            .set_rrd_data(
                "pve2.3-vm/100".to_string(),
                "1234567890:4:2.5:4096:2048:100000:50000:1000000:500000:10000:5000".to_string(),
            )
            .await; // May fail if rrdcached not running, but in-memory storage succeeds

        let plugin = RrdPlugin::new(status);
        let result = plugin.generate_content();

        // Should contain both entries (from in-memory storage)
        assert!(result.contains("pve2-node/testnode"));
        assert!(result.contains("pve2.3-vm/100"));
        assert!(result.contains("1234567890"));
    }
}
