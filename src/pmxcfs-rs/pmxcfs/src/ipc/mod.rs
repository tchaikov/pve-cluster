//! IPC (Inter-Process Communication) subsystem
//!
//! This module handles libqb-compatible IPC communication between pmxcfs
//! and client applications (e.g., pvestatd, pvesh, etc.).
//!
//! The IPC subsystem consists of:
//! - Operation codes (CfsIpcOp) defining available IPC operations
//! - Request types (IpcRequest) representing parsed client requests
//! - Service handler (IpcHandler) implementing the request processing logic

mod request;
mod service;

// Re-export public types
pub use request::{CfsIpcOp, IpcRequest};
pub use service::IpcHandler;
