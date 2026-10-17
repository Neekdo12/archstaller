//! Autoloading live system state (system identity, explicit packages, services, users) from the running host.

use crate::services;
use crate::system;
use crate::users;

/// Autoloads the running host's current configuration into a [`config::Config`].
pub fn autoload_live_system(cfg: &mut config::Config) {
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
    for u in host_users {
        if let Some(existing) = cfg.users.iter_mut().find(|e| e.name == u.name) {
            existing.groups = u.groups;
            existing.shell = u.shell;
            if let Some(hash) = u.password_hash {
                existing.password_hash = hash;
            }
        } else if let Some(hash) = u.password_hash {
            cfg.users.push(config::User {
                name: u.name,
                password_hash: hash,
                groups: u.groups,
                shell: u.shell,
            });
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
            dry_run: false,
            scripts: vec![],
            custom_services: vec![],
            aur: vec![],
        };

        autoload_live_system(&mut cfg);
        // Should have updated hostname to at least non-empty
        assert!(!cfg.hostname.is_empty());
        assert!(!cfg.timezone.is_empty());
        assert!(!cfg.locale.is_empty());
        assert!(!cfg.keymap.is_empty());
    }
}
