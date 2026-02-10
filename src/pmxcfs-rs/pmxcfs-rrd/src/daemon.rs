/// RRDCached Daemon Client (wrapper around vendored rrdcached client)
///
/// This module provides a thin wrapper around our vendored rrdcached client.
use anyhow::{Context, Result};
use std::path::Path;

/// Wrapper around vendored rrdcached client
#[allow(dead_code)] // Used in backend_daemon.rs via module-level access
pub struct RrdCachedClient {
    pub(crate) client:
        tokio::sync::Mutex<crate::rrdcached::RRDCachedClient<tokio::net::UnixStream>>,
}

impl RrdCachedClient {
    /// Connect to rrdcached daemon via Unix socket
    ///
    /// # Arguments
    /// * `socket_path` - Path to rrdcached Unix socket (default: /var/run/rrdcached.sock)
    #[allow(dead_code)] // Used via backend modules
    pub async fn connect<P: AsRef<Path>>(socket_path: P) -> Result<Self> {
        let socket_path = socket_path.as_ref().to_string_lossy().to_string();

        tracing::debug!("Connecting to rrdcached at {}", socket_path);

        // Connect to daemon (async operation)
        let client = crate::rrdcached::RRDCachedClient::connect_unix(&socket_path)
            .await
            .with_context(|| format!("Failed to connect to rrdcached: {socket_path}"))?;

        tracing::info!("Connected to rrdcached at {}", socket_path);

        Ok(Self {
            client: tokio::sync::Mutex::new(client),
        })
    }

    /// Update RRD file via rrdcached
    ///
    /// # Arguments
    /// * `file_path` - Full path to RRD file
    /// * `data` - Update data in format "timestamp:value1:value2:..."
    #[allow(dead_code)] // Used via backend modules
    pub async fn update<P: AsRef<Path>>(&self, file_path: P, data: &str) -> Result<()> {
        let file_path = file_path.as_ref();

        // Parse the update data
        let parts: Vec<&str> = data.split(':').collect();
        if parts.len() < 2 {
            anyhow::bail!("Invalid update data format: {data}");
        }

        let timestamp = if parts[0] == "N" {
            None
        } else {
            Some(
                parts[0]
                    .parse::<usize>()
                    .with_context(|| format!("Invalid timestamp: {}", parts[0]))?,
            )
        };

        let values: Vec<f64> = parts[1..]
            .iter()
            .map(|v| {
                if *v == "U" {
                    Ok(f64::NAN)
                } else {
                    v.parse::<f64>()
                        .with_context(|| format!("Invalid value: {v}"))
                }
            })
            .collect::<Result<Vec<_>>>()?;

        // Get file path without .rrd extension (rrdcached-client adds it)
        let path_str = file_path.to_string_lossy();
        let path_without_ext = path_str.strip_suffix(".rrd").unwrap_or(&path_str);

        // Send update via rrdcached
        let mut client = self.client.lock().await;
        client
            .update(path_without_ext, timestamp, values)
            .await
            .context("Failed to send update to rrdcached")?;

        tracing::trace!("Updated RRD via daemon: {:?} -> {}", file_path, data);

        Ok(())
    }

    /// Create RRD file via rrdcached
    #[allow(dead_code)] // Used via backend modules
    pub async fn create(&self, args: crate::rrdcached::create::CreateArguments) -> Result<()> {
        let mut client = self.client.lock().await;
        client
            .create(args)
            .await
            .context("Failed to create RRD via rrdcached")?;
        Ok(())
    }

    /// Flush all pending updates
    #[allow(dead_code)] // Used via backend modules
    pub async fn flush(&self) -> Result<()> {
        let mut client = self.client.lock().await;
        client
            .flush_all()
            .await
            .context("Failed to flush rrdcached")?;

        tracing::debug!("Flushed all RRD files");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Only runs if rrdcached daemon is actually running
    async fn test_connect_to_daemon() {
        // This test requires a running rrdcached daemon
        let result = RrdCachedClient::connect("/var/run/rrdcached.sock").await;

        match result {
            Ok(client) => {
                // Try to flush (basic connectivity test)
                let result = client.flush().await;
                println!("RRDCached flush result: {:?}", result);

                // Connection successful (flush may fail if no files, that's OK)
                assert!(result.is_ok() || result.is_err());
            }
            Err(e) => {
                println!("Note: rrdcached not running (expected in test env): {}", e);
            }
        }
    }
}
