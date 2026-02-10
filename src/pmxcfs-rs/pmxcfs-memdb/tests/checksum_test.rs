//! Unit tests for database checksum computation
//!
//! These tests verify that:
//! 1. Checksums are deterministic (same data = same checksum)
//! 2. Checksums change when data changes
//! 3. Checksums depend on insertion order (matching C implementation)

use pmxcfs_memdb::MemDb;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

#[test]
fn test_checksum_deterministic() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;
    
    // Create first database
    let db1 = MemDb::open(&db_path, true)?;
    db1.create("/test1.txt", 0, 0, now)?;
    db1.write("/test1.txt", 0, 0, now, b"content1", false)?;
    db1.create("/test2.txt", 0, 0, now)?;
    db1.write("/test2.txt", 0, 0, now, b"content2", false)?;
    
    let checksum1 = db1.compute_database_checksum()?;
    drop(db1);
    
    // Create second database with same data
    std::fs::remove_file(&db_path)?;
    let db2 = MemDb::open(&db_path, true)?;
    db2.create("/test1.txt", 0, 0, now)?;
    db2.write("/test1.txt", 0, 0, now, b"content1", false)?;
    db2.create("/test2.txt", 0, 0, now)?;
    db2.write("/test2.txt", 0, 0, now, b"content2", false)?;
    
    let checksum2 = db2.compute_database_checksum()?;
    
    assert_eq!(checksum1, checksum2, "Checksums should be identical for same data");
    
    Ok(())
}

#[test]
fn test_checksum_changes_with_data() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let db = MemDb::open(&db_path, true)?;
    
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;
    
    // Initial checksum
    let checksum1 = db.compute_database_checksum()?;
    
    // Add a file
    db.create("/test.txt", 0, 0, now)?;
    db.write("/test.txt", 0, 0, now, b"content", false)?;
    let checksum2 = db.compute_database_checksum()?;
    
    assert_ne!(checksum1, checksum2, "Checksum should change after adding file");
    
    // Modify the file
    db.write("/test.txt", 0, 0, now + 1, b"modified", false)?;
    let checksum3 = db.compute_database_checksum()?;
    
    assert_ne!(checksum2, checksum3, "Checksum should change after modifying file");
    
    Ok(())
}

/// NOTE: This test is intentionally removed because it tests for incorrect behavior.
///
/// The C implementation includes the version field in checksum computation, which means
/// databases with different insertion orders will have different version numbers and
/// therefore different checksums. This is correct behavior - it allows the cluster to
/// detect when nodes have different histories.
///
/// Example:
/// - db1: /a.txt (version=2), /b.txt (version=4), /c.txt (version=6)
/// - db2: /c.txt (version=2), /b.txt (version=4), /a.txt (version=6)
/// These have different checksums because the files have different version numbers.
///
/// The original test expected checksums to be identical regardless of insertion order,
/// but this is not how the C implementation works.
#[test]
fn test_checksum_depends_on_insertion_order() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;

    // Create first database with files in order A, B, C
    let db_path1 = temp_dir.path().join("test1.db");
    let db1 = MemDb::open(&db_path1, true)?;
    db1.create("/a.txt", 0, 0, now)?;
    db1.write("/a.txt", 0, 0, now, b"content_a", false)?;
    db1.create("/b.txt", 0, 0, now)?;
    db1.write("/b.txt", 0, 0, now, b"content_b", false)?;
    db1.create("/c.txt", 0, 0, now)?;
    db1.write("/c.txt", 0, 0, now, b"content_c", false)?;
    let checksum1 = db1.compute_database_checksum()?;

    // Create second database with files in order C, B, A
    let db_path2 = temp_dir.path().join("test2.db");
    let db2 = MemDb::open(&db_path2, true)?;
    db2.create("/c.txt", 0, 0, now)?;
    db2.write("/c.txt", 0, 0, now, b"content_c", false)?;
    db2.create("/b.txt", 0, 0, now)?;
    db2.write("/b.txt", 0, 0, now, b"content_b", false)?;
    db2.create("/a.txt", 0, 0, now)?;
    db2.write("/a.txt", 0, 0, now, b"content_a", false)?;
    let checksum2 = db2.compute_database_checksum()?;

    // Checksums SHOULD differ because files have different version numbers
    // This matches C implementation behavior where version is included in checksum
    assert_ne!(checksum1, checksum2,
        "Checksums should differ when insertion order differs (different version numbers)");

    Ok(())
}

#[test]
fn test_checksum_with_corosync_conf() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let db = MemDb::open(&db_path, true)?;
    
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;
    
    // Simulate what happens when corosync.conf is imported
    let corosync_content = b"totem {\n  version: 2\n}\n";
    db.create("/corosync.conf", 0, 0, now)?;
    db.write("/corosync.conf", 0, 0, now, corosync_content, false)?;
    
    let checksum_with_corosync = db.compute_database_checksum()?;
    
    // Create another database without corosync.conf
    std::fs::remove_file(&db_path)?;
    let db2 = MemDb::open(&db_path, true)?;
    let checksum_without_corosync = db2.compute_database_checksum()?;
    
    assert_ne!(
        checksum_with_corosync, 
        checksum_without_corosync,
        "Checksum should differ when corosync.conf is present"
    );
    
    Ok(())
}

#[test]
fn test_checksum_with_different_mtimes() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as u32;
    
    // Create first database with mtime = now
    let db_path1 = temp_dir.path().join("test1.db");
    let db1 = MemDb::open(&db_path1, true)?;
    db1.create("/test.txt", 0, 0, now)?;
    db1.write("/test.txt", 0, 0, now, b"content", false)?;
    let checksum1 = db1.compute_database_checksum()?;
    
    // Create second database with mtime = now + 1
    let db_path2 = temp_dir.path().join("test2.db");
    let db2 = MemDb::open(&db_path2, true)?;
    db2.create("/test.txt", 0, 0, now + 1)?;
    db2.write("/test.txt", 0, 0, now + 1, b"content", false)?;
    let checksum2 = db2.compute_database_checksum()?;
    
    assert_ne!(
        checksum1, 
        checksum2,
        "Checksum should differ when mtime differs (even with same content)"
    );
    
    Ok(())
}
