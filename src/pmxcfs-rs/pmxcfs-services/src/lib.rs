//! Service framework for pmxcfs
//!
//! This crate provides a simplified, tokio-based service management framework with:
//! - Automatic retry on failure (5 second interval)
//! - Event-driven file descriptor monitoring
//! - Optional periodic timer callbacks
//! - Graceful shutdown

mod error;
mod manager;
mod service;

pub use error::{Result, ServiceError};
pub use manager::ServiceManager;
pub use service::Service;
