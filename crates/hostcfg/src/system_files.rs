//! System file resolution: loads local system files from disk into `content` at ISO build time.

use crate::Result;
use config::Config;
use std::fs;
use std::path::{Path, PathBuf};

/// The largest individual system file we embed (16 MiB).
pub const SYSTEM_FILE_MAX: usize = 16 * 1024 * 1024;

fn collect_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files_recursive(&path, out)?;
        } else if file_type.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// Resolves local system files, reading file paths into `content`.
/// If a configured path points to a directory, it is recursively traversed and embedded as individual files.
pub fn resolve(cfg: &Config, base: &Path) -> Result<Config> {
    let mut out = cfg.clone();
    let mut resolved_files = Vec::new();

    for (i, sf) in cfg.system_files.iter().enumerate() {
        if let Some(f) = &sf.file {
            let path = if Path::new(f).is_absolute() {
                PathBuf::from(f)
            } else {
                base.join(f)
            };
            if !path.exists() {
                return Err(format!(
                    "system_files[{}]: cannot read {}: No such file or directory",
                    i + 1,
                    path.display()
                )
                .into());
            }
            if path.is_dir() {
                let mut files = Vec::new();
                collect_files_recursive(&path, &mut files).map_err(|e| {
                    format!("system_files[{}]: cannot scan {}: {e}", i + 1, path.display())
                })?;
                files.sort();
                for file_path in files {
                    let rel = file_path.strip_prefix(&path).map_err(|e| {
                        format!("system_files[{}]: invalid subpath: {e}", i + 1)
                    })?;
                    let target_sub = rel.to_string_lossy();
                    let target_path = if sf.path.is_empty() || sf.path == "." {
                        target_sub.trim_start_matches('/').to_string()
                    } else {
                        format!("{}/{}", sf.path.trim_end_matches('/'), target_sub.trim_start_matches('/'))
                    };
                    let data = fs::read(&file_path).map_err(|e| {
                        format!("system_files[{}]: cannot read {}: {e}", i + 1, file_path.display())
                    })?;
                    if data.len() > SYSTEM_FILE_MAX {
                        return Err(format!(
                            "system_files[{}]: {} is larger than 16 MiB",
                            i + 1,
                            file_path.display()
                        )
                        .into());
                    }
                    #[cfg(unix)]
                    let mode = {
                        use std::os::unix::fs::PermissionsExt;
                        if let Ok(meta) = fs::metadata(&file_path) {
                            let perm = meta.permissions().mode();
                            if perm & 0o111 != 0 {
                                0o755
                            } else {
                                0o644
                            }
                        } else {
                            sf.mode
                        }
                    };
                    #[cfg(not(unix))]
                    let mode = sf.mode;

                    resolved_files.push(config::SystemFile {
                        path: target_path,
                        mode,
                        file: None,
                        content: Some(data),
                    });
                }
            } else {
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
                let mut resolved = sf.clone();
                resolved.content = Some(data);
                resolved.file = None;
                resolved_files.push(resolved);
            }
        } else {
            resolved_files.push(sf.clone());
        }
    }
    out.system_files = resolved_files;
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

    #[test]
    fn resolves_directory_system_file_recursively() {
        let dir = std::env::temp_dir().join(format!("archstaller-sysdir-test-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        let overlay_dir = dir.join("my_overlay");
        let sub_dir = overlay_dir.join("etc").join("systemd").join("system");
        std::fs::create_dir_all(&sub_dir).unwrap();
        fs::write(sub_dir.join("foo.service"), b"[Unit]\nDescription=Foo\n").unwrap();
        let bin_dir = overlay_dir.join("usr").join("local").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let script_path = bin_dir.join("foo.sh");
        fs::write(&script_path, b"#!/bin/sh\necho hi\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

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
            user_archives: vec![],
            system_files: vec![SystemFile {
                path: "".into(),
                mode: 0o644,
                file: Some("my_overlay".into()),
                content: None,
            }],
            dry_run: false,
            scripts: vec![],
            custom_services: vec![],
            aur: vec![],
        };

        let resolved = resolve(&cfg, &dir).unwrap();
        assert_eq!(resolved.system_files.len(), 2);
        let s_foo = resolved.system_files.iter().find(|s| s.path == "etc/systemd/system/foo.service").unwrap();
        assert_eq!(s_foo.content, Some(b"[Unit]\nDescription=Foo\n".to_vec()));
        assert_eq!(s_foo.mode, 0o644);

        let s_script = resolved.system_files.iter().find(|s| s.path == "usr/local/bin/foo.sh").unwrap();
        assert_eq!(s_script.content, Some(b"#!/bin/sh\necho hi\n".to_vec()));
        #[cfg(unix)]
        assert_eq!(s_script.mode, 0o755);

        drop(std::fs::remove_dir_all(&dir));
    }
}
