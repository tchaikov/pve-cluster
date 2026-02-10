/// RRD Key Type Parsing and Path Resolution
///
/// This module handles parsing RRD status update keys and mapping them
/// to the appropriate file paths and schemas.
use super::schema::{RrdFormat, RrdSchema};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Metric type for determining column skipping rules
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricType {
    Node,
    Vm,
    Storage,
}

impl MetricType {
    /// Number of non-archivable columns to skip from the start of the data string
    ///
    /// The data from pvestatd has non-archivable fields at the beginning:
    /// - Node: skip 2 (uptime, sublevel) - then ctime:loadavg:maxcpu:...
    /// - VM: skip 4 (uptime, name, status, template) - then ctime:maxcpu:cpu:...
    /// - Storage: skip 0 - data starts with ctime:total:used
    ///
    /// C implementation: status.c:1300 (node skip=2), status.c:1335 (VM skip=4)
    pub fn skip_columns(self) -> usize {
        match self {
            MetricType::Node => 2,
            MetricType::Vm => 4,
            MetricType::Storage => 0,
        }
    }

    /// Get column count for a specific RRD format
    #[allow(dead_code)]
    pub fn column_count(self, format: RrdFormat) -> usize {
        match (format, self) {
            (RrdFormat::Pve2, MetricType::Node) => 12,
            (RrdFormat::Pve9_0, MetricType::Node) => 19,
            (RrdFormat::Pve2, MetricType::Vm) => 10,
            (RrdFormat::Pve9_0, MetricType::Vm) => 17,
            (_, MetricType::Storage) => 2, // Same for both formats
        }
    }
}

/// RRD key types for routing to correct schema and path
///
/// This enum represents the different types of RRD metrics that pmxcfs tracks:
/// - Node metrics (CPU, memory, network for a node)
/// - VM metrics (CPU, memory, disk, network for a VM/CT)
/// - Storage metrics (total/used space for a storage)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RrdKeyType {
    /// Node metrics: pve2-node/{nodename} or pve-node-9.0/{nodename}
    Node { nodename: String, format: RrdFormat },
    /// VM metrics: pve2.3-vm/{vmid} or pve-vm-9.0/{vmid}
    Vm { vmid: String, format: RrdFormat },
    /// Storage metrics: pve2-storage/{node}/{storage} or pve-storage-9.0/{node}/{storage}
    Storage {
        nodename: String,
        storage: String,
        format: RrdFormat,
    },
}

impl RrdKeyType {
    /// Parse RRD key from status update key
    ///
    /// Supported formats:
    /// - "pve2-node/node1" → Node { nodename: "node1", format: Pve2 }
    /// - "pve-node-9.0/node1" → Node { nodename: "node1", format: Pve9_0 }
    /// - "pve2.3-vm/100" → Vm { vmid: "100", format: Pve2 }
    /// - "pve-storage-9.0/node1/local" → Storage { nodename: "node1", storage: "local", format: Pve9_0 }
    ///
    /// # Security
    ///
    /// Path components are validated to prevent directory traversal attacks:
    /// - Rejects paths containing ".."
    /// - Rejects absolute paths
    /// - Rejects paths with special characters that could be exploited
    pub(crate) fn parse(key: &str) -> Result<Self> {
        let parts: Vec<&str> = key.split('/').collect();

        if parts.is_empty() {
            anyhow::bail!("Empty RRD key");
        }

        // Validate all path components for security
        for part in &parts[1..] {
            Self::validate_path_component(part)?;
        }

        match parts[0] {
            "pve2-node" => {
                let nodename = parts.get(1).context("Missing nodename")?.to_string();
                Ok(RrdKeyType::Node {
                    nodename,
                    format: RrdFormat::Pve2,
                })
            }
            prefix if prefix.starts_with("pve-node-") => {
                let nodename = parts.get(1).context("Missing nodename")?.to_string();
                Ok(RrdKeyType::Node {
                    nodename,
                    format: RrdFormat::Pve9_0,
                })
            }
            "pve2.3-vm" => {
                let vmid = parts.get(1).context("Missing vmid")?.to_string();
                Ok(RrdKeyType::Vm {
                    vmid,
                    format: RrdFormat::Pve2,
                })
            }
            prefix if prefix.starts_with("pve-vm-") => {
                let vmid = parts.get(1).context("Missing vmid")?.to_string();
                Ok(RrdKeyType::Vm {
                    vmid,
                    format: RrdFormat::Pve9_0,
                })
            }
            "pve2-storage" => {
                let nodename = parts.get(1).context("Missing nodename")?.to_string();
                let storage = parts.get(2).context("Missing storage")?.to_string();
                Ok(RrdKeyType::Storage {
                    nodename,
                    storage,
                    format: RrdFormat::Pve2,
                })
            }
            prefix if prefix.starts_with("pve-storage-") => {
                let nodename = parts.get(1).context("Missing nodename")?.to_string();
                let storage = parts.get(2).context("Missing storage")?.to_string();
                Ok(RrdKeyType::Storage {
                    nodename,
                    storage,
                    format: RrdFormat::Pve9_0,
                })
            }
            _ => anyhow::bail!("Unknown RRD key format: {key}"),
        }
    }

    /// Validate a path component for security
    ///
    /// Prevents directory traversal attacks by rejecting:
    /// - ".." (parent directory)
    /// - Absolute paths (starting with "/")
    /// - Empty components
    /// - Components with null bytes or other dangerous characters
    fn validate_path_component(component: &str) -> Result<()> {
        if component.is_empty() {
            anyhow::bail!("Empty path component");
        }

        if component == ".." {
            anyhow::bail!("Path traversal attempt: '..' not allowed");
        }

        if component.starts_with('/') {
            anyhow::bail!("Absolute paths not allowed");
        }

        if component.contains('\0') {
            anyhow::bail!("Null byte in path component");
        }

        // Reject other potentially dangerous characters
        if component.contains(['\\', '\n', '\r']) {
            anyhow::bail!("Invalid characters in path component");
        }

        Ok(())
    }

    /// Get the RRD file path for this key type
    ///
    /// Always returns paths using the current format (9.0), regardless of the input format.
    /// This enables transparent format migration: old PVE8 nodes can send `pve2-node/` keys,
    /// and they'll be written to `pve-node-9.0/` files automatically.
    ///
    /// # Format Migration Strategy
    ///
    /// The C implementation always creates files in the current format directory
    /// (see status.c:1287). This Rust implementation follows the same approach:
    /// - Input: `pve2-node/node1` → Output: `/var/lib/rrdcached/db/pve-node-9.0/node1`
    /// - Input: `pve-node-9.0/node1` → Output: `/var/lib/rrdcached/db/pve-node-9.0/node1`
    ///
    /// This allows rolling upgrades where old and new nodes coexist in the same cluster.
    pub(crate) fn file_path(&self, base_dir: &Path) -> PathBuf {
        match self {
            RrdKeyType::Node { nodename, .. } => {
                // Always use current format path
                base_dir.join("pve-node-9.0").join(nodename)
            }
            RrdKeyType::Vm { vmid, .. } => {
                // Always use current format path
                base_dir.join("pve-vm-9.0").join(vmid)
            }
            RrdKeyType::Storage {
                nodename, storage, ..
            } => {
                // Always use current format path
                base_dir
                    .join("pve-storage-9.0")
                    .join(nodename)
                    .join(storage)
            }
        }
    }

    /// Get the source format from the input key
    ///
    /// This is used for data transformation (padding/truncation).
    pub(crate) fn source_format(&self) -> RrdFormat {
        match self {
            RrdKeyType::Node { format, .. }
            | RrdKeyType::Vm { format, .. }
            | RrdKeyType::Storage { format, .. } => *format,
        }
    }

    /// Get the target RRD schema (always current format)
    ///
    /// Files are always created using the current format (Pve9_0),
    /// regardless of the source format in the key.
    pub(crate) fn schema(&self) -> RrdSchema {
        match self {
            RrdKeyType::Node { .. } => RrdSchema::node(RrdFormat::Pve9_0),
            RrdKeyType::Vm { .. } => RrdSchema::vm(RrdFormat::Pve9_0),
            RrdKeyType::Storage { .. } => RrdSchema::storage(RrdFormat::Pve9_0),
        }
    }

    /// Get the metric type for this key
    pub(crate) fn metric_type(&self) -> MetricType {
        match self {
            RrdKeyType::Node { .. } => MetricType::Node,
            RrdKeyType::Vm { .. } => MetricType::Vm,
            RrdKeyType::Storage { .. } => MetricType::Storage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_node_keys() {
        let key = RrdKeyType::parse("pve2-node/testnode").unwrap();
        assert_eq!(
            key,
            RrdKeyType::Node {
                nodename: "testnode".to_string(),
                format: RrdFormat::Pve2
            }
        );

        let key = RrdKeyType::parse("pve-node-9.0/testnode").unwrap();
        assert_eq!(
            key,
            RrdKeyType::Node {
                nodename: "testnode".to_string(),
                format: RrdFormat::Pve9_0
            }
        );
    }

    #[test]
    fn test_parse_vm_keys() {
        let key = RrdKeyType::parse("pve2.3-vm/100").unwrap();
        assert_eq!(
            key,
            RrdKeyType::Vm {
                vmid: "100".to_string(),
                format: RrdFormat::Pve2
            }
        );

        let key = RrdKeyType::parse("pve-vm-9.0/100").unwrap();
        assert_eq!(
            key,
            RrdKeyType::Vm {
                vmid: "100".to_string(),
                format: RrdFormat::Pve9_0
            }
        );
    }

    #[test]
    fn test_parse_storage_keys() {
        let key = RrdKeyType::parse("pve2-storage/node1/local").unwrap();
        assert_eq!(
            key,
            RrdKeyType::Storage {
                nodename: "node1".to_string(),
                storage: "local".to_string(),
                format: RrdFormat::Pve2
            }
        );

        let key = RrdKeyType::parse("pve-storage-9.0/node1/local").unwrap();
        assert_eq!(
            key,
            RrdKeyType::Storage {
                nodename: "node1".to_string(),
                storage: "local".to_string(),
                format: RrdFormat::Pve9_0
            }
        );
    }

    #[test]
    fn test_file_paths() {
        let base = Path::new("/var/lib/rrdcached/db");

        // New format key → new format path
        let key = RrdKeyType::Node {
            nodename: "node1".to_string(),
            format: RrdFormat::Pve9_0,
        };
        assert_eq!(
            key.file_path(base),
            PathBuf::from("/var/lib/rrdcached/db/pve-node-9.0/node1")
        );

        // Old format key → new format path (auto-upgrade!)
        let key = RrdKeyType::Node {
            nodename: "node1".to_string(),
            format: RrdFormat::Pve2,
        };
        assert_eq!(
            key.file_path(base),
            PathBuf::from("/var/lib/rrdcached/db/pve-node-9.0/node1"),
            "Old format keys should create new format files"
        );

        // VM: Old format → new format
        let key = RrdKeyType::Vm {
            vmid: "100".to_string(),
            format: RrdFormat::Pve2,
        };
        assert_eq!(
            key.file_path(base),
            PathBuf::from("/var/lib/rrdcached/db/pve-vm-9.0/100"),
            "Old VM format should upgrade to new format"
        );

        // Storage: Always uses current format
        let key = RrdKeyType::Storage {
            nodename: "node1".to_string(),
            storage: "local".to_string(),
            format: RrdFormat::Pve2,
        };
        assert_eq!(
            key.file_path(base),
            PathBuf::from("/var/lib/rrdcached/db/pve-storage-9.0/node1/local"),
            "Old storage format should upgrade to new format"
        );
    }

    #[test]
    fn test_source_format() {
        let key = RrdKeyType::Node {
            nodename: "node1".to_string(),
            format: RrdFormat::Pve2,
        };
        assert_eq!(key.source_format(), RrdFormat::Pve2);

        let key = RrdKeyType::Vm {
            vmid: "100".to_string(),
            format: RrdFormat::Pve9_0,
        };
        assert_eq!(key.source_format(), RrdFormat::Pve9_0);
    }

    #[test]
    fn test_schema_always_current_format() {
        // Even with Pve2 source format, schema should return Pve9_0
        let key = RrdKeyType::Node {
            nodename: "node1".to_string(),
            format: RrdFormat::Pve2,
        };
        let schema = key.schema();
        assert_eq!(
            schema.format,
            RrdFormat::Pve9_0,
            "Schema should always use current format"
        );
        assert_eq!(schema.column_count(), 19, "Should have Pve9_0 column count");

        // Pve9_0 source also gets Pve9_0 schema
        let key = RrdKeyType::Node {
            nodename: "node1".to_string(),
            format: RrdFormat::Pve9_0,
        };
        let schema = key.schema();
        assert_eq!(schema.format, RrdFormat::Pve9_0);
        assert_eq!(schema.column_count(), 19);
    }
}
