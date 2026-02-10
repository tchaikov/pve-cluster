//! Service framework for pmxcfs
//!
//! This crate provides a robust, tokio-based service management framework with:
//! - Automatic retry on failure
//! - Event-driven file descriptor monitoring
//! - Periodic timer callbacks
//! - Error tracking and throttled logging
//! - Graceful shutdown

mod error;
mod manager;
mod service;

pub use error::{Result, ServiceError};
pub use manager::ServiceManager;
pub use service::{DispatchAction, InitResult, Service};
