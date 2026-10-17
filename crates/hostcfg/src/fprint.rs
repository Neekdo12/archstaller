//! Fingerprint scanning discovery: detects PAM configuration and enrolled biometric templates.

use crate::privilege::read_tree_privileged;
use config::SystemFile;
use std::fs;

/// Discovered fingerprint PAM settings and enrolled biometric templates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostFprint {
    /// Whether fingerprint authentication was detected on the host.
    pub enabled: bool,
    /// System files to restore (PAM configs in `/etc/pam.d/` and templates in `/var/lib/fprint/`).
    pub files: Vec<SystemFile>,
    /// Packages to ensure are installed (`fprintd`, and optionally `pam-fprint-grosshack`).
    pub packages: Vec<String>,
}

/// Discovers whether fingerprint scanning is configured on the host and gathers files.
pub fn discover_host_fprint(pw: Option<&str>) -> HostFprint {
    let mut result = HostFprint::default();

    // 1. Check PAM configs for pam_fprintd.so
    let pam_files = [
        "/etc/pam.d/sudo",
        "/etc/pam.d/login",
        "/etc/pam.d/ly",
        "/etc/pam.d/system-auth",
    ];

    for path in pam_files {
        if let Ok(content) = fs::read_to_string(path) {
            if content.contains("pam_fprintd.so") || content.contains("pam-fprint-grosshack") {
                result.enabled = true;
                result.files.push(SystemFile {
                    path: path.trim_start_matches('/').to_string(),
                    mode: 0o644,
                    file: None,
                    content: Some(content.into_bytes()),
                });
            }
        }
    }

    if !result.enabled {
        // Also check if /var/lib/fprint exists or fprintd service exists
        if fs::metadata("/var/lib/fprint").is_ok() {
            result.enabled = true;
        }
    }

    if result.enabled {
        result.packages.push("fprintd".into());

        // Check if pam-fprint-grosshack is installed
        if let Ok(out) = std::process::Command::new("pacman")
            .args(["-Q", "pam-fprint-grosshack"])
            .output()
        {
            if out.status.success() {
                result.packages.push("pam-fprint-grosshack".into());
            }
        }

        // 2. Read biometric templates from /var/lib/fprint/ if accessible
        let fprint_dir = "/var/lib/fprint";
        if let Ok(templates) = read_tree_privileged(fprint_dir, pw) {
            for (full_path, content) in templates {
                if !content.is_empty() {
                    let rel_path = full_path.trim_start_matches('/').to_string();
                    result.files.push(SystemFile {
                        path: rel_path,
                        mode: 0o600,
                        file: None,
                        content: Some(content),
                    });
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_pam_line() {
        let pam = "#%PAM-1.0\nauth sufficient pam_fprintd.so\nauth include system-auth\n";
        assert!(pam.contains("pam_fprintd.so"));
    }
}
