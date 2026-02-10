use super::errors::RRDCachedClientError;

pub fn now_timestamp() -> Result<usize, RRDCachedClientError> {
    let now = std::time::SystemTime::now();
    now.duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| RRDCachedClientError::SystemTimeError)
        .map(|d| d.as_secs() as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_now_timestamp() {
        assert!(now_timestamp().is_ok());
    }
}
