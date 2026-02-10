use thiserror::Error;

/// Error types for pmxcfs operations
#[derive(Error, Debug)]
pub enum PmxcfsError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Database(String),

    #[error("FUSE error: {0}")]
    Fuse(String),

    #[error("Cluster error: {0}")]
    Cluster(String),

    #[error("Corosync error: {0}")]
    Corosync(String),

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("System error: {0}")]
    System(String),

    #[error("IPC error: {0}")]
    Ipc(String),

    #[error("Permission denied")]
    PermissionDenied,

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Already exists: {0}")]
    AlreadyExists(String),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Not a directory: {0}")]
    NotADirectory(String),

    #[error("Is a directory: {0}")]
    IsADirectory(String),

    #[error("Directory not empty: {0}")]
    DirectoryNotEmpty(String),

    #[error("No quorum")]
    NoQuorum,

    #[error("Read-only filesystem")]
    ReadOnlyFilesystem,

    #[error("File too large")]
    FileTooLarge,

    #[error("Filesystem full")]
    FilesystemFull,

    #[error("Lock error: {0}")]
    Lock(String),

    #[error("Timeout")]
    Timeout,

    #[error("Invalid path: {0}")]
    InvalidPath(String),
}

impl PmxcfsError {
    /// Convert error to errno value for FUSE operations
    pub fn to_errno(&self) -> i32 {
        match self {
            // File/directory errors
            PmxcfsError::NotFound(_) => libc::ENOENT,
            PmxcfsError::AlreadyExists(_) => libc::EEXIST,
            PmxcfsError::NotADirectory(_) => libc::ENOTDIR,
            PmxcfsError::IsADirectory(_) => libc::EISDIR,
            PmxcfsError::DirectoryNotEmpty(_) => libc::ENOTEMPTY,
            PmxcfsError::FileTooLarge => libc::EFBIG,
            PmxcfsError::FilesystemFull => libc::ENOSPC,
            PmxcfsError::ReadOnlyFilesystem => libc::EROFS,

            // Permission and access errors
            // EACCES: Permission denied for file operations (standard POSIX)
            // C implementation uses EACCES as default for access/quorum issues
            PmxcfsError::PermissionDenied => libc::EACCES,
            PmxcfsError::NoQuorum => libc::EACCES,

            // Validation errors
            PmxcfsError::InvalidArgument(_) => libc::EINVAL,
            PmxcfsError::InvalidPath(_) => libc::EINVAL,

            // Lock errors - use EAGAIN for temporary failures
            PmxcfsError::Lock(_) => libc::EAGAIN,

            // Timeout
            PmxcfsError::Timeout => libc::ETIMEDOUT,

            // I/O errors with automatic errno extraction
            PmxcfsError::Io(e) => match e.raw_os_error() {
                Some(errno) => errno,
                None => libc::EIO,
            },

            // Fallback to EIO for internal/system errors
            PmxcfsError::Database(_) |
            PmxcfsError::Fuse(_) |
            PmxcfsError::Cluster(_) |
            PmxcfsError::Corosync(_) |
            PmxcfsError::Configuration(_) |
            PmxcfsError::System(_) |
            PmxcfsError::Ipc(_) => libc::EIO,
        }
    }
}

/// Result type for pmxcfs operations
pub type Result<T> = std::result::Result<T, PmxcfsError>;
