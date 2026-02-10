/// Plugin registry and initialization
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use super::clusterlog::ClusterlogPlugin;
use super::debug::DebugPlugin;
use super::members::MembersPlugin;
use super::rrd::RrdPlugin;
use super::types::{LinkPlugin, Plugin};
use super::version::VersionPlugin;
use super::vmlist::VmlistPlugin;

/// Plugin registry
pub struct PluginRegistry {
    plugins: RwLock<HashMap<String, Arc<dyn Plugin>>>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
        }
    }

    /// Register a plugin
    pub fn register(&self, plugin: Arc<dyn Plugin>) {
        let name = plugin.name().to_string();
        self.plugins.write().insert(name, plugin);
    }

    /// Get a plugin by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn Plugin>> {
        self.plugins.read().get(name).cloned()
    }

    /// Check if a path is a plugin
    pub fn is_plugin(&self, name: &str) -> bool {
        self.plugins.read().contains_key(name)
    }

    /// List all plugin names
    pub fn list(&self) -> Vec<String> {
        self.plugins.read().keys().cloned().collect()
    }
}

/// Initialize the plugin registry with default plugins
pub fn init_plugins(
    config: Arc<pmxcfs_config::Config>,
    status: Arc<pmxcfs_status::Status>,
) -> Arc<PluginRegistry> {
    tracing::info!("Initializing plugin system for node: {}", config.nodename());

    let registry = Arc::new(PluginRegistry::new());

    // .version - cluster version information
    let version_plugin = Arc::new(VersionPlugin::new(config.clone(), status.clone()));
    registry.register(version_plugin);

    // .members - cluster member list
    let members_plugin = Arc::new(MembersPlugin::new(config.clone(), status.clone()));
    registry.register(members_plugin);

    // .vmlist - VM list
    let vmlist_plugin = Arc::new(VmlistPlugin::new(status.clone()));
    registry.register(vmlist_plugin);

    // .rrd - RRD data
    let rrd_plugin = Arc::new(RrdPlugin::new(status.clone()));
    registry.register(rrd_plugin);

    // .clusterlog - cluster log
    let clusterlog_plugin = Arc::new(ClusterlogPlugin::new(status.clone()));
    registry.register(clusterlog_plugin);

    // .debug - debug settings (read/write)
    let debug_plugin = Arc::new(DebugPlugin::new(config.clone()));
    registry.register(debug_plugin);

    // Symbolic link plugins - point to nodes/{nodename}/ subdirectories
    // These provide convenient access to node-specific directories from the root
    let nodename = config.nodename();

    // local -> nodes/{nodename}/local
    let local_link = Arc::new(LinkPlugin::new("local", format!("nodes/{nodename}")));
    registry.register(local_link);

    // qemu-server -> nodes/{nodename}/qemu-server
    let qemu_link = Arc::new(LinkPlugin::new(
        "qemu-server",
        format!("nodes/{nodename}/qemu-server"),
    ));
    registry.register(qemu_link);

    // openvz -> nodes/{nodename}/openvz (legacy support)
    let openvz_link = Arc::new(LinkPlugin::new(
        "openvz",
        format!("nodes/{nodename}/openvz"),
    ));
    registry.register(openvz_link);

    // lxc -> nodes/{nodename}/lxc
    let lxc_link = Arc::new(LinkPlugin::new("lxc", format!("nodes/{nodename}/lxc")));
    registry.register(lxc_link);

    tracing::info!(
        "Registered {} plugins ({} func plugins, 4 link plugins)",
        registry.list().len(),
        registry.list().len() - 4
    );

    registry
}

#[cfg(test)]
/// Test-only helper to create a plugin registry with a simple nodename
pub fn init_plugins_for_test(nodename: &str) -> Arc<PluginRegistry> {
    use pmxcfs_config::Config;

    // Create config with the specified nodename for testing
    let config = Config::shared(
        nodename.to_string(),
        "127.0.0.1".parse().unwrap(),
        33, // www-data gid
        false,
        false,
        "pmxcfs".to_string(),
    );
    let status = pmxcfs_status::init_with_config(config.clone());

    init_plugins(config, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_func_plugins_exist() {
        let registry = init_plugins_for_test("testnode");

        let func_plugins = vec![
            ".version",
            ".members",
            ".vmlist",
            ".rrd",
            ".clusterlog",
            ".debug",
        ];

        for plugin_name in func_plugins {
            assert!(
                registry.is_plugin(plugin_name),
                "{plugin_name} should be registered"
            );

            let plugin = registry.get(plugin_name);
            assert!(plugin.is_some(), "{plugin_name} should be accessible");
            assert_eq!(plugin.unwrap().name(), plugin_name);
        }
    }

    #[test]
    fn test_registry_link_plugins_exist() {
        let registry = init_plugins_for_test("testnode");

        let link_plugins = vec!["local", "qemu-server", "openvz", "lxc"];

        for plugin_name in link_plugins {
            assert!(
                registry.is_plugin(plugin_name),
                "{plugin_name} link should be registered"
            );

            let plugin = registry.get(plugin_name);
            assert!(plugin.is_some(), "{plugin_name} link should be accessible");
            assert_eq!(plugin.unwrap().name(), plugin_name);
        }
    }

    #[test]
    fn test_registry_link_targets_use_nodename() {
        // Test with different nodenames
        let test_cases = vec![
            ("node1", "nodes/node1"),
            ("pve-test", "nodes/pve-test"),
            ("cluster-node-03", "nodes/cluster-node-03"),
        ];

        for (nodename, expected_local_target) in test_cases {
            let registry = init_plugins_for_test(nodename);

            // Test local link
            let local = registry.get("local").expect("local link should exist");
            let data = local.read().expect("should read link target");
            let target = String::from_utf8(data).expect("target should be UTF-8");
            assert_eq!(
                target, expected_local_target,
                "local link should point to nodes/{nodename} for {nodename}"
            );

            // Test qemu-server link
            let qemu = registry
                .get("qemu-server")
                .expect("qemu-server link should exist");
            let data = qemu.read().expect("should read link target");
            let target = String::from_utf8(data).expect("target should be UTF-8");
            assert_eq!(
                target,
                format!("nodes/{nodename}/qemu-server"),
                "qemu-server link should include nodename"
            );

            // Test lxc link
            let lxc = registry.get("lxc").expect("lxc link should exist");
            let data = lxc.read().expect("should read link target");
            let target = String::from_utf8(data).expect("target should be UTF-8");
            assert_eq!(
                target,
                format!("nodes/{nodename}/lxc"),
                "lxc link should include nodename"
            );

            // Test openvz link (legacy)
            let openvz = registry.get("openvz").expect("openvz link should exist");
            let data = openvz.read().expect("should read link target");
            let target = String::from_utf8(data).expect("target should be UTF-8");
            assert_eq!(
                target,
                format!("nodes/{nodename}/openvz"),
                "openvz link should include nodename"
            );
        }
    }

    #[test]
    fn test_registry_nonexistent_plugin() {
        let registry = init_plugins_for_test("testnode");

        assert!(!registry.is_plugin(".nonexistent"));
        assert!(registry.get(".nonexistent").is_none());
    }

    #[test]
    fn test_registry_plugin_modes() {
        let registry = init_plugins_for_test("testnode");

        // .debug should be writable (0o640)
        let debug = registry.get(".debug").expect(".debug should exist");
        assert_eq!(debug.mode(), 0o640, ".debug should have writable mode");

        // All other func plugins should be read-only (0o440)
        let readonly_plugins = vec![".version", ".members", ".vmlist", ".rrd", ".clusterlog"];
        for plugin_name in readonly_plugins {
            let plugin = registry.get(plugin_name).unwrap();
            assert_eq!(plugin.mode(), 0o440, "{plugin_name} should be read-only");
        }

        // Link plugins should have 0o777
        let links = vec!["local", "qemu-server", "openvz", "lxc"];
        for link_name in links {
            let link = registry.get(link_name).unwrap();
            assert_eq!(link.mode(), 0o777, "{link_name} should have 777 mode");
        }
    }

    #[test]
    fn test_link_plugins_are_symlinks() {
        let registry = init_plugins_for_test("testnode");

        // Link plugins should be identified as symlinks
        let link_plugins = vec!["local", "qemu-server", "openvz", "lxc"];
        for link_name in link_plugins {
            let link = registry.get(link_name).unwrap();
            assert!(
                link.is_symlink(),
                "{link_name} should be identified as a symlink"
            );
        }

        // Func plugins should NOT be identified as symlinks
        let func_plugins = vec![
            ".version",
            ".members",
            ".vmlist",
            ".rrd",
            ".clusterlog",
            ".debug",
        ];
        for plugin_name in func_plugins {
            let plugin = registry.get(plugin_name).unwrap();
            assert!(
                !plugin.is_symlink(),
                "{plugin_name} should NOT be identified as a symlink"
            );
        }
    }
}
