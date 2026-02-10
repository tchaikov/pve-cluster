use super::{
    consolidation_function::ConsolidationFunction,
    errors::RRDCachedClientError,
    sanitisation::{check_data_source_name, check_rrd_path},
};

/// RRD data source types
///
/// Only the types we actually use are included.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CreateDataSourceType {
    /// Values are stored as-is
    Gauge,
    /// Rate of change, counter wraps handled
    Counter,
    /// Rate of change, can increase or decrease
    Derive,
    /// Reset to value, then set to 0
    Absolute,
}

impl CreateDataSourceType {
    pub fn to_str(self) -> &'static str {
        match self {
            CreateDataSourceType::Gauge => "GAUGE",
            CreateDataSourceType::Counter => "COUNTER",
            CreateDataSourceType::Derive => "DERIVE",
            CreateDataSourceType::Absolute => "ABSOLUTE",
        }
    }
}

/// Arguments for a data source (DS).
#[derive(Debug)]
pub struct CreateDataSource {
    /// Name of the data source.
    /// Must be between 1 and 64 characters and only contain alphanumeric characters and underscores
    /// and dashes.
    pub name: String,

    /// Minimum value
    pub minimum: Option<f64>,

    /// Maximum value
    pub maximum: Option<f64>,

    /// Heartbeat, if no data is received for this amount of time,
    /// the value is unknown.
    pub heartbeat: i64,

    /// Type of the data source
    pub serie_type: CreateDataSourceType,
}

impl CreateDataSource {
    /// Check that the content is valid.
    pub fn validate(&self) -> Result<(), RRDCachedClientError> {
        if self.heartbeat <= 0 {
            return Err(RRDCachedClientError::InvalidCreateDataSerie(
                "heartbeat must be greater than 0".to_string(),
            ));
        }
        if let Some(minimum) = self.minimum
            && let Some(maximum) = self.maximum
                && maximum <= minimum {
                    return Err(RRDCachedClientError::InvalidCreateDataSerie(
                        "maximum must be greater than to minimum".to_string(),
                    ));
                }

        check_data_source_name(&self.name)?;

        Ok(())
    }

    /// Convert to a string argument parameter.
    pub fn to_str(&self) -> String {
        format!(
            "DS:{}:{}:{}:{}:{}",
            self.name,
            self.serie_type.to_str(),
            self.heartbeat,
            match self.minimum {
                Some(minimum) => minimum.to_string(),
                None => "U".to_string(),
            },
            match self.maximum {
                Some(maximum) => maximum.to_string(),
                None => "U".to_string(),
            }
        )
    }
}

/// Arguments for a round robin archive (RRA).
#[derive(Debug)]
pub struct CreateRoundRobinArchive {
    /// Archive types are AVERAGE, MIN, MAX, LAST.
    pub consolidation_function: ConsolidationFunction,

    /// Number between 0 and 1 to accept unknown data
    /// 0.5 means that if more of 50% of the data points are unknown,
    /// the value is unknown.
    pub xfiles_factor: f64,

    /// Number of steps that are used to calculate the value
    pub steps: i64,

    /// Number of rows in the archive
    pub rows: i64,
}

impl CreateRoundRobinArchive {
    /// Check that the content is valid.
    pub fn validate(&self) -> Result<(), RRDCachedClientError> {
        if self.xfiles_factor < 0.0 || self.xfiles_factor > 1.0 {
            return Err(RRDCachedClientError::InvalidCreateDataSerie(
                "xfiles_factor must be between 0 and 1".to_string(),
            ));
        }
        if self.steps <= 0 {
            return Err(RRDCachedClientError::InvalidCreateDataSerie(
                "steps must be greater than 0".to_string(),
            ));
        }
        if self.rows <= 0 {
            return Err(RRDCachedClientError::InvalidCreateDataSerie(
                "rows must be greater than 0".to_string(),
            ));
        }
        Ok(())
    }

    /// Convert to a string argument parameter.
    pub fn to_str(&self) -> String {
        format!(
            "RRA:{}:{}:{}:{}",
            self.consolidation_function.to_str(),
            self.xfiles_factor,
            self.steps,
            self.rows
        )
    }
}

/// Arguments to create a new RRD file
#[derive(Debug)]
pub struct CreateArguments {
    /// Path to the RRD file
    /// The path must be between 1 and 64 characters and only contain alphanumeric characters and underscores
    ///
    /// Does **not** end with .rrd
    pub path: String,

    /// List of data sources, the order is important
    /// Must be at least one.
    pub data_sources: Vec<CreateDataSource>,

    /// List of round robin archives.
    /// Must be at least one.
    pub round_robin_archives: Vec<CreateRoundRobinArchive>,

    /// Start time of the first data point
    pub start_timestamp: u64,

    /// Number of seconds between two data points
    pub step_seconds: u64,
}

impl CreateArguments {
    /// Check that the content is valid.
    pub fn validate(&self) -> Result<(), RRDCachedClientError> {
        if self.data_sources.is_empty() {
            return Err(RRDCachedClientError::InvalidCreateDataSerie(
                "at least one data serie is required".to_string(),
            ));
        }
        if self.round_robin_archives.is_empty() {
            return Err(RRDCachedClientError::InvalidCreateDataSerie(
                "at least one round robin archive is required".to_string(),
            ));
        }
        for data_serie in &self.data_sources {
            data_serie.validate()?;
        }
        for rr_archive in &self.round_robin_archives {
            rr_archive.validate()?;
        }
        check_rrd_path(&self.path)?;
        Ok(())
    }

    /// Convert to a string argument parameter.
    pub fn to_str(&self) -> String {
        let mut result = format!(
            "{}.rrd -s {} -b {}",
            self.path, self.step_seconds, self.start_timestamp
        );
        for data_serie in &self.data_sources {
            result.push(' ');
            result.push_str(&data_serie.to_str());
        }
        for rr_archive in &self.round_robin_archives {
            result.push(' ');
            result.push_str(&rr_archive.to_str());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test for CreateDataSourceType to_str method
    #[test]
    fn test_create_data_source_type_to_str() {
        assert_eq!(CreateDataSourceType::Gauge.to_str(), "GAUGE");
        assert_eq!(CreateDataSourceType::Counter.to_str(), "COUNTER");
        assert_eq!(CreateDataSourceType::Derive.to_str(), "DERIVE");
        assert_eq!(CreateDataSourceType::Absolute.to_str(), "ABSOLUTE");
    }

    // Test for CreateDataSource validate method
    #[test]
    fn test_create_data_source_validate() {
        let valid_ds = CreateDataSource {
            name: "valid_name_1".to_string(),
            minimum: Some(0.0),
            maximum: Some(100.0),
            heartbeat: 300,
            serie_type: CreateDataSourceType::Gauge,
        };
        assert!(valid_ds.validate().is_ok());

        let invalid_ds_name = CreateDataSource {
            name: "Invalid Name!".to_string(), // Invalid due to space and exclamation
            ..valid_ds
        };
        assert!(invalid_ds_name.validate().is_err());

        let invalid_ds_heartbeat = CreateDataSource {
            heartbeat: -1, // Invalid heartbeat
            name: "valid_name_2".to_string(),
            ..valid_ds
        };
        assert!(invalid_ds_heartbeat.validate().is_err());

        let invalid_ds_min_max = CreateDataSource {
            minimum: Some(100.0),
            maximum: Some(50.0), // Invalid minimum and maximum
            name: "valid_name_3".to_string(),
            ..valid_ds
        };
        assert!(invalid_ds_min_max.validate().is_err());

        // Maximum below minimum
        let invalid_ds_max = CreateDataSource {
            minimum: Some(100.0),
            maximum: Some(0.0),
            name: "valid_name_5".to_string(),
            ..valid_ds
        };
        assert!(invalid_ds_max.validate().is_err());

        // Maximum but no minimum
        let valid_ds_max = CreateDataSource {
            maximum: Some(100.0),
            name: "valid_name_6".to_string(),
            ..valid_ds
        };
        assert!(valid_ds_max.validate().is_ok());

        // Minimum but no maximum
        let valid_ds_min = CreateDataSource {
            minimum: Some(-100.0),
            name: "valid_name_7".to_string(),
            ..valid_ds
        };
        assert!(valid_ds_min.validate().is_ok());
    }

    // Test for CreateDataSource to_str method
    #[test]
    fn test_create_data_source_to_str() {
        let ds = CreateDataSource {
            name: "test_ds".to_string(),
            minimum: Some(10.0),
            maximum: Some(100.0),
            heartbeat: 600,
            serie_type: CreateDataSourceType::Gauge,
        };
        assert_eq!(ds.to_str(), "DS:test_ds:GAUGE:600:10:100");

        let ds = CreateDataSource {
            name: "test_ds".to_string(),
            minimum: None,
            maximum: None,
            heartbeat: 600,
            serie_type: CreateDataSourceType::Gauge,
        };
        assert_eq!(ds.to_str(), "DS:test_ds:GAUGE:600:U:U");
    }

    // Test for CreateRoundRobinArchive validate method
    #[test]
    fn test_create_round_robin_archive_validate() {
        let valid_rra = CreateRoundRobinArchive {
            consolidation_function: ConsolidationFunction::Average,
            xfiles_factor: 0.5,
            steps: 1,
            rows: 100,
        };
        assert!(valid_rra.validate().is_ok());

        let invalid_rra_xff = CreateRoundRobinArchive {
            xfiles_factor: -0.1, // Invalid xfiles_factor
            ..valid_rra
        };
        assert!(invalid_rra_xff.validate().is_err());

        let invalid_rra_steps = CreateRoundRobinArchive {
            steps: 0, // Invalid steps
            ..valid_rra
        };
        assert!(invalid_rra_steps.validate().is_err());

        let invalid_rra_rows = CreateRoundRobinArchive {
            rows: -100, // Invalid rows
            ..valid_rra
        };
        assert!(invalid_rra_rows.validate().is_err());
    }

    // Test for CreateRoundRobinArchive to_str method
    #[test]
    fn test_create_round_robin_archive_to_str() {
        let rra = CreateRoundRobinArchive {
            consolidation_function: ConsolidationFunction::Max,
            xfiles_factor: 0.5,
            steps: 1,
            rows: 100,
        };
        assert_eq!(rra.to_str(), "RRA:MAX:0.5:1:100");
    }

    // Test for CreateArguments validate method
    #[test]
    fn test_create_arguments_validate() {
        let valid_args = CreateArguments {
            path: "valid_path".to_string(),
            data_sources: vec![CreateDataSource {
                name: "ds1".to_string(),
                minimum: Some(0.0),
                maximum: Some(100.0),
                heartbeat: 300,
                serie_type: CreateDataSourceType::Gauge,
            }],
            round_robin_archives: vec![CreateRoundRobinArchive {
                consolidation_function: ConsolidationFunction::Average,
                xfiles_factor: 0.5,
                steps: 1,
                rows: 100,
            }],
            start_timestamp: 1609459200,
            step_seconds: 300,
        };
        assert!(valid_args.validate().is_ok());

        let invalid_args_no_ds = CreateArguments {
            data_sources: vec![],
            path: "valid_path".to_string(),
            ..valid_args
        };
        assert!(invalid_args_no_ds.validate().is_err());

        let invalid_args_no_rra = CreateArguments {
            round_robin_archives: vec![],
            path: "valid_path".to_string(),
            ..valid_args
        };
        assert!(invalid_args_no_rra.validate().is_err());
    }

    // Test for CreateArguments to_str method
    #[test]
    fn test_create_arguments_to_str() {
        let args = CreateArguments {
            path: "test_path".to_string(),
            data_sources: vec![CreateDataSource {
                name: "ds1".to_string(),
                minimum: Some(0.0),
                maximum: Some(100.0),
                heartbeat: 300,
                serie_type: CreateDataSourceType::Gauge,
            }],
            round_robin_archives: vec![CreateRoundRobinArchive {
                consolidation_function: ConsolidationFunction::Average,
                xfiles_factor: 0.5,
                steps: 1,
                rows: 100,
            }],
            start_timestamp: 1609459200,
            step_seconds: 300,
        };
        let expected_str =
            "test_path.rrd -s 300 -b 1609459200 DS:ds1:GAUGE:300:0:100 RRA:AVERAGE:0.5:1:100";
        assert_eq!(args.to_str(), expected_str);
    }
}
