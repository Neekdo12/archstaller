//! Host Bluetooth discovery: detects paired Bluetooth devices and controller configurations.

use crate::privilege::read_tree_privileged;
use config::SystemFile;
use std::fs;

/// Discovered Bluetooth configurations and paired devices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostBluetooth {
    /// Whether Bluetooth configuration was detected on the host.
    pub enabled: bool,
    /// System files to restore (`/var/lib/bluetooth/` controllers, pairings, and keys).
    pub files: Vec<SystemFile>,
    /// Packages to ensure are installed (`bluez`, `bluez-utils`).
    pub packages: Vec<String>,
    /// Systemd service to enable (`bluetooth.service`).
    pub services: Vec<String>,
}

/// Discovers Bluetooth adapter state and paired devices from `/var/lib/bluetooth`.
pub fn discover_host_bluetooth(pw: Option<&str>) -> HostBluetooth {
    let mut result = HostBluetooth::default();
    let bt_dir = "/var/lib/bluetooth";

    if fs::metadata(bt_dir).is_err() {
        return result;
    }

    result.enabled = true;
    result.packages.push("bluez".into());
    result.packages.push("bluez-utils".into());
    result.services.push("bluetooth.service".into());

    // Read pairing keys and controller configurations
    if let Ok(tree) = read_tree_privileged(bt_dir, pw) {
        for (full_path, content) in tree {
            if !content.is_empty() {
                let rel_path = full_path.trim_start_matches('/').to_string();
                let mode = 0o600;
                result.files.push(SystemFile {
                    path: rel_path,
                    mode,
                    file: None,
                    content: Some(content),
                });
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bluetooth_defaults_are_empty() {
        let bt = HostBluetooth::default();
        assert!(!bt.enabled);
        assert!(bt.files.is_empty());
        assert!(bt.packages.is_empty());
        assert!(bt.services.is_empty());
    }
}
