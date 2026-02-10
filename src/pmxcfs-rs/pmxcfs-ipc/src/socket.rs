/// Abstract Unix socket utilities
///
/// This module provides functions for working with Linux abstract Unix sockets,
/// which are used by libqb for IPC communication.
use anyhow::Result;
use std::os::unix::io::FromRawFd;
use std::os::unix::net::UnixListener;

/// Bind to an abstract Unix socket (Linux-specific)
///
/// Abstract sockets are identified by a name in the kernel's socket namespace,
/// not a filesystem path. They are automatically removed when all references are closed.
///
/// libqb clients create abstract sockets with FULL 108-byte sun_path (null-padded).
/// Linux abstract sockets are length-sensitive, so we must match exactly.
pub(super) fn bind_abstract_socket(name: &str) -> Result<UnixListener> {
    // Create a Unix socket using libc directly
    let sock_fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if sock_fd < 0 {
        anyhow::bail!(
            "Failed to create Unix socket: {}",
            std::io::Error::last_os_error()
        );
    }

    // RAII guard to ensure socket is closed on error
    struct SocketGuard(i32);
    impl Drop for SocketGuard {
        fn drop(&mut self) {
            unsafe { libc::close(self.0) };
        }
    }
    let guard = SocketGuard(sock_fd);

    // Create sockaddr_un with full 108-byte abstract address (matching libqb)
    // libqb format: sun_path[0] = '\0', sun_path[1..] = "name\0\0..." (null-padded)
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;

    // sun_path[0] is already 0 (abstract socket marker)
    // Copy name starting at sun_path[1]
    let name_bytes = name.as_bytes();
    let copy_len = name_bytes.len().min(107); // Leave room for initial \0
    unsafe {
        std::ptr::copy_nonoverlapping(
            name_bytes.as_ptr(),
            addr.sun_path.as_mut_ptr().offset(1) as *mut u8,
            copy_len,
        );
    }

    // Use FULL sockaddr_un length for libqb compatibility!
    // libqb clients use the full 110-byte structure (2 + 108) when connecting,
    // so we MUST bind with the same length. Verified via strace.
    let addr_len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
    let bind_res = unsafe {
        libc::bind(
            sock_fd,
            &addr as *const _ as *const libc::sockaddr,
            addr_len,
        )
    };
    if bind_res < 0 {
        anyhow::bail!(
            "Failed to bind abstract socket: {}",
            std::io::Error::last_os_error()
        );
    }

    // Set socket to listen mode (backlog = 128)
    let listen_res = unsafe { libc::listen(sock_fd, 128) };
    if listen_res < 0 {
        anyhow::bail!(
            "Failed to listen on socket: {}",
            std::io::Error::last_os_error()
        );
    }

    // Convert raw fd to UnixListener (takes ownership, forget guard)
    std::mem::forget(guard);
    let listener = unsafe { UnixListener::from_raw_fd(sock_fd) };

    Ok(listener)
}
