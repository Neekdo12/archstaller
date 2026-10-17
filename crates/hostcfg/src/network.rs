//! Host network discovery: extracts NetworkManager and iwd connection profiles from the host system.

use crate::privilege::{read_dir_privileged, read_file_privileged};
use config::SystemFile;

/// Discovered network configuration from the host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostNetwork {
    /// Saved network connection files (mode 0600).
    pub connection_files: Vec<SystemFile>,
    /// Associated package (e.g. "networkmanager").
    pub package: Option<String>,
    /// Associated systemd service (e.g. "NetworkManager.service").
    pub service: Option<String>,
}

/// Discovers NetworkManager system-connections and iwd configurations on the running host.
pub fn discover_host_network(pw: Option<&str>) -> HostNetwork {
    let mut net = HostNetwork::default();

    // 1. Discover NetworkManager connection profiles in /etc/NetworkManager/system-connections/
    let nm_dir = "/etc/NetworkManager/system-connections";
    if let Ok(entries) = read_dir_privileged(nm_dir, pw) {
        for entry in entries {
            if entry.starts_with('.') {
                continue;
            }
            let full_path = format!("{nm_dir}/{entry}");
            if let Ok(content) = read_file_privileged(&full_path, pw) {
                if !content.is_empty() {
                    net.connection_files.push(SystemFile {
                        path: format!("etc/NetworkManager/system-connections/{entry}"),
                        mode: 0o600,
                        file: None,
                        content: Some(content),
                    });
                }
            }
        }
        if !net.connection_files.is_empty() {
            net.package = Some("networkmanager".into());
            net.service = Some("NetworkManager.service".into());
        }
    }

    // 2. Discover iwd connection profiles in /var/lib/iwd/
    let iwd_dir = "/var/lib/iwd";
    if let Ok(entries) = read_dir_privileged(iwd_dir, pw) {
        let mut found_iwd = false;
        for entry in entries {
            if entry.ends_with(".psk") || entry.ends_with(".open") || entry.ends_with(".8021x") {
                let full_path = format!("{iwd_dir}/{entry}");
                if let Ok(content) = read_file_privileged(&full_path, pw) {
                    if !content.is_empty() {
                        net.connection_files.push(SystemFile {
                            path: format!("var/lib/iwd/{entry}"),
                            mode: 0o600,
                            file: None,
                            content: Some(content),
                        });
                        found_iwd = true;
                    }
                }
            }
        }
        if found_iwd && net.package.is_none() {
            net.package = Some("iwd".into());
            net.service = Some("iwd.service".into());
        }
    }

    net
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_from_mock_dir() {
        let dir = std::env::temp_dir().join(format!("archstaller-net-test-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        let conn_file = dir.join("WiFi.nmconnection");
        std::fs::write(&conn_file, b"[connection]\nid=WiFi\n").unwrap();

        let entries = read_dir_privileged(dir.to_str().unwrap(), None).unwrap();
        assert_eq!(entries, vec!["WiFi.nmconnection".to_string()]);
        drop(std::fs::remove_dir_all(&dir));
    }
}
