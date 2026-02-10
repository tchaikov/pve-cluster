//! Vendored and trimmed rrdcached client implementation
//!
//! This module contains a trimmed version of the rrdcached-client crate (v0.1.5),
//! containing only the functionality we actually use.
//!
//! ## Why vendor and trim?
//!
//! - Gain full control over the implementation
//! - Remove unused code and dependencies
//! - Simplify our dependency tree
//! - Avoid external dependency churn for critical infrastructure
//! - No dead code warnings
//!
//! ## What we kept
//!
//! - `connect_unix()` - Connect to rrdcached via Unix socket
//! - `create()` - Create new RRD files
//! - `update()` - Update RRD data
//! - `flush_all()` - Flush pending updates
//! - Supporting types: `CreateArguments`, `CreateDataSource`, `ConsolidationFunction`, etc.
//!
//! ## What we removed
//!
//! - TCP connection support (`connect_tcp`)
//! - Fetch/read operations (we only write RRD data)
//! - Batch update operations (we use individual updates)
//! - Administrative operations (ping, queue, stats, suspend, resume, etc.)
//! - All test code
//!
//! ## Original source
//!
//! - Repository: https://github.com/SINTEF/rrdcached-client
//! - Version: 0.1.5
//! - License: Apache-2.0
//! - Copyright: SINTEF

pub mod client;
pub mod consolidation_function;
pub mod create;
pub mod errors;
pub mod now;
pub mod parsers;
pub mod sanitisation;

pub use client::RRDCachedClient;
