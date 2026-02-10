//! Error types for the service framework

use thiserror::Error;

/// Errors that can occur during service operations
#[derive(Error, Debug)]
pub enum ServiceError {
    /// Service initialization failed
    #[error("Failed to initialize service: {0}")]
    InitializationFailed(String),

    /// Service dispatch failed
    #[error("Failed to dispatch service events: {0}")]
    DispatchFailed(String),
}

pub type Result<T> = std::result::Result<T, ServiceError>;
