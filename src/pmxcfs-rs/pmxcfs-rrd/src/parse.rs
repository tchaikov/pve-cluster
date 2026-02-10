/// RRD Update Data Parsing
///
/// Shared parsing logic to ensure consistent behavior across all backends.
use anyhow::{Context, Result};

/// Parsed RRD update data
#[derive(Debug, Clone)]
pub struct UpdateData {
    /// Timestamp (None for "N" = now)
    pub timestamp: Option<i64>,
    /// Values to update (NaN for "U" = unknown)
    pub values: Vec<f64>,
}

impl UpdateData {
    /// Parse RRD update data string
    ///
    /// Format: "timestamp:value1:value2:..."
    /// - timestamp: Unix timestamp or "N" for current time
    /// - values: Numeric values or "U" for unknown
    ///
    /// # Error Handling
    /// Both daemon and direct backends use the same parsing logic:
    /// - Invalid timestamps fail immediately
    /// - Invalid values (non-numeric, non-"U") fail immediately
    /// - This ensures consistent behavior regardless of backend
    pub fn parse(data: &str) -> Result<Self> {
        let parts: Vec<&str> = data.split(':').collect();
        if parts.len() < 2 {
            anyhow::bail!("Invalid update data format: {data}");
        }

        // Parse timestamp
        let timestamp = if parts[0] == "N" {
            None
        } else {
            Some(
                parts[0]
                    .parse::<i64>()
                    .with_context(|| format!("Invalid timestamp: {}", parts[0]))?,
            )
        };

        // Parse values
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

        Ok(Self { timestamp, values })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_data() {
        let data = "1234567890:100.5:200.0:300.0";
        let result = UpdateData::parse(data).unwrap();

        assert_eq!(result.timestamp, Some(1234567890));
        assert_eq!(result.values.len(), 3);
        assert_eq!(result.values[0], 100.5);
        assert_eq!(result.values[1], 200.0);
        assert_eq!(result.values[2], 300.0);
    }

    #[test]
    fn test_parse_with_n_timestamp() {
        let data = "N:100:200";
        let result = UpdateData::parse(data).unwrap();

        assert_eq!(result.timestamp, None);
        assert_eq!(result.values.len(), 2);
    }

    #[test]
    fn test_parse_with_unknown_values() {
        let data = "1234567890:100:U:300";
        let result = UpdateData::parse(data).unwrap();

        assert_eq!(result.values.len(), 3);
        assert_eq!(result.values[0], 100.0);
        assert!(result.values[1].is_nan());
        assert_eq!(result.values[2], 300.0);
    }

    #[test]
    fn test_parse_invalid_timestamp() {
        let data = "invalid:100:200";
        let result = UpdateData::parse(data);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_invalid_value() {
        let data = "1234567890:100:invalid:300";
        let result = UpdateData::parse(data);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_data() {
        let data = "";
        let result = UpdateData::parse(data);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_no_values() {
        let data = "1234567890";
        let result = UpdateData::parse(data);
        assert!(result.is_err());
    }
}
