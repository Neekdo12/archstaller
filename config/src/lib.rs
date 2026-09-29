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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Disk {
    /// Substring matched against the disk model; must select exactly one disk. If absent, the
    /// serial alone selects the disk.
    pub model: Option<String>,
    /// Serial of the disk that will be wiped; installation aborts on mismatch.
    pub confirm_serial: String,
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
        if self.disk.confirm_serial.trim().is_empty() {
            return Err("disk.confirm_serial must be set".into());
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
        Ok(())
    }
}
