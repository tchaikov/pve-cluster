//! Handler trait for processing IPC requests
//!
//! This module defines the core `Handler` trait that users implement to process
//! IPC requests. The trait-based approach provides a more idiomatic and extensible
//! API compared to raw function closures.

use crate::protocol::{Request, Response};
use async_trait::async_trait;

/// Permissions for IPC connections
///
/// Determines the access level for authenticated connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permissions {
    /// Read-only access
    ReadOnly,
    /// Read-write access
    ReadWrite,
}

/// Handler trait for processing IPC requests and authentication
///
/// Implement this trait to define custom request handling logic and authentication
/// policy for your IPC server. The handler receives a `Request` containing the
/// message ID, payload data, and connection context, and returns a `Response` with
/// an error code and response data.
///
/// ## Authentication
///
/// The `authenticate` method is called during connection setup to determine whether
/// a client with given credentials should be accepted. This allows the handler to
/// implement application-specific authentication policies.
///
/// ## Async Support
///
/// The `handle` method is async, allowing you to perform I/O operations, database
/// queries, or other async work within your handler.
///
/// ## Thread Safety
///
/// Handlers must be `Send + Sync` as they may be called from multiple tokio tasks
/// concurrently. Use `Arc<Mutex<T>>` or other synchronization primitives if you need
/// mutable shared state.
///
/// ## Error Handling
///
/// Return negative errno values in `Response::error_code` to indicate errors.
/// Use 0 for success. See `libc::*` constants for standard errno values.
#[async_trait]
pub trait Handler: Send + Sync {
    /// Authenticate a connecting client and determine access level
    ///
    /// Called during connection setup to determine whether to accept the connection
    /// and what access level to grant.
    ///
    /// # Arguments
    ///
    /// * `uid` - Client user ID (from SO_PEERCRED)
    /// * `gid` - Client group ID (from SO_PEERCRED)
    ///
    /// # Returns
    ///
    /// - `Some(Permissions::ReadWrite)` to accept with read-write access
    /// - `Some(Permissions::ReadOnly)` to accept with read-only access
    /// - `None` to reject the connection
    fn authenticate(&self, uid: u32, gid: u32) -> Option<Permissions>;

    /// Handle an IPC request
    ///
    /// # Arguments
    ///
    /// * `request` - The incoming request with message ID, data, and connection context
    ///
    /// # Returns
    ///
    /// A `Response` containing the error code (0 = success, negative = errno) and
    /// optional response data to send back to the client.
    async fn handle(&self, request: Request) -> Response;
}

/// Blanket implementation for Arc<T> where T: Handler
///
/// This allows passing `Arc<MyHandler>` directly to `Server::new()`.
#[async_trait]
impl<T: Handler> Handler for std::sync::Arc<T> {
    fn authenticate(&self, uid: u32, gid: u32) -> Option<Permissions> {
        (**self).authenticate(uid, gid)
    }

    async fn handle(&self, request: Request) -> Response {
        (**self).handle(request).await
    }
}
