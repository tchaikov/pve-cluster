/// In-memory database with SQLite persistence
///
/// This module provides a cluster-synchronized in-memory database with SQLite persistence.
/// The implementation is organized into focused submodules:
///
/// - `types`: Type definitions and constants
/// - `database`: Core MemDb struct and CRUD operations
/// - `locks`: Resource locking functionality
/// - `sync`: State synchronization and serialization
/// - `index`: C-compatible memdb index structures for efficient state comparison
/// - `traits`: Trait abstractions for dependency injection and testing
mod database;
mod index;
mod locks;
mod sync;
mod traits;
mod types;
mod vmlist;

// Re-export public types
pub use database::MemDb;
pub use index::{IndexEntry, MemDbIndex};
pub use locks::is_lock_path;
pub use traits::MemDbOps;
pub use types::{ROOT_INODE, TreeEntry};
pub use vmlist::{is_valid_nodename, parse_vm_config_name, parse_vm_config_path, recreate_vmlist};
