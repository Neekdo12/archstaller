//! Host SSH discovery: detects host keys and sshd service configuration.

use crate::privilege::read_file_privileged;
use config::SystemFile;
use std::path::Path;

/// Discovered SSH host keys and service settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostSsh {
    /// Whether SSH host keys were detected on the host.
    pub enabled: bool,
    /// System files to restore (`/etc/ssh/ssh_host_*` keys and configs).
    pub files: Vec<SystemFile>,
    /// Packages to ensure are installed (`openssh`).
    pub packages: Vec<String>,
    /// Systemd service to enable (`sshd.service` if configured).
    pub services: Vec<String>,
}

/// Known SSH host key filenames in `/etc/ssh/`.
pub const SSH_HOST_KEY_NAMES: &[&str] = &[
    "ssh_host_rsa_key",
    "ssh_host_rsa_key.pub",
    "ssh_host_ed25519_key",
    "ssh_host_ed25519_key.pub",
    "ssh_host_ecdsa_key",
    "ssh_host_ecdsa_key.pub",
];

/// Discovers host SSH keys and daemon configuration.
pub fn discover_host_ssh(pw: Option<&str>) -> HostSsh {
    let mut result = HostSsh::default();
    let ssh_dir = Path::new("/etc/ssh");

    if !ssh_dir.exists() {
        return result;
    }

    for name in SSH_HOST_KEY_NAMES {
        let full = format!("/etc/ssh/{name}");
        if let Ok(content) = read_file_privileged(&full, pw) {
            if !content.is_empty() {
                result.enabled = true;
                let mode = if name.ends_with(".pub") { 0o644 } else { 0o600 };
                result.files.push(SystemFile {
                    path: format!("etc/ssh/{name}"),
                    mode,
                    file: None,
                    content: Some(content),
                });
            }
        }
    }

    if result.enabled {
        result.packages.push("openssh".into());

        // Check if sshd is enabled
        let sshd_wants = Path::new("/etc/systemd/system/multi-user.target.wants/sshd.service");
        if sshd_wants.exists() {
            result.services.push("sshd.service".into());
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_key_names_are_correct() {
        assert!(SSH_HOST_KEY_NAMES.contains(&"ssh_host_ed25519_key"));
        assert!(SSH_HOST_KEY_NAMES.contains(&"ssh_host_ed25519_key.pub"));
    }
}
