//! Autoloading live system state (system identity, explicit packages, services, users) from the running host.

use crate::privilege::collect_privileged_system_files;
use crate::services;
use crate::system;
use crate::users;

/// Discovered system overlay data and accounts from privileged inspection.
#[derive(Debug, Clone, Default)]
pub struct PrivilegedAutoloadData {
    /// Raw `/etc/shadow` text if successfully retrieved.
    pub shadow: Option<String>,
    /// Discovered host user accounts populated with shadow hashes.
    pub users: Vec<config::User>,
    /// Root password crypt hash if present in shadow.
    pub root_password_hash: Option<String>,
    /// Discovered privileged system files (NetworkManager, iwd, biometrics, zram).
    pub system_files: Vec<config::SystemFile>,
    /// Packages to add to config (e.g. `networkmanager`, `iwd`, `fprintd`, `zram-generator`).
    pub extra_packages: Vec<String>,
    /// Systemd services to enable (e.g. `NetworkManager.service`, `iwd.service`).
    pub extra_services: Vec<String>,
}

/// Discovers privileged system state (shadow hashes, network connections, biometric templates, zram)
/// in a single root operation. This is computationally/IO intensive and blocking, so it should
/// be called in a background task (e.g. `gtk::gio::spawn_blocking`).
pub fn discover_live_system_privileged(
    pw: Option<&str>,
    use_pkexec: bool,
) -> Result<PrivilegedAutoloadData, String> {
    let snapshot = collect_privileged_system_files(pw, use_pkexec)?;
    let mut data = PrivilegedAutoloadData::default();

    // 1. Shadow and users
    if let Some(shadow_content) = &snapshot.shadow {
        data.shadow = Some(shadow_content.clone());
        if let Some(rh) = users::parse_shadow_hash(shadow_content, "root") {
            data.root_password_hash = Some(rh);
        }

        let host_users = users::discover_host_users();
        for u in host_users {
            let hash = users::parse_shadow_hash(shadow_content, &u.name).unwrap_or_else(|| {
                "$6$archstlr$gnUg60P8TrWaKT.8/oW8Iq1kc8LlPnk4A6UWw5ij0AAQNe2IN7TuPFnw7PmXMXuobrVIYBSbNWUgRbpafXqcc.".into()
            });
            data.users.push(config::User {
                name: u.name,
                password_hash: hash,
                groups: u.groups,
                shell: u.shell,
                dotfiles: None,
            });
        }
    }

    // 2. Process privileged system files (NetworkManager, iwd, fprint, bluetooth, ssh, zram)
    let mut has_nm = false;
    let mut has_iwd = false;
    let mut has_fprint = false;
    let mut has_bluetooth = false;
    let mut has_ssh = false;
    let mut has_zram = false;

    for (path, mode, content) in snapshot.files {
        if path.starts_with("etc/NetworkManager/system-connections/") {
            has_nm = true;
            data.system_files.push(config::SystemFile {
                path,
                mode: 0o600,
                file: None,
                content: Some(content),
            });
        } else if path.starts_with("var/lib/iwd/") {
            if path.ends_with(".psk") || path.ends_with(".open") || path.ends_with(".8021x") {
                has_iwd = true;
                data.system_files.push(config::SystemFile {
                    path,
                    mode: 0o600,
                    file: None,
                    content: Some(content),
                });
            }
        } else if path.starts_with("var/lib/fprint/") {
            has_fprint = true;
            data.system_files.push(config::SystemFile {
                path,
                mode: 0o600,
                file: None,
                content: Some(content),
            });
        } else if path.starts_with("var/lib/bluetooth/") {
            has_bluetooth = true;
            data.system_files.push(config::SystemFile {
                path,
                mode: (mode & 0o7777) as u16,
                file: None,
                content: Some(content),
            });
        } else if path.starts_with("etc/ssh/ssh_host_") {
            has_ssh = true;
            let key_mode = if path.ends_with(".pub") { 0o644 } else { 0o600 };
            data.system_files.push(config::SystemFile {
                path,
                mode: key_mode,
                file: None,
                content: Some(content),
            });
        } else if path == "etc/systemd/zram-generator.conf" {
            has_zram = true;
            data.system_files.push(config::SystemFile {
                path,
                mode: (mode & 0o7777) as u16,
                file: None,
                content: Some(content),
            });
        }
    }

    if has_nm {
        data.extra_packages.push("networkmanager".into());
        data.extra_services.push("NetworkManager.service".into());
    }
    if has_iwd {
        data.extra_packages.push("iwd".into());
        data.extra_services.push("iwd.service".into());
    }
    if has_bluetooth {
        data.extra_packages.push("bluez".into());
        data.extra_packages.push("bluez-utils".into());
        data.extra_services.push("bluetooth.service".into());
    }
    if has_ssh {
        data.extra_packages.push("openssh".into());
        if std::path::Path::new("/etc/systemd/system/multi-user.target.wants/sshd.service").exists()
        {
            data.extra_services.push("sshd.service".into());
        }
    }
    if has_zram {
        data.extra_packages.push("zram-generator".into());
    }

    // 3. Unprivileged PAM settings check (for fprintd / pam-fprint-grosshack)
    let fprint_unpriv = crate::fprint::discover_host_fprint(None);
    if fprint_unpriv.enabled || has_fprint {
        for f in fprint_unpriv.files {
            if !data.system_files.iter().any(|sf| sf.path == f.path) {
                data.system_files.push(f);
            }
        }
        for pkg in fprint_unpriv.packages {
            if !data.extra_packages.contains(&pkg) {
                data.extra_packages.push(pkg);
            }
        }
        if !data.extra_packages.contains(&"fprintd".to_string()) {
            data.extra_packages.push("fprintd".into());
        }
    }

    Ok(data)
}

/// Autoloads the running host's current configuration into a [`config::Config`].
pub fn autoload_live_system(cfg: &mut config::Config) {
    autoload_live_system_authenticated(cfg, None);
}

/// Autoloads the running host's current configuration with optional root/sudo password for protected files.
pub fn autoload_live_system_authenticated(cfg: &mut config::Config, pw: Option<&str>) {
    // 1. System identity and localization
    let sys = system::discover_host_system();
    cfg.hostname = sys.hostname;
    cfg.timezone = sys.timezone;
    cfg.locale = sys.locale;
    cfg.keymap = sys.keymap;

    // 2. Explicit native packages from pacman -Qqen
    if let Ok(out) = std::process::Command::new("pacman").arg("-Qqen").output() {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                if !cfg.packages.iter().any(|p| p == line) {
                    cfg.packages.push(line.to_string());
                }
            }
        }
    }

    // 3. Enabled services and custom unit files
    let discovered_services = services::discover_local_services();
    for s in discovered_services.system_package_services {
        if !cfg.services.contains(&s) {
            cfg.services.push(s);
        }
    }
    for cs in discovered_services.custom_system_services {
        if !cfg
            .custom_services
            .iter()
            .any(|existing| existing.name == cs.name && existing.user == cs.user)
        {
            cfg.custom_services.push(cs);
        }
    }

    // 4. Users from /etc/passwd and /etc/group
    let host_users = users::discover_host_users();
    for u in &host_users {
        if let Some(existing) = cfg.users.iter_mut().find(|e| e.name == u.name) {
            existing.groups = u.groups.clone();
            existing.shell = u.shell.clone();
            if let Some(hash) = &u.password_hash {
                existing.password_hash = hash.clone();
            }
        } else {
            let hash = u.password_hash.clone().unwrap_or_else(|| {
                // Placeholder password hash that passes validation until shadow is read
                "$6$archstlr$gnUg60P8TrWaKT.8/oW8Iq1kc8LlPnk4A6UWw5ij0AAQNe2IN7TuPFnw7PmXMXuobrVIYBSbNWUgRbpafXqcc.".into()
            });
            cfg.users.push(config::User {
                name: u.name.clone(),
                password_hash: hash,
                groups: u.groups.clone(),
                shell: u.shell.clone(),
                dotfiles: None,
            });
        }
    }
    if let Ok(curr) = std::env::var("USER") {
        if curr != "kubik" && host_users.iter().any(|u| u.name == curr) {
            cfg.users.retain(|u| u.name != "kubik" || u.name == curr);
        }
    }

    // 5. If privileged password provided, run unified privileged discovery
    if let Some(pw) = pw {
        if let Ok(priv_data) = discover_live_system_privileged(Some(pw), false) {
            if let Some(shadow) = &priv_data.shadow {
                users::update_users_from_shadow(&mut cfg.users, shadow);
            }
            if let Some(root_h) = priv_data.root_password_hash {
                cfg.root_password_hash = Some(root_h);
            }
            for sf in priv_data.system_files {
                if !cfg.system_files.iter().any(|existing| existing.path == sf.path) {
                    cfg.system_files.push(sf);
                }
            }
            for p in priv_data.extra_packages {
                if !cfg.packages.contains(&p) {
                    cfg.packages.push(p);
                }
            }
            for s in priv_data.extra_services {
                if !cfg.services.contains(&s) {
                    cfg.services.push(s);
                }
            }
            return;
        }
    }

    // 6. Fallback unprivileged discovery for network & fingerprint & zram
    let host_net = crate::network::discover_host_network(None);
    for cf in host_net.connection_files {
        if !cfg.system_files.iter().any(|existing| existing.path == cf.path) {
            cfg.system_files.push(cf);
        }
    }
    if let Some(pkg) = host_net.package {
        if !cfg.packages.iter().any(|p| p == &pkg) {
            cfg.packages.push(pkg);
        }
    }
    if let Some(svc) = host_net.service {
        if !cfg.services.iter().any(|s| s == &svc) {
            cfg.services.push(svc);
        }
    }

    let host_fprint = crate::fprint::discover_host_fprint(None);
    if host_fprint.enabled {
        for f in host_fprint.files {
            if !cfg.system_files.iter().any(|existing| existing.path == f.path) {
                cfg.system_files.push(f);
            }
        }
        for pkg in host_fprint.packages {
            if !cfg.packages.iter().any(|p| p == &pkg) {
                cfg.packages.push(pkg);
            }
        }
    }

    let host_bt = crate::bluetooth::discover_host_bluetooth(None);
    if host_bt.enabled {
        for f in host_bt.files {
            if !cfg.system_files.iter().any(|existing| existing.path == f.path) {
                cfg.system_files.push(f);
            }
        }
        for pkg in host_bt.packages {
            if !cfg.packages.iter().any(|p| p == &pkg) {
                cfg.packages.push(pkg);
            }
        }
        for svc in host_bt.services {
            if !cfg.services.iter().any(|s| s == &svc) {
                cfg.services.push(svc);
            }
        }
    }

    let host_ssh = crate::ssh::discover_host_ssh(None);
    if host_ssh.enabled {
        for f in host_ssh.files {
            if !cfg.system_files.iter().any(|existing| existing.path == f.path) {
                cfg.system_files.push(f);
            }
        }
        for pkg in host_ssh.packages {
            if !cfg.packages.iter().any(|p| p == &pkg) {
                cfg.packages.push(pkg);
            }
        }
        for svc in host_ssh.services {
            if !cfg.services.iter().any(|s| s == &svc) {
                cfg.services.push(svc);
            }
        }
    }

    if let Ok(zram_conf) = std::fs::read("/etc/systemd/zram-generator.conf") {
        if !cfg.system_files.iter().any(|f| f.path == "etc/systemd/zram-generator.conf") {
            cfg.system_files.push(config::SystemFile {
                path: "etc/systemd/zram-generator.conf".into(),
                mode: 0o644,
                file: None,
                content: Some(zram_conf),
            });
            if !cfg.packages.iter().any(|p| p == "zram-generator") {
                cfg.packages.push("zram-generator".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autoload_preserves_valid_config() {
        let mut cfg = config::Config {
            hostname: "original".into(),
            timezone: "UTC".into(),
            locale: "en_US.UTF-8".into(),
            keymap: "us".into(),
            disk: config::Disk {
                model: None,
                confirm_serial: "SERIAL123".into(),
                auto_largest: false,
                esp_mib: 1024,
                filesystem: config::FsType::Ext4,
                btrfs: None,
            },
            mirrors: vec!["https://example.com".into()],
            packages: vec!["base".into()],
            providers: vec![],
            root_password_hash: None,
            users: vec![],
            services: vec![],
            kernel_params: vec![],
            user_files: vec![],
            user_archives: vec![],
            system_files: vec![],
            dry_run: false,
            scripts: vec![],
            custom_services: vec![],
            aur: vec![],
        };

        autoload_live_system(&mut cfg);
        assert!(!cfg.hostname.is_empty());
        assert!(!cfg.timezone.is_empty());
        assert!(!cfg.locale.is_empty());
        assert!(!cfg.keymap.is_empty());
    }
}
