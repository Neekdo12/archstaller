//! The `as` authoring model: what a Lua config returns (`return { as = { ... } }`).
//!
//! These types only describe the shape of the file. They reject unknown keys and wrong value types
//! while deserializing, then [`AsConfig::into_parts`] normalizes them into the installer's
//! [`config::Config`] and the host-only [`HostConfig`]; the values are validated there, by the same
//! code as before. [`AsConfig::from_parts`] goes the other way for the writer. The LuaLS annotations
//! in `configs/archstaler.lua` are generated from these types (`cargo xtask gen-luals`).
use crate::host::{Build, HostConfig, DRIVERS};
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The schema version this build reads and writes.
pub const SCHEMA: u32 = 1;

/// The whole file: exactly one application namespace, `as`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    #[serde(rename = "as")]
    pub as_: AsConfig,
}

/// An Archstaler configuration.
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
    /// Files downloaded into every user's home directory (a failed download only warns).
    #[serde(default)]
    pub user_files: Vec<UserFile>,
    /// Zip archives extracted into every user's home directory (a failed download only warns).
    #[serde(default)]
    pub user_archives: Vec<UserArchive>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserFile {
    /// `https://` URL of the file (at most 1 MiB).
    pub url: String,
    /// Destination relative to the home directory, for example `.config/hypr/hyprland.lua`.
    pub dest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserArchive {
    /// `https://` URL of a zip file (at most 16 MiB).
    pub url: String,
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

impl Envelope {
    /// Reads the Lua value a config returned; errors carry the full path (`as.system.hostname: ...`),
    /// with list positions counted from 1 like Lua does.
    pub fn from_lua(value: mlua::Value) -> Result<Envelope, String> {
        let de = mlua::serde::Deserializer::new(value);
        serde_path_to_error::deserialize(de).map_err(|e| {
            let msg = e.inner().to_string();
            format!("{}: {}", path(e.path()), msg.strip_prefix("deserialize error: ").unwrap_or(&msg))
        })
    }
}

fn path(p: &serde_path_to_error::Path) -> String {
    use serde_path_to_error::Segment;
    let mut s = String::new();
    for seg in p.iter() {
        match seg {
            Segment::Seq { index } => s.push_str(&format!("[{}]", index + 1)),
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
            return Err(format!("as.schema: this version reads schema {SCHEMA}, the file says {}", self.schema));
        }
        let cfg = config::Config {
            hostname: self.system.hostname,
            timezone: self.system.timezone,
            locale: self.system.locale,
            keymap: self.system.keymap,
            disk: config::Disk { model: self.install.disk.model, confirm_serial: self.install.disk.confirm_serial, auto_largest: self.install.disk.auto_largest, esp_mib: self.install.disk.esp_mib },
            mirrors: self.install.mirrors,
            packages: self.packages.explicit,
            providers: self.packages.providers.into_iter().collect(),
            root_password_hash: self.system.root_password_hash,
            users: self.users.into_iter().map(|u| config::User { name: u.name, password_hash: u.password_hash, groups: u.groups, shell: u.shell }).collect(),
            services: self.first_boot.services,
            kernel_params: self.first_boot.kernel_params,
            user_files: self.first_boot.user_files.into_iter().map(|f| config::UserFile { url: f.url, dest: f.dest }).collect(),
            user_archives: self.first_boot.user_archives.into_iter().map(|a| config::UserArchive { url: a.url }).collect(),
            dry_run: self.install.dry_run,
            scripts: self.first_boot.scripts.into_iter().map(|s| config::Script { id: s.id, args: s.args, file: s.file, url: s.url, sha256: s.sha256, content: None }).collect(),
            aur: self
                .packages
                .aur
                .into_iter()
                .map(|a| config::AurPackage { name: a.name, pkgbase: a.pkgbase, commit: a.commit, sha256: a.sha256, vcs: a.vcs, as_dep: a.as_dep, deps: a.deps, build_deps: a.build_deps, services: a.services })
                .collect(),
        };
        let host = HostConfig { build: Build { profile: self.build.profile, tethering: self.build.tethering }, installer_drivers: self.build.installer_drivers };
        Ok((cfg, host))
    }

    /// The inverse of [`AsConfig::into_parts`], for the writer. `Script::content` is not kept.
    /// Providers become a table, so they are written sorted by dependency name.
    pub fn from_parts(cfg: &config::Config, host: &HostConfig) -> AsConfig {
        AsConfig {
            schema: SCHEMA,
            system: System { hostname: cfg.hostname.clone(), timezone: cfg.timezone.clone(), locale: cfg.locale.clone(), keymap: cfg.keymap.clone(), root_password_hash: cfg.root_password_hash.clone() },
            install: Install {
                disk: Disk { model: cfg.disk.model.clone(), confirm_serial: cfg.disk.confirm_serial.clone(), auto_largest: cfg.disk.auto_largest, esp_mib: cfg.disk.esp_mib },
                mirrors: cfg.mirrors.clone(),
                dry_run: cfg.dry_run,
            },
            packages: Packages {
                explicit: cfg.packages.clone(),
                providers: cfg.providers.iter().cloned().collect(),
                aur: cfg
                    .aur
                    .iter()
                    .map(|a| Aur { name: a.name.clone(), pkgbase: a.pkgbase.clone(), commit: a.commit.clone(), sha256: a.sha256.clone(), vcs: a.vcs, as_dep: a.as_dep, deps: a.deps.clone(), build_deps: a.build_deps.clone(), services: a.services.clone() })
                    .collect(),
            },
            users: cfg.users.iter().map(|u| User { name: u.name.clone(), password_hash: u.password_hash.clone(), groups: u.groups.clone(), shell: u.shell.clone() }).collect(),
            first_boot: FirstBoot {
                services: cfg.services.clone(),
                kernel_params: cfg.kernel_params.clone(),
                scripts: cfg.scripts.iter().map(|s| Script { id: s.id.clone(), args: s.args.clone(), file: s.file.clone(), url: s.url.clone(), sha256: s.sha256.clone() }).collect(),
                user_files: cfg.user_files.iter().map(|f| UserFile { url: f.url.clone(), dest: f.dest.clone() }).collect(),
                user_archives: cfg.user_archives.iter().map(|a| UserArchive { url: a.url.clone() }).collect(),
            },
            build: AsBuild { profile: host.build.profile.clone(), tethering: host.build.tethering, installer_drivers: host.installer_drivers.clone() },
        }
    }
}

/// The JSON Schema of the file, the single source of the LuaLS annotations.
pub fn json_schema() -> serde_json::Value {
    let schema = schemars::generate::SchemaSettings::draft2020_12().into_generator().into_root_schema_for::<Envelope>();
    serde_json::to_value(schema).expect("a schema is JSON")
}
