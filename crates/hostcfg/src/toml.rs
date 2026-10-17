//! Evaluating and loading a TOML configuration.
use crate::asconfig::AsConfig;
use crate::host::HostConfig;
use crate::Result;
use std::path::Path;

/// Everything a TOML config says: what the installer gets and how the ISO is built.
#[derive(Debug)]
pub struct Loaded {
    pub config: config::Config,
    pub host: HostConfig,
    /// The directory of the config file (local script paths are relative to it).
    pub dir: std::path::PathBuf,
    /// Things worth telling the user that do not stop the build.
    pub warnings: Vec<String>,
}

#[cfg(test)]
pub mod tests_support {
    pub fn minimal_config() -> config::Config {
        config::Config {
            hostname: "testbox".into(),
            timezone: "Europe/Prague".into(),
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
        }
    }
}

/// Evaluates a TOML config and validates both halves.
pub fn load(path: &Path) -> Result<Loaded> {
    let loaded = load_unvalidated(path)?;
    loaded.config.validate()?;
    loaded.host.validate()?;
    crate::scripts::validate(&loaded.config)?;
    Ok(loaded)
}

/// Evaluates a TOML config with the strict shape and type checks but without the domain validation, for an
/// editor that wants to show an incomplete config and let its forms point at what is missing.
pub fn load_unvalidated(path: &Path) -> Result<Loaded> {
    let path = path.canonicalize()?;
    let path = path.as_path();
    let src = std::fs::read_to_string(path)?;
    load_str_unvalidated(&src, path.parent().unwrap_or(Path::new(".")))
}

/// Evaluates a TOML string against the schema and converts it into (Config, HostConfig).
pub fn load_str_unvalidated(src: &str, dir: &Path) -> Result<Loaded> {
    let as_config = AsConfig::from_toml(src).map_err(|e| format!("{}: {e}", dir.display()))?;
    let (config, host) = as_config
        .into_parts()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(Loaded {
        config,
        host,
        dir: dir.to_path_buf(),
        warnings: Vec::new(),
    })
}

/// Evaluates and validates a TOML config, returning only the installer's part.
pub fn load_config(path: &Path) -> Result<config::Config> {
    Ok(load(path)?.config)
}

/// The installer's config, with `extra_kernel_params` appended, serialized for `config.bin`.
pub fn config_bin(loaded: &Loaded, extra_kernel_params: &[String]) -> Result<Vec<u8>> {
    let cfg = crate::scripts::resolve(&loaded.config, &loaded.dir)?;
    let mut cfg = crate::services::resolve(&cfg, &loaded.dir)?;
    cfg.kernel_params
        .extend(extra_kernel_params.iter().cloned());
    cfg.validate()?;
    Ok(postcard::to_stdvec(&cfg)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_toml_config() {
        let toml_src = r#"
schema = 1

[system]
hostname = "testbox"
timezone = "Europe/Prague"
locale = "en_US.UTF-8"
keymap = "us"

[install]
mirrors = ["https://geo.mirror.pkgbuild.com/$repo/os/$arch"]

[install.disk]
auto_largest = true
esp_mib = 1024

[packages]
explicit = ["base", "linux"]
"#;
        let loaded = load_str_unvalidated(toml_src, Path::new(".")).unwrap();
        assert_eq!(loaded.config.hostname, "testbox");
        assert_eq!(loaded.config.timezone, "Europe/Prague");
        assert!(loaded.config.disk.auto_largest);
        assert_eq!(loaded.config.packages, vec!["base", "linux"]);
    }

    #[test]
    fn rejects_unknown_keys() {
        let toml_src = r#"
schema = 1
bogus_key = 123

[system]
hostname = "testbox"
timezone = "Europe/Prague"
locale = "en_US.UTF-8"
keymap = "us"

[install]
mirrors = ["https://geo.mirror.pkgbuild.com/$repo/os/$arch"]

[install.disk]
auto_largest = true
esp_mib = 1024

[packages]
explicit = ["base"]
"#;
        let err = load_str_unvalidated(toml_src, Path::new(".")).unwrap_err();
        assert!(err.to_string().contains("unknown field `bogus_key`"));
    }

    #[test]
    fn parses_custom_services_in_toml() {
        let toml_src = r#"
schema = 1

[system]
hostname = "testbox"
timezone = "Europe/Prague"
locale = "en_US.UTF-8"
keymap = "us"

[install]
mirrors = ["https://geo.mirror.pkgbuild.com/$repo/os/$arch"]

[install.disk]
auto_largest = true
esp_mib = 1024

[packages]
explicit = ["base"]

[[first_boot.custom_services]]
name = "my-service.service"
user = "root"
enable = true
content = """
[Unit]
Description=Test
"""

[[first_boot.custom_services]]
name = "user-daemon.service"
user = "kubik"
enable = false
file = "services/user-daemon.service"
"#;
        let loaded = load_str_unvalidated(toml_src, Path::new(".")).unwrap();
        assert_eq!(loaded.config.custom_services.len(), 2);
        assert_eq!(loaded.config.custom_services[0].name, "my-service.service");
        assert_eq!(loaded.config.custom_services[0].user, Some("root".into()));
        assert!(loaded.config.custom_services[0].enable);
        assert_eq!(
            loaded.config.custom_services[0].content,
            Some(b"[Unit]\nDescription=Test\n".to_vec())
        );

        assert_eq!(loaded.config.custom_services[1].name, "user-daemon.service");
        assert_eq!(loaded.config.custom_services[1].user, Some("kubik".into()));
        assert!(!loaded.config.custom_services[1].enable);
        assert_eq!(
            loaded.config.custom_services[1].file,
            Some("services/user-daemon.service".into())
        );
    }
}
