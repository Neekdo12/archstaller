//! Value lists for the suggestion fields. Each one is read from this computer when it can be (the
//! same data the installed system uses) and falls back to a short built-in list.
use std::path::Path;

fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            continue; // posix/, right/, tzdata.zi, zone.tab, ...
        }
        let p = e.path();
        let full = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        if p.is_dir() {
            walk(&p, &full, out);
        } else {
            out.push(full);
        }
    }
}

pub fn timezones() -> Vec<String> {
    let mut v = Vec::new();
    walk(Path::new("/usr/share/zoneinfo"), "", &mut v);
    v.retain(|z| z.contains('/') || z == "UTC");
    if v.is_empty() {
        v = ["UTC", "Europe/Prague", "Europe/London", "Europe/Berlin", "America/New_York", "America/Los_Angeles", "Asia/Tokyo", "Australia/Sydney"].map(String::from).to_vec();
    }
    v.sort();
    v
}

pub fn locales() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_to_string("/usr/share/i18n/SUPPORTED")
        .map(|t| t.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_whitespace().next()).filter(|l| l.ends_with("UTF-8")).map(String::from).collect())
        .unwrap_or_default();
    if v.is_empty() {
        v = ["en_US.UTF-8", "en_GB.UTF-8", "cs_CZ.UTF-8", "de_DE.UTF-8", "fr_FR.UTF-8", "es_ES.UTF-8", "pl_PL.UTF-8"].map(String::from).to_vec();
    }
    v.sort();
    v.dedup();
    v
}

pub fn keymaps() -> Vec<String> {
    let mut v: Vec<String> = std::process::Command::new("localectl")
        .arg("list-keymaps")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();
    if v.is_empty() {
        v = ["us", "uk", "de", "fr", "es", "cz", "cz-qwertz", "pl", "it", "dvorak"].map(String::from).to_vec();
    }
    v.sort();
    v
}

pub fn shells() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_to_string("/etc/shells").map(|t| t.lines().filter(|l| l.starts_with('/')).map(String::from).collect()).unwrap_or_default();
    for s in ["/bin/bash", "/bin/zsh", "/bin/fish", "/bin/sh"] {
        if !v.iter().any(|x| x == s) {
            v.push(s.into());
        }
    }
    v.sort();
    v
}

pub fn groups() -> Vec<String> {
    ["wheel", "audio", "video", "input", "storage", "optical", "network", "power", "lp", "scanner", "uucp", "kvm", "libvirt", "docker"].map(String::from).to_vec()
}

pub fn services() -> Vec<String> {
    [
        "NetworkManager.service", "sshd.service", "systemd-networkd.service", "systemd-resolved.service", "bluetooth.service",
        "cups.service", "avahi-daemon.service", "fstrim.timer", "ufw.service", "firewalld.service", "docker.service",
        "libvirtd.service", "sddm.service", "gdm.service", "lightdm.service", "ly.service", "cronie.service", "tlp.service",
        "power-profiles-daemon.service", "reflector.timer", "paccache.timer", "systemd-timesyncd.service", "nftables.service",
    ]
    .map(String::from)
    .to_vec()
}

pub fn kernel_params() -> Vec<String> {
    ["quiet", "splash", "nomodeset", "loglevel=3", "mitigations=off", "nvidia-drm.modeset=1", "amd_pstate=active", "i915.enable_psr=0", "video=1920x1080", "console=ttyS0", "intel_iommu=on", "amd_iommu=on", "iommu=pt", "acpi_osi=Linux", "noapic", "nowatchdog"]
        .map(String::from)
        .to_vec()
}
