/// RRD Backend: rrdcached daemon
///
/// Uses rrdcached for batched, high-performance RRD updates.
/// This is the preferred backend when the daemon is available.
use super::super::rrdcached::consolidation_function::ConsolidationFunction;
use super::super::rrdcached::create::{
    CreateArguments, CreateDataSource, CreateDataSourceType, CreateRoundRobinArchive,
};
use super::super::rrdcached::RRDCachedClient;
use super::super::schema::RrdSchema;
use super::RRD_STEP_SECONDS;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::Path;

/// RRD backend using rrdcached daemon
pub struct RrdCachedBackend {
    client: RRDCachedClient<tokio::net::UnixStream>,
}

impl RrdCachedBackend {
    /// Connect to rrdcached daemon
    ///
    /// # Arguments
    /// * `socket_path` - Path to rrdcached Unix socket (default: /var/run/rrdcached.sock)
    pub async fn connect(socket_path: &str) -> Result<Self> {
        let client = RRDCachedClient::connect_unix(socket_path)
            .await
            .with_context(|| format!("Failed to connect to rrdcached at {socket_path}"))?;

        tracing::info!("Connected to rrdcached at {}", socket_path);

        Ok(Self { client })
    }
}

#[async_trait]
impl super::super::backend::RrdBackend for RrdCachedBackend {
    async fn update(&mut self, file_path: &Path, data: &str) -> Result<()> {
        // Parse update data using shared logic (consistent across all backends)
        let parsed = super::super::parse::UpdateData::parse(data)?;

        // Get file path without .rrd extension (rrdcached-client adds it)
        let path_str = file_path.to_string_lossy();
        let path_without_ext = path_str.strip_suffix(".rrd").unwrap_or(&path_str);

        // Convert timestamp to usize for rrdcached-client
        let timestamp = parsed.timestamp.map(|t| t as usize);

        // Send update via rrdcached
        self.client
            .update(path_without_ext, timestamp, parsed.values)
            .await
            .with_context(|| format!("rrdcached update failed for {:?}", file_path))?;

        tracing::trace!("Updated RRD via daemon: {:?} -> {}", file_path, data);

        Ok(())
    }

    async fn create(
        &mut self,
        file_path: &Path,
        schema: &RrdSchema,
        start_timestamp: i64,
    ) -> Result<()> {
        tracing::debug!(
            "Creating RRD file via daemon: {:?} with {} data sources",
            file_path,
            schema.column_count()
        );

        // Convert our data sources to rrdcached-client CreateDataSource objects
        let mut data_sources = Vec::new();
        for ds in &schema.data_sources {
            let serie_type = match ds.ds_type {
                "GAUGE" => CreateDataSourceType::Gauge,
                "DERIVE" => CreateDataSourceType::Derive,
                "COUNTER" => CreateDataSourceType::Counter,
                "ABSOLUTE" => CreateDataSourceType::Absolute,
                _ => anyhow::bail!("Unsupported data source type: {}", ds.ds_type),
            };

            // Parse min/max values
            let minimum = if ds.min == "U" {
                None
            } else {
                ds.min.parse().ok()
            };
            let maximum = if ds.max == "U" {
                None
            } else {
                ds.max.parse().ok()
            };

            let data_source = CreateDataSource {
                name: ds.name.to_string(),
                minimum,
                maximum,
                heartbeat: ds.heartbeat as i64,
                serie_type,
            };

            data_sources.push(data_source);
        }

        // Convert our RRA definitions to rrdcached-client CreateRoundRobinArchive objects
        let mut archives = Vec::new();
        for rra in &schema.archives {
            // Parse RRA string: "RRA:AVERAGE:0.5:1:70"
            let parts: Vec<&str> = rra.split(':').collect();
            if parts.len() != 5 || parts[0] != "RRA" {
                anyhow::bail!("Invalid RRA format: {rra}");
            }

            let consolidation_function = match parts[1] {
                "AVERAGE" => ConsolidationFunction::Average,
                "MIN" => ConsolidationFunction::Min,
                "MAX" => ConsolidationFunction::Max,
                "LAST" => ConsolidationFunction::Last,
                _ => anyhow::bail!("Unsupported consolidation function: {}", parts[1]),
            };

            let xfiles_factor: f64 = parts[2]
                .parse()
                .with_context(|| format!("Invalid xff in RRA: {rra}"))?;
            let steps: i64 = parts[3]
                .parse()
                .with_context(|| format!("Invalid steps in RRA: {rra}"))?;
            let rows: i64 = parts[4]
                .parse()
                .with_context(|| format!("Invalid rows in RRA: {rra}"))?;

            let archive = CreateRoundRobinArchive {
                consolidation_function,
                xfiles_factor,
                steps,
                rows,
            };
            archives.push(archive);
        }

        // Get path without .rrd extension (rrdcached-client adds it)
        let path_str = file_path.to_string_lossy();
        let path_without_ext = path_str
            .strip_suffix(".rrd")
            .unwrap_or(&path_str)
            .to_string();

        // Create CreateArguments
        let create_args = CreateArguments {
            path: path_without_ext,
            data_sources,
            round_robin_archives: archives,
            start_timestamp: start_timestamp as u64,
            step_seconds: RRD_STEP_SECONDS,
        };

        // Validate before sending
        create_args.validate().context("Invalid CREATE arguments")?;

        // Send CREATE command via rrdcached
        self.client
            .create(create_args)
            .await
            .with_context(|| format!("Failed to create RRD file via daemon: {file_path:?}"))?;

        tracing::info!("Created RRD file via daemon: {:?} ({})", file_path, schema);

        Ok(())
    }

    async fn flush(&mut self) -> Result<()> {
        self.client
            .flush_all()
            .await
            .context("Failed to flush rrdcached")?;

        tracing::debug!("Flushed all pending RRD updates");

        Ok(())
    }

    fn name(&self) -> &str {
        "rrdcached"
    }
}
