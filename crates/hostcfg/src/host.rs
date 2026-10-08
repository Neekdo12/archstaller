//! Build-host settings: the `build` table, the installer driver catalogue, reserved fields.
use serde::{Deserialize, Serialize};

/// Compiler profile of the installer. Saved in Lua as `build.profile`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Profile {
    /// Size-optimized (Cargo profile `small`: build-std, panics abort without messages). The default.
    SuperSmall,
    /// The regular release build (panic messages kept).
    Large,
}

impl Profile {
    pub const DEFAULT: Profile = Profile::SuperSmall;

    pub fn name(self) -> &'static str {
        match self {
            Profile::SuperSmall => "super-small",
            Profile::Large => "large",
        }
    }

    /// The Cargo profile the installer is built with.
    pub fn cargo_profile(self) -> &'static str {
        match self {
            Profile::SuperSmall => "small",
            Profile::Large => "release",
        }
    }

    /// Parses a profile name. `extra-large` is reserved: it is not defined until the build has a
    /// second optional feature, and silently treating it as `large` would mislead.
    pub fn parse(s: &str) -> Result<Profile, String> {
        match s {
            "super-small" => Ok(Profile::SuperSmall),
            "large" => Ok(Profile::Large),
            "extra-large" => Err("build profile \"extra-large\" is reserved and not available yet (use \"super-small\" or \"large\")".into()),
            other => Err(format!("unknown build profile {other:?} (use \"super-small\" or \"large\")")),
        }
    }
}

/// The `build` table of a config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Build {
    /// `super-small` (default) or `large`.
    pub profile: Option<String>,
    /// Include USB tethering (iPhone, Android, USB Ethernet) in the installer.
    pub tethering: bool,
}

/// Settings that control how the ISO is produced. They are not serialized into the ISO.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostConfig {
    pub build: Build,
    /// Installer-kernel drivers to include (ids from [`DRIVERS`]); `None` means all of them.
    pub installer_drivers: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverClass {
    Storage,
    Network,
}

/// One installer-kernel driver. These are drivers of the installer itself, not packages that end
/// up in the installed system.
pub struct Driver {
    pub id: &'static str,
    pub class: DriverClass,
    pub description: &'static str,
    /// How far it has been tested.
    pub status: &'static str,
}

/// The catalogue of installer drivers; each id is also a cargo feature of `drivers` and `kernel`.
pub const DRIVERS: &[Driver] = &[
    Driver { id: "virtio-blk", class: DriverClass::Storage, description: "virtio block device (QEMU, most hypervisors)", status: "tested in QEMU" },
    Driver { id: "ahci", class: DriverClass::Storage, description: "SATA through AHCI", status: "tested in QEMU" },
    Driver { id: "ata", class: DriverClass::Storage, description: "legacy IDE-mode SATA and parallel ATA disks (PIO, slow)", status: "tested in QEMU" },
    Driver { id: "nvme", class: DriverClass::Storage, description: "NVMe SSDs", status: "tested in QEMU" },
    Driver { id: "alx", class: DriverClass::Network, description: "Qualcomm Atheros AR8131/8132/8151/8152 (atl1c) and AR8161/8162/8171/8172, Killer E2x00 (alx)", status: "untested (no hardware)" },
    Driver { id: "vmxnet3", class: DriverClass::Network, description: "VMware vmxnet3 virtual NIC", status: "tested in QEMU" },
    Driver { id: "virtio-net", class: DriverClass::Network, description: "virtio network device", status: "tested in QEMU" },
    Driver { id: "e1000", class: DriverClass::Network, description: "Intel e1000 / e1000e", status: "tested in QEMU" },
    Driver { id: "igb", class: DriverClass::Network, description: "Intel igb / igc", status: "tested in QEMU" },
    Driver { id: "r8169", class: DriverClass::Network, description: "Realtek RTL8168/8111/8125/8126", status: "verified on one real board" },
    Driver { id: "rtl8139", class: DriverClass::Network, description: "Realtek RTL8139", status: "tested in QEMU" },
];

pub fn driver(id: &str) -> Option<&'static Driver> {
    DRIVERS.iter().find(|d| d.id == id)
}

impl HostConfig {
    /// The profile this config asks for (`super-small` if it names none).
    pub fn profile(&self) -> Result<Profile, String> {
        self.build.profile.as_deref().map_or(Ok(Profile::DEFAULT), Profile::parse)
    }

    /// The installer drivers to build: the listed ones, or all.
    pub fn drivers(&self) -> Vec<&'static str> {
        match &self.installer_drivers {
            Some(l) => l.iter().filter_map(|i| driver(i)).map(|d| d.id).collect(),
            None => DRIVERS.iter().map(|d| d.id).collect(),
        }
    }

    /// Rejects what the GUI and the CLI must both reject, with the field path in the message.
    pub fn validate(&self) -> Result<(), String> {
        self.profile().map_err(|e| format!("build.profile: {e}"))?;
        if let Some(list) = &self.installer_drivers {
            for (i, id) in list.iter().enumerate() {
                if driver(id).is_none() {
                    return Err(format!("installer_drivers[{}]: unknown installer driver {id:?}", i + 1));
                }
                if list[..i].contains(id) {
                    return Err(format!("installer_drivers[{}]: {id:?} is listed twice", i + 1));
                }
            }
            let has = |c: DriverClass| list.iter().filter_map(|i| driver(i)).any(|d| d.class == c);
            if !has(DriverClass::Storage) {
                return Err("installer_drivers: no storage driver selected, the installer could not see any disk".into());
            }
            if !has(DriverClass::Network) && !self.build.tethering {
                return Err("installer_drivers: no network driver selected (and build.tethering is off), the installer could not download anything".into());
            }
        }
        Ok(())
    }
}

/// How a build's profile was chosen, for the build summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub profile: Profile,
    pub warnings: Vec<String>,
}

/// Picks the profile. Precedence: `--profile`, then the deprecated `--small`/`--super-small`
/// switches (which warn), then `build.profile` in the config, then the default (`super-small`).
pub fn resolve_profile(cli_profile: Option<Profile>, legacy_small: bool, host: &HostConfig) -> Result<Resolved, String> {
    let mut warnings = Vec::new();
    if legacy_small {
        warnings.push("--small/--super-small are deprecated: set build.profile = \"super-small\" in the config (or pass --profile)".to_string());
    }
    let profile = if let Some(p) = cli_profile {
        p
    } else if legacy_small {
        Profile::SuperSmall
    } else {
        host.profile()?
    };
    Ok(Resolved { profile, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(profile: Option<&str>) -> HostConfig {
        HostConfig { build: Build { profile: profile.map(String::from), tethering: false }, ..Default::default() }
    }

    #[test]
    fn default_is_super_small() {
        assert_eq!(resolve_profile(None, false, &host(None)).unwrap().profile, Profile::SuperSmall);
    }

    #[test]
    fn config_value_beats_default() {
        assert_eq!(resolve_profile(None, false, &host(Some("large"))).unwrap().profile, Profile::Large);
    }

    #[test]
    fn cli_beats_everything() {
        let r = resolve_profile(Some(Profile::Large), true, &host(Some("super-small"))).unwrap();
        assert_eq!(r.profile, Profile::Large);
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn legacy_small_overrides_config_and_warns() {
        let r = resolve_profile(None, true, &host(Some("large"))).unwrap();
        assert_eq!(r.profile, Profile::SuperSmall);
        assert!(r.warnings[0].contains("deprecated"));
    }

    #[test]
    fn extra_large_and_unknown_profiles_are_rejected() {
        assert!(host(Some("extra-large")).validate().unwrap_err().contains("reserved"));
        assert!(host(Some("huge")).validate().unwrap_err().contains("unknown build profile"));
    }

    #[test]
    fn driver_lists_are_checked() {
        let with = |ids: &[&str], tethering: bool| HostConfig {
            installer_drivers: Some(ids.iter().map(|s| s.to_string()).collect()),
            build: Build { profile: None, tethering },
            ..Default::default()
        };
        assert!(with(&["virtio-blk", "virtio-net"], false).validate().is_ok());
        assert!(with(&["virtio-blk", "bogus"], false).validate().unwrap_err().contains("installer_drivers[2]"));
        assert!(with(&["virtio-blk", "virtio-blk", "e1000"], false).validate().unwrap_err().contains("twice"));
        assert!(with(&["e1000"], false).validate().unwrap_err().contains("no storage driver"));
        assert!(with(&["nvme"], false).validate().unwrap_err().contains("no network driver"));
        assert!(with(&["nvme"], true).validate().is_ok());
    }
}
