//! User archive resolution: loads local archive files from host or USB drive into `content` at ISO build time.

use crate::Result;
use config::{is_supported_archive, Config, USER_ARCHIVE_MAX};
use std::fs;
use std::path::{Path, PathBuf};

/// Resolves local archive files, reading file paths into `content` for embedding in the ISO payload.
pub fn resolve(cfg: &Config, base: &Path) -> Result<Config> {
    let mut out = cfg.clone();
    for (i, a) in out.user_archives.iter_mut().enumerate() {
        if let Some(f) = &a.file {
            let path = if Path::new(f).is_absolute() {
                PathBuf::from(f)
            } else {
                base.join(f)
            };
            let data = fs::read(&path).map_err(|e| {
                format!("user_archives[{}]: cannot read {}: {e}", i + 1, path.display())
            })?;
            if data.len() > USER_ARCHIVE_MAX {
                return Err(format!(
                    "user_archives[{}]: {} is larger than 256 MiB ({} bytes)",
                    i + 1,
                    path.display(),
                    data.len()
                )
                .into());
            }
            if !is_supported_archive(&data) {
                return Err(format!(
                    "user_archives[{}]: {} is not a supported archive format (.tar.zst, .tar.xz, .tar.gz, .zip)",
                    i + 1,
                    path.display()
                )
                .into());
            }
            a.content = Some(data);
            a.file = None;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::UserArchive;

    #[test]
    fn resolves_local_zstd_archive() {
        let dir = std::env::temp_dir().join(format!("archstaller-arch-test-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        let archive_path = dir.join("dotfiles.tar.zst");
        // Magic bytes for Zstandard: 0x28, 0xb5, 0x2f, 0xfd
        let mut sample = vec![0x28, 0xb5, 0x2f, 0xfd];
        sample.extend_from_slice(b"fake-zstd-content");
        fs::write(&archive_path, &sample).unwrap();

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
                filesystem: config::FsType::Ext4,
                btrfs: None,
            },
            mirrors: vec![],
            packages: vec![],
            providers: vec![],
            root_password_hash: None,
            users: vec![],
            services: vec![],
            kernel_params: vec![],
            user_files: vec![],
            user_archives: vec![UserArchive {
                file: Some("dotfiles.tar.zst".into()),
                ..Default::default()
            }],
            system_files: vec![],
            dry_run: false,
            scripts: vec![],
            custom_services: vec![],
            aur: vec![],
        };

        let resolved = resolve(&cfg, &dir).unwrap();
        assert_eq!(resolved.user_archives[0].content, Some(sample));
        assert_eq!(resolved.user_archives[0].file, None);
        drop(std::fs::remove_dir_all(&dir));
    }
}
