/// RRD File Writer
///
/// Handles creating and updating RRD files via pluggable backends.
/// Supports daemon-based (rrdcached) and direct file writing modes.
use super::backend::{DEFAULT_SOCKET_PATH, RrdFallbackBackend};
use super::key_type::{MetricType, RrdKeyType};
use super::schema::{RrdFormat, RrdSchema};
use anyhow::{Context, Result};
use chrono::Local;
use std::fs;
use std::path::{Path, PathBuf};


/// RRD writer for persistent metric storage
///
/// Uses pluggable backends (daemon, direct, or fallback) for RRD operations.
pub struct RrdWriter {
    /// Base directory for RRD files (default: /var/lib/rrdcached/db)
    base_dir: PathBuf,
    /// Backend for RRD operations (daemon, direct, or fallback)
    backend: Box<dyn super::backend::RrdBackend>,
}

impl RrdWriter {
    /// Create new RRD writer with default fallback backend
    ///
    /// Uses the fallback backend that tries daemon first, then falls back to direct file writes.
    /// This matches the C implementation's behavior.
    ///
    /// # Arguments
    /// * `base_dir` - Base directory for RRD files
    pub async fn new<P: AsRef<Path>>(base_dir: P) -> Result<Self> {
        let backend = Self::default_backend().await?;
        Self::with_backend(base_dir, backend).await
    }

    /// Create new RRD writer with specific backend
    ///
    /// # Arguments
    /// * `base_dir` - Base directory for RRD files
    /// * `backend` - RRD backend to use (daemon, direct, or fallback)
    pub(crate) async fn with_backend<P: AsRef<Path>>(
        base_dir: P,
        backend: Box<dyn super::backend::RrdBackend>,
    ) -> Result<Self> {
        let base_dir = base_dir.as_ref().to_path_buf();

        // Create base directory if it doesn't exist
        fs::create_dir_all(&base_dir)
            .with_context(|| format!("Failed to create RRD base directory: {base_dir:?}"))?;

        tracing::info!("RRD writer using backend: {}", backend.name());

        Ok(Self { base_dir, backend })
    }

    /// Create default backend (fallback: daemon + direct)
    ///
    /// This matches the C implementation's behavior:
    /// - Tries rrdcached daemon first for performance
    /// - Falls back to direct file writes if daemon fails
    async fn default_backend() -> Result<Box<dyn super::backend::RrdBackend>> {
        let backend = RrdFallbackBackend::new(DEFAULT_SOCKET_PATH).await;
        Ok(Box::new(backend))
    }

    /// Update RRD file with metric data
    ///
    /// This will:
    /// 1. Transform data from source format to target format (padding/truncation/column skipping)
    /// 2. Create the RRD file if it doesn't exist
    /// 3. Update via rrdcached daemon
    ///
    /// # Arguments
    /// * `key` - RRD key (e.g., "pve2-node/node1", "pve-vm-9.0/100")
    /// * `data` - Raw metric data string from pvestatd (format: "skipped_fields...:ctime:val1:val2:...")
    pub async fn update(&mut self, key: &str, data: &str) -> Result<()> {
        // Parse the key to determine file path and schema
        let key_type = RrdKeyType::parse(key).with_context(|| format!("Invalid RRD key: {key}"))?;

        // Get source format and target schema
        let source_format = key_type.source_format();
        let target_schema = key_type.schema();
        let metric_type = key_type.metric_type();

        // Transform data from source to target format
        let transformed_data =
            Self::transform_data(data, source_format, &target_schema, metric_type)
                .with_context(|| format!("Failed to transform RRD data for key: {key}"))?;

        // Get the file path (always uses current format)
        let file_path = key_type.file_path(&self.base_dir);

        // Ensure the RRD file exists
        // Always check file existence directly - handles file deletion/rotation
        if !file_path.exists() {
            self.create_rrd_file(&key_type, &file_path).await?;
        }

        // Update the RRD file via backend
        self.backend.update(&file_path, &transformed_data).await?;

        Ok(())
    }

    /// Create RRD file with appropriate schema via backend
    async fn create_rrd_file(&mut self, key_type: &RrdKeyType, file_path: &Path) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {parent:?}"))?;
        }

        // Get schema for this RRD type
        let schema = key_type.schema();

        // Calculate start time (at day boundary, matching C implementation)
        // C uses localtime() (status.c:1206-1219), not UTC
        let now = Local::now();
        let start = now
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .expect("00:00:00 is always a valid time")
            .and_local_timezone(Local)
            .single()
            .expect("Local midnight should have single timezone mapping");
        let start_timestamp = start.timestamp();

        tracing::debug!(
            "Creating RRD file: {:?} with {} data sources via {}",
            file_path,
            schema.column_count(),
            self.backend.name()
        );

        // Delegate to backend for creation
        self.backend
            .create(file_path, &schema, start_timestamp)
            .await?;

        tracing::info!("Created RRD file: {:?} ({})", file_path, schema);

        Ok(())
    }

    /// Transform data from source format to target format
    ///
    /// This implements the C behavior from status.c (rrd_skip_data + padding/truncation):
    /// 1. Skip non-archivable columns from the beginning of the data string
    /// 2. The field after the skipped columns is the timestamp (ctime from pvestatd)
    /// 3. Pad with `:U` if the source has fewer archivable columns than the target
    /// 4. Truncate if the source has more columns than the target
    ///
    /// The data format from pvestatd (see PVE::Service::pvestatd) is:
    ///   Node:    "uptime:sublevel:ctime:loadavg:maxcpu:cpu:..."
    ///   VM:      "uptime:name:status:template:ctime:maxcpu:cpu:..."
    ///   Storage: "ctime:total:used"
    ///
    /// After skipping, the result starts with the timestamp and is a valid RRD update string:
    ///   Node:    "ctime:loadavg:maxcpu:cpu:..."  (skip 2)
    ///   VM:      "ctime:maxcpu:cpu:..."          (skip 4)
    ///   Storage: "ctime:total:used"              (skip 0)
    ///
    /// # Arguments
    /// * `data` - Raw data string from pvestatd status update
    /// * `source_format` - Format indicated by the input key
    /// * `target_schema` - Target RRD schema (always Pve9_0 currently)
    /// * `metric_type` - Type of metric (Node, VM, Storage) for column skipping
    ///
    /// # Returns
    /// Transformed data string ready for RRD update ("timestamp:v1:v2:...")
    fn transform_data(
        data: &str,
        _source_format: RrdFormat,
        target_schema: &RrdSchema,
        metric_type: MetricType,
    ) -> Result<String> {
        // Skip non-archivable columns from the start of the data string.
        // This matches C's rrd_skip_data(data, skip, ':') in status.c:1385
        // which skips `skip` colon-separated fields from the beginning.
        let skip_count = metric_type.skip_columns();
        let target_cols = target_schema.column_count();

        // After skip, we need: timestamp + target_cols values = target_cols + 1 fields
        let total_needed = target_cols + 1;

        let mut iter = data
            .split(':')
            .skip(skip_count)
            .chain(std::iter::repeat("U"))
            .take(total_needed);

        match iter.next() {
            Some(first) => {
                let result = iter.fold(first.to_string(), |mut acc, value| {
                    acc.push(':');
                    acc.push_str(value);
                    acc
                });
                Ok(result)
            }
            None => anyhow::bail!(
                "Not enough fields in data after skipping {} columns",
                skip_count
            ),
        }
    }

    /// Flush all pending updates
    #[allow(dead_code)] // Used via RRD update cycle
    pub(crate) async fn flush(&mut self) -> Result<()> {
        self.backend.flush().await
    }

    /// Get base directory
    #[allow(dead_code)] // Used for path resolution in updates
    pub(crate) fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

impl Drop for RrdWriter {
    fn drop(&mut self) {
        // Note: We can't flush in Drop since it's async
        // Users should call flush() explicitly before dropping if needed
        tracing::debug!("RrdWriter dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::super::schema::{RrdFormat, RrdSchema};
    use super::*;

    #[test]
    fn test_rrd_file_path_generation() {
        let temp_dir = std::path::PathBuf::from("/tmp/test");

        let key_node = RrdKeyType::Node {
            nodename: "testnode".to_string(),
            format: RrdFormat::Pve9_0,
        };
        let path = key_node.file_path(&temp_dir);
        assert_eq!(path, temp_dir.join("pve-node-9.0").join("testnode"));
    }

    // ===== Format Adaptation Tests =====

    #[test]
    fn test_transform_data_node_pve2_to_pve9() {
        // Test padding old format (12 archivable cols) to new format (19 archivable cols)
        // pvestatd data format for node: "uptime:sublevel:ctime:loadavg:maxcpu:cpu:iowait:memtotal:memused:swap_t:swap_u:root_t:root_u:netin:netout"
        // = 2 non-archivable + 1 timestamp + 12 archivable = 15 fields
        let data = "1000:0:1234567890:1.5:4:2.0:0.5:8000000000:6000000000:0:0:0:0:1000000:500000";

        let schema = RrdSchema::node(RrdFormat::Pve9_0);
        let result =
            RrdWriter::transform_data(data, RrdFormat::Pve2, &schema, MetricType::Node).unwrap();

        // After skip(2): "1234567890:1.5:4:2.0:0.5:...:500000" = 13 fields
        // Pad to 20 total (timestamp + 19 values): 13 + 7 "U" = 20
        let parts: Vec<&str> = result.split(':').collect();
        assert_eq!(parts[0], "1234567890", "Timestamp should be preserved");
        assert_eq!(parts.len(), 20, "Should have timestamp + 19 values");
        assert_eq!(parts[1], "1.5", "First value after skip should be loadavg");
        assert_eq!(parts[2], "4", "Second value should be maxcpu");
        assert_eq!(parts[12], "500000", "Last data value should be netout");

        // Check padding (7 columns: 19 - 12 = 7)
        for (i, item) in parts.iter().enumerate().take(20).skip(13) {
            assert_eq!(item, &"U", "Column {} should be padded with U", i);
        }
    }

    #[test]
    fn test_transform_data_vm_pve2_to_pve9() {
        // Test VM transformation with 4 columns skipped
        // pvestatd data format for VM: "uptime:name:status:template:ctime:maxcpu:cpu:maxmem:mem:maxdisk:disk:netin:netout:diskread:diskwrite"
        // = 4 non-archivable + 1 timestamp + 10 archivable = 15 fields
        let data = "1000:myvm:1:0:1234567890:4:2:4096:2048:100000:50000:1000:500:100:50";

        let schema = RrdSchema::vm(RrdFormat::Pve9_0);
        let result =
            RrdWriter::transform_data(data, RrdFormat::Pve2, &schema, MetricType::Vm).unwrap();

        // After skip(4): "1234567890:4:2:4096:...:50" = 11 fields
        // Pad to 18 total (timestamp + 17 values): 11 + 7 "U" = 18
        let parts: Vec<&str> = result.split(':').collect();
        assert_eq!(parts[0], "1234567890");
        assert_eq!(parts.len(), 18, "Should have timestamp + 17 values");
        assert_eq!(parts[1], "4", "First value after skip should be maxcpu");
        assert_eq!(parts[10], "50", "Last data value should be diskwrite");

        // Check padding (7 columns: 17 - 10 = 7)
        for (i, item) in parts.iter().enumerate().take(18).skip(11) {
            assert_eq!(item, &"U", "Column {} should be padded", i);
        }
    }

    #[test]
    fn test_transform_data_no_padding_needed() {
        // Test when source and target have same column count (Pve9_0 node: 19 archivable cols)
        // pvestatd format: "uptime:sublevel:ctime:loadavg:maxcpu:cpu:iowait:memtotal:memused:swap_t:swap_u:root_t:root_u:netin:netout:memavail:arcsize:cpu_some:io_some:io_full:mem_some:mem_full"
        // = 2 non-archivable + 1 timestamp + 19 archivable = 22 fields
        let data = "1000:0:1234567890:1.5:4:2.0:0.5:8000000000:6000000000:0:0:0:0:1000000:500000:7000000000:0:0.12:0.05:0.02:0.08:0.03";

        let schema = RrdSchema::node(RrdFormat::Pve9_0);
        let result =
            RrdWriter::transform_data(data, RrdFormat::Pve9_0, &schema, MetricType::Node).unwrap();

        // After skip(2): 20 fields = timestamp + 19 values (exact match, no padding)
        let parts: Vec<&str> = result.split(':').collect();
        assert_eq!(parts.len(), 20, "Should have timestamp + 19 values");
        assert_eq!(parts[0], "1234567890", "Timestamp should be ctime");
        assert_eq!(parts[1], "1.5", "First value after skip should be loadavg");
        assert_eq!(parts[19], "0.03", "Last value should be mem_full (no padding)");
    }

    #[test]
    fn test_transform_data_future_format_truncation() {
        // Test truncation when a future format sends more columns than current pve9.0
        // Simulating: uptime:sublevel:ctime:1:2:3:...:25 (2 skipped + timestamp + 25 archivable = 28 fields)
        let data =
            "999:0:1234567890:1:2:3:4:5:6:7:8:9:10:11:12:13:14:15:16:17:18:19:20:21:22:23:24:25";

        let schema = RrdSchema::node(RrdFormat::Pve9_0);
        let result =
            RrdWriter::transform_data(data, RrdFormat::Pve9_0, &schema, MetricType::Node).unwrap();

        // After skip(2): "1234567890:1:2:...:25" = 26 fields
        // take(20): truncate to timestamp + 19 values
        let parts: Vec<&str> = result.split(':').collect();
        assert_eq!(parts.len(), 20, "Should truncate to timestamp + 19 values");
        assert_eq!(parts[0], "1234567890", "Timestamp should be ctime");
        assert_eq!(parts[1], "1", "First archivable value");
        assert_eq!(parts[19], "19", "Last value should be column 19 (truncated)");
    }

    #[test]
    fn test_transform_data_storage_no_change() {
        // Storage format is same for Pve2 and Pve9_0 (2 columns, no skipping)
        let data = "1234567890:1000000000000:500000000000";

        let schema = RrdSchema::storage(RrdFormat::Pve9_0);
        let result =
            RrdWriter::transform_data(data, RrdFormat::Pve2, &schema, MetricType::Storage).unwrap();

        assert_eq!(result, data, "Storage data should not be transformed");
    }

    #[test]
    fn test_metric_type_methods() {
        assert_eq!(MetricType::Node.skip_columns(), 2);
        assert_eq!(MetricType::Vm.skip_columns(), 4);
        assert_eq!(MetricType::Storage.skip_columns(), 0);
    }

    #[test]
    fn test_format_column_counts() {
        assert_eq!(MetricType::Node.column_count(RrdFormat::Pve2), 12);
        assert_eq!(MetricType::Node.column_count(RrdFormat::Pve9_0), 19);
        assert_eq!(MetricType::Vm.column_count(RrdFormat::Pve2), 10);
        assert_eq!(MetricType::Vm.column_count(RrdFormat::Pve9_0), 17);
        assert_eq!(MetricType::Storage.column_count(RrdFormat::Pve2), 2);
        assert_eq!(MetricType::Storage.column_count(RrdFormat::Pve9_0), 2);
    }

    // ===== Critical Bug Fix Tests =====

    #[test]
    fn test_transform_data_node_pve9_skips_columns() {
        // CRITICAL: Test that skip(2) correctly removes uptime+sublevel, leaving ctime as first field
        // pvestatd format: "uptime:sublevel:ctime:loadavg:maxcpu:cpu:iowait:..."
        // = 2 non-archivable + 1 timestamp + 19 archivable = 22 fields
        let data = "1000:0:1234567890:1.5:4:2.0:0.5:8000000000:6000000000:0:0:0:0:1000000:500000:7000000000:0:0.12:0.05:0.02:0.08:0.03";

        let schema = RrdSchema::node(RrdFormat::Pve9_0);
        let result =
            RrdWriter::transform_data(data, RrdFormat::Pve9_0, &schema, MetricType::Node).unwrap();

        // After skip(2): "1234567890:1.5:4:2.0:..." = 20 fields (exact match)
        let parts: Vec<&str> = result.split(':').collect();
        assert_eq!(parts[0], "1234567890", "Timestamp should be ctime (not uptime)");
        assert_eq!(parts.len(), 20, "Should have timestamp + 19 values");
        assert_eq!(
            parts[1], "1.5",
            "First value after skip should be loadavg (not uptime)"
        );
        assert_eq!(parts[2], "4", "Second value should be maxcpu (not sublevel)");
        assert_eq!(parts[3], "2.0", "Third value should be cpu");
    }

    #[test]
    fn test_transform_data_vm_pve9_skips_columns() {
        // CRITICAL: Test that skip(4) correctly removes uptime+name+status+template,
        // leaving ctime as first field
        // pvestatd format: "uptime:name:status:template:ctime:maxcpu:cpu:maxmem:..."
        // = 4 non-archivable + 1 timestamp + 17 archivable = 22 fields
        let data = "1000:myvm:1:0:1234567890:4:2:4096:2048:100000:50000:1000:500:100:50:8192:0.10:0.05:0.08:0.03:0.12:0.06";

        let schema = RrdSchema::vm(RrdFormat::Pve9_0);
        let result =
            RrdWriter::transform_data(data, RrdFormat::Pve9_0, &schema, MetricType::Vm).unwrap();

        // After skip(4): "1234567890:4:2:4096:..." = 18 fields (exact match)
        let parts: Vec<&str> = result.split(':').collect();
        assert_eq!(parts[0], "1234567890", "Timestamp should be ctime (not uptime)");
        assert_eq!(parts.len(), 18, "Should have timestamp + 17 values");
        assert_eq!(
            parts[1], "4",
            "First value after skip should be maxcpu (not uptime)"
        );
        assert_eq!(parts[2], "2", "Second value should be cpu (not name)");
        assert_eq!(parts[3], "4096", "Third value should be maxmem");
    }

    #[tokio::test]
    async fn test_writer_recreates_deleted_file() {
        // CRITICAL: Test that file recreation works after deletion
        // This verifies the fix for the cache invalidation bug
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let backend = Box::new(super::super::backend::RrdDirectBackend::new());
        let mut writer = RrdWriter::with_backend(temp_dir.path(), backend)
            .await
            .unwrap();

        // First update creates the file
        writer
            .update("pve2-storage/node1/local", "N:1000:500")
            .await
            .unwrap();

        let file_path = temp_dir
            .path()
            .join("pve-storage-9.0")
            .join("node1")
            .join("local");

        assert!(file_path.exists(), "File should exist after first update");

        // Simulate file deletion (e.g., log rotation)
        std::fs::remove_file(&file_path).unwrap();
        assert!(!file_path.exists(), "File should be deleted");

        // Second update should recreate the file
        writer
            .update("pve2-storage/node1/local", "N:2000:750")
            .await
            .unwrap();

        assert!(
            file_path.exists(),
            "File should be recreated after deletion"
        );
    }
}
