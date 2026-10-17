//! Custom systemd services: local file resolution and discovery of services on the host machine.
use crate::Result;
use config::{Config, CustomService, CUSTOM_SERVICE_MAX};
use std::fs;
use std::path::Path;

/// The config as the installer receives it: local service files carry their text in `content`.
/// `base` is the directory local service paths are relative to.
pub fn resolve(cfg: &Config, base: &Path) -> Result<Config> {
    let mut out = cfg.clone();
    for (i, cs) in out.custom_services.iter_mut().enumerate() {
        if let Some(f) = &cs.file {
            let path = base.join(f);
            let data = fs::read(&path).map_err(|e| {
                format!(
                    "custom_services[{}]: cannot read {}: {e}",
                    i + 1,
                    path.display()
                )
            })?;
            if data.len() > CUSTOM_SERVICE_MAX {
                return Err(format!(
                    "custom_services[{}]: {} is larger than 64 KiB",
                    i + 1,
                    path.display()
                )
                .into());
            }
            cs.content = Some(data);
            cs.file = None;
        }
    }
    Ok(out)
}

/// Services discovered on the host system.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveredServices {
    /// Package-provided system services enabled on the host (e.g. "NetworkManager.service").
    pub system_package_services: Vec<String>,
    /// Custom system units found in `/etc/systemd/system/` (regular unit files with content).
    pub custom_system_services: Vec<CustomService>,
    /// Custom user units found in `~/.config/systemd/user/` (regular unit files with content).
    pub custom_user_services: Vec<CustomService>,
}

/// Discovers enabled package services and custom unit files on the running system.
pub fn discover_local_services() -> DiscoveredServices {
    let mut discovered = DiscoveredServices::default();

    // 1. Discover enabled system services using `systemctl list-unit-files`.
    if let Ok(output) = std::process::Command::new("systemctl")
        .args([
            "list-unit-files",
            "--state=enabled",
            "--type=service",
            "--no-legend",
            "--no-pager",
        ])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                let unit = match line.split_whitespace().next() {
                    Some(u) => u.trim(),
                    None => continue,
                };
                if unit.is_empty() || unit.ends_with("@.service") {
                    continue;
                }
                // Check if this unit is a custom unit file in /etc/systemd/system/
                let etc_path = Path::new("/etc/systemd/system").join(unit);
                if is_regular_file(&etc_path) {
                    if let Ok(content) = fs::read(&etc_path) {
                        if content.len() <= CUSTOM_SERVICE_MAX {
                            discovered.custom_system_services.push(CustomService {
                                name: unit.to_string(),
                                user: Some("root".into()),
                                enable: true,
                                file: None,
                                content: Some(content),
                            });
                            continue;
                        }
                    }
                }
                if !discovered.system_package_services.iter().any(|s| s == unit) {
                    discovered.system_package_services.push(unit.to_string());
                }
            }
        }
    }

    // 2. Scan /etc/systemd/system/ for any other custom .service files not yet found.
    if let Ok(entries) = fs::read_dir("/etc/systemd/system") {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_regular_file(&path) {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".service")
                    && !discovered
                        .custom_system_services
                        .iter()
                        .any(|s| s.name == name)
                {
                    if let Ok(content) = fs::read(&path) {
                        if content.len() <= CUSTOM_SERVICE_MAX {
                            // Check if enabled (e.g. symlink in /etc/systemd/system/*.wants/<name>)
                            let enabled = is_system_unit_enabled(&name);
                            discovered.custom_system_services.push(CustomService {
                                name,
                                user: Some("root".into()),
                                enable: enabled,
                                file: None,
                                content: Some(content),
                            });
                        }
                    }
                }
            }
        }
    }

    // 3. Scan ~/.config/systemd/user/ for custom user unit files.
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        let user_unit_dir = home.join(".config/systemd/user");
        if let Ok(entries) = fs::read_dir(&user_unit_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if is_regular_file(&path) {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if config::valid_unit_name(&name) {
                        if let Ok(content) = fs::read(&path) {
                            if content.len() <= CUSTOM_SERVICE_MAX {
                                let enabled = is_user_unit_enabled(&user_unit_dir, &name);
                                discovered.custom_user_services.push(CustomService {
                                    name,
                                    user: None, // Caller assigns target user
                                    enable: enabled,
                                    file: None,
                                    content: Some(content),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    discovered
}

fn is_regular_file(path: &Path) -> bool {
    // metadata() follows symlinks; symlink_metadata() checks the file itself.
    if let Ok(meta) = fs::symlink_metadata(path) {
        meta.is_file() && !meta.file_type().is_symlink()
    } else {
        false
    }
}

fn is_system_unit_enabled(name: &str) -> bool {
    let etc = Path::new("/etc/systemd/system");
    if let Ok(entries) = fs::read_dir(etc) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir()
                && p.extension().is_some_and(|ext| ext == "wants")
                && p.join(name).exists()
            {
                return true;
            }
        }
    }
    false
}

fn is_user_unit_enabled(user_unit_dir: &Path, name: &str) -> bool {
    if let Ok(entries) = fs::read_dir(user_unit_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir()
                && p.extension().is_some_and(|ext| ext == "wants")
                && p.join(name).exists()
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_loads_file_content() {
        let temp_dir = std::env::temp_dir().join("archstaller_test_svc");
        drop(fs::remove_dir_all(&temp_dir));
        fs::create_dir_all(&temp_dir).unwrap();
        let svc_file = temp_dir.join("test.service");
        fs::write(&svc_file, "[Unit]\nDescription=Test\n").unwrap();

        let cfg = Config {
            hostname: "box".into(),
            timezone: "Europe/Prague".into(),
            locale: "en_US.UTF-8".into(),
            keymap: "us".into(),
            disk: config::Disk {
                model: None,
                confirm_serial: "S".into(),
                auto_largest: false,
                esp_mib: 512,
            },
            mirrors: vec!["https://example.org".into()],
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
            custom_services: vec![CustomService {
                name: "test.service".into(),
                user: None,
                enable: true,
                file: Some("test.service".into()),
                content: None,
            }],
            aur: vec![],
        };

        let resolved = resolve(&cfg, &temp_dir).unwrap();
        assert_eq!(resolved.custom_services[0].file, None);
        assert_eq!(
            resolved.custom_services[0].content,
            Some(b"[Unit]\nDescription=Test\n".to_vec())
        );

        drop(fs::remove_dir_all(&temp_dir));
    }
}
