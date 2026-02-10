//! Simple test server for debugging libqb connectivity

use async_trait::async_trait;
use pmxcfs_ipc::{Handler, Permissions, Request, Response, Server};

/// Example handler implementation
struct TestHandler;

#[async_trait]
impl Handler for TestHandler {
    fn authenticate(&self, uid: u32, gid: u32) -> Option<Permissions> {
        // Accept root with read-write access
        if uid == 0 {
            eprintln!("Authenticated uid={uid}, gid={gid} as root (read-write)");
            return Some(Permissions::ReadWrite);
        }

        // Accept all other users with read-only access for testing
        eprintln!("Authenticated uid={uid}, gid={gid} as regular user (read-only)");
        Some(Permissions::ReadOnly)
    }

    async fn handle(&self, request: Request) -> Response {
        eprintln!(
            "Received request: id={}, data_len={}, conn={}, uid={}, gid={}, pid={}, read_only={}",
            request.msg_id,
            request.data.len(),
            request.conn_id,
            request.uid,
            request.gid,
            request.pid,
            request.is_read_only
        );

        match request.msg_id {
            1 => {
                // CFS_IPC_GET_FS_VERSION
                let response_str = r#"{"version":1,"protocol":1}"#;
                eprintln!("Responding with: {response_str}");
                Response::ok(response_str.as_bytes().to_vec())
            }
            2 => {
                // CFS_IPC_GET_CLUSTER_INFO
                let response_str = r#"{"nodes":["node1","node2"],"quorate":true}"#;
                eprintln!("Responding with: {response_str}");
                Response::ok(response_str.as_bytes().to_vec())
            }
            3 => {
                // CFS_IPC_GET_GUEST_LIST
                let response_str = r#"{"data":[{"vmid":100}]}"#;
                eprintln!("Responding with: {response_str}");
                Response::ok(response_str.as_bytes().to_vec())
            }
            _ => {
                eprintln!("Unknown message id: {}", request.msg_id);
                Response::err(-libc::EINVAL)
            }
        }
    }
}

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_target(true)
        .init();

    println!("Starting QB IPC test server on 'pve2'...");

    // Create handler and server
    let handler = TestHandler;
    let mut server = Server::new("pve2", handler);

    println!("Server created, starting...");

    if let Err(e) = server.start() {
        eprintln!("Failed to start server: {e}");
        std::process::exit(1);
    }

    println!("Server started successfully!");
    println!("Waiting for connections...");

    // Keep server running
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to wait for Ctrl-C");

    println!("Shutting down...");
}
