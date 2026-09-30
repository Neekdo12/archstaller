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
}

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
        Ok(())
    }
}
