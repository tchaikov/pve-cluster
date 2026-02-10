use super::errors::RRDCachedClientError;

pub fn check_data_source_name(name: &str) -> Result<(), RRDCachedClientError> {
    if name.is_empty() || name.len() > 64 {
        return Err(RRDCachedClientError::InvalidDataSourceName(
            "name must be between 1 and 64 characters".to_string(),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err(RRDCachedClientError::InvalidDataSourceName(
            "name must only contain alphanumeric characters and underscores".to_string(),
        ));
    }
    Ok(())
}

pub fn check_rrd_path(name: &str) -> Result<(), RRDCachedClientError> {
    if name.is_empty() || name.len() > 64 {
        return Err(RRDCachedClientError::InvalidCreateDataSerie(
            "name must be between 1 and 64 characters".to_string(),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err(RRDCachedClientError::InvalidCreateDataSerie(
            "name must only contain alphanumeric characters and underscores".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_data_source_name() {
        let result = check_data_source_name("test");
        assert!(result.is_ok());

        let result = check_data_source_name("test_");
        assert!(result.is_ok());

        let result = check_data_source_name("test-");
        assert!(result.is_ok());

        let result = check_data_source_name("test_1_a");
        assert!(result.is_ok());

        let result = check_data_source_name("");
        assert!(result.is_err());

        let result = check_data_source_name("a".repeat(65).as_str());
        assert!(result.is_err());

        let result = check_data_source_name("test!");
        assert!(result.is_err());

        let result = check_data_source_name("test\n");
        assert!(result.is_err());

        let result = check_data_source_name("test:GAUGE");
        assert!(result.is_err());
    }

    #[test]
    fn test_check_rrd_path() {
        let result = check_rrd_path("test");
        assert!(result.is_ok());

        let result = check_rrd_path("test_");
        assert!(result.is_ok());

        let result = check_rrd_path("test-");
        assert!(result.is_ok());

        let result = check_rrd_path("test_1_a");
        assert!(result.is_ok());

        let result = check_rrd_path("");
        assert!(result.is_err());

        let result = check_rrd_path("a".repeat(65).as_str());
        assert!(result.is_err());

        let result = check_rrd_path("test!");
        assert!(result.is_err());

        let result = check_rrd_path("test\n");
        assert!(result.is_err());

        let result = check_rrd_path("test.rrd");
        assert!(result.is_err());
    }
}
