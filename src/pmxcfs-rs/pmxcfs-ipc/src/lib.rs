/// libqb-compatible IPC server implementation in pure Rust
///
/// This crate implements a minimal libqb IPC server that is wire-compatible
/// with libqb clients (qb_ipcc_*), without depending on the libqb C library.
///
/// ## Protocol Overview
///
/// 1. **Connection Handshake** (SOCK_STREAM):
///    - Server listens on `/var/run/{service_name}`
///    - Client connects and sends `qb_ipc_connection_request`
///    - Server authenticates (uid/gid), creates per-connection datagram sockets
///    - Server sends `qb_ipc_connection_response` with socket paths
///
/// 2. **Request/Response** (SOCK_DGRAM):
///    - Client sends requests on datagram socket
///    - Server receives, processes, and sends responses
///
/// ## Module Structure
///
/// - `protocol` - Wire protocol structures and constants
/// - `socket` - Abstract Unix socket utilities
/// - `connection` - Per-connection handling and request processing
/// - `server` - Main IPC server and connection acceptance
///
/// References:
/// - libqb source: ~/dev/libqb/lib/ipc_socket.c, ipc_setup.c
mod connection;
mod handler;
mod protocol;
mod ringbuffer;
mod server;
mod socket;

// Public API
pub use handler::{Handler, Permissions};
pub use protocol::{Request, Response};
pub use server::Server;
