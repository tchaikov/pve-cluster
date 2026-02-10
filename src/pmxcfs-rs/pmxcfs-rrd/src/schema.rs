/// RRD Schema Definitions
///
/// Defines RRD database schemas matching the C pmxcfs implementation.
/// Each schema specifies data sources (DS) and round-robin archives (RRA).
use std::fmt;

/// RRD format version
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RrdFormat {
    /// Legacy pve2 format (12 columns for node, 10 for VM, 2 for storage)
    Pve2,
    /// New pve-9.0 format (19 columns for node, 17 for VM, 2 for storage)
    Pve9_0,
}

/// RRD data source definition
#[derive(Debug, Clone)]
pub struct RrdDataSource {
    /// Data source name
    pub name: &'static str,
    /// Data source type (GAUGE, COUNTER, DERIVE, ABSOLUTE)
    pub ds_type: &'static str,
    /// Heartbeat (seconds before marking as unknown)
    pub heartbeat: u32,
    /// Minimum value (U for unknown)
    pub min: &'static str,
    /// Maximum value (U for unknown)
    pub max: &'static str,
}

impl RrdDataSource {
    /// Create GAUGE data source with no min/max limits
    pub(super) const fn gauge(name: &'static str) -> Self {
        Self {
            name,
            ds_type: "GAUGE",
            heartbeat: 120,
            min: "0",
            max: "U",
        }
    }

    /// Create DERIVE data source (for counters that can wrap)
    pub(super) const fn derive(name: &'static str) -> Self {
        Self {
            name,
            ds_type: "DERIVE",
            heartbeat: 120,
            min: "0",
            max: "U",
        }
    }

    /// Format as RRD command line argument
    ///
    /// Matches C implementation format: "DS:name:TYPE:heartbeat:min:max"
    /// (see rrd_def_node in src/pmxcfs/status.c:1100)
    ///
    /// Currently unused but kept for debugging/testing and C format compatibility.
    #[allow(dead_code)]
    pub(super) fn to_arg(&self) -> String {
        format!(
            "DS:{}:{}:{}:{}:{}",
            self.name, self.ds_type, self.heartbeat, self.min, self.max
        )
    }
}

/// RRD schema with data sources and archives
#[derive(Debug, Clone)]
pub struct RrdSchema {
    /// RRD format version
    pub format: RrdFormat,
    /// Data sources
    pub data_sources: Vec<RrdDataSource>,
    /// Round-robin archives (RRA definitions)
    pub archives: Vec<String>,
}

impl RrdSchema {
    /// Create node RRD schema
    pub fn node(format: RrdFormat) -> Self {
        let data_sources = match format {
            RrdFormat::Pve2 => vec![
                RrdDataSource::gauge("loadavg"),
                RrdDataSource::gauge("maxcpu"),
                RrdDataSource::gauge("cpu"),
                RrdDataSource::gauge("iowait"),
                RrdDataSource::gauge("memtotal"),
                RrdDataSource::gauge("memused"),
                RrdDataSource::gauge("swaptotal"),
                RrdDataSource::gauge("swapused"),
                RrdDataSource::gauge("roottotal"),
                RrdDataSource::gauge("rootused"),
                RrdDataSource::derive("netin"),
                RrdDataSource::derive("netout"),
            ],
            RrdFormat::Pve9_0 => vec![
                RrdDataSource::gauge("loadavg"),
                RrdDataSource::gauge("maxcpu"),
                RrdDataSource::gauge("cpu"),
                RrdDataSource::gauge("iowait"),
                RrdDataSource::gauge("memtotal"),
                RrdDataSource::gauge("memused"),
                RrdDataSource::gauge("swaptotal"),
                RrdDataSource::gauge("swapused"),
                RrdDataSource::gauge("roottotal"),
                RrdDataSource::gauge("rootused"),
                RrdDataSource::derive("netin"),
                RrdDataSource::derive("netout"),
                RrdDataSource::gauge("memavailable"),
                RrdDataSource::gauge("arcsize"),
                RrdDataSource::gauge("pressurecpusome"),
                RrdDataSource::gauge("pressureiosome"),
                RrdDataSource::gauge("pressureiofull"),
                RrdDataSource::gauge("pressurememorysome"),
                RrdDataSource::gauge("pressurememoryfull"),
            ],
        };

        Self {
            format,
            data_sources,
            archives: Self::default_archives(),
        }
    }

    /// Create VM RRD schema
    pub fn vm(format: RrdFormat) -> Self {
        let data_sources = match format {
            RrdFormat::Pve2 => vec![
                RrdDataSource::gauge("maxcpu"),
                RrdDataSource::gauge("cpu"),
                RrdDataSource::gauge("maxmem"),
                RrdDataSource::gauge("mem"),
                RrdDataSource::gauge("maxdisk"),
                RrdDataSource::gauge("disk"),
                RrdDataSource::derive("netin"),
                RrdDataSource::derive("netout"),
                RrdDataSource::derive("diskread"),
                RrdDataSource::derive("diskwrite"),
            ],
            RrdFormat::Pve9_0 => vec![
                RrdDataSource::gauge("maxcpu"),
                RrdDataSource::gauge("cpu"),
                RrdDataSource::gauge("maxmem"),
                RrdDataSource::gauge("mem"),
                RrdDataSource::gauge("maxdisk"),
                RrdDataSource::gauge("disk"),
                RrdDataSource::derive("netin"),
                RrdDataSource::derive("netout"),
                RrdDataSource::derive("diskread"),
                RrdDataSource::derive("diskwrite"),
                RrdDataSource::gauge("memhost"),
                RrdDataSource::gauge("pressurecpusome"),
                RrdDataSource::gauge("pressurecpufull"),
                RrdDataSource::gauge("pressureiosome"),
                RrdDataSource::gauge("pressureiofull"),
                RrdDataSource::gauge("pressurememorysome"),
                RrdDataSource::gauge("pressurememoryfull"),
            ],
        };

        Self {
            format,
            data_sources,
            archives: Self::default_archives(),
        }
    }

    /// Create storage RRD schema
    pub fn storage(format: RrdFormat) -> Self {
        let data_sources = vec![RrdDataSource::gauge("total"), RrdDataSource::gauge("used")];

        Self {
            format,
            data_sources,
            archives: Self::default_archives(),
        }
    }

    /// Default RRA (Round-Robin Archive) definitions
    ///
    /// These match the C implementation's archives for 60-second step size:
    /// - RRA:AVERAGE:0.5:1:1440      -> 1 min * 1440 => 1 day
    /// - RRA:AVERAGE:0.5:30:1440     -> 30 min * 1440 => 30 days
    /// - RRA:AVERAGE:0.5:360:1440    -> 6 hours * 1440 => 360 days (~1 year)
    /// - RRA:AVERAGE:0.5:10080:570   -> 1 week * 570 => ~10 years
    /// - RRA:MAX:0.5:1:1440          -> 1 min * 1440 => 1 day
    /// - RRA:MAX:0.5:30:1440         -> 30 min * 1440 => 30 days
    /// - RRA:MAX:0.5:360:1440        -> 6 hours * 1440 => 360 days (~1 year)
    /// - RRA:MAX:0.5:10080:570       -> 1 week * 570 => ~10 years
    pub(super) fn default_archives() -> Vec<String> {
        vec![
            "RRA:AVERAGE:0.5:1:1440".to_string(),
            "RRA:AVERAGE:0.5:30:1440".to_string(),
            "RRA:AVERAGE:0.5:360:1440".to_string(),
            "RRA:AVERAGE:0.5:10080:570".to_string(),
            "RRA:MAX:0.5:1:1440".to_string(),
            "RRA:MAX:0.5:30:1440".to_string(),
            "RRA:MAX:0.5:360:1440".to_string(),
            "RRA:MAX:0.5:10080:570".to_string(),
        ]
    }

    /// Get number of data sources
    pub fn column_count(&self) -> usize {
        self.data_sources.len()
    }
}

impl fmt::Display for RrdSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} schema with {} data sources",
            self.format,
            self.column_count()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_ds_properties(
        ds: &RrdDataSource,
        expected_name: &str,
        expected_type: &str,
        index: usize,
    ) {
        assert_eq!(ds.name, expected_name, "DS[{}] name mismatch", index);
        assert_eq!(ds.ds_type, expected_type, "DS[{}] type mismatch", index);
        assert_eq!(ds.heartbeat, 120, "DS[{}] heartbeat should be 120", index);
        assert_eq!(ds.min, "0", "DS[{}] min should be 0", index);
        assert_eq!(ds.max, "U", "DS[{}] max should be U", index);
    }

    #[test]
    fn test_datasource_construction() {
        let gauge_ds = RrdDataSource::gauge("cpu");
        assert_eq!(gauge_ds.name, "cpu");
        assert_eq!(gauge_ds.ds_type, "GAUGE");
        assert_eq!(gauge_ds.heartbeat, 120);
        assert_eq!(gauge_ds.min, "0");
        assert_eq!(gauge_ds.max, "U");
        assert_eq!(gauge_ds.to_arg(), "DS:cpu:GAUGE:120:0:U");

        let derive_ds = RrdDataSource::derive("netin");
        assert_eq!(derive_ds.name, "netin");
        assert_eq!(derive_ds.ds_type, "DERIVE");
        assert_eq!(derive_ds.heartbeat, 120);
        assert_eq!(derive_ds.min, "0");
        assert_eq!(derive_ds.max, "U");
        assert_eq!(derive_ds.to_arg(), "DS:netin:DERIVE:120:0:U");
    }

    #[test]
    fn test_node_schema_pve2() {
        let schema = RrdSchema::node(RrdFormat::Pve2);

        assert_eq!(schema.column_count(), 12);
        assert_eq!(schema.format, RrdFormat::Pve2);

        let expected_ds = vec![
            ("loadavg", "GAUGE"),
            ("maxcpu", "GAUGE"),
            ("cpu", "GAUGE"),
            ("iowait", "GAUGE"),
            ("memtotal", "GAUGE"),
            ("memused", "GAUGE"),
            ("swaptotal", "GAUGE"),
            ("swapused", "GAUGE"),
            ("roottotal", "GAUGE"),
            ("rootused", "GAUGE"),
            ("netin", "DERIVE"),
            ("netout", "DERIVE"),
        ];

        for (i, (name, ds_type)) in expected_ds.iter().enumerate() {
            assert_ds_properties(&schema.data_sources[i], name, ds_type, i);
        }
    }

    #[test]
    fn test_node_schema_pve9() {
        let schema = RrdSchema::node(RrdFormat::Pve9_0);

        assert_eq!(schema.column_count(), 19);
        assert_eq!(schema.format, RrdFormat::Pve9_0);

        let pve2_schema = RrdSchema::node(RrdFormat::Pve2);
        for i in 0..12 {
            assert_eq!(
                schema.data_sources[i].name, pve2_schema.data_sources[i].name,
                "First 12 DS should match pve2"
            );
            assert_eq!(
                schema.data_sources[i].ds_type, pve2_schema.data_sources[i].ds_type,
                "First 12 DS types should match pve2"
            );
        }

        let pve9_additions = vec![
            ("memavailable", "GAUGE"),
            ("arcsize", "GAUGE"),
            ("pressurecpusome", "GAUGE"),
            ("pressureiosome", "GAUGE"),
            ("pressureiofull", "GAUGE"),
            ("pressurememorysome", "GAUGE"),
            ("pressurememoryfull", "GAUGE"),
        ];

        for (i, (name, ds_type)) in pve9_additions.iter().enumerate() {
            assert_ds_properties(&schema.data_sources[12 + i], name, ds_type, 12 + i);
        }
    }

    #[test]
    fn test_vm_schema_pve2() {
        let schema = RrdSchema::vm(RrdFormat::Pve2);

        assert_eq!(schema.column_count(), 10);
        assert_eq!(schema.format, RrdFormat::Pve2);

        let expected_ds = vec![
            ("maxcpu", "GAUGE"),
            ("cpu", "GAUGE"),
            ("maxmem", "GAUGE"),
            ("mem", "GAUGE"),
            ("maxdisk", "GAUGE"),
            ("disk", "GAUGE"),
            ("netin", "DERIVE"),
            ("netout", "DERIVE"),
            ("diskread", "DERIVE"),
            ("diskwrite", "DERIVE"),
        ];

        for (i, (name, ds_type)) in expected_ds.iter().enumerate() {
            assert_ds_properties(&schema.data_sources[i], name, ds_type, i);
        }
    }

    #[test]
    fn test_vm_schema_pve9() {
        let schema = RrdSchema::vm(RrdFormat::Pve9_0);

        assert_eq!(schema.column_count(), 17);
        assert_eq!(schema.format, RrdFormat::Pve9_0);

        let pve2_schema = RrdSchema::vm(RrdFormat::Pve2);
        for i in 0..10 {
            assert_eq!(
                schema.data_sources[i].name, pve2_schema.data_sources[i].name,
                "First 10 DS should match pve2"
            );
            assert_eq!(
                schema.data_sources[i].ds_type, pve2_schema.data_sources[i].ds_type,
                "First 10 DS types should match pve2"
            );
        }

        let pve9_additions = vec![
            ("memhost", "GAUGE"),
            ("pressurecpusome", "GAUGE"),
            ("pressurecpufull", "GAUGE"),
            ("pressureiosome", "GAUGE"),
            ("pressureiofull", "GAUGE"),
            ("pressurememorysome", "GAUGE"),
            ("pressurememoryfull", "GAUGE"),
        ];

        for (i, (name, ds_type)) in pve9_additions.iter().enumerate() {
            assert_ds_properties(&schema.data_sources[10 + i], name, ds_type, 10 + i);
        }
    }

    #[test]
    fn test_storage_schema() {
        for format in [RrdFormat::Pve2, RrdFormat::Pve9_0] {
            let schema = RrdSchema::storage(format);

            assert_eq!(schema.column_count(), 2);
            assert_eq!(schema.format, format);

            assert_ds_properties(&schema.data_sources[0], "total", "GAUGE", 0);
            assert_ds_properties(&schema.data_sources[1], "used", "GAUGE", 1);
        }
    }

    #[test]
    fn test_rra_archives() {
        let expected_rras = [
            "RRA:AVERAGE:0.5:1:1440",
            "RRA:AVERAGE:0.5:30:1440",
            "RRA:AVERAGE:0.5:360:1440",
            "RRA:AVERAGE:0.5:10080:570",
            "RRA:MAX:0.5:1:1440",
            "RRA:MAX:0.5:30:1440",
            "RRA:MAX:0.5:360:1440",
            "RRA:MAX:0.5:10080:570",
        ];

        let schemas = vec![
            RrdSchema::node(RrdFormat::Pve2),
            RrdSchema::node(RrdFormat::Pve9_0),
            RrdSchema::vm(RrdFormat::Pve2),
            RrdSchema::vm(RrdFormat::Pve9_0),
            RrdSchema::storage(RrdFormat::Pve2),
            RrdSchema::storage(RrdFormat::Pve9_0),
        ];

        for schema in schemas {
            assert_eq!(schema.archives.len(), 8);

            for (i, expected) in expected_rras.iter().enumerate() {
                assert_eq!(
                    &schema.archives[i], expected,
                    "RRA[{}] mismatch in {:?}",
                    i, schema.format
                );
            }
        }
    }

    #[test]
    fn test_heartbeat_consistency() {
        let schemas = vec![
            RrdSchema::node(RrdFormat::Pve2),
            RrdSchema::node(RrdFormat::Pve9_0),
            RrdSchema::vm(RrdFormat::Pve2),
            RrdSchema::vm(RrdFormat::Pve9_0),
            RrdSchema::storage(RrdFormat::Pve2),
            RrdSchema::storage(RrdFormat::Pve9_0),
        ];

        for schema in schemas {
            for ds in &schema.data_sources {
                assert_eq!(ds.heartbeat, 120);
                assert_eq!(ds.min, "0");
                assert_eq!(ds.max, "U");
            }
        }
    }

    #[test]
    fn test_gauge_vs_derive_correctness() {
        // GAUGE: instantaneous values (CPU%, memory bytes)
        // DERIVE: cumulative counters that can wrap (network/disk bytes)

        let node = RrdSchema::node(RrdFormat::Pve2);
        let node_derive_indices = [10, 11]; // netin, netout
        for (i, ds) in node.data_sources.iter().enumerate() {
            if node_derive_indices.contains(&i) {
                assert_eq!(
                    ds.ds_type, "DERIVE",
                    "Node DS[{}] ({}) should be DERIVE",
                    i, ds.name
                );
            } else {
                assert_eq!(
                    ds.ds_type, "GAUGE",
                    "Node DS[{}] ({}) should be GAUGE",
                    i, ds.name
                );
            }
        }

        let vm = RrdSchema::vm(RrdFormat::Pve2);
        let vm_derive_indices = [6, 7, 8, 9]; // netin, netout, diskread, diskwrite
        for (i, ds) in vm.data_sources.iter().enumerate() {
            if vm_derive_indices.contains(&i) {
                assert_eq!(
                    ds.ds_type, "DERIVE",
                    "VM DS[{}] ({}) should be DERIVE",
                    i, ds.name
                );
            } else {
                assert_eq!(
                    ds.ds_type, "GAUGE",
                    "VM DS[{}] ({}) should be GAUGE",
                    i, ds.name
                );
            }
        }

        let storage = RrdSchema::storage(RrdFormat::Pve2);
        for ds in &storage.data_sources {
            assert_eq!(
                ds.ds_type, "GAUGE",
                "Storage DS ({}) should be GAUGE",
                ds.name
            );
        }
    }

    #[test]
    fn test_pve9_backward_compatibility() {
        let node_pve2 = RrdSchema::node(RrdFormat::Pve2);
        let node_pve9 = RrdSchema::node(RrdFormat::Pve9_0);

        assert!(node_pve9.column_count() > node_pve2.column_count());

        for i in 0..node_pve2.column_count() {
            assert_eq!(
                node_pve2.data_sources[i].name, node_pve9.data_sources[i].name,
                "Node DS[{}] name must match between pve2 and pve9.0",
                i
            );
            assert_eq!(
                node_pve2.data_sources[i].ds_type, node_pve9.data_sources[i].ds_type,
                "Node DS[{}] type must match between pve2 and pve9.0",
                i
            );
        }

        let vm_pve2 = RrdSchema::vm(RrdFormat::Pve2);
        let vm_pve9 = RrdSchema::vm(RrdFormat::Pve9_0);

        assert!(vm_pve9.column_count() > vm_pve2.column_count());

        for i in 0..vm_pve2.column_count() {
            assert_eq!(
                vm_pve2.data_sources[i].name, vm_pve9.data_sources[i].name,
                "VM DS[{}] name must match between pve2 and pve9.0",
                i
            );
            assert_eq!(
                vm_pve2.data_sources[i].ds_type, vm_pve9.data_sources[i].ds_type,
                "VM DS[{}] type must match between pve2 and pve9.0",
                i
            );
        }

        let storage_pve2 = RrdSchema::storage(RrdFormat::Pve2);
        let storage_pve9 = RrdSchema::storage(RrdFormat::Pve9_0);
        assert_eq!(storage_pve2.column_count(), storage_pve9.column_count());
    }

    #[test]
    fn test_schema_display() {
        let test_cases = vec![
            (RrdSchema::node(RrdFormat::Pve2), "Pve2", "12 data sources"),
            (
                RrdSchema::node(RrdFormat::Pve9_0),
                "Pve9_0",
                "19 data sources",
            ),
            (RrdSchema::vm(RrdFormat::Pve2), "Pve2", "10 data sources"),
            (
                RrdSchema::vm(RrdFormat::Pve9_0),
                "Pve9_0",
                "17 data sources",
            ),
            (
                RrdSchema::storage(RrdFormat::Pve2),
                "Pve2",
                "2 data sources",
            ),
        ];

        for (schema, expected_format, expected_count) in test_cases {
            let display = format!("{}", schema);
            assert!(
                display.contains(expected_format),
                "Display should contain format: {}",
                display
            );
            assert!(
                display.contains(expected_count),
                "Display should contain count: {}",
                display
            );
        }
    }
}
