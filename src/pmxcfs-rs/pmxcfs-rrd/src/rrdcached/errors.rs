use thiserror::Error;

/// Errors that can occur when interacting with rrdcached
#[derive(Error, Debug)]
pub enum RRDCachedClientError {
    /// I/O error communicating with rrdcached
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Error parsing rrdcached response
    #[error("parsing error: {0}")]
    Parsing(String),

    /// Unexpected response from rrdcached (code, message)
    #[error("unexpected response {0}: {1}")]
    UnexpectedResponse(i64, String),

    /// Invalid parameters for CREATE command
    #[error("Invalid create data serie: {0}")]
    InvalidCreateDataSerie(String),

    /// Invalid data source name
    #[error("Invalid data source name: {0}")]
    InvalidDataSourceName(String),

    /// Unable to get system time
    #[error("Unable to get system time")]
    SystemTimeError,
}
