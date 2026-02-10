//! Restart flag management
//!
//! This module provides RAII-based restart flag management. The flag is
//! created on shutdown to signal that pmxcfs is restarting (not stopping).

use std::ffi::CString;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// RAII wrapper for restart flag
///
/// Creates a flag file on construction to signal pmxcfs restart.
/// The file is NOT automatically removed (it's consumed by the next startup).
pub struct RestartFlag;

impl RestartFlag {
    /// Create a restart flag file
    ///
    /// This signals that pmxcfs is restarting (not permanently shutting down).
    ///
    /// # Arguments
    ///
    /// * `path` - Path where the restart flag should be created
    /// * `gid` - Group ID to set for the file
    pub fn create(path: PathBuf, gid: u32) -> Self {
        // Create the restart flag file
        match File::create(&path) {
            Ok(mut file) => {
                if let Err(e) = file.flush() {
                    warn!(error = %e, path = %path.display(), "Failed to flush restart flag");
                }

                // Set ownership (root:gid)
                Self::set_ownership(&path, gid);
                info!(path = %path.display(), "Created restart flag");
            }
            Err(e) => {
                warn!(error = %e, path = %path.display(), "Failed to create restart flag");
            }
        }

        Self
    }

    /// Set file ownership to root:gid
    fn set_ownership(path: &Path, gid: u32) {
        let path_str = path.to_string_lossy();
        if let Ok(path_cstr) = CString::new(path_str.as_ref()) {
            // Safety: chown is called with a valid C string and valid UID/GID
            unsafe {
                if libc::chown(path_cstr.as_ptr(), 0, gid as libc::gid_t) != 0 {
                    let error = std::io::Error::last_os_error();
                    warn!(error = %error, "Failed to change ownership of restart flag");
                }
            }
        }
    }
}
