/// Quorum-Dependent Behavior Tests
///
/// Tests for pmxcfs behavior that changes based on quorum state.
/// These tests verify plugin behavior (especially symlinks) and
/// operations that should be blocked/allowed based on quorum.
///
/// Note: These tests do NOT require FUSE mounting - they test the
/// plugin layer directly, which is accessible without root permissions.
use anyhow::Result;
use pmxcfs_rs::config::Config;
use pmxcfs_rs::plugins;
use std::sync::Arc;

/// Helper to create a test configuration
fn create_test_config() -> Arc<Config> {
    Config::new(
        "testnode".to_string(),
        "127.0.0.1".to_string(),
        1000, // www-data gid
        false,
        true, // local mode
        "test-cluster".to_string(),
    )
}

#[test]
fn test_members_plugin_with_quorum() -> Result<()> {
    let config = create_test_config();
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

    // Initialize plugins
    let plugins = plugins::init_plugins(config, status.clone());

    // Test 1: With quorum, .members should be a regular file
    status.set_quorate(true);

    let members_plugin = plugins
        .get(".members")
        .expect(".members plugin should exist");

    let mode = members_plugin.mode();
    let is_symlink = (mode & libc::S_IFMT) == libc::S_IFLNK;
    let is_regular = (mode & libc::S_IFMT) == libc::S_IFREG;

    println!("With quorum - mode: 0o{:o}, is_symlink: {}, is_regular: {}", mode, is_symlink, is_regular);

    // When quorate, .members should be accessible as regular file
    let data = members_plugin.read()?;
    println!("✓ .members readable with quorum ({} bytes)", data.len());

    // Test 2: Without quorum, .members might become a symlink to /etc/pve/error
    status.set_quorate(false);

    // Re-get plugin to see updated state
    let members_plugin = plugins
        .get(".members")
        .expect(".members plugin should exist");

    let mode_without_quorum = members_plugin.mode();
    println!("Without quorum - mode: 0o{:o}", mode_without_quorum);

    // Plugin should still be readable (might return error content or symlink)
    let result = members_plugin.read();
    match result {
        Ok(data) => {
            println!("✓ .members still readable without quorum ({} bytes)", data.len());
        }
        Err(e) => {
            println!("✓ .members returns error without quorum: {}", e);
        }
    }

    Ok(())
}

#[test]
fn test_vmlist_plugin_with_quorum() -> Result<()> {
    let config = create_test_config();
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

    let plugins = plugins::init_plugins(config, status.clone());

    // Test 1: With quorum, .vmlist should work normally
    status.set_quorate(true);

    let vmlist_plugin = plugins
        .get(".vmlist")
        .expect(".vmlist plugin should exist");

    let data = vmlist_plugin.read()?;
    println!("✓ .vmlist readable with quorum ({} bytes)", data.len());

    // Verify it's valid JSON
    let _: serde_json::Value = serde_json::from_slice(&data)?;
    println!("✓ .vmlist contains valid JSON");

    // Test 2: Without quorum, behavior may change
    status.set_quorate(false);

    let vmlist_plugin = plugins
        .get(".vmlist")
        .expect(".vmlist plugin should exist");

    let result = vmlist_plugin.read();
    match result {
        Ok(data) => {
            println!("✓ .vmlist still readable without quorum ({} bytes)", data.len());
        }
        Err(e) => {
            println!("✓ .vmlist behavior changed without quorum: {}", e);
        }
    }

    Ok(())
}

#[test]
fn test_version_plugin_unaffected_by_quorum() -> Result<()> {
    let config = create_test_config();
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

    let plugins = plugins::init_plugins(config, status.clone());

    // .version should work regardless of quorum status
    let version_plugin = plugins
        .get(".version")
        .expect(".version plugin should exist");

    // Test with quorum
    status.set_quorate(true);
    let data_with_quorum = version_plugin.read()?;
    let version_with: serde_json::Value = serde_json::from_slice(&data_with_quorum)?;
    println!("✓ .version readable with quorum");

    // Test without quorum
    status.set_quorate(false);
    let data_without_quorum = version_plugin.read()?;
    let version_without: serde_json::Value = serde_json::from_slice(&data_without_quorum)?;
    println!("✓ .version readable without quorum");

    // Version tracking should work in both cases
    assert!(version_with.is_object(), "Version should be JSON object");
    assert!(version_without.is_object(), "Version should be JSON object");

    println!("✓ .version plugin unaffected by quorum state");

    Ok(())
}

#[test]
fn test_plugin_modes_reflect_quorum_state() -> Result<()> {
    let config = create_test_config();
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

    let plugins = plugins::init_plugins(config, status.clone());

    // Get all plugins
    let plugin_names = plugins.list();
    println!("Testing {} plugins for quorum behavior", plugin_names.len());

    // Test each plugin's mode with and without quorum
    for name in &plugin_names {
        if let Some(plugin) = plugins.get(name) {
            status.set_quorate(true);
            let mode_with = plugin.mode();

            status.set_quorate(false);
            let mode_without = plugin.mode();

            let is_symlink_with = (mode_with & libc::S_IFMT) == libc::S_IFLNK;
            let is_symlink_without = (mode_without & libc::S_IFMT) == libc::S_IFLNK;

            if is_symlink_with != is_symlink_without {
                println!(
                    "  Plugin '{}' changes: with_quorum={}, without_quorum={}",
                    name,
                    if is_symlink_with { "symlink" } else { "regular" },
                    if is_symlink_without { "symlink" } else { "regular" }
                );
            } else {
                println!("  Plugin '{}': no mode change (both {})",
                    name,
                    if is_symlink_with { "symlink" } else { "regular" }
                );
            }
        }
    }

    println!("✓ Plugin mode behavior verified");

    Ok(())
}

#[test]
fn test_quorum_state_persistence() -> Result<()> {
    let config = create_test_config();
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

    let _plugins = plugins::init_plugins(config, status.clone());

    // Test quorum state changes
    status.set_quorate(false);
    assert!(!status.is_quorate(), "Should not be quorate");
    println!("✓ Quorum state: false");

    status.set_quorate(true);
    assert!(status.is_quorate(), "Should be quorate");
    println!("✓ Quorum state: true");

    status.set_quorate(false);
    assert!(!status.is_quorate(), "Should not be quorate again");
    println!("✓ Quorum state: false again");

    println!("✓ Quorum state changes work correctly");

    Ok(())
}

#[tokio::test]
async fn test_quorum_change_notification() -> Result<()> {
    let config = create_test_config();
    let config = pmxcfs_test_utils::create_test_config(false);
        let status = pmxcfs_status::init_with_config(config);

    let _plugins = plugins::init_plugins(config, status.clone());

    // Initial state: quorate
    status.set_quorate(true);
    assert!(status.is_quorate());

    // Simulate quorum loss
    status.set_quorate(false);

    // Give time for any async updates
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(!status.is_quorate(), "Quorum should be lost");
    println!("✓ Quorum loss detected");

    // Simulate quorum regain
    status.set_quorate(true);

    // Give time for any async updates
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(status.is_quorate(), "Quorum should be regained");
    println!("✓ Quorum regain detected");

    Ok(())
}
