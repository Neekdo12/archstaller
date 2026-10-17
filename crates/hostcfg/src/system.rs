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

    let locale = std::fs::read_to_string("/etc/locale.conf")
        .ok()
        .and_then(|c| parse_locale_conf(&c))
        .or_else(|| {
            std::env::var("LANG").ok().map(|l| {
                l.split('.').next().unwrap_or(&l).to_string() + ".UTF-8"
            })
        })
        .unwrap_or_else(|| "en_US.UTF-8".to_string());

    let keymap = std::fs::read_to_string("/etc/vconsole.conf")
        .ok()
        .and_then(|c| parse_vconsole_conf(&c))
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
}
