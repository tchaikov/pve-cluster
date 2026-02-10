/// Main libqb IPC server implementation
///
/// This module contains the Server struct and its implementation,
/// including connection acceptance and server lifecycle management.
use anyhow::{Context, Result};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::net::UnixListener;
use tokio_util::sync::CancellationToken;

use super::connection::QbConnection;
use super::handler::Handler;
use super::socket::bind_abstract_socket;

/// Server-level connection statistics (matches libqb qb_ipcs_stats)
#[derive(Debug, Default)]
pub struct ServerStats {
    /// Number of currently active connections
    pub active_connections: AtomicUsize,
    /// Total number of closed connections since server start
    pub closed_connections: AtomicUsize,
}

impl ServerStats {
    fn new() -> Self {
        Self {
            active_connections: AtomicUsize::new(0),
            closed_connections: AtomicUsize::new(0),
        }
    }

    /// Increment active connections count (new connection established)
    fn connection_created(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(
            active = self.active_connections.load(Ordering::Relaxed),
            closed = self.closed_connections.load(Ordering::Relaxed),
            "Connection created"
        );
    }

    /// Decrement active, increment closed (connection terminated)
    fn connection_closed(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
        self.closed_connections.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(
            active = self.active_connections.load(Ordering::Relaxed),
            closed = self.closed_connections.load(Ordering::Relaxed),
            "Connection closed"
        );
    }

    /// Get current statistics (for monitoring/debugging)
    pub fn get(&self) -> (usize, usize) {
        (
            self.active_connections.load(Ordering::Relaxed),
            self.closed_connections.load(Ordering::Relaxed),
        )
    }
}

/// libqb-compatible IPC server
pub struct Server {
    service_name: String,

    // Setup socket (SOCK_STREAM) - accepts new connections
    setup_listener: Option<Arc<UnixListener>>,

    // Per-connection state
    connections: Arc<Mutex<HashMap<u64, QbConnection>>>,
    next_conn_id: Arc<AtomicU64>,

    // Connection statistics (matches libqb behavior)
    stats: Arc<ServerStats>,

    // Message handler (trait object, also handles authentication)
    handler: Arc<dyn Handler>,

    // Cancellation token for graceful shutdown
    cancellation_token: CancellationToken,
}

impl Server {
    /// Create a new libqb-compatible IPC server
    ///
    /// Uses Linux abstract Unix sockets for IPC (no filesystem paths needed).
    ///
    /// # Arguments
    /// * `service_name` - Service name (e.g., "pve2"), used as abstract socket name
    /// * `handler` - Handler implementing the Handler trait (handles both authentication and requests)
    pub fn new(service_name: &str, handler: impl Handler + 'static) -> Self {
        Self {
            service_name: service_name.to_string(),
            setup_listener: None,
            connections: Arc::new(Mutex::new(HashMap::new())),
            next_conn_id: Arc::new(AtomicU64::new(1)),
            stats: Arc::new(ServerStats::new()),
            handler: Arc::new(handler),
            cancellation_token: CancellationToken::new(),
        }
    }

    /// Start the IPC server
    ///
    /// Creates abstract Unix socket that libqb clients can connect to
    pub fn start(&mut self) -> Result<()> {
        tracing::info!(
            "Starting libqb-compatible IPC server: {}",
            self.service_name
        );

        // Create abstract Unix socket (no filesystem paths needed)
        let std_listener =
            bind_abstract_socket(&self.service_name).context("Failed to bind abstract socket")?;

        // Convert to tokio listener
        std_listener.set_nonblocking(true)?;
        let listener = UnixListener::from_std(std_listener)?;

        tracing::info!("Bound abstract Unix socket: @{}", self.service_name);

        let listener_arc = Arc::new(listener);
        self.setup_listener = Some(listener_arc.clone());

        // Start connection acceptor task
        let context = AcceptorContext {
            listener: listener_arc,
            service_name: self.service_name.clone(),
            connections: self.connections.clone(),
            next_conn_id: self.next_conn_id.clone(),
            stats: self.stats.clone(),
            handler: self.handler.clone(),
            cancellation_token: self.cancellation_token.child_token(),
        };

        tokio::spawn(async move {
            context.run().await;
        });

        tracing::info!("libqb IPC server started: {}", self.service_name);
        Ok(())
    }

    /// Stop the IPC server
    pub fn stop(&mut self) {
        tracing::info!("Stopping libqb IPC server: {}", self.service_name);

        // Signal all tasks to stop
        self.cancellation_token.cancel();

        // Close all connections
        let connections = std::mem::take(&mut *self.connections.lock());
        let num_connections = connections.len();

        for (_id, conn) in connections {
            // Clean up ring buffer files
            for rb_path in &conn.ring_buffer_paths {
                if let Err(e) = std::fs::remove_file(rb_path) {
                    tracing::debug!(
                        "Failed to remove ring buffer file {} (may already be cleaned up): {}",
                        rb_path.display(),
                        e
                    );
                }
            }

            // Update statistics
            self.stats.connection_closed();

            // Task handles will be aborted when dropped
        }

        // Final stats
        if num_connections > 0 {
            let (active, closed) = self.stats.get();
            tracing::info!(
                "Closed {} connections (final stats: active={}, closed={})",
                num_connections,
                active,
                closed
            );
        }

        self.setup_listener = None;

        tracing::info!("libqb IPC server stopped");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Context for the connection acceptor task
///
/// Bundles all the state needed by the acceptor loop to avoid passing many parameters.
struct AcceptorContext {
    listener: Arc<UnixListener>,
    service_name: String,
    connections: Arc<Mutex<HashMap<u64, QbConnection>>>,
    next_conn_id: Arc<AtomicU64>,
    stats: Arc<ServerStats>,
    handler: Arc<dyn Handler>,
    cancellation_token: CancellationToken,
}

impl AcceptorContext {
    /// Run the connection acceptor loop
    ///
    /// Accepts new connections and spawns handler tasks for each.
    async fn run(self) {
        tracing::debug!("libqb IPC connection acceptor started");

        loop {
            // Accept new connection with cancellation support
            let accept_result = tokio::select! {
                _ = self.cancellation_token.cancelled() => {
                    tracing::debug!("Connection acceptor cancelled");
                    break;
                }
                result = self.listener.accept() => result,
            };

            let (stream, _addr) = match accept_result {
                Ok((stream, addr)) => (stream, addr),
                Err(e) => {
                    if !self.cancellation_token.is_cancelled() {
                        tracing::error!("Error accepting connection: {}", e);
                    }
                    break;
                }
            };

            tracing::debug!("Accepted new setup connection");

            // Handle connection
            let conn_id = self.next_conn_id.fetch_add(1, Ordering::SeqCst);
            match QbConnection::accept(
                stream,
                conn_id,
                &self.service_name,
                self.handler.clone(),
                self.cancellation_token.child_token(),
            )
            .await
            {
                Ok(conn) => {
                    self.connections.lock().insert(conn_id, conn);
                    // Update statistics
                    self.stats.connection_created();
                }
                Err(e) => {
                    tracing::error!("Failed to accept connection {}: {}", conn_id, e);
                }
            }
        }

        tracing::debug!("libqb IPC connection acceptor finished");
    }
}

#[cfg(test)]
mod tests {
    use crate::protocol::*;

    #[test]
    fn test_header_sizes() {
        // Verify C struct compatibility
        assert_eq!(std::mem::size_of::<RequestHeader>(), 16);
        assert_eq!(std::mem::align_of::<RequestHeader>(), 8);
        assert_eq!(std::mem::size_of::<ResponseHeader>(), 24);
        assert_eq!(std::mem::align_of::<ResponseHeader>(), 8);
    }
}
