//! System file resolution: loads local system files from disk into `content` at ISO build time.

use crate::Result;
use config::Config;
use std::fs;
use std::path::{Path, PathBuf};

/// The largest individual system file we embed (16 MiB).
pub const SYSTEM_FILE_MAX: usize = 16 * 1024 * 1024;

/// Resolves local system files, reading file paths into `content`.
pub fn resolve(cfg: &Config, base: &Path) -> Result<Config> {
    let mut out = cfg.clone();
    for (i, sf) in out.system_files.iter_mut().enumerate() {
        if let Some(f) = &sf.file {
            let path = if Path::new(f).is_absolute() {
                PathBuf::from(f)
            } else {
                base.join(f)
            };
            let data = fs::read(&path).map_err(|e| {
                format!("system_files[{}]: cannot read {}: {e}", i + 1, path.display())
            })?;
            if data.len() > SYSTEM_FILE_MAX {
                return Err(format!(
                    "system_files[{}]: {} is larger than 16 MiB",
                    i + 1,
                    path.display()
                )
                .into());
            }
            sf.content = Some(data);
            sf.file = None;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::SystemFile;

    #[test]
    fn resolves_local_system_file() {
        let dir = std::env::temp_dir().join(format!("archstaller-sysfile-test-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("test.conf");
        fs::write(&file_path, b"key=value\n").unwrap();

        let cfg = Config {
            hostname: "box".into(),
            timezone: "UTC".into(),
            locale: "en_US.UTF-8".into(),
            keymap: "us".into(),
            disk: config::Disk {
                model: None,
                confirm_serial: "S".into(),
                auto_largest: false,
                esp_mib: 512,
            },
            mirrors: vec![],
            packages: vec![],
            providers: vec![],
            root_password_hash: None,
            users: vec![],
            services: vec![],
            kernel_params: vec![],
            user_files: vec![],
            user_archives: vec![],
            system_files: vec![SystemFile {
                path: "etc/test.conf".into(),
                mode: 0o644,
                file: Some("test.conf".into()),
                content: None,
            }],
            dry_run: false,
            scripts: vec![],
            custom_services: vec![],
            aur: vec![],
        };

        let resolved = resolve(&cfg, &dir).unwrap();
        assert_eq!(
            resolved.system_files[0].content,
            Some(b"key=value\n".to_vec())
        );
        assert_eq!(resolved.system_files[0].file, None);
        drop(std::fs::remove_dir_all(&dir));
    }
}
