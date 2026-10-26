//! Host system localization and identity discovery (hostname, timezone, locale, keymap).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSystem {
    pub hostname: String,
    pub timezone: String,
    pub locale: String,
    pub keymap: String,
}

/// Parses `/etc/locale.conf` for `LANG` or `LC_ALL`.
pub fn parse_locale_conf(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line
            .strip_prefix("LANG=")
            .or_else(|| line.strip_prefix("LC_ALL="))
        {
            let val = rest.trim().trim_matches('"').trim_matches('\'').trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

/// Parses `/etc/vconsole.conf` for `KEYMAP`.
pub fn parse_vconsole_conf(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("KEYMAP=") {
            let val = rest.trim().trim_matches('"').trim_matches('\'').trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

/// Parses output from `hyprctl getoption input:kb_layout`.
pub fn parse_hyprctl_kb_layout(output: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("str:") {
            let val = rest.trim().trim_matches('"').trim_matches('\'').trim();
            if !val.is_empty() && val != "none" {
                let first = val.split(',').next().unwrap_or(val).trim();
                if !first.is_empty() {
                    return Some(first.to_string());
                }
            }
        }
    }
    None
}

/// Parses keymap from `localectl status` output.
pub fn parse_localectl_keymap(output: &str) -> Option<String> {
    let mut x11 = None;
    let mut vc = None;
    for line in output.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("X11 Layout:") {
            let val = rest.trim().split(',').next().unwrap_or("").trim();
            if !val.is_empty() && val != "(unset)" && val != "n/a" {
                x11 = Some(val.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("VC Keymap:") {
            let val = rest.trim().split(',').next().unwrap_or("").trim();
            if !val.is_empty() && val != "(unset)" && val != "n/a" {
                vc = Some(val.to_string());
            }
        }
    }
    x11.or(vc)
}

/// Parses locale from `localectl status` output.
pub fn parse_localectl_locale(output: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("System Locale:") {
            if let Some((_, lang_part)) = rest.split_once("LANG=") {
                let val = lang_part
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_matches('"')
                    .trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

/// Detects active desktop or console keyboard layout from the environment.
pub fn detect_desktop_keymap() -> Option<String> {
    // 1. Hyprland via hyprctl
    if let Ok(out) = std::process::Command::new("hyprctl")
        .args(["getoption", "input:kb_layout"])
        .output()
    {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(km) = parse_hyprctl_kb_layout(&text) {
                return Some(km);
            }
        }
    }

    // 2. Environment variables
    if let Ok(env_k) = std::env::var("XKB_DEFAULT_LAYOUT").or_else(|_| std::env::var("KEYMAP")) {
        let trimmed = env_k.trim();
        if !trimmed.is_empty() {
            let first = trimmed.split(',').next().unwrap_or(trimmed).trim();
            if !first.is_empty() {
                return Some(first.to_string());
            }
        }
    }

    // 3. localectl status
    if let Ok(out) = std::process::Command::new("localectl")
        .arg("status")
        .output()
    {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(km) = parse_localectl_keymap(&text) {
                return Some(km);
            }
        }
    }

    None
}

/// Discovers system hostname, timezone, locale, and keymap from the running host.
pub fn discover_host_system() -> HostSystem {
    let hostname = std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "arch".to_string());

    let timezone = std::fs::read_link("/etc/localtime")
        .ok()
        .and_then(|p| {
            let s = p.to_string_lossy().into_owned();
            s.split("zoneinfo/")
                .nth(1)
                .map(std::string::ToString::to_string)
        })
        .or_else(|| {
            std::fs::read_to_string("/etc/timezone")
                .map(|s| s.trim().to_string())
                .ok()
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "UTC".to_string());

    let locale = std::env::var("LC_ALL")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("LANG").ok().filter(|s| !s.is_empty()))
        .or_else(|| {
            std::fs::read_to_string("/etc/locale.conf")
                .ok()
                .and_then(|c| parse_locale_conf(&c))
        })
        .or_else(|| {
            std::process::Command::new("localectl")
                .arg("status")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| parse_localectl_locale(&String::from_utf8_lossy(&o.stdout)))
        })
        .unwrap_or_else(|| "en_US.UTF-8".to_string());

    let keymap = detect_desktop_keymap()
        .or_else(|| {
            std::fs::read_to_string("/etc/vconsole.conf")
                .ok()
                .and_then(|c| parse_vconsole_conf(&c))
        })
        .unwrap_or_else(|| "us".to_string());

    HostSystem {
        hostname,
        timezone,
        locale,
        keymap,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_locale_conf_variants() {
        assert_eq!(
            parse_locale_conf("LANG=cs_CZ.UTF-8\nLC_TIME=en_US.UTF-8"),
            Some("cs_CZ.UTF-8".into())
        );
        assert_eq!(
            parse_locale_conf("# comment\nLANG=\"en_US.UTF-8\""),
            Some("en_US.UTF-8".into())
        );
        assert_eq!(
            parse_locale_conf("LC_ALL='de_DE.UTF-8'"),
            Some("de_DE.UTF-8".into())
        );
        assert_eq!(parse_locale_conf("# nothing here"), None);
    }

    #[test]
    fn parses_vconsole_conf_variants() {
        assert_eq!(
            parse_vconsole_conf("KEYMAP=cz-qwertz\nFONT=eurlatgr"),
            Some("cz-qwertz".into())
        );
        assert_eq!(
            parse_vconsole_conf("# comment\nKEYMAP=\"us\""),
            Some("us".into())
        );
        assert_eq!(parse_vconsole_conf("# empty"), None);
    }

    #[test]
    fn parses_hyprctl_kb_layout_output() {
        assert_eq!(
            parse_hyprctl_kb_layout("str: cz\nset: true\n"),
            Some("cz".into())
        );
        assert_eq!(
            parse_hyprctl_kb_layout("str: \"cz,us\"\nset: true\n"),
            Some("cz".into())
        );
        assert_eq!(parse_hyprctl_kb_layout("str: none\nset: false\n"), None);
        assert_eq!(parse_hyprctl_kb_layout("something else"), None);
    }

    #[test]
    fn parses_localectl_status() {
        let text = "System Locale: LANG=en_US.UTF-8\n    VC Keymap: us\n   X11 Layout: cz\n";
        assert_eq!(parse_localectl_keymap(text), Some("cz".into()));
        assert_eq!(parse_localectl_locale(text), Some("en_US.UTF-8".into()));

        let unset_x11 = "System Locale: LANG=en_US.UTF-8\n    VC Keymap: us\n   X11 Layout: (unset)\n";
        assert_eq!(parse_localectl_keymap(unset_x11), Some("us".into()));
    }
}
