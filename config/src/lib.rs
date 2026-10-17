//! Config types shared by xtask (serializes) and kernel (deserializes).
#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub hostname: String,
    pub timezone: String,
    pub locale: String,
    pub keymap: String,
    pub disk: Disk,
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
    /// Hardware test mode: probe devices, read disks (never write), bring up the network, fetch
    /// and resolve the package databases, print a report and reboot. No disk is selected, so
    /// `disk.confirm_serial` is not needed. The kernel wraps every disk so writes are refused.
    #[serde(default)]
    pub dry_run: bool,
    /// Scripts run as root, in order, at the end of the first boot (after users and services).
    #[serde(default)]
    pub scripts: Vec<Script>,
    /// Custom systemd units installed into /etc/systemd/system/ (root) or ~/.config/systemd/user/ (user).
    #[serde(default)]
    pub custom_services: Vec<CustomService>,
    /// AUR packages, in build order. They are not in the ISO: the installed system builds them at its
    /// first boot (see `plans-implement/aur.md`). Written as `aur` under `[packages]`.
    #[serde(default, rename = "aur_packages")]
    pub aur: Vec<AurPackage>,
}

/// One AUR package, pinned to the reviewed recipe.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

/// The most AUR packages a config can name.
pub const AUR_MAX: usize = 16;

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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserArchive {
    /// `https://` URL of a zip file (at most 16 MiB).
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserFile {
    /// `https://` URL of the file (at most 1 MiB).
    pub url: String,
    /// Destination relative to the home directory, for example `.config/hypr/hyprland.conf`.
    pub dest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub name: String,
    /// SHA-512 crypt hash (`$6$...`); plaintext is never accepted.
    pub password_hash: String,
    pub groups: Vec<String>,
    pub shell: String,
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
    h.starts_with("$6$")
        && h.len() > 20
        && h.chars()
            .all(|c| c.is_ascii_alphanumeric() || "$./".contains(c))
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
        if self.disk.esp_mib < 64 || self.disk.esp_mib > 8192 {
            return Err(format!("disk.esp_mib out of range: {}", self.disk.esp_mib));
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
                    "root_password_hash (need a $6$ SHA-512 crypt hash)",
                    "<hidden>",
                );
            }
        }
        for (i, u) in self.users.iter().enumerate() {
            // useradd refuses names longer than 32 characters and all-digit ones, and the first boot stops
            // at a user it cannot create.
            if !ident(&u.name, "_-")
                || u.name.starts_with('-')
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
                    "password_hash (need a $6$ SHA-512 crypt hash) for user",
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
        for a in &self.user_archives {
            if !a.url.starts_with("https://")
                || a.url
                    .chars()
                    .any(|c| c.is_whitespace() || c == '\'' || c == '"')
            {
                return bad("user_archives url (https:// only)", &a.url);
            }
        }
        if self.aur.len() > AUR_MAX {
            return Err(format!("at most {AUR_MAX} aur_packages"));
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
            },
            mirrors: vec!["https://example.org/$repo/os/$arch".into()],
            packages: vec!["base".into()],
            providers: vec![],
            root_password_hash: None,
            users: vec![],
            services: vec!["NetworkManager.service".into()],
            kernel_params: vec![],
            user_files: vec![],
            user_archives: vec![],
            dry_run: false,
            scripts: vec![],
            custom_services: vec![],
            aur: vec![],
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
        c.aur = (0..=AUR_MAX)
            .map(|i| AurPackage {
                name: alloc::format!("p{i}"),
                pkgbase: alloc::format!("p{i}"),
                ..pkg()
            })
            .collect();
        assert!(c.validate().unwrap_err().contains("at most"));
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
}
