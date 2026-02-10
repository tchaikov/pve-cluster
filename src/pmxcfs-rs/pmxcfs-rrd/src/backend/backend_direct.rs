/// RRD Backend: Direct file writing
///
/// Uses the `rrd` crate (librrd bindings) for direct RRD file operations.
/// This backend is used as a fallback when rrdcached is unavailable.
///
/// This matches the C implementation's behavior in status.c:1416-1420 where
/// it falls back to rrd_update_r() and rrd_create_r() for direct file access.
use super::super::schema::RrdSchema;
use super::RRD_STEP_SECONDS;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::Path;
use std::time::Duration;

/// RRD backend using direct file operations via librrd
pub struct RrdDirectBackend {
    // Currently stateless, but kept as struct for future enhancements
}

impl RrdDirectBackend {
    /// Create a new direct file backend
    pub fn new() -> Self {
        tracing::info!("Using direct RRD file backend (via librrd)");
        Self {}
    }
}

impl Default for RrdDirectBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl super::super::backend::RrdBackend for RrdDirectBackend {
    async fn update(&mut self, file_path: &Path, data: &str) -> Result<()> {
        // Parse update data using shared logic (consistent across all backends)
        let parsed = super::super::parse::UpdateData::parse(data)?;

        let path = file_path.to_path_buf();
        let data_str = data.to_string();

        // Use tokio::task::spawn_blocking for sync rrd operations
        // This prevents blocking the async runtime
        tokio::task::spawn_blocking(move || {
            // Determine timestamp
            let timestamp: i64 = parsed.timestamp.unwrap_or_else(|| {
                // "N" means "now" in RRD terminology
                chrono::Utc::now().timestamp()
            });

            let timestamp = chrono::DateTime::from_timestamp(timestamp, 0)
                .ok_or_else(|| anyhow::anyhow!("Invalid timestamp value: {}", timestamp))?;

            // Convert values to Datum
            // Note: We convert NaN (from "U" or invalid values) to Unspecified
            let values: Vec<rrd::ops::update::Datum> = parsed
                .values
                .iter()
                .map(|v| {
                    if v.is_nan() {
                        rrd::ops::update::Datum::Unspecified
                    } else if let Some(int_val) = v.is_finite().then_some(*v as u64) {
                        if (*v as u64 as f64 - *v).abs() < f64::EPSILON {
                            rrd::ops::update::Datum::Int(int_val)
                        } else {
                            rrd::ops::update::Datum::Float(*v)
                        }
                    } else {
                        rrd::ops::update::Datum::Float(*v)
                    }
                })
                .collect();

            // Perform the update
            rrd::ops::update::update_all(
                &path,
                rrd::ops::update::ExtraFlags::empty(),
                &[(
                    rrd::ops::update::BatchTime::Timestamp(timestamp),
                    values.as_slice(),
                )],
            )
            .with_context(|| format!("Direct RRD update failed for {:?}", path))?;

            tracing::trace!("Updated RRD via direct file: {:?} -> {}", path, data_str);

            Ok::<(), anyhow::Error>(())
        })
        .await
        .context("Failed to spawn blocking task for RRD update")??;

        Ok(())
    }

    async fn create(
        &mut self,
        file_path: &Path,
        schema: &RrdSchema,
        start_timestamp: i64,
    ) -> Result<()> {
        tracing::debug!(
            "Creating RRD file via direct: {:?} with {} data sources",
            file_path,
            schema.column_count()
        );

        let path = file_path.to_path_buf();
        let schema = schema.clone();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {parent:?}"))?;
        }

        // Use tokio::task::spawn_blocking for sync rrd operations
        tokio::task::spawn_blocking(move || {
            // Convert timestamp
            let start = chrono::DateTime::from_timestamp(start_timestamp, 0)
                .ok_or_else(|| anyhow::anyhow!("Invalid start timestamp: {}", start_timestamp))?;

            // Convert data sources
            let data_sources: Vec<rrd::ops::create::DataSource> = schema
                .data_sources
                .iter()
                .map(|ds| {
                    let name = rrd::ops::create::DataSourceName::new(ds.name);

                    match ds.ds_type {
                        "GAUGE" => {
                            let min = if ds.min == "U" {
                                None
                            } else {
                                Some(ds.min.parse().context("Invalid min value")?)
                            };
                            let max = if ds.max == "U" {
                                None
                            } else {
                                Some(ds.max.parse().context("Invalid max value")?)
                            };
                            Ok(rrd::ops::create::DataSource::gauge(
                                name,
                                ds.heartbeat,
                                min,
                                max,
                            ))
                        }
                        "DERIVE" => {
                            let min = if ds.min == "U" {
                                None
                            } else {
                                Some(ds.min.parse().context("Invalid min value")?)
                            };
                            let max = if ds.max == "U" {
                                None
                            } else {
                                Some(ds.max.parse().context("Invalid max value")?)
                            };
                            Ok(rrd::ops::create::DataSource::derive(
                                name,
                                ds.heartbeat,
                                min,
                                max,
                            ))
                        }
                        "COUNTER" => {
                            let min = if ds.min == "U" {
                                None
                            } else {
                                Some(ds.min.parse().context("Invalid min value")?)
                            };
                            let max = if ds.max == "U" {
                                None
                            } else {
                                Some(ds.max.parse().context("Invalid max value")?)
                            };
                            Ok(rrd::ops::create::DataSource::counter(
                                name,
                                ds.heartbeat,
                                min,
                                max,
                            ))
                        }
                        "ABSOLUTE" => {
                            let min = if ds.min == "U" {
                                None
                            } else {
                                Some(ds.min.parse().context("Invalid min value")?)
                            };
                            let max = if ds.max == "U" {
                                None
                            } else {
                                Some(ds.max.parse().context("Invalid max value")?)
                            };
                            Ok(rrd::ops::create::DataSource::absolute(
                                name,
                                ds.heartbeat,
                                min,
                                max,
                            ))
                        }
                        _ => anyhow::bail!("Unsupported data source type: {}", ds.ds_type),
                    }
                })
                .collect::<Result<Vec<_>>>()?;

            // Convert RRAs
            let archives: Result<Vec<rrd::ops::create::Archive>> = schema
                .archives
                .iter()
                .map(|rra| {
                    // Parse RRA string: "RRA:AVERAGE:0.5:1:1440"
                    let parts: Vec<&str> = rra.split(':').collect();
                    if parts.len() != 5 || parts[0] != "RRA" {
                        anyhow::bail!("Invalid RRA format: {}", rra);
                    }

                    let cf = match parts[1] {
                        "AVERAGE" => rrd::ConsolidationFn::Avg,
                        "MIN" => rrd::ConsolidationFn::Min,
                        "MAX" => rrd::ConsolidationFn::Max,
                        "LAST" => rrd::ConsolidationFn::Last,
                        _ => anyhow::bail!("Unsupported consolidation function: {}", parts[1]),
                    };

                    let xff: f64 = parts[2]
                        .parse()
                        .with_context(|| format!("Invalid xff in RRA: {}", rra))?;
                    let steps: u32 = parts[3]
                        .parse()
                        .with_context(|| format!("Invalid steps in RRA: {}", rra))?;
                    let rows: u32 = parts[4]
                        .parse()
                        .with_context(|| format!("Invalid rows in RRA: {}", rra))?;

                    rrd::ops::create::Archive::new(cf, xff, steps, rows)
                        .map_err(|e| anyhow::anyhow!("Failed to create archive: {}", e))
                })
                .collect();

            let archives = archives?;

            // Call rrd::ops::create::create with no_overwrite = true to prevent race condition
            rrd::ops::create::create(
                &path,
                start,
                Duration::from_secs(RRD_STEP_SECONDS),
                true, // no_overwrite = true (prevent concurrent create race)
                None, // template
                &[],  // sources
                data_sources.iter(),
                archives.iter(),
            )
            .with_context(|| format!("Direct RRD create failed for {:?}", path))?;

            tracing::info!("Created RRD file via direct: {:?} ({})", path, schema);

            Ok::<(), anyhow::Error>(())
        })
        .await
        .context("Failed to spawn blocking task for RRD create")??;

        Ok(())
    }

    async fn flush(&mut self) -> Result<()> {
        // No-op for direct backend - writes are immediate
        tracing::trace!("Flush called on direct backend (no-op)");
        Ok(())
    }

    fn name(&self) -> &str {
        "direct"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::RrdBackend;
    use crate::schema::{RrdFormat, RrdSchema};
    use std::path::PathBuf;
    use tempfile::TempDir;

    // ===== Test Helpers =====

    /// Create a temporary directory for RRD files
    fn setup_temp_dir() -> TempDir {
        TempDir::new().expect("Failed to create temp directory")
    }

    /// Create a test RRD file path
    fn test_rrd_path(dir: &TempDir, name: &str) -> PathBuf {
        dir.path().join(format!("{}.rrd", name))
    }

    // ===== RrdDirectBackend Tests =====

    #[tokio::test]
    async fn test_direct_backend_create_node_rrd() {
        let temp_dir = setup_temp_dir();
        let rrd_path = test_rrd_path(&temp_dir, "node_test");

        let mut backend = RrdDirectBackend::new();
        let schema = RrdSchema::node(RrdFormat::Pve9_0);
        let start_time = 1704067200; // 2024-01-01 00:00:00

        // Create RRD file
        let result = backend.create(&rrd_path, &schema, start_time).await;
        assert!(
            result.is_ok(),
            "Failed to create node RRD: {:?}",
            result.err()
        );

        // Verify file was created
        assert!(rrd_path.exists(), "RRD file should exist after create");

        // Verify backend name
        assert_eq!(backend.name(), "direct");
    }

    #[tokio::test]
    async fn test_direct_backend_create_vm_rrd() {
        let temp_dir = setup_temp_dir();
        let rrd_path = test_rrd_path(&temp_dir, "vm_test");

        let mut backend = RrdDirectBackend::new();
        let schema = RrdSchema::vm(RrdFormat::Pve9_0);
        let start_time = 1704067200;

        let result = backend.create(&rrd_path, &schema, start_time).await;
        assert!(
            result.is_ok(),
            "Failed to create VM RRD: {:?}",
            result.err()
        );
        assert!(rrd_path.exists());
    }

    #[tokio::test]
    async fn test_direct_backend_create_storage_rrd() {
        let temp_dir = setup_temp_dir();
        let rrd_path = test_rrd_path(&temp_dir, "storage_test");

        let mut backend = RrdDirectBackend::new();
        let schema = RrdSchema::storage(RrdFormat::Pve2);
        let start_time = 1704067200;

        let result = backend.create(&rrd_path, &schema, start_time).await;
        assert!(
            result.is_ok(),
            "Failed to create storage RRD: {:?}",
            result.err()
        );
        assert!(rrd_path.exists());
    }

    #[tokio::test]
    async fn test_direct_backend_update_with_timestamp() {
        let temp_dir = setup_temp_dir();
        let rrd_path = test_rrd_path(&temp_dir, "update_test");

        let mut backend = RrdDirectBackend::new();
        let schema = RrdSchema::storage(RrdFormat::Pve2);
        let start_time = 1704067200;

        // Create RRD file
        backend
            .create(&rrd_path, &schema, start_time)
            .await
            .expect("Failed to create RRD");

        // Update with explicit timestamp and values
        // Format: "timestamp:value1:value2"
        let update_data = "1704067260:1000000:500000"; // total=1MB, used=500KB
        let result = backend.update(&rrd_path, update_data).await;

        assert!(result.is_ok(), "Failed to update RRD: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_direct_backend_update_with_n_timestamp() {
        let temp_dir = setup_temp_dir();
        let rrd_path = test_rrd_path(&temp_dir, "update_n_test");

        let mut backend = RrdDirectBackend::new();
        let schema = RrdSchema::storage(RrdFormat::Pve2);
        let start_time = 1704067200;

        backend
            .create(&rrd_path, &schema, start_time)
            .await
            .expect("Failed to create RRD");

        // Update with "N" (current time) timestamp
        let update_data = "N:2000000:750000";
        let result = backend.update(&rrd_path, update_data).await;

        assert!(
            result.is_ok(),
            "Failed to update RRD with N timestamp: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn test_direct_backend_update_with_unknown_values() {
        let temp_dir = setup_temp_dir();
        let rrd_path = test_rrd_path(&temp_dir, "update_u_test");

        let mut backend = RrdDirectBackend::new();
        let schema = RrdSchema::storage(RrdFormat::Pve2);
        let start_time = 1704067200;

        backend
            .create(&rrd_path, &schema, start_time)
            .await
            .expect("Failed to create RRD");

        // Update with "U" (unknown) values
        let update_data = "N:U:1000000"; // total unknown, used known
        let result = backend.update(&rrd_path, update_data).await;

        assert!(
            result.is_ok(),
            "Failed to update RRD with U values: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn test_direct_backend_update_invalid_data() {
        let temp_dir = setup_temp_dir();
        let rrd_path = test_rrd_path(&temp_dir, "invalid_test");

        let mut backend = RrdDirectBackend::new();
        let schema = RrdSchema::storage(RrdFormat::Pve2);
        let start_time = 1704067200;

        backend
            .create(&rrd_path, &schema, start_time)
            .await
            .expect("Failed to create RRD");

        // Test invalid data formats (all should fail for consistent behavior across backends)
        // Per review: Both daemon and direct backends now use same strict parsing
        // Storage schema has 2 data sources: total, used
        let invalid_cases = vec![
            "",              // Empty string
            ":",             // Only separator
            "timestamp",     // Missing values
            "N",             // No colon separator
            "abc:123:456",   // Invalid timestamp (not N or integer)
            "1234567890:abc:456", // Invalid value (abc)
            "1234567890:123:def", // Invalid value (def)
        ];

        for invalid_data in invalid_cases {
            let result = backend.update(&rrd_path, invalid_data).await;
            assert!(
                result.is_err(),
                "Update should fail for invalid data: '{}', but got Ok",
                invalid_data
            );
        }

        // Test valid data with "U" (unknown) values (storage has 2 columns: total, used)
        let mut timestamp = start_time + 60;
        let valid_u_cases = vec![
            "U:U",       // All unknown
            "100:U",     // Mixed known and unknown
            "U:500",     // Mixed unknown and known
        ];

        for valid_data in valid_u_cases {
            let update_data = format!("{}:{}", timestamp, valid_data);
            let result = backend.update(&rrd_path, &update_data).await;
            assert!(
                result.is_ok(),
                "Update should succeed for data with U: '{}', but got Err: {:?}",
                update_data,
                result.err()
            );
            timestamp += 60; // Increment timestamp for next update
        }
    }

    #[tokio::test]
    async fn test_direct_backend_update_nonexistent_file() {
        let temp_dir = setup_temp_dir();
        let rrd_path = test_rrd_path(&temp_dir, "nonexistent");

        let mut backend = RrdDirectBackend::new();

        // Try to update a file that doesn't exist
        let result = backend.update(&rrd_path, "N:100:200").await;

        assert!(result.is_err(), "Update should fail for nonexistent file");
    }

    #[tokio::test]
    async fn test_direct_backend_flush() {
        let mut backend = RrdDirectBackend::new();

        // Flush should always succeed for direct backend (no-op)
        let result = backend.flush().await;
        assert!(
            result.is_ok(),
            "Flush should always succeed for direct backend"
        );
    }


    #[tokio::test]
    async fn test_direct_backend_multiple_updates() {
        let temp_dir = setup_temp_dir();
        let rrd_path = test_rrd_path(&temp_dir, "multi_update_test");

        let mut backend = RrdDirectBackend::new();
        let schema = RrdSchema::storage(RrdFormat::Pve2);
        let start_time = 1704067200;

        backend
            .create(&rrd_path, &schema, start_time)
            .await
            .expect("Failed to create RRD");

        // Perform multiple updates
        for i in 0..10 {
            let timestamp = start_time + 60 * (i + 1); // 1 minute intervals
            let total = 1000000 + (i * 100000);
            let used = 500000 + (i * 50000);
            let update_data = format!("{}:{}:{}", timestamp, total, used);

            let result = backend.update(&rrd_path, &update_data).await;
            assert!(result.is_ok(), "Update {} failed: {:?}", i, result.err());
        }
    }

    #[tokio::test]
    async fn test_direct_backend_no_overwrite() {
        let temp_dir = setup_temp_dir();
        let rrd_path = test_rrd_path(&temp_dir, "no_overwrite_test");

        let mut backend = RrdDirectBackend::new();
        let schema = RrdSchema::storage(RrdFormat::Pve2);
        let start_time = 1704067200;

        // Create file first time
        backend
            .create(&rrd_path, &schema, start_time)
            .await
            .expect("First create failed");

        // Create same file again - should fail (no_overwrite=true prevents race condition)
        // This matches C implementation's behavior to prevent concurrent create races
        let result = backend.create(&rrd_path, &schema, start_time).await;
        assert!(
            result.is_err(),
            "Creating file again should fail with no_overwrite=true"
        );
    }

    #[tokio::test]
    async fn test_direct_backend_large_schema() {
        let temp_dir = setup_temp_dir();
        let rrd_path = test_rrd_path(&temp_dir, "large_schema_test");

        let mut backend = RrdDirectBackend::new();
        let schema = RrdSchema::node(RrdFormat::Pve9_0); // 19 data sources
        let start_time = 1704067200;

        // Create RRD with large schema
        let result = backend.create(&rrd_path, &schema, start_time).await;
        assert!(result.is_ok(), "Failed to create RRD with large schema");

        // Update with all values
        let values = "100:200:50.5:10.2:8000000:4000000:2000000:500000:50000000:25000000:1000000:2000000:6000000:1000000:0.5:1.2:0.8:0.3:0.1";
        let update_data = format!("N:{}", values);

        let result = backend.update(&rrd_path, &update_data).await;
        assert!(result.is_ok(), "Failed to update RRD with large schema");
    }
}
