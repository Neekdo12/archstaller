//! Config types shared by xtask (serializes) and kernel (deserializes).
#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// The bootloader to install on the target system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Bootloader {
    #[default]
    Grub,
    SystemdBoot,
    Limine,
}

impl Bootloader {
    /// Canonical lowercase identifier for the bootloader ("grub", "systemd-boot", "limine").
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Grub => "grub",
            Self::SystemdBoot => "systemd-boot",
            Self::Limine => "limine",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub hostname: String,
    pub timezone: String,
    pub locale: String,
    pub keymap: String,
    pub disk: Disk,
    /// Bootloader to install on the installed system.
    #[serde(default)]
    pub bootloader: Bootloader,
    /// Mirror base URLs; `$repo` and `$arch` are substituted (pacman mirrorlist style).
    pub mirrors: Vec<String>,
    /// Package or group names installed explicitly.
    pub packages: Vec<String>,
    /// Dependency name -> package chosen to provide it.
    pub providers: Vec<(String, String)>,
    pub root_password_hash: Option<String>,
    pub users: Vec<User>,
    /// Units enabled on first boot.
    pub services: Vec<String>,
    /// Extra kernel command line parameters.
    pub kernel_params: Vec<String>,
    /// Files downloaded during installation into every user's home directory. A file whose
    /// server is unreachable is skipped with a warning; it never fails the installation.
    #[serde(default)]
    pub user_files: Vec<UserFile>,
    /// Zip archives downloaded during installation and extracted into every user's home
    /// directory (paths inside the archive are relative to the home). Same failure handling as
    /// `user_files`.
    #[serde(default)]
    pub user_archives: Vec<UserArchive>,
    /// System files written into the root filesystem during installation and preserved via the first-boot overlay.
    #[serde(default)]
    pub system_files: Vec<SystemFile>,
    /// Hardware test mode: probe devices, read disks (never write), bring up the network, fetch
    /// and resolve the package databases, print a report and reboot. No disk is selected, so
    /// `disk.confirm_serial` is not needed. The kernel wraps every disk so writes are refused.
    #[serde(default)]
    pub dry_run: bool,
    /// Scripts run as root, in order, at the end of the first boot (after users and services).
    #[serde(default)]
    pub scripts: Vec<Script>,
    /// Declarative curl-based application installers run during first boot.
    #[serde(default)]
    pub curl_installers: Vec<CurlInstaller>,
    /// Custom systemd units installed into /etc/systemd/system/ (root) or ~/.config/systemd/user/ (user).
    #[serde(default)]
    pub custom_services: Vec<CustomService>,
    /// AUR packages, in build order. They are not in the ISO: the installed system builds them at its
    /// first boot (see `plans-implement/aur.md`). Written as `aur` under `[packages]`.
    #[serde(default, rename = "aur_packages")]
    pub aur: Vec<AurPackage>,
}

/// A curl-based application installer executed on first boot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurlInstaller {
    /// Unique identifier for the installer (e.g. "rustup", "starship").
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Remote URL of the installation script (HTTP or HTTPS).
    pub url: String,
    /// Lower-case hex SHA-256 digest of the script (64 characters).
    pub sha256: String,
    /// Command-line arguments passed to the script.
    #[serde(default)]
    pub args: Vec<String>,
    /// Target user to execute as (None = system-wide / root).
    #[serde(default)]
    pub user: Option<String>,
}

/// One AUR package, pinned to the reviewed recipe.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AurPackage {
    /// The package to install (a `pkgname` of the recipe).
    pub name: String,
    /// The AUR git repository (`https://aur.archlinux.org/<pkgbase>.git`).
    pub pkgbase: String,
    /// Full 40-digit lower-case hex commit of the reviewed recipe.
    pub commit: String,
    /// SHA-256 over the reviewed tree (see `plans-implement/aur.md`, "Pinning"), 64 lower-case hex digits.
    pub sha256: String,
    /// The recipe builds from a VCS source that the commit does not pin (the user acknowledged it).
    #[serde(default)]
    pub vcs: bool,
    /// Install it as a dependency (another entry needs it) instead of as an explicit package.
    #[serde(default)]
    pub as_dep: bool,
    /// Official packages the recipe needs, to build and to run; the installer adds them to the install.
    #[serde(default)]
    pub deps: Vec<String>,
    /// The part of `deps` that only the build needs.
    #[serde(default)]
    pub build_deps: Vec<String>,
    /// Units to enable once this package is installed.
    #[serde(default)]
    pub services: Vec<String>,
}


fn hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Package names as the AUR and pacman allow them.
fn pkg_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && !s.starts_with(['-', '.'])
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "@._+-".contains(c))
}

/// A first-boot script. Exactly one source: none of `file`/`url` (a built-in script of that `id`),
/// `file` (a local file; `xtask` reads it into `content` at build time), or `url` + `sha256`
/// (downloaded by the installer and checked against the digest).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script {
    pub id: String,
    /// Extra arguments, passed to the script as separate words (no spaces, quotes or shell syntax).
    #[serde(default)]
    pub args: Vec<String>,
    /// Host only: path of a local script, relative to the config file.
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// Lower-case hex SHA-256 of the script at `url`; required with `url`.
    #[serde(default)]
    pub sha256: Option<String>,
    /// The script itself, filled in by `xtask`.
    #[serde(default)]
    pub content: Option<Vec<u8>>,
    /// Execution phase: "pre-service" (before service enablement) or "post-service" (after, default).
    #[serde(default)]
    pub phase: Option<String>,
}

/// The largest script the installer will embed or download.
pub const SCRIPT_MAX: usize = 64 * 1024;

pub fn default_true() -> bool {
    true
}

/// Valid systemd unit extensions.
pub const UNIT_EXTENSIONS: &[&str] = &[
    ".service",
    ".timer",
    ".socket",
    ".path",
    ".slice",
    ".target",
    ".mount",
    ".automount",
    ".swap",
];

pub fn valid_unit_name(s: &str) -> bool {
    if s.is_empty() || s.len() > 128 || s.starts_with('.') || s.contains('/') || s.contains('\\') {
        return false;
    }
    if !UNIT_EXTENSIONS.iter().any(|ext| s.ends_with(ext)) {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "@._-".contains(c))
}

/// A custom systemd unit installed into `/etc/systemd/system/` (root) or `~/.config/systemd/user/` (user).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomService {
    /// Unit file name, for example `rclone-onedrive.service`.
    pub name: String,
    /// Target user: `"root"` (or None) for system-wide, or a username from `users` for per-user.
    #[serde(default)]
    pub user: Option<String>,
    /// Whether the unit is enabled on first boot (default: true).
    #[serde(default = "default_true")]
    pub enable: bool,
    /// Host only: path of a local unit file relative to the config file.
    #[serde(default)]
    pub file: Option<String>,
    /// Unit content (read from file at build time, or provided inline).
    #[serde(default)]
    pub content: Option<Vec<u8>>,
}

impl Default for CustomService {
    fn default() -> Self {
        CustomService {
            name: String::new(),
            user: None,
            enable: true,
            file: None,
            content: None,
        }
    }
}

/// The largest custom service unit file the installer will embed.
pub const CUSTOM_SERVICE_MAX: usize = 64 * 1024;

/// The largest archive the installer will embed or download (256 MiB).
pub const USER_ARCHIVE_MAX: usize = 256 * 1024 * 1024;

/// Detects whether `data` begins with valid magic bytes for `.tar.zst`, `.tar.xz`, `.tar.gz`,
/// `.tar.bz2`, `.zip`, or POSIX `.tar`. All of these formats are supported by `bsdtar`.
pub fn is_supported_archive(data: &[u8]) -> bool {
    // PKZip (.zip)
    data.starts_with(b"PK\x03\x04") || data.starts_with(b"PK\x05\x06")
    // Zstandard (.tar.zst, .zst)
    || data.starts_with(&[0x28, 0xb5, 0x2f, 0xfd])
    // XZ (.tar.xz, .xz)
    || data.starts_with(&[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00])
    // Gzip (.tar.gz, .gz)
    || data.starts_with(&[0x1f, 0x8b])
    // Bzip2 (.tar.bz2, .bz2)
    || data.starts_with(b"BZh")
    // POSIX tar (.tar) - "ustar" at offset 257
    || data.get(257..262) == Some(b"ustar")
}

/// A user archive extracted into every configured user's home directory on the installed system.
/// Supports `.tar.zst`, `.tar.xz`, `.tar.gz`, `.tar.bz2`, and `.zip` formats.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserArchive {
    /// Remote HTTP/HTTPS URL of the archive, downloaded during first boot.
    #[serde(default)]
    pub url: Option<String>,
    /// Host-only path of a local archive file (relative to config or absolute), embedded in the ISO payload.
    #[serde(default)]
    pub file: Option<String>,
    /// Archive bytes embedded in the ISO payload at build time.
    #[serde(default)]
    pub content: Option<Vec<u8>>,
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

/// A system file written into the root filesystem during installation and preserved in the first-boot overlay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemFile {
    /// Target path relative to root, e.g. "etc/NetworkManager/system-connections/Home.nmconnection".
    pub path: String,
    /// Unix permission bits (e.g. 0o600 for network keys, 0o644 for general configs).
    pub mode: u16,
    /// Host-only: path of a local file to read at build time.
    #[serde(default)]
    pub file: Option<String>,
    /// File content embedded directly.
    #[serde(default)]
    pub content: Option<Vec<u8>>,
}

impl Default for SystemFile {
    fn default() -> Self {
        SystemFile {
            path: String::new(),
            mode: 0o644,
            file: None,
            content: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserFile {
    /// `https://` URL of the file (at most 1 MiB).
    pub url: String,
    /// Destination relative to the home directory, for example `.config/hypr/hyprland.conf`.
    pub dest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsType {
    #[default]
    Ext4,
    Btrfs,
    Xfs,
    F2fs,
}

impl FsType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ext4 => "ext4",
            Self::Btrfs => "btrfs",
            Self::Xfs => "xfs",
            Self::F2fs => "f2fs",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BtrfsSubvol {
    pub name: String,
    pub mount: String,
}

pub fn default_zstd() -> String {
    "zstd".into()
}

pub fn default_btrfs_subvolumes() -> Vec<BtrfsSubvol> {
    alloc::vec![
        BtrfsSubvol {
            name: "@".into(),
            mount: "/".into(),
        },
        BtrfsSubvol {
            name: "@home".into(),
            mount: "/home".into(),
        },
        BtrfsSubvol {
            name: "@snapshots".into(),
            mount: "/.snapshots".into(),
        },
        BtrfsSubvol {
            name: "@var_log".into(),
            mount: "/var/log".into(),
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BtrfsConfig {
    #[serde(default = "default_btrfs_subvolumes")]
    pub subvolumes: Vec<BtrfsSubvol>,
    #[serde(default = "default_zstd")]
    pub compress: String,
}

impl Default for BtrfsConfig {
    fn default() -> Self {
        Self {
            subvolumes: default_btrfs_subvolumes(),
            compress: default_zstd(),
        }
    }
}

/// Partitioning strategy for the target disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PartitionMode {
    /// Wipe disk and install automated ESP + root partition (default).
    #[default]
    EraseDisk,
    /// Install alongside existing operating systems / unallocated space.
    Alongside,
    /// Manual partition table configuration.
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disk {
    /// Substring matched against the disk model; must select exactly one disk. If absent, the
    /// serial alone selects the disk.
    pub model: Option<String>,
    /// Serial of the disk that will be wiped; installation aborts on mismatch. Not used when
    /// `auto_largest` is set.
    #[serde(default)]
    pub confirm_serial: String,
    /// Opt-in: wipe and install onto the largest disk without any confirmation. Fails (writing
    /// nothing) if several disks tie for the largest size.
    #[serde(default)]
    pub auto_largest: bool,
    /// ESP size in MiB.
    pub esp_mib: u32,
    /// Root filesystem type (ext4, btrfs, xfs, f2fs). Defaults to ext4.
    #[serde(default)]
    pub filesystem: FsType,
    /// Partitioning strategy (erase-disk, alongside, manual). Defaults to erase-disk.
    #[serde(default)]
    pub partition_mode: PartitionMode,
    /// Btrfs options (subvolumes, compression) when filesystem is btrfs.
    #[serde(default)]
    pub btrfs: Option<BtrfsConfig>,
}

impl Default for Disk {
    fn default() -> Self {
        Self {
            model: None,
            confirm_serial: String::new(),
            auto_largest: false,
            esp_mib: 1024,
            filesystem: FsType::Ext4,
            partition_mode: PartitionMode::EraseDisk,
            btrfs: None,
        }
    }
}

pub fn default_dotfiles_dir() -> String {
    "dotfiles".into()
}

/// Validates GitHub repository identifier (e.g. `owner/repo` or `https://github.com/owner/repo`).
pub fn valid_github_repo(s: &str) -> bool {
    let clean = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("http://github.com/"))
        .unwrap_or(s);
    let clean = clean.strip_suffix(".git").unwrap_or(clean).trim_matches('/');

    let mut parts = clean.split('/');
    let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if owner.is_empty()
        || repo.is_empty()
        || owner == "."
        || owner == ".."
        || repo == "."
        || repo == ".."
        || owner.len() > 100
        || repo.len() > 100
    {
        return false;
    }
    let valid_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.';
    owner.chars().all(valid_char) && repo.chars().all(valid_char)
}

/// Normalizes a GitHub repository identifier to canonical `owner/repo`.
pub fn normalize_github_repo(s: &str) -> Option<String> {
    if !valid_github_repo(s) {
        return None;
    }
    let clean = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("http://github.com/"))
        .unwrap_or(s);
    let clean = clean.strip_suffix(".git").unwrap_or(clean).trim_matches('/');
    Some(String::from(clean))
}

/// User dotfiles configured from a GitHub repository, with optional GNU Stow linking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserDotfiles {
    /// GitHub repository: "owner/repo" or "https://github.com/owner/repo".
    pub repo: String,
    /// Target directory inside user's home (default: "dotfiles").
    #[serde(default = "default_dotfiles_dir")]
    pub dir: String,
    /// Git branch or tag (default: "main").
    #[serde(default)]
    pub branch: Option<String>,
    /// Whether to run GNU Stow to symlink configuration files into $HOME.
    #[serde(default = "default_true")]
    pub stow: bool,
    /// Specific package subdirectories to stow (empty = stow all or top-level).
    #[serde(default)]
    pub packages: Vec<String>,
}

impl Default for UserDotfiles {
    fn default() -> Self {
        UserDotfiles {
            repo: String::new(),
            dir: default_dotfiles_dir(),
            branch: None,
            stow: true,
            packages: Vec::new(),
        }
    }
}

/// Desktop autostart entry for a user (`~/.config/autostart/*.desktop`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAutostart {
    /// Name of the application / desktop entry.
    pub name: String,
    /// Exec command line.
    pub exec: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub name: String,
    /// SHA-512 crypt hash (`$6$...`); plaintext is never accepted.
    pub password_hash: String,
    pub groups: Vec<String>,
    pub shell: String,
    #[serde(default)]
    pub dotfiles: Option<UserDotfiles>,
    /// Whether user lingering is enabled (`loginctl enable-linger <user>`).
    #[serde(default)]
    pub linger: bool,
    /// Desktop autostart applications.
    #[serde(default)]
    pub autostart: Vec<UserAutostart>,
    /// Environment variables set for the user profile.
    #[serde(default)]
    pub env_vars: Vec<(String, String)>,
    /// Optional script content appended to the user's login shell profile.
    #[serde(default)]
    pub login_script: Option<String>,
}

/// Services that give the installed system a network (AUR packages are built once it has one).
pub const NETWORK_SERVICES: &[&str] = &[
    "NetworkManager.service",
    "systemd-networkd.service",
    "dhcpcd.service",
    "connman.service",
];

/// Room for `kernel_params` (each with its separating space) on the first boot's command line: the BIOS
/// stage 2 holds `bootinfo::bios::CMDLINE_MAX` (512) bytes including a NUL, and the installer adds
/// `root=UUID=<36 characters> rw` (49) and ` systemd.unit=archstaller-firstboot.target` (42). Checked
/// here so that an install does not fail at its very end, after every package was downloaded.
pub const KERNEL_PARAMS_MAX: usize = 512 - 1 - 49 - 42;

fn ident(s: &str, extra: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

fn crypt_hash(h: &str) -> bool {
    let valid_prefix = h.starts_with("$y$")
        || h.starts_with("$gy$")
        || h.starts_with("$7$")
        || h.starts_with("$6$")
        || h.starts_with("$5$")
        || h.starts_with("$2b$")
        || h.starts_with("$2y$")
        || h.starts_with("$2a$")
        || h.starts_with("$2x$")
        || h.starts_with("$1$")
        || h.starts_with("$argon2id$")
        || h.starts_with("$argon2i$")
        || h.starts_with("$argon2d$");
    valid_prefix
        && h.len() > 13
        && h.chars()
            .all(|c| c.is_ascii_alphanumeric() || "$./=,+-".contains(c))
}

impl Config {
    /// Rejects values that could not be represented safely in the files generated from them
    /// (they end up in shell-read data files, unit files and boot loader configuration).
    pub fn validate(&self) -> Result<(), String> {
        let bad = |what: &str, v: &str| Err(format!("invalid {what}: {v:?}"));
        // systemd ignores /etc/hostname unless it is dot-separated non-empty labels.
        if !ident(&self.hostname, ".-")
            || self
                .hostname
                .split('.')
                .any(|l| l.is_empty() || l.starts_with('-'))
        {
            return bad("hostname", &self.hostname);
        }
        if !ident(&self.timezone, "/_+-") || self.timezone.contains("..") {
            return bad("timezone", &self.timezone);
        }
        if !ident(&self.locale, ".@_-") {
            return bad("locale", &self.locale);
        }
        if !ident(&self.keymap, "._-") {
            return bad("keymap", &self.keymap);
        }
        if !self.dry_run && !self.disk.auto_largest && self.disk.confirm_serial.trim().is_empty() {
            return Err("disk.confirm_serial must be set (or enable disk.auto_largest)".into());
        }
        if let Some(m) = &self.disk.model {
            if m.trim().is_empty() {
                return Err("disk.model must not be empty".into());
            }
        }
        if self.disk.esp_mib < 64 || self.disk.esp_mib > 8192 {
            return Err(format!("disk.esp_mib out of range: {}", self.disk.esp_mib));
        }
        if self.disk.filesystem == FsType::Btrfs {
            if let Some(b) = &self.disk.btrfs {
                if b.subvolumes.is_empty() {
                    return Err("btrfs.subvolumes must not be empty".into());
                }
                if !b.subvolumes.iter().any(|s| s.mount == "/") {
                    return Err("btrfs.subvolumes must include a subvolume mounted at '/'".into());
                }
                for (i, s) in b.subvolumes.iter().enumerate() {
                    if !ident(&s.name, "@_-") {
                        return bad("btrfs subvolume name", &s.name);
                    }
                    if !s.mount.starts_with('/') || s.mount.chars().any(|c| c.is_whitespace() || c.is_control()) {
                        return bad("btrfs mount point (must start with '/' and contain no whitespace)", &s.mount);
                    }
                    if b.subvolumes.iter().take(i).any(|o| o.name == s.name) {
                        return bad("duplicate btrfs subvolume name", &s.name);
                    }
                    if b.subvolumes.iter().take(i).any(|o| o.mount == s.mount) {
                        return bad("duplicate btrfs subvolume mount", &s.mount);
                    }
                }
                let c = &b.compress;
                let valid_compress = c == "none"
                    || c == "zstd"
                    || c.starts_with("zstd:")
                    || c == "lzo";
                if !valid_compress {
                    return bad(
                        "btrfs compression (supported: 'zstd', 'zstd:<level>', 'lzo', 'none')",
                        c,
                    );
                }
            }
        }
        if self.mirrors.is_empty() {
            return Err("at least one mirror is required".into());
        }
        for m in &self.mirrors {
            if !(m.starts_with("https://") || m.starts_with("http://"))
                || m.chars()
                    .any(|c| c.is_whitespace() || c == '\'' || c == '"')
            {
                return bad("mirror", m);
            }
        }
        if self.packages.is_empty() {
            return Err("packages must not be empty".into());
        }
        for p in self
            .packages
            .iter()
            .chain(self.providers.iter().flat_map(|(a, b)| [a, b]))
        {
            if !ident(p, "@._+-") {
                return bad("package name", p);
            }
        }
        if let Some(h) = &self.root_password_hash {
            if !crypt_hash(h) {
                return bad(
                    "root_password_hash (need a valid crypt hash, e.g. yescrypt $y$ or SHA-512 $6$)",
                    "<hidden>",
                );
            }
        }
        for (i, u) in self.users.iter().enumerate() {
            // useradd refuses names longer than 32 characters, names starting with digits, and all-digit ones
            if !ident(&u.name, "_-")
                || u.name.starts_with('-')
                || u.name.starts_with(|c: char| c.is_ascii_digit())
                || u.name == "root"
                || u.name.len() > 32
                || u.name.bytes().all(|b| b.is_ascii_digit())
            {
                return bad("user name", &u.name);
            }
            // The first-boot script runs `useradd` once per entry and stops at the second one for a name.
            if self.users.iter().take(i).any(|o| o.name == u.name) {
                return bad("duplicate user name", &u.name);
            }
            if !crypt_hash(&u.password_hash) {
                return bad(
                    "password_hash (need a valid crypt hash, e.g. yescrypt $y$ or SHA-512 $6$) for user",
                    &u.name,
                );
            }
            for g in &u.groups {
                if !ident(g, "_-") {
                    return bad("group", g);
                }
            }
            if !u.shell.starts_with('/') || !ident(&u.shell, "/_-.") {
                return bad("shell", &u.shell);
            }
            if let Some(ref dotfiles) = u.dotfiles {
                if !valid_github_repo(&dotfiles.repo) {
                    return Err(format!(
                        "user '{}': invalid dotfiles repository '{}'",
                        u.name, dotfiles.repo
                    ));
                }
                if dotfiles.dir.is_empty()
                    || dotfiles.dir.starts_with('/')
                    || dotfiles.dir.contains("..")
                    || dotfiles.dir.contains(':')
                    || dotfiles.dir.chars().any(|c| c.is_whitespace() || c.is_control())
                    || dotfiles.dir.len() > 64
                {
                    return Err(format!(
                        "user '{}': invalid dotfiles directory '{}'",
                        u.name, dotfiles.dir
                    ));
                }
                if let Some(ref branch) = dotfiles.branch {
                    if branch.is_empty()
                        || branch.contains(':')
                        || branch.chars().any(|c| c.is_whitespace() || c.is_control())
                    {
                        return Err(format!(
                            "user '{}': invalid dotfiles branch '{branch}'",
                            u.name
                        ));
                    }
                }
                for pkg in &dotfiles.packages {
                    if pkg.is_empty()
                        || pkg.contains('/')
                        || pkg.contains("..")
                        || pkg.contains(':')
                        || pkg.starts_with('-')
                        || pkg.chars().any(|c| c.is_whitespace() || c.is_control())
                    {
                        return Err(format!(
                            "user '{}': invalid stow package '{pkg}'",
                            u.name
                        ));
                    }
                }
            }
            for as_entry in &u.autostart {
                if as_entry.name.is_empty()
                    || as_entry.name.len() > 64
                    || as_entry.name.contains('/')
                    || as_entry.name.contains('\\')
                    || as_entry.name.chars().any(|c| c.is_control() || c == '\n' || c == '\r')
                {
                    return Err(format!(
                        "user '{}': invalid autostart name {:?}",
                        u.name, as_entry.name
                    ));
                }
                if as_entry.exec.trim().is_empty()
                    || as_entry.exec.chars().any(|c| c.is_control() || c == '\n' || c == '\r')
                {
                    return Err(format!(
                        "user '{}': invalid autostart exec for {:?}",
                        u.name, as_entry.name
                    ));
                }
            }
            for (k, v) in &u.env_vars {
                let valid_key = !k.is_empty()
                    && k.len() <= 64
                    && !k.starts_with(|c: char| c.is_ascii_digit())
                    && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if !valid_key {
                    return Err(format!(
                        "user '{}': invalid env_var name {:?}",
                        u.name, k
                    ));
                }
                if v.chars().any(|c| c == '\0' || c == '\n' || c == '\r') {
                    return Err(format!(
                        "user '{}': invalid env_var value for {:?}",
                        u.name, k
                    ));
                }
            }
            if let Some(ref script) = u.login_script {
                if script.len() > SCRIPT_MAX || script.contains('\0') {
                    return Err(format!(
                        "user '{}': login_script exceeds {} bytes or contains null",
                        u.name, SCRIPT_MAX
                    ));
                }
            }
        }
        for s in &self.services {
            if !ident(s, "@._:-\\") {
                return bad("service", s);
            }
        }
        for k in &self.kernel_params {
            if k.is_empty()
                || k.chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == '"')
            {
                return bad("kernel parameter", k);
            }
        }
        let params_len: usize = self.kernel_params.iter().map(|k| k.len() + 1).sum();
        if params_len > KERNEL_PARAMS_MAX {
            return Err(format!("kernel_params are {params_len} bytes long with their spaces; at most {KERNEL_PARAMS_MAX} fit the first boot's command line"));
        }
        for f in &self.user_files {
            if !f.url.starts_with("https://")
                || f.url
                    .chars()
                    .any(|c| c.is_whitespace() || c == '\'' || c == '"')
            {
                return bad("user_files url (https:// only)", &f.url);
            }
            let ok_dest = !f.dest.is_empty()
                && !f.dest.starts_with('/')
                && f.dest
                    .split('/')
                    .all(|c| !c.is_empty() && c != "." && c != "..")
                && f.dest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c));
            if !ok_dest {
                return bad(
                    "user_files dest (relative path of [A-Za-z0-9._-/])",
                    &f.dest,
                );
            }
        }
        for (i, a) in self.user_archives.iter().enumerate() {
            let at = |m: &str| Err(format!("user_archives[{}]: {m}", i + 1));
            if let Some(c) = &a.content {
                if c.len() > USER_ARCHIVE_MAX {
                    return at(&format!("content size {} exceeds limit of {} bytes", c.len(), USER_ARCHIVE_MAX));
                }
            }
            match (&a.url, &a.file, &a.content, &a.disk_uuid) {
                (None, None, None, None) => {
                    return at("must specify at least one of url, file, content, or disk_uuid");
                }
                (Some(u), ..)
                    if (!u.starts_with("https://") && !u.starts_with("http://"))
                        || u.chars().any(|c| c.is_whitespace() || c == '\'' || c == '"') =>
                {
                    return at(&format!("invalid url {u:?}"));
                }
                _ => {}
            }
            if let Some(h) = &a.sha256 {
                if !hex(h, 64) {
                    return at(&format!("invalid sha256 {h:?} (expected 64 lower-case hex characters)"));
                }
            }
        }
        for (i, sf) in self.system_files.iter().enumerate() {
            let at = |m: &str| Err(format!("system_files[{}]: {m}", i + 1));
            if sf.path.is_empty() || sf.path.starts_with('/') || sf.path.contains("..") {
                return at(&format!("invalid relative path {:?}", sf.path));
            }
            if sf.file.is_none() && sf.content.is_none() {
                return at("must specify file or content");
            }
        }
        for (i, a) in self.aur.iter().enumerate() {
            let at = |m: &str| Err(format!("aur_packages[{}]: {m}", i + 1));
            if !pkg_name(&a.name) {
                return at(&format!("invalid package name {:?}", a.name));
            }
            if !pkg_name(&a.pkgbase) {
                return at(&format!("invalid pkgbase {:?}", a.pkgbase));
            }
            if !hex(&a.commit, 40) {
                return at("commit must be a full 40-digit lower-case hex commit id (a branch name is not a pin)");
            }
            if !hex(&a.sha256, 64) {
                return at("sha256 must be 64 lower-case hex digits");
            }
            if self.aur.iter().take(i).any(|o| o.name == a.name) {
                return at(&format!("{:?} is listed twice", a.name));
            }
            if a.deps.len() > 256 || a.deps.iter().any(|d| !pkg_name(d)) {
                return at("deps must be at most 256 official package names");
            }
            if a.build_deps.iter().any(|d| !a.deps.contains(d)) {
                return at("build_deps must be part of deps");
            }
            if a.services.len() > 16 || a.services.iter().any(|u| !ident(u, "._@:-")) {
                return at("services must be at most 16 systemd unit names");
            }
        }
        if !self.aur.is_empty()
            && !self
                .services
                .iter()
                .any(|u| NETWORK_SERVICES.contains(&u.as_str()))
        {
            return Err("aur_packages: the installed system builds them at its first boot and needs a network then; enable NetworkManager.service (or another network service) in services".into());
        }
        if self.scripts.len() > 32 {
            return Err("at most 32 scripts".into());
        }
        for (i, sc) in self.scripts.iter().enumerate() {
            let at = |m: &str| Err(format!("scripts[{}]: {m}", i + 1));
            if !ident(&sc.id, "._-") {
                return at(&format!("invalid id {:?}", sc.id));
            }
            if self.scripts.iter().take(i).any(|o| o.id == sc.id) {
                return at(&format!("id {:?} is used twice", sc.id));
            }
            if sc.args.len() > 16 || sc.args.iter().any(|a| !ident(a, "._=:/@+,-")) {
                return at("args must be at most 16 words of [A-Za-z0-9._=:/@+,-]");
            }
            if sc.file.is_some() && sc.url.is_some() {
                return at("give either file or url, not both");
            }
            if sc.file.as_deref() == Some("") {
                return at("file is empty");
            }
            match (&sc.url, &sc.sha256) {
                (Some(u), Some(h)) => {
                    if !u.starts_with("https://")
                        || u.chars()
                            .any(|c| c.is_whitespace() || c == '\'' || c == '"')
                    {
                        return at("url must be https:// without spaces or quotes");
                    }
                    if h.len() != 64
                        || !h
                            .chars()
                            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
                    {
                        return at("sha256 must be 64 lower-case hex digits");
                    }
                }
                (Some(_), None) => return at("a downloaded script needs its sha256"),
                (None, Some(_)) => return at("sha256 only makes sense with url"),
                (None, None) => {}
            }
            if sc.content.as_ref().is_some_and(|c| c.len() > SCRIPT_MAX) {
                return at("script is larger than 64 KiB");
            }
            if let Some(ref ph) = sc.phase {
                if ph != "pre-service" && ph != "post-service" {
                    return at(&format!(
                        "invalid script phase '{ph}' (must be 'pre-service' or 'post-service')"
                    ));
                }
            }
        }
        if self.custom_services.len() > 32 {
            return Err("at most 32 custom_services".into());
        }
        for (i, cs) in self.custom_services.iter().enumerate() {
            let at = |m: &str| Err(format!("custom_services[{}]: {m}", i + 1));
            if !valid_unit_name(&cs.name) {
                return at(&format!("invalid unit name {:?}", cs.name));
            }
            if self
                .custom_services
                .iter()
                .take(i)
                .any(|o| o.name == cs.name && o.user == cs.user)
            {
                return at(&format!(
                    "unit {:?} for user {:?} is defined twice",
                    cs.name,
                    cs.user.as_deref().unwrap_or("root")
                ));
            }
            if let Some(u) = &cs.user {
                if u != "root" && !self.users.iter().any(|user| user.name == *u) {
                    return at(&format!(
                        "unknown user {:?}; must be \"root\" or one of {:?}",
                        u,
                        self.users.iter().map(|u| &u.name).collect::<Vec<_>>()
                    ));
                }
            }
            if cs.file.is_some() && cs.content.is_some() {
                return at("give either file or content, not both");
            }
            if cs.file.is_none() && cs.content.is_none() {
                return at("either file or content must be specified");
            }
            if cs.file.as_deref() == Some("") {
                return at("file is empty");
            }
            if cs
                .content
                .as_ref()
                .is_some_and(|c| c.len() > CUSTOM_SERVICE_MAX)
            {
                return at("custom service content is larger than 64 KiB");
            }
        }
        if self.curl_installers.len() > 32 {
            return Err("at most 32 curl_installers".into());
        }
        for (i, c) in self.curl_installers.iter().enumerate() {
            let at = |m: &str| Err(format!("curl_installers[{}]: {m}", i + 1));
            if !ident(&c.id, "-_") {
                return at(&format!("invalid id {:?}", c.id));
            }
            if self.curl_installers.iter().take(i).any(|o| o.id == c.id) {
                return at(&format!("duplicate id {:?}", c.id));
            }
            if c.name.trim().is_empty() || c.name.len() > 64 || c.name.contains('\n') {
                return at(&format!("invalid name {:?}", c.name));
            }
            if (!c.url.starts_with("https://") && !c.url.starts_with("http://"))
                || c.url.chars().any(|ch| ch.is_whitespace() || ch == '\'' || ch == '"')
            {
                return at(&format!("invalid url {:?}", c.url));
            }
            if !hex(&c.sha256, 64) {
                return at("sha256 must be 64 lower-case hex digits");
            }
            for a in &c.args {
                if a.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
                    return at(&format!("invalid arg {:?}", a));
                }
            }
            if let Some(ref u) = c.user {
                if u != "root" && !self.users.iter().any(|user| user.name == *u) {
                    return at(&format!("unknown user {:?}", u));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn base() -> Config {
        Config {
            hostname: "box".into(),
            timezone: "Europe/Prague".into(),
            locale: "en_US.UTF-8".into(),
            keymap: "us".into(),
            disk: Disk {
                model: None,
                confirm_serial: "S".into(),
                auto_largest: false,
                esp_mib: 512,
                filesystem: FsType::Ext4,
                partition_mode: PartitionMode::EraseDisk,
                btrfs: None,
            },
            bootloader: Bootloader::Grub,
            mirrors: vec!["https://example.org/$repo/os/$arch".into()],
            packages: vec!["base".into()],
            providers: vec![],
            root_password_hash: None,
            users: vec![],
            services: vec!["NetworkManager.service".into()],
            kernel_params: vec![],
            user_files: vec![],
            user_archives: vec![],
            system_files: vec![],
            dry_run: false,
            scripts: vec![],
            custom_services: vec![],
            aur: vec![],
            curl_installers: vec![],
        }
    }

    fn pkg() -> AurPackage {
        AurPackage {
            name: "zen-browser-bin".into(),
            pkgbase: "zen-browser-bin".into(),
            commit: "a".repeat(40),
            sha256: "b".repeat(64),
            ..Default::default()
        }
    }

    fn err_of(f: impl FnOnce(&mut AurPackage)) -> String {
        let mut c = base();
        let mut p = pkg();
        f(&mut p);
        c.aur = vec![p];
        c.validate().unwrap_err().to_string()
    }

    #[test]
    fn duplicate_user_names_are_refused() {
        let user = |n: &str| User {
            name: n.into(),
            password_hash: format!("$6$abc${}", "a".repeat(86)),
            groups: vec!["wheel".into()],
            shell: "/bin/bash".into(),
            dotfiles: None,
            ..Default::default()
        };
        let mut c = base();
        c.users = vec![user("alice"), user("bob")];
        c.validate().unwrap();
        c.users.push(user("alice"));
        let e = c.validate().unwrap_err().to_string();
        assert!(
            e.contains("duplicate user name") && e.contains("alice"),
            "{e}"
        );
    }

    #[test]
    fn user_names_and_hostnames_useradd_and_systemd_accept() {
        let user = |n: &str| User {
            name: n.into(),
            password_hash: format!("$6$abc${}", "a".repeat(86)),
            groups: vec![],
            shell: "/bin/bash".into(),
            dotfiles: None,
            ..Default::default()
        };
        let with_user = |n: &str| Config {
            users: vec![user(n)],
            ..base()
        };
        with_user("alice_2-x").validate().unwrap();
        with_user(&"a".repeat(32)).validate().unwrap();
        for bad in ["1234", &"a".repeat(33), "-x", "root"] {
            assert!(with_user(bad).validate().is_err(), "user name {bad:?}");
        }
        let with_host = |h: &str| Config {
            hostname: h.into(),
            ..base()
        };
        with_host("box").validate().unwrap();
        with_host("my-box.lan").validate().unwrap();
        for bad in [".box", "box.", "a..b", "-box", "a.-b"] {
            assert!(with_host(bad).validate().is_err(), "hostname {bad:?}");
        }
    }

    #[test]
    fn accepts_modern_crypt_hashes_and_rejects_bad_ones() {
        let with_hash = |h: &str| {
            let mut c = base();
            c.root_password_hash = Some(h.into());
            c.validate()
        };
        // SHA-512 ($6$)
        with_hash("$6$abc$12345678901234567890").unwrap();
        // Yescrypt ($y$)
        with_hash("$y$j9T$12345678901234567890").unwrap();
        // Bcrypt ($2b$)
        with_hash("$2b$12$12345678901234567890").unwrap();
        // Argon2 ($argon2id$)
        with_hash("$argon2id$v=19$m=65536,t=3,p=4$12345678$123456789012").unwrap();
        // Bad hashes
        assert!(with_hash("plaintext_password").is_err());
        assert!(with_hash("$6$short").is_err());
        assert!(with_hash("").is_err());
        assert!(with_hash("$invalid$abc1234567890").is_err());
    }

    #[test]
    fn kernel_params_fit_the_first_boot_command_line() {
        // One parameter of n bytes takes n + 1 with its space.
        let with = |n: usize| Config {
            kernel_params: vec!["x".repeat(n)],
            ..base()
        };
        with(KERNEL_PARAMS_MAX - 1).validate().unwrap();
        assert!(with(KERNEL_PARAMS_MAX)
            .validate()
            .unwrap_err()
            .contains("kernel_params"));
        let full = format!(
            "root=UUID={} rw {} systemd.unit=archstaller-firstboot.target",
            "0".repeat(36),
            "x".repeat(KERNEL_PARAMS_MAX - 1)
        );
        assert_eq!(
            full.len(),
            511,
            "the longest accepted command line leaves exactly the NUL of the 512-byte field"
        );
    }

    #[test]
    fn aur_entry_rules() {
        let mut c = base();
        c.aur = vec![pkg()];
        c.validate().unwrap();
        assert!(err_of(|p| p.commit = "master".into()).contains("40-digit"));
        assert!(err_of(|p| p.commit = "A".repeat(40)).contains("40-digit"));
        assert!(err_of(|p| p.sha256 = "b".repeat(63)).contains("sha256"));
        assert!(err_of(|p| p.name = "Bad Name".into()).contains("invalid package name"));
        assert!(err_of(|p| p.pkgbase = "../x".into()).contains("pkgbase"));
        assert!(err_of(|p| p.deps = vec!["gtk3; rm".into()]).contains("deps"));
        assert!(err_of(|p| p.build_deps = vec!["gcc".into()]).contains("part of deps"));
        assert!(err_of(|p| p.services = vec!["a b".into()]).contains("services"));
    }

    #[test]
    fn aur_duplicates_count_and_network() {
        let mut c = base();
        c.aur = vec![pkg(), pkg()];
        assert!(c.validate().unwrap_err().contains("twice"));
        c.aur = (0..50)
            .map(|i| AurPackage {
                name: alloc::format!("p{i}"),
                pkgbase: alloc::format!("p{i}"),
                ..pkg()
            })
            .collect();
        c.validate().unwrap();
        c.aur = vec![pkg()];
        c.services.clear();
        assert!(c.validate().unwrap_err().contains("needs a network"));
    }

    #[test]
    fn custom_service_rules() {
        let mut c = base();
        c.users = vec![User {
            name: "kubik".into(),
            password_hash: format!("$6$abc${}", "a".repeat(86)),
            groups: vec![],
            shell: "/bin/bash".into(),
            dotfiles: None,
            ..Default::default()
        }];
        c.custom_services = vec![CustomService {
            name: "my-service.service".into(),
            user: None,
            enable: true,
            file: None,
            content: Some(b"[Unit]\nDescription=Test\n".to_vec()),
        }];
        c.validate().unwrap();

        // Valid user
        c.custom_services[0].user = Some("kubik".into());
        c.validate().unwrap();

        // Unknown user
        c.custom_services[0].user = Some("nonexistent".into());
        assert!(c.validate().unwrap_err().contains("unknown user"));

        // User "root" explicitly
        c.custom_services[0].user = Some("root".into());
        c.validate().unwrap();

        // Invalid unit names
        c.custom_services[0].name = "bad_unit".into();
        assert!(c.validate().unwrap_err().contains("invalid unit name"));
        c.custom_services[0].name = "../evil.service".into();
        assert!(c.validate().unwrap_err().contains("invalid unit name"));

        // File and content mutual exclusion
        c.custom_services[0].name = "good.service".into();
        c.custom_services[0].file = Some("services/good.service".into());
        assert!(c
            .validate()
            .unwrap_err()
            .contains("give either file or content"));

        // Neither file nor content
        c.custom_services[0].content = None;
        c.custom_services[0].file = None;
        assert!(c.validate().unwrap_err().contains("either file or content"));

        // Valid file
        c.custom_services[0].file = Some("services/good.service".into());
        c.validate().unwrap();

        // Content size limit
        c.custom_services[0].file = None;
        c.custom_services[0].content = Some(alloc::vec![b'a'; CUSTOM_SERVICE_MAX + 1]);
        assert!(c.validate().unwrap_err().contains("larger than 64 KiB"));
    }

    #[test]
    fn user_archive_rules() {
        let mut c = base();
        c.user_archives = vec![UserArchive {
            url: Some("https://example.com/dotfiles.tar.zst".into()),
            ..Default::default()
        }];
        c.validate().unwrap();

        // Disk UUID source
        c.user_archives = vec![UserArchive {
            disk_uuid: Some("1234-5678".into()),
            path: Some("home.tar.zst".into()),
            ..Default::default()
        }];
        c.validate().unwrap();

        // Local file source
        c.user_archives = vec![UserArchive {
            file: Some("/path/to/archive.tar.zst".into()),
            ..Default::default()
        }];
        c.validate().unwrap();

        // Embedded content source
        c.user_archives = vec![UserArchive {
            content: Some(b"dummy".to_vec()),
            ..Default::default()
        }];
        c.validate().unwrap();

        // Empty archive source
        c.user_archives = vec![UserArchive::default()];
        assert!(c.validate().unwrap_err().contains("at least one of"));

        // Invalid URL
        c.user_archives = vec![UserArchive {
            url: Some("ftp://bad.com".into()),
            ..Default::default()
        }];
        assert!(c.validate().unwrap_err().contains("invalid url"));
    }

    #[test]
    fn system_file_rules() {
        let mut c = base();
        c.system_files = vec![SystemFile {
            path: "etc/NetworkManager/system-connections/Home.nmconnection".into(),
            mode: 0o600,
            file: Some("connections/Home.nmconnection".into()),
            content: None,
        }];
        c.validate().unwrap();

        // Invalid path (absolute)
        c.system_files[0].path = "/etc/passwd".into();
        assert!(c.validate().unwrap_err().contains("invalid relative path"));

        // Invalid path (path traversal)
        c.system_files[0].path = "etc/../etc/passwd".into();
        assert!(c.validate().unwrap_err().contains("invalid relative path"));

        // Neither file nor content
        c.system_files[0].path = "etc/good".into();
        c.system_files[0].file = None;
        c.system_files[0].content = None;
        assert!(c.validate().unwrap_err().contains("must specify file or content"));
    }

    #[test]
    fn filesystem_rules() {
        let mut c = base();
        assert_eq!(c.disk.filesystem, FsType::Ext4);
        assert_eq!(c.disk.filesystem.as_str(), "ext4");
        c.validate().unwrap();

        // XFS and F2FS
        c.disk.filesystem = FsType::Xfs;
        assert_eq!(c.disk.filesystem.as_str(), "xfs");
        c.validate().unwrap();

        c.disk.filesystem = FsType::F2fs;
        assert_eq!(c.disk.filesystem.as_str(), "f2fs");
        c.validate().unwrap();

        // Btrfs with default config
        c.disk.filesystem = FsType::Btrfs;
        assert_eq!(c.disk.filesystem.as_str(), "btrfs");
        c.disk.btrfs = Some(BtrfsConfig::default());
        c.validate().unwrap();

        // Empty subvolumes
        c.disk.btrfs = Some(BtrfsConfig {
            subvolumes: vec![],
            compress: "zstd".into(),
        });
        assert!(c.validate().unwrap_err().contains("subvolumes must not be empty"));

        // Missing root mount
        c.disk.btrfs = Some(BtrfsConfig {
            subvolumes: vec![BtrfsSubvol {
                name: "@home".into(),
                mount: "/home".into(),
            }],
            compress: "zstd".into(),
        });
        assert!(c.validate().unwrap_err().contains("must include a subvolume mounted at '/'"));

        // Duplicate subvolume name
        c.disk.btrfs = Some(BtrfsConfig {
            subvolumes: vec![
                BtrfsSubvol {
                    name: "@".into(),
                    mount: "/".into(),
                },
                BtrfsSubvol {
                    name: "@".into(),
                    mount: "/home".into(),
                },
            ],
            compress: "zstd".into(),
        });
        assert!(c.validate().unwrap_err().contains("duplicate btrfs subvolume name"));

        // Duplicate mount point
        c.disk.btrfs = Some(BtrfsConfig {
            subvolumes: vec![
                BtrfsSubvol {
                    name: "@".into(),
                    mount: "/".into(),
                },
                BtrfsSubvol {
                    name: "@root2".into(),
                    mount: "/".into(),
                },
            ],
            compress: "zstd".into(),
        });
        assert!(c.validate().unwrap_err().contains("duplicate btrfs subvolume mount"));

        // Invalid mount point (not starting with /)
        c.disk.btrfs = Some(BtrfsConfig {
            subvolumes: vec![
                BtrfsSubvol {
                    name: "@".into(),
                    mount: "/".into(),
                },
                BtrfsSubvol {
                    name: "@home".into(),
                    mount: "home".into(),
                },
            ],
            compress: "zstd".into(),
        });
        assert!(c.validate().unwrap_err().contains("must start with '/'"));

        // Invalid compression
        c.disk.btrfs = Some(BtrfsConfig {
            subvolumes: default_btrfs_subvolumes(),
            compress: "gzip".into(),
        });
        assert!(c.validate().unwrap_err().contains("btrfs compression"));

        // Valid compression variations
        c.disk.btrfs.as_mut().unwrap().compress = "zstd:3".into();
        c.validate().unwrap();
        c.disk.btrfs.as_mut().unwrap().compress = "lzo".into();
        c.validate().unwrap();
        c.disk.btrfs.as_mut().unwrap().compress = "none".into();
        c.validate().unwrap();
    }

    #[test]
    fn dotfiles_validation_rules() {
        let mut c = base();
        let mut u = User {
            name: "alice".into(),
            password_hash: "$6$rounds=5000$usesomehash$9876543210zyxwvutsrqponmlkjihgfedcba".into(),
            groups: vec!["wheel".into()],
            shell: "/bin/bash".into(),
            dotfiles: Some(UserDotfiles {
                repo: "alice/dotfiles".into(),
                dir: "dotfiles".into(),
                branch: Some("main".into()),
                stow: true,
                packages: vec!["hypr".into(), "bash".into()],
            }),
            ..Default::default()
        };
        c.users = vec![u.clone()];
        c.validate().unwrap();

        // Valid full HTTPS URL
        u.dotfiles = Some(UserDotfiles {
            repo: "https://github.com/alice/my-dotfiles.git".into(),
            dir: ".dotfiles".into(),
            branch: None,
            stow: true,
            packages: vec![],
        });
        c.users = vec![u.clone()];
        c.validate().unwrap();

        // Invalid repo: empty
        u.dotfiles = Some(UserDotfiles {
            repo: "".into(),
            dir: "dotfiles".into(),
            branch: None,
            stow: true,
            packages: vec![],
        });
        c.users = vec![u.clone()];
        assert!(c.validate().unwrap_err().contains("dotfiles repository"));

        // Invalid repo: no slash (not owner/repo)
        u.dotfiles = Some(UserDotfiles {
            repo: "just-repo-name".into(),
            dir: "dotfiles".into(),
            branch: None,
            stow: true,
            packages: vec![],
        });
        c.users = vec![u.clone()];
        assert!(c.validate().unwrap_err().contains("dotfiles repository"));

        // Invalid repo: non-github domain
        u.dotfiles = Some(UserDotfiles {
            repo: "https://gitlab.com/alice/dotfiles".into(),
            dir: "dotfiles".into(),
            branch: None,
            stow: true,
            packages: vec![],
        });
        c.users = vec![u.clone()];
        assert!(c.validate().unwrap_err().contains("dotfiles repository"));

        // Invalid dir: absolute path
        u.dotfiles = Some(UserDotfiles {
            repo: "alice/dotfiles".into(),
            dir: "/root/dotfiles".into(),
            branch: None,
            stow: true,
            packages: vec![],
        });
        c.users = vec![u.clone()];
        assert!(c.validate().unwrap_err().contains("dotfiles directory"));

        // Invalid dir: path traversal
        u.dotfiles = Some(UserDotfiles {
            repo: "alice/dotfiles".into(),
            dir: "../dotfiles".into(),
            branch: None,
            stow: true,
            packages: vec![],
        });
        c.users = vec![u.clone()];
        assert!(c.validate().unwrap_err().contains("dotfiles directory"));

        // Invalid package name: path traversal
        u.dotfiles = Some(UserDotfiles {
            repo: "alice/dotfiles".into(),
            dir: "dotfiles".into(),
            branch: None,
            stow: true,
            packages: vec!["../../etc".into()],
        });
        c.users = vec![u.clone()];
        assert!(c.validate().unwrap_err().contains("stow package"));
    }

    #[test]
    fn script_phase_validation_rules() {
        let mut c = base();
        c.scripts = vec![Script {
            id: "setup-pre".into(),
            phase: Some("pre-service".into()),
            content: Some(b"echo pre".to_vec()),
            ..Default::default()
        }];
        c.validate().unwrap();

        c.scripts = vec![Script {
            id: "setup-post".into(),
            phase: Some("post-service".into()),
            content: Some(b"echo post".to_vec()),
            ..Default::default()
        }];
        c.validate().unwrap();

        // Invalid phase
        c.scripts = vec![Script {
            id: "setup-bad".into(),
            phase: Some("during-install".into()),
            content: Some(b"echo bad".to_vec()),
            ..Default::default()
        }];
        assert!(c.validate().unwrap_err().contains("script phase"));
    }

    #[test]
    fn test_bootloader_enum_and_config() {
        let mut c = base();
        assert_eq!(c.bootloader, Bootloader::Grub);
        assert_eq!(c.bootloader.as_str(), "grub");

        c.bootloader = Bootloader::SystemdBoot;
        assert_eq!(c.bootloader.as_str(), "systemd-boot");
        c.validate().unwrap();

        c.bootloader = Bootloader::Limine;
        assert_eq!(c.bootloader.as_str(), "limine");
        c.validate().unwrap();

        // Check serde round-trip
        let json = serde_json::to_string(&c).expect("serialize");
        assert!(json.contains("\"bootloader\":\"limine\""));
        let de: Config = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(de.bootloader, Bootloader::Limine);
    }

    #[test]
    fn valid_github_repo_accepts_https_without_git_suffix() {
        assert!(valid_github_repo("https://github.com/alice/dotfiles"));
        assert!(valid_github_repo("http://github.com/alice/dotfiles"));
        assert!(valid_github_repo("https://github.com/alice/dotfiles.git"));
        assert_eq!(
            normalize_github_repo("https://github.com/alice/dotfiles.git").as_deref(),
            Some("alice/dotfiles")
        );
        assert_eq!(
            normalize_github_repo("https://github.com/alice/dotfiles").as_deref(),
            Some("alice/dotfiles")
        );
    }

    #[test]
    fn rejects_dotfiles_colon_delimiter_injection() {
        let mut c = base();
        let mut u = User {
            name: "alice".into(),
            password_hash: "$6$abc$12345678901234567890".into(),
            groups: vec![],
            shell: "/bin/bash".into(),
            dotfiles: Some(UserDotfiles {
                repo: "alice/dotfiles".into(),
                dir: "dir:inject".into(),
                branch: Some("main".into()),
                stow: true,
                packages: vec![],
            }),
            ..Default::default()
        };
        c.users = vec![u.clone()];
        assert!(c.validate().is_err(), "dir containing ':' must be rejected");

        u.dotfiles.as_mut().unwrap().dir = "dotfiles".into();
        u.dotfiles.as_mut().unwrap().branch = Some("branch:name".into());
        c.users = vec![u.clone()];
        assert!(c.validate().is_err(), "branch containing ':' must be rejected");

        u.dotfiles.as_mut().unwrap().branch = Some("main".into());
        u.dotfiles.as_mut().unwrap().packages = vec!["pkg:bad".into()];
        c.users = vec![u.clone()];
        assert!(c.validate().is_err(), "package containing ':' must be rejected");

        u.dotfiles.as_mut().unwrap().packages = vec!["-D".into()];
        c.users = vec![u];
        assert!(c.validate().is_err(), "package starting with '-' must be rejected");
    }

    #[test]
    fn rejects_btrfs_subvolume_mount_with_whitespace_or_newlines() {
        let mut c = base();
        c.disk.filesystem = FsType::Btrfs;
        c.disk.btrfs = Some(BtrfsConfig {
            subvolumes: vec![
                BtrfsSubvol { name: "@".into(), mount: "/".into() },
                BtrfsSubvol { name: "@home".into(), mount: "/home\nUUID=bad /mnt ext4 defaults 0 0".into() },
            ],
            compress: "zstd".into(),
        });
        assert!(c.validate().is_err(), "mount points with newlines must be rejected");

        c.disk.btrfs.as_mut().unwrap().subvolumes[1].mount = "/home dir".into();
        assert!(c.validate().is_err(), "mount points with spaces must be rejected");
    }

    #[test]
    fn usernames_cannot_start_with_digit() {
        let mut c = base();
        c.users = vec![User {
            name: "1alice".into(),
            password_hash: "$6$abc$12345678901234567890".into(),
            groups: vec![],
            shell: "/bin/bash".into(),
            dotfiles: None,
            ..Default::default()
        }];
        assert!(c.validate().is_err(), "username starting with digit must be rejected");
    }

    #[test]
    fn user_archive_content_exceeding_max_is_rejected() {
        let mut c = base();
        c.user_archives = vec![UserArchive {
            content: Some(vec![0u8; USER_ARCHIVE_MAX + 1]),
            ..Default::default()
        }];
        assert!(c.validate().is_err(), "archive content exceeding USER_ARCHIVE_MAX must be rejected");
    }

    #[test]
    fn disk_model_cannot_be_empty() {
        let mut c = base();
        c.disk.model = Some("   ".into());
        assert!(c.validate().is_err(), "disk model with only whitespace must be rejected");
    }

    #[test]
    fn user_per_user_settings_rules() {
        let mut c = base();
        let mut u = User {
            name: "kubik".into(),
            password_hash: format!("$6$abc${}", "a".repeat(86)),
            groups: vec![],
            shell: "/bin/bash".into(),
            dotfiles: None,
            linger: true,
            autostart: vec![UserAutostart {
                name: "waybar".into(),
                exec: "waybar".into(),
            }],
            env_vars: vec![("EDITOR".into(), "nvim".into()), ("MOZ_ENABLE_WAYLAND".into(), "1".into())],
            login_script: Some("export BROWSER=firefox\n".into()),
        };
        c.users = vec![u.clone()];
        c.validate().unwrap();

        // Autostart invalid name
        u.autostart = vec![UserAutostart { name: "".into(), exec: "waybar".into() }];
        c.users = vec![u.clone()];
        assert!(c.validate().unwrap_err().contains("autostart"));

        u.autostart = vec![UserAutostart { name: "bad/name".into(), exec: "waybar".into() }];
        c.users = vec![u.clone()];
        assert!(c.validate().unwrap_err().contains("autostart"));

        u.autostart = vec![UserAutostart { name: "good".into(), exec: "".into() }];
        c.users = vec![u.clone()];
        assert!(c.validate().unwrap_err().contains("autostart"));

        // Env vars invalid
        u.autostart = vec![];
        u.env_vars = vec![("1BAD".into(), "val".into())];
        c.users = vec![u.clone()];
        assert!(c.validate().unwrap_err().contains("env_var"));

        u.env_vars = vec![("BAD-NAME".into(), "val".into())];
        c.users = vec![u.clone()];
        assert!(c.validate().unwrap_err().contains("env_var"));

        u.env_vars = vec![("GOOD".into(), "val\nwith\nnewlines".into())];
        c.users = vec![u.clone()];
        assert!(c.validate().unwrap_err().contains("env_var"));

        // Login script too large
        u.env_vars = vec![];
        u.login_script = Some("a".repeat(SCRIPT_MAX + 1));
        c.users = vec![u.clone()];
        assert!(c.validate().unwrap_err().contains("login_script"));
    }

    #[test]
    fn curl_installer_rules() {
        let mut c = base();
        c.users = vec![User {
            name: "kubik".into(),
            password_hash: format!("$6$abc${}", "a".repeat(86)),
            groups: vec![],
            shell: "/bin/bash".into(),
            dotfiles: None,
            linger: false,
            autostart: vec![],
            env_vars: vec![],
            login_script: None,
        }];
        c.curl_installers = vec![CurlInstaller {
            id: "rustup".into(),
            name: "Rust Toolchain".into(),
            url: "https://sh.rustup.rs".into(),
            sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            args: vec!["-y".into(), "--no-modify-path".into()],
            user: Some("kubik".into()),
        }];
        c.validate().unwrap();

        // System-wide installer
        c.curl_installers[0].user = None;
        c.validate().unwrap();

        // Invalid sha256
        c.curl_installers[0].sha256 = "invalid_hash".into();
        assert!(c.validate().unwrap_err().contains("sha256"));

        // Invalid url
        c.curl_installers[0].sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into();
        c.curl_installers[0].url = "ftp://bad".into();
        assert!(c.validate().unwrap_err().contains("url"));

        // Unknown user
        c.curl_installers[0].url = "https://sh.rustup.rs".into();
        c.curl_installers[0].user = Some("unknown_user".into());
        assert!(c.validate().unwrap_err().contains("unknown user"));

        // Duplicate id
        c.curl_installers[0].user = None;
        c.curl_installers.push(c.curl_installers[0].clone());
        assert!(c.validate().unwrap_err().contains("duplicate"));
    }

    #[test]
    fn partition_mode_rules() {
        let mut c = base();
        assert_eq!(c.disk.partition_mode, PartitionMode::EraseDisk);
        c.disk.partition_mode = PartitionMode::Alongside;
        c.validate().unwrap();
        c.disk.partition_mode = PartitionMode::Manual;
        c.validate().unwrap();
    }
}
