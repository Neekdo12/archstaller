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
    /// AUR packages, in build order. They are not in the ISO: the installed system builds them at its
    /// first boot (see `plans-implement/aur.md`). Written as `aur_packages` in Lua.
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
    s.len() == n && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Package names as the AUR and pacman allow them.
fn pkg_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && !s.starts_with(['-', '.']) && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "@._+-".contains(c))
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserArchive {
    /// `https://` URL of a zip file (at most 16 MiB).
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserFile {
    /// `https://` URL of the file (at most 1 MiB).
    pub url: String,
    /// Destination relative to the home directory, for example `.config/hypr/hyprland.lua`.
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

fn ident(s: &str, extra: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

fn crypt_hash(h: &str) -> bool {
    h.starts_with("$6$") && h.len() > 20 && h.chars().all(|c| c.is_ascii_alphanumeric() || "$./".contains(c))
}

impl Config {
    /// Rejects values that could not be represented safely in the files generated from them
    /// (they end up in shell-read data files, unit files and boot loader configuration).
    pub fn validate(&self) -> Result<(), String> {
        let bad = |what: &str, v: &str| Err(format!("invalid {what}: {v:?}"));
        if !ident(&self.hostname, ".-") {
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
            if !(m.starts_with("https://") || m.starts_with("http://")) || m.chars().any(|c| c.is_whitespace() || c == '\'' || c == '"') {
                return bad("mirror", m);
            }
        }
        if self.packages.is_empty() {
            return Err("packages must not be empty".into());
        }
        for p in self.packages.iter().chain(self.providers.iter().flat_map(|(a, b)| [a, b])) {
            if !ident(p, "@._+-") {
                return bad("package name", p);
            }
        }
        if let Some(h) = &self.root_password_hash {
            if !crypt_hash(h) {
                return bad("root_password_hash (need a $6$ SHA-512 crypt hash)", "<hidden>");
            }
        }
        for u in &self.users {
            if !ident(&u.name, "_-") || u.name.starts_with('-') || u.name == "root" {
                return bad("user name", &u.name);
            }
            if !crypt_hash(&u.password_hash) {
                return bad("password_hash (need a $6$ SHA-512 crypt hash) for user", &u.name);
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
            if k.is_empty() || k.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"') {
                return bad("kernel parameter", k);
            }
        }
        for f in &self.user_files {
            if !f.url.starts_with("https://") || f.url.chars().any(|c| c.is_whitespace() || c == '\'' || c == '"') {
                return bad("user_files url (https:// only)", &f.url);
            }
            let ok_dest = !f.dest.is_empty()
                && !f.dest.starts_with('/')
                && f.dest.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
                && f.dest.chars().all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c));
            if !ok_dest {
                return bad("user_files dest (relative path of [A-Za-z0-9._-/])", &f.dest);
            }
        }
        for a in &self.user_archives {
            if !a.url.starts_with("https://") || a.url.chars().any(|c| c.is_whitespace() || c == '\'' || c == '"') {
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
            if self.aur[..i].iter().any(|o| o.name == a.name) {
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
        if !self.aur.is_empty() && !self.services.iter().any(|u| ["NetworkManager.service", "systemd-networkd.service", "dhcpcd.service", "connman.service"].contains(&u.as_str())) {
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
            if self.scripts[..i].iter().any(|o| o.id == sc.id) {
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
                    if !u.starts_with("https://") || u.chars().any(|c| c.is_whitespace() || c == '\'' || c == '"') {
                        return at("url must be https:// without spaces or quotes");
                    }
                    if h.len() != 64 || !h.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)) {
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
            disk: Disk { model: None, confirm_serial: "S".into(), auto_largest: false, esp_mib: 512 },
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
            aur: vec![],
        }
    }

    fn pkg() -> AurPackage {
        AurPackage { name: "zen-browser-bin".into(), pkgbase: "zen-browser-bin".into(), commit: "a".repeat(40), sha256: "b".repeat(64), ..Default::default() }
    }

    fn err_of(f: impl FnOnce(&mut AurPackage)) -> String {
        let mut c = base();
        let mut p = pkg();
        f(&mut p);
        c.aur = vec![p];
        c.validate().unwrap_err().to_string()
    }

    #[test]
    fn aur_entry_rules() {
        let mut c = base();
        c.aur = vec![pkg()];
        assert!(c.validate().is_ok());
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
        c.aur = (0..=AUR_MAX).map(|i| AurPackage { name: alloc::format!("p{i}"), pkgbase: alloc::format!("p{i}"), ..pkg() }).collect();
        assert!(c.validate().unwrap_err().contains("at most"));
        c.aur = vec![pkg()];
        c.services.clear();
        assert!(c.validate().unwrap_err().contains("needs a network"));
    }
}
