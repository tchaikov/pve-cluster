//! Daemon builder with integrated PID file management
//!
//! This module provides a builder-based API for daemonization that combines
//! process forking, parent-child signaling, and PID file management into a
//! cohesive, easy-to-use abstraction.
//!
//! Inspired by the daemonize crate but tailored for pmxcfs needs with async support.

use anyhow::{Context, Result};
use nix::unistd::{ForkResult, fork, pipe};
use pmxcfs_api_types::PmxcfsError;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::PathBuf;

/// RAII guard for PID file - automatically removes file on drop
pub struct PidFileGuard {
    path: PathBuf,
}

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_file(&self.path) {
            tracing::warn!(
                "Failed to remove PID file at {}: {}",
                self.path.display(),
                e
            );
        } else {
            tracing::debug!("Removed PID file at {}", self.path.display());
        }
    }
}

/// Represents the daemon process after daemonization
pub enum DaemonProcess {
    /// Parent process - should exit after receiving this
    Parent,
    /// Child process - contains RAII guard for PID file cleanup
    Child(PidFileGuard),
}

/// Builder for daemon configuration with integrated PID file management
///
/// Provides a fluent API for configuring daemonization behavior including
/// PID file location, group ownership, and parent-child signaling.
pub struct Daemon {
    pid_file: Option<PathBuf>,
    group: Option<u32>,
}

impl Daemon {
    /// Create a new daemon builder with default settings
    pub fn new() -> Self {
        Self {
            pid_file: None,
            group: None,
        }
    }

    /// Set the PID file path
    ///
    /// The PID file will be created with 0o644 permissions and owned by root:group.
    pub fn pid_file<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.pid_file = Some(path.into());
        self
    }

    /// Set the group ID for PID file ownership
    pub fn group(mut self, gid: u32) -> Self {
        self.group = Some(gid);
        self
    }

    /// Start the daemonization process (foreground mode)
    ///
    /// Returns a guard that manages PID file lifecycle.
    /// The PID file is written immediately and cleaned up when the guard is dropped.
    pub fn start_foreground(self) -> Result<PidFileGuard> {
        let pid_file_path = self
            .pid_file
            .ok_or_else(|| PmxcfsError::System("PID file path must be specified".into()))?;

        let gid = self.group.unwrap_or(0);

        // Write PID file with current process ID
        write_pid_file(&pid_file_path, std::process::id(), gid)?;

        tracing::info!("Running in foreground mode with PID {}", std::process::id());

        Ok(PidFileGuard {
            path: pid_file_path,
        })
    }

    /// Start the daemonization process (daemon mode)
    ///
    /// Forks the process and returns either:
    /// - `DaemonProcess::Parent` - The parent should exit after cleanup
    /// - `DaemonProcess::Child(guard)` - The child should continue with daemon operations
    ///
    /// This uses a pipe-based signaling mechanism where the parent waits for the
    /// child to signal readiness before writing the PID file and exiting.
    pub fn start_daemon(self) -> Result<DaemonProcess> {
        let pid_file_path = self
            .pid_file
            .ok_or_else(|| PmxcfsError::System("PID file path must be specified".into()))?;

        let gid = self.group.unwrap_or(0);

        // Create pipe for parent-child signaling
        let (read_fd, write_fd) = pipe().context("Failed to create pipe for daemonization")?;

        match unsafe { fork() } {
            Ok(ForkResult::Parent { child }) => {
                // Parent: wait for child to signal readiness
                unsafe { libc::close(write_fd) };

                let mut buffer = [0u8; 1];
                let bytes_read =
                    unsafe { libc::read(read_fd, buffer.as_mut_ptr() as *mut libc::c_void, 1) };
                let errno = std::io::Error::last_os_error();
                unsafe { libc::close(read_fd) };

                if bytes_read == -1 {
                    return Err(
                        PmxcfsError::System(format!("Failed to read from child: {errno}")).into(),
                    );
                } else if bytes_read != 1 || buffer[0] != b'1' {
                    return Err(
                        PmxcfsError::System("Child failed to send ready signal".into()).into(),
                    );
                }

                // Child is ready - write PID file with child's PID
                let child_pid = child.as_raw() as u32;
                write_pid_file(&pid_file_path, child_pid, gid)?;

                tracing::info!("Child process {} signaled ready, parent exiting", child_pid);

                Ok(DaemonProcess::Parent)
            }
            Ok(ForkResult::Child) => {
                // Child: become daemon and return signal handle
                unsafe { libc::close(read_fd) };

                // Create new session
                unsafe {
                    if libc::setsid() == -1 {
                        return Err(
                            PmxcfsError::System("Failed to create new session".into()).into()
                        );
                    }
                }

                // Change to root directory
                std::env::set_current_dir("/")?;

                // Redirect standard streams to /dev/null
                let devnull = File::open("/dev/null")?;
                unsafe {
                    libc::dup2(devnull.as_raw_fd(), 0);
                    libc::dup2(devnull.as_raw_fd(), 1);
                    libc::dup2(devnull.as_raw_fd(), 2);
                }

                // Return child variant - we don't use the write_fd in this simplified version
                // Note: This method is not actually used - use start_daemon_with_signal instead
                unsafe { libc::close(write_fd) };
                Ok(DaemonProcess::Child(PidFileGuard {
                    path: pid_file_path,
                }))
            }
            Err(e) => Err(PmxcfsError::System(format!("Failed to fork: {e}")).into()),
        }
    }

    /// Start daemonization with deferred signaling
    ///
    /// Returns (DaemonProcess, Option<SignalHandle>) where SignalHandle
    /// must be used to signal the parent when ready.
    pub fn start_daemon_with_signal(self) -> Result<(DaemonProcess, Option<SignalHandle>)> {
        let pid_file_path = self
            .pid_file
            .clone()
            .ok_or_else(|| PmxcfsError::System("PID file path must be specified".into()))?;

        let gid = self.group.unwrap_or(0);

        // Create pipe for parent-child signaling
        let (read_fd, write_fd) = pipe().context("Failed to create pipe for daemonization")?;

        match unsafe { fork() } {
            Ok(ForkResult::Parent { child }) => {
                // Parent: wait for child to signal readiness
                unsafe { libc::close(write_fd) };

                let mut buffer = [0u8; 1];
                let bytes_read =
                    unsafe { libc::read(read_fd, buffer.as_mut_ptr() as *mut libc::c_void, 1) };
                let errno = std::io::Error::last_os_error();
                unsafe { libc::close(read_fd) };

                if bytes_read == -1 {
                    return Err(
                        PmxcfsError::System(format!("Failed to read from child: {errno}")).into(),
                    );
                } else if bytes_read != 1 || buffer[0] != b'1' {
                    return Err(
                        PmxcfsError::System("Child failed to send ready signal".into()).into(),
                    );
                }

                // Child is ready - write PID file with child's PID
                let child_pid = child.as_raw() as u32;
                write_pid_file(&pid_file_path, child_pid, gid)?;

                tracing::info!("Child process {} signaled ready, parent exiting", child_pid);

                Ok((DaemonProcess::Parent, None))
            }
            Ok(ForkResult::Child) => {
                // Child: become daemon and return signal handle
                unsafe { libc::close(read_fd) };

                // Create new session
                unsafe {
                    if libc::setsid() == -1 {
                        return Err(
                            PmxcfsError::System("Failed to create new session".into()).into()
                        );
                    }
                }

                // Change to root directory
                std::env::set_current_dir("/")?;

                // Redirect standard streams to /dev/null
                let devnull = File::open("/dev/null")?;
                unsafe {
                    libc::dup2(devnull.as_raw_fd(), 0);
                    libc::dup2(devnull.as_raw_fd(), 1);
                    libc::dup2(devnull.as_raw_fd(), 2);
                }

                let signal_handle = SignalHandle { write_fd };
                let guard = PidFileGuard {
                    path: pid_file_path,
                };

                Ok((DaemonProcess::Child(guard), Some(signal_handle)))
            }
            Err(e) => Err(PmxcfsError::System(format!("Failed to fork: {e}")).into()),
        }
    }
}

impl Default for Daemon {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle for signaling parent process readiness
///
/// The child process must call `signal_ready()` to inform the parent
/// that all initialization is complete and it's safe to write the PID file.
pub struct SignalHandle {
    write_fd: RawFd,
}

impl SignalHandle {
    /// Signal parent that child is ready
    ///
    /// This must be called after all initialization is complete.
    /// The parent will write the PID file and exit after receiving this signal.
    pub fn signal_ready(self) -> Result<()> {
        unsafe {
            let result = libc::write(self.write_fd, b"1".as_ptr() as *const libc::c_void, 1);
            libc::close(self.write_fd);

            if result != 1 {
                return Err(PmxcfsError::System("Failed to signal parent process".into()).into());
            }
        }
        tracing::debug!("Signaled parent process - child ready");
        Ok(())
    }
}

/// Write PID file with specified process ID
fn write_pid_file(path: &PathBuf, pid: u32, gid: u32) -> Result<()> {
    let content = format!("{pid}\n");

    fs::write(path, content)
        .with_context(|| format!("Failed to write PID file to {}", path.display()))?;

    // Set permissions (0o644 = rw-r--r--)
    let metadata = fs::metadata(path)?;
    let mut perms = metadata.permissions();
    perms.set_mode(0o644);
    fs::set_permissions(path, perms)?;

    // Set ownership (root:gid)
    let path_cstr = std::ffi::CString::new(path.to_string_lossy().as_ref()).unwrap();
    unsafe {
        libc::chown(path_cstr.as_ptr(), 0, gid as libc::gid_t);
    }

    tracing::info!("Created PID file at {} with PID {}", path.display(), pid);

    Ok(())
}
