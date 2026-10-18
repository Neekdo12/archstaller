//! The configuration authoring model for TOML configs.
//!
//! These types describe the shape of the TOML file. They reject unknown keys and wrong value types
//! while deserializing, then [`AsConfig::into_parts`] normalizes them into the installer's
//! [`config::Config`] and the host-only [`HostConfig`]; the values are validated there, by the same
//! code as before. [`AsConfig::from_parts`] goes the other way for the writer. The JSON Schema
//! in `configs/archstaller.schema.json` is generated from these types (`cargo xtask gen-schema`).
use crate::host::{Build, HostConfig, DRIVERS};
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The schema version this build reads and writes.
pub const SCHEMA: u32 = 1;

/// An Archstaller configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AsConfig {
    /// Schema version; must be 1.
    pub schema: u32,
    /// Identity and locale of the installed system.
    pub system: System,
    /// How and where the installer writes the system.
    pub install: Install,
    /// What gets installed.
    pub packages: Packages,
    /// Login accounts of the installed system.
    #[serde(default)]
    pub users: Vec<User>,
    /// What runs on the installed system's first boot.
    #[serde(default)]
    pub first_boot: FirstBoot,
    /// Build-host options: how the ISO is produced. Not part of what the installer receives.
    #[serde(default)]
    pub build: AsBuild,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct System {
    /// Host name, for example `archbox`.
    pub hostname: String,
    /// IANA time zone, for example `Europe/Prague`.
    pub timezone: String,
    /// Locale, for example `en_US.UTF-8`.
    pub locale: String,
    /// Console keymap, for example `us`.
    pub keymap: String,
    /// SHA-512 crypt hash (`$6$...`) of root's password. Leave it out to keep root locked.
    #[serde(default)]
    pub root_password_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Install {
    /// Which disk the installer erases.
    pub disk: Disk,
    /// Mirror base URLs; `$repo` and `$arch` are substituted (pacman mirrorlist style).
    pub mirrors: Vec<String>,
    /// Hardware test mode: probe, read disks (never write), resolve packages, print a report, reboot.
    #[serde(default)]
    pub dry_run: bool,
    /// Bootloader to install: "grub", "systemd-boot", or "limine". Defaults to "grub".
    #[serde(default)]
    pub bootloader: Bootloader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Bootloader {
    #[default]
    Grub,
    SystemdBoot,
    Limine,
}

impl From<Bootloader> for config::Bootloader {
    fn from(b: Bootloader) -> Self {
        match b {
            Bootloader::Grub => config::Bootloader::Grub,
            Bootloader::SystemdBoot => config::Bootloader::SystemdBoot,
            Bootloader::Limine => config::Bootloader::Limine,
        }
    }
}

impl From<config::Bootloader> for Bootloader {
    fn from(b: config::Bootloader) -> Self {
        match b {
            config::Bootloader::Grub => Bootloader::Grub,
            config::Bootloader::SystemdBoot => Bootloader::SystemdBoot,
            config::Bootloader::Limine => Bootloader::Limine,
        }
    }
}


#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Disk {
    /// Substring of the disk model; must select exactly one disk. Without it the serial alone selects the disk.
    #[serde(default)]
    pub model: Option<String>,
    /// Serial of the one disk that may be erased. Not used with `auto_largest`.
    #[serde(default)]
    pub confirm_serial: String,
    /// WARNING: erase and install onto the largest disk without any confirmation.
    #[serde(default)]
    pub auto_largest: bool,
    /// Size of the EFI system partition in MiB.
    pub esp_mib: u32,
    /// Root filesystem type (ext4, btrfs, xfs, f2fs). Defaults to ext4.
    #[serde(default)]
    pub filesystem: FsType,
    /// Btrfs options (subvolumes, compression) when filesystem is btrfs.
    #[serde(default)]
    pub btrfs: Option<BtrfsConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FsType {
    #[default]
    Ext4,
    Btrfs,
    Xfs,
    F2fs,
}

impl From<FsType> for config::FsType {
    fn from(f: FsType) -> Self {
        match f {
            FsType::Ext4 => config::FsType::Ext4,
            FsType::Btrfs => config::FsType::Btrfs,
            FsType::Xfs => config::FsType::Xfs,
            FsType::F2fs => config::FsType::F2fs,
        }
    }
}

impl From<config::FsType> for FsType {
    fn from(f: config::FsType) -> Self {
        match f {
            config::FsType::Ext4 => FsType::Ext4,
            config::FsType::Btrfs => FsType::Btrfs,
            config::FsType::Xfs => FsType::Xfs,
            config::FsType::F2fs => FsType::F2fs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BtrfsSubvol {
    pub name: String,
    pub mount: String,
}

impl From<BtrfsSubvol> for config::BtrfsSubvol {
    fn from(s: BtrfsSubvol) -> Self {
        config::BtrfsSubvol {
            name: s.name,
            mount: s.mount,
        }
    }
}

impl From<&config::BtrfsSubvol> for BtrfsSubvol {
    fn from(s: &config::BtrfsSubvol) -> Self {
        BtrfsSubvol {
            name: s.name.clone(),
            mount: s.mount.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BtrfsConfig {
    #[serde(default = "default_btrfs_subvolumes")]
    pub subvolumes: Vec<BtrfsSubvol>,
    #[serde(default = "config::default_zstd")]
    pub compress: String,
}

fn default_btrfs_subvolumes() -> Vec<BtrfsSubvol> {
    config::default_btrfs_subvolumes()
        .iter()
        .map(Into::into)
        .collect()
}

impl From<BtrfsConfig> for config::BtrfsConfig {
    fn from(b: BtrfsConfig) -> Self {
        config::BtrfsConfig {
            subvolumes: b.subvolumes.into_iter().map(Into::into).collect(),
            compress: b.compress,
        }
    }
}

impl From<&config::BtrfsConfig> for BtrfsConfig {
    fn from(b: &config::BtrfsConfig) -> Self {
        BtrfsConfig {
            subvolumes: b.subvolumes.iter().map(Into::into).collect(),
            compress: b.compress.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Packages {
    /// Package or group names installed explicitly. Names are suggestions: the resolver decides.
    pub explicit: Vec<String>,
    /// Dependency name to the package that provides it, for example `{ initramfs = "mkinitcpio" }`.
    #[serde(default)]
    pub providers: BTreeMap<String, String>,
    /// AUR packages, in build order, pinned to the reviewed recipe (see `plans-implement/aur.md`).
    #[serde(default)]
    pub aur: Vec<Aur>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserDotfiles {
    /// GitHub repository: "owner/repo" or "https://github.com/owner/repo".
    pub repo: String,
    /// Target directory inside user's home (default: "dotfiles").
    #[serde(default = "config::default_dotfiles_dir")]
    pub dir: String,
    /// Git branch or tag (default: "main").
    #[serde(default)]
    pub branch: Option<String>,
    /// Whether to run GNU Stow to symlink configuration files into $HOME.
    #[serde(default = "config::default_true")]
    pub stow: bool,
    /// Specific package subdirectories to stow (empty = stow all or top-level).
    #[serde(default)]
    pub packages: Vec<String>,
}

impl From<UserDotfiles> for config::UserDotfiles {
    fn from(d: UserDotfiles) -> Self {
        config::UserDotfiles {
            repo: d.repo,
            dir: d.dir,
            branch: d.branch,
            stow: d.stow,
            packages: d.packages,
        }
    }
}

impl From<&config::UserDotfiles> for UserDotfiles {
    fn from(d: &config::UserDotfiles) -> Self {
        UserDotfiles {
            repo: d.repo.clone(),
            dir: d.dir.clone(),
            branch: d.branch.clone(),
            stow: d.stow,
            packages: d.packages.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct User {
    /// Login name.
    pub name: String,
    /// SHA-512 crypt hash (`$6$...`); plaintext is never accepted.
    pub password_hash: String,
    /// Supplementary groups, for example `wheel`.
    pub groups: Vec<String>,
    /// Login shell, for example `/bin/bash`.
    pub shell: String,
    /// User dotfiles configured from a GitHub repository, with optional GNU Stow linking.
    #[serde(default)]
    pub dotfiles: Option<UserDotfiles>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FirstBoot {
    /// Units enabled on first boot.
    #[serde(default)]
    pub services: Vec<String>,
    /// Extra kernel command line parameters.
    #[serde(default)]
    pub kernel_params: Vec<String>,
    /// Scripts run as root, in order, at the end of the first boot.
    #[serde(default)]
    pub scripts: Vec<Script>,
    /// Custom systemd units installed into /etc/systemd/system/ (root) or ~/.config/systemd/user/ (user).
    #[serde(default)]
    pub custom_services: Vec<CustomService>,
    /// Files downloaded into every user's home directory (a failed download only warns).
    #[serde(default)]
    pub user_files: Vec<UserFile>,
    /// Zip archives extracted into every user's home directory (a failed download only warns).
    #[serde(default)]
    pub user_archives: Vec<UserArchive>,
    /// System files written into the root filesystem during installation and preserved via first-boot overlay.
    #[serde(default)]
    pub system_files: Vec<SystemFile>,
}

/// A custom systemd service unit, either inline or from a local file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomService {
    /// Unit file name, for example `rclone-onedrive.service`.
    pub name: String,
    /// Target user: `"root"` (or omit) for system service, or a username for user service.
    #[serde(default)]
    pub user: Option<String>,
    /// Whether the service is enabled on first boot (default: true).
    #[serde(default = "config::default_true")]
    pub enable: bool,
    /// Path of a local unit file, relative to the config file. Mutually exclusive with `content`.
    #[serde(default)]
    pub file: Option<String>,
    /// Inline unit file content. Mutually exclusive with `file`.
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserFile {
    /// `https://` URL of the file (at most 1 MiB).
    pub url: String,
    /// Destination relative to the home directory, for example `.config/hypr/hyprland.conf`.
    pub dest: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserArchive {
    /// Remote HTTP/HTTPS URL of the archive, downloaded during first boot.
    #[serde(default)]
    pub url: Option<String>,
    /// Path of a local archive file (relative to config or absolute), embedded in the ISO payload.
    #[serde(default)]
    pub file: Option<String>,
    /// Filesystem UUID of a disk/partition containing the archive.
    #[serde(default)]
    pub disk_uuid: Option<String>,
    /// Path to the archive inside the partition identified by `disk_uuid` (defaults to "home.tar.zst").
    #[serde(default)]
    pub path: Option<String>,
    /// Optional lower-case hex SHA-256 digest of the archive.
    #[serde(default)]
    pub sha256: Option<String>,
}

fn default_mode_644() -> u16 {
    0o644
}

/// A system file written into the root filesystem during installation and preserved via first-boot overlay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SystemFile {
    /// Target path relative to root, e.g. "etc/NetworkManager/system-connections/Home.nmconnection".
    pub path: String,
    /// Unix permission mode, e.g. 0o600 or 0o644.
    #[serde(default = "default_mode_644")]
    pub mode: u16,
    /// Path of a local file, relative to the config file or absolute.
    #[serde(default)]
    pub file: Option<String>,
    /// Inline file content.
    #[serde(default)]
    pub content: Option<String>,
}

/// A first-boot script. Exactly one source: none of `file`/`url` (a built-in script of that `id`),
/// `file` (a local file, relative to the config), or `url` with `sha256`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Script {
    /// Built-in script name (`enable-sshd`, `enable-fstrim`) or a name of your own for `file`/`url`.
    pub id: String,
    /// Extra arguments, passed as separate words.
    #[serde(default)]
    pub args: Vec<String>,
    /// Path of a local script, relative to the config file.
    #[serde(default)]
    pub file: Option<String>,
    /// `https://` URL of the script; needs `sha256`.
    #[serde(default)]
    pub url: Option<String>,
    /// Lower-case hex SHA-256 of the script at `url`.
    #[serde(default)]
    pub sha256: Option<String>,
    /// Execution phase: "pre-service" (before service enablement) or "post-service" (after, default).
    #[serde(default)]
    pub phase: Option<String>,
}

/// One AUR package, pinned to the reviewed recipe.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Aur {
    /// The package to install (a `pkgname` of the recipe).
    pub name: String,
    /// The AUR git repository name.
    pub pkgbase: String,
    /// Full 40-digit lower-case hex commit of the reviewed recipe.
    pub commit: String,
    /// SHA-256 over the reviewed tree, 64 lower-case hex digits.
    pub sha256: String,
    /// The recipe builds from a VCS source that the commit does not pin.
    #[serde(default)]
    pub vcs: bool,
    /// Install it as a dependency of another entry.
    #[serde(default)]
    pub as_dep: bool,
    /// Official packages the recipe needs, to build and to run.
    #[serde(default)]
    pub deps: Vec<String>,
    /// The part of `deps` that only the build needs.
    #[serde(default)]
    pub build_deps: Vec<String>,
    /// Units to enable once the package is installed.
    #[serde(default)]
    pub services: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AsBuild {
    /// Compiler profile of the installer: `super-small` (default) or `large` (keeps panic messages).
    #[serde(default)]
    #[schemars(schema_with = "profile_schema")]
    pub profile: Option<String>,
    /// Include USB tethering (iPhone, Android, USB Ethernet) in the installer.
    #[serde(default)]
    pub tethering: bool,
    /// Installer-kernel drivers to include; leave it out for all of them.
    #[serde(default)]
    #[schemars(schema_with = "drivers_schema")]
    pub installer_drivers: Option<Vec<String>>,
}

fn profile_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": ["string", "null"], "enum": ["super-small", "large", null] })
}

fn drivers_schema(_: &mut SchemaGenerator) -> Schema {
    let ids: Vec<&str> = DRIVERS.iter().map(|d| d.id).collect();
    json_schema!({ "type": ["array", "null"], "items": { "type": "string", "enum": ids } })
}

impl AsConfig {
    /// Reads an Archstaller TOML configuration string; errors carry the full path (`system.hostname: ...`).
    pub fn from_toml(src: &str) -> Result<AsConfig, String> {
        let de = toml::Deserializer::new(src);
        let cfg: AsConfig = serde_path_to_error::deserialize(de)
            .map_err(|e| format!("{}: {}", path(e.path()), e.inner()))?;
        Ok(cfg)
    }
}

fn path(p: &serde_path_to_error::Path) -> String {
    use serde_path_to_error::Segment;
    let mut s = String::new();
    for seg in p.iter() {
        match seg {
            Segment::Seq { index } => s.push_str(&format!("[{index}]")),
            Segment::Map { key } | Segment::Enum { variant: key } => {
                if !s.is_empty() {
                    s.push('.');
                }
                s.push_str(key);
            }
            Segment::Unknown => s.push_str(".?"),
        }
    }
    if s.is_empty() {
        "config".into()
    } else {
        s
    }
}

impl AsConfig {
    /// The installer's config and the host-only settings this file stands for. Not validated yet.
    pub fn into_parts(self) -> Result<(config::Config, HostConfig), String> {
        if self.schema != SCHEMA {
            return Err(format!(
                "schema: this version reads schema {SCHEMA}, the file says {}",
                self.schema
            ));
        }
        let cfg = config::Config {
            hostname: self.system.hostname,
            timezone: self.system.timezone,
            locale: self.system.locale,
            keymap: self.system.keymap,
            disk: config::Disk {
                model: self.install.disk.model,
                confirm_serial: self.install.disk.confirm_serial,
                auto_largest: self.install.disk.auto_largest,
                esp_mib: self.install.disk.esp_mib,
                filesystem: self.install.disk.filesystem.into(),
                btrfs: self.install.disk.btrfs.map(Into::into),
            },
            bootloader: self.install.bootloader.into(),
            mirrors: self.install.mirrors,
            packages: {
                let mut pkgs = self.packages.explicit;
                if self
                    .users
                    .iter()
                    .any(|u| u.dotfiles.as_ref().is_some_and(|d| d.stow))
                    && !pkgs.iter().any(|p| p == "stow")
                {
                    pkgs.push("stow".into());
                }
                pkgs
            },
            providers: self.packages.providers.into_iter().collect(),
            root_password_hash: self.system.root_password_hash,
            users: self
                .users
                .into_iter()
                .map(|u| config::User {
                    name: u.name,
                    password_hash: u.password_hash,
                    groups: u.groups,
                    shell: u.shell,
                    dotfiles: u.dotfiles.map(Into::into),
                })
                .collect(),
            services: self.first_boot.services,
            kernel_params: self.first_boot.kernel_params,
            user_files: self
                .first_boot
                .user_files
                .into_iter()
                .map(|f| config::UserFile {
                    url: f.url,
                    dest: f.dest,
                })
                .collect(),
            user_archives: self
                .first_boot
                .user_archives
                .into_iter()
                .map(|a| config::UserArchive {
                    url: a.url,
                    file: a.file,
                    content: None,
                    disk_uuid: a.disk_uuid,
                    path: a.path,
                    sha256: a.sha256,
                })
                .collect(),
            system_files: self
                .first_boot
                .system_files
                .into_iter()
                .map(|sf| config::SystemFile {
                    path: sf.path,
                    mode: sf.mode,
                    file: sf.file,
                    content: sf.content.map(|c| c.into_bytes()),
                })
                .collect(),
            dry_run: self.install.dry_run,
            scripts: self
                .first_boot
                .scripts
                .into_iter()
                .map(|s| config::Script {
                    id: s.id,
                    args: s.args,
                    file: s.file,
                    url: s.url,
                    sha256: s.sha256,
                    content: None,
                    phase: s.phase,
                })
                .collect(),
            custom_services: self
                .first_boot
                .custom_services
                .into_iter()
                .map(|cs| config::CustomService {
                    name: cs.name,
                    user: cs.user,
                    enable: cs.enable,
                    file: cs.file,
                    content: cs.content.map(|c| c.into_bytes()),
                })
                .collect(),
            aur: self
                .packages
                .aur
                .into_iter()
                .map(|a| config::AurPackage {
                    name: a.name,
                    pkgbase: a.pkgbase,
                    commit: a.commit,
                    sha256: a.sha256,
                    vcs: a.vcs,
                    as_dep: a.as_dep,
                    deps: a.deps,
                    build_deps: a.build_deps,
                    services: a.services,
                })
                .collect(),
        };
        let host = HostConfig {
            build: Build {
                profile: self.build.profile,
                tethering: self.build.tethering,
            },
            installer_drivers: self.build.installer_drivers,
        };
        Ok((cfg, host))
    }

    /// The inverse of [`AsConfig::into_parts`], for the writer. `Script::content` is not kept.
    /// Providers become a table, so they are written sorted by dependency name.
    pub fn from_parts(cfg: &config::Config, host: &HostConfig) -> AsConfig {
        AsConfig {
            schema: SCHEMA,
            system: System {
                hostname: cfg.hostname.clone(),
                timezone: cfg.timezone.clone(),
                locale: cfg.locale.clone(),
                keymap: cfg.keymap.clone(),
                root_password_hash: cfg.root_password_hash.clone(),
            },
            install: Install {
                disk: Disk {
                    model: cfg.disk.model.clone(),
                    confirm_serial: cfg.disk.confirm_serial.clone(),
                    auto_largest: cfg.disk.auto_largest,
                    esp_mib: cfg.disk.esp_mib,
                    filesystem: cfg.disk.filesystem.into(),
                    btrfs: cfg.disk.btrfs.as_ref().map(Into::into),
                },
                mirrors: cfg.mirrors.clone(),
                dry_run: cfg.dry_run,
                bootloader: cfg.bootloader.into(),
            },
            packages: Packages {
                explicit: cfg.packages.clone(),
                providers: cfg.providers.iter().cloned().collect(),
                aur: cfg
                    .aur
                    .iter()
                    .map(|a| Aur {
                        name: a.name.clone(),
                        pkgbase: a.pkgbase.clone(),
                        commit: a.commit.clone(),
                        sha256: a.sha256.clone(),
                        vcs: a.vcs,
                        as_dep: a.as_dep,
                        deps: a.deps.clone(),
                        build_deps: a.build_deps.clone(),
                        services: a.services.clone(),
                    })
                    .collect(),
            },
            users: cfg
                .users
                .iter()
                .map(|u| User {
                    name: u.name.clone(),
                    password_hash: u.password_hash.clone(),
                    groups: u.groups.clone(),
                    shell: u.shell.clone(),
                    dotfiles: u.dotfiles.as_ref().map(Into::into),
                })
                .collect(),
            first_boot: FirstBoot {
                services: cfg.services.clone(),
                custom_services: cfg
                    .custom_services
                    .iter()
                    .map(|cs| CustomService {
                        name: cs.name.clone(),
                        user: cs.user.clone(),
                        enable: cs.enable,
                        file: cs.file.clone(),
                        content: cs
                            .content
                            .as_ref()
                            .map(|c| String::from_utf8_lossy(c).into_owned()),
                    })
                    .collect(),
                kernel_params: cfg.kernel_params.clone(),
                scripts: cfg
                    .scripts
                    .iter()
                    .map(|s| Script {
                        id: s.id.clone(),
                        args: s.args.clone(),
                        file: s.file.clone(),
                        url: s.url.clone(),
                        sha256: s.sha256.clone(),
                        phase: s.phase.clone(),
                    })
                    .collect(),
                user_files: cfg
                    .user_files
                    .iter()
                    .map(|f| UserFile {
                        url: f.url.clone(),
                        dest: f.dest.clone(),
                    })
                    .collect(),
                user_archives: cfg
                    .user_archives
                    .iter()
                    .map(|a| UserArchive {
                        url: a.url.clone(),
                        file: a.file.clone(),
                        disk_uuid: a.disk_uuid.clone(),
                        path: a.path.clone(),
                        sha256: a.sha256.clone(),
                    })
                    .collect(),
                system_files: cfg
                    .system_files
                    .iter()
                    .map(|sf| SystemFile {
                        path: sf.path.clone(),
                        mode: sf.mode,
                        file: sf.file.clone(),
                        content: sf
                            .content
                            .as_ref()
                            .and_then(|c| std::str::from_utf8(c).ok().map(|s| s.to_string())),
                    })
                    .collect(),
            },
            build: AsBuild {
                profile: host.build.profile.clone(),
                tethering: host.build.tethering,
                installer_drivers: host.installer_drivers.clone(),
            },
        }
    }
}

/// The JSON Schema of an Archstaller TOML configuration.
pub fn json_schema() -> serde_json::Value {
    let schema = schemars::generate::SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<AsConfig>();
    serde_json::to_value(schema).expect("a schema is JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_dotfiles_and_stow_in_toml() {
        let toml_str = r#"
schema = 1

[system]
hostname = "archbox"
timezone = "UTC"
locale = "en_US.UTF-8"
keymap = "us"

[install]
mirrors = ["https://geo.mirror.pkgbuild.com/$repo/os/$arch"]
[install.disk]
confirm_serial = "S123"
esp_mib = 512

[packages]
explicit = ["base", "linux"]

[[users]]
name = "kubik"
password_hash = "$6$rounds=5000$saltsalt$9876543210zyxwvutsrqponmlkjihgfedcba"
groups = ["wheel"]
shell = "/bin/bash"

[users.dotfiles]
repo = "kubik/dotfiles"
dir = "dotfiles"
stow = true
packages = ["hypr", "bash"]

[[first_boot.scripts]]
id = "pre-setup"
phase = "pre-service"
file = "scripts/pre.sh"
"#;

        let as_cfg: AsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(as_cfg.users.len(), 1);
        let u = &as_cfg.users[0];
        let df = u.dotfiles.as_ref().unwrap();
        assert_eq!(df.repo, "kubik/dotfiles");
        assert_eq!(df.dir, "dotfiles");
        assert!(df.stow);
        assert_eq!(df.packages, vec!["hypr", "bash"]);

        let sc = &as_cfg.first_boot.scripts[0];
        assert_eq!(sc.id, "pre-setup");
        assert_eq!(sc.phase.as_deref(), Some("pre-service"));

        let (cfg, _host) = as_cfg.into_parts().unwrap();
        assert!(cfg.packages.contains(&"stow".to_string()));
        assert_eq!(cfg.users[0].dotfiles.as_ref().unwrap().repo, "kubik/dotfiles");
        assert_eq!(cfg.scripts[0].phase.as_deref(), Some("pre-service"));
    }
}
