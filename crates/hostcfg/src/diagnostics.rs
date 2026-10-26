//! Installer log parsing, ring buffer inspection, and failure diagnosis.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogSubsystem {
    Net,
    Disk,
    Pkg,
    Pci,
    Usb,
    Init,
    General,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub raw: String,
    pub timestamp_ms: Option<u64>,
    pub level: LogLevel,
    pub subsystem: LogSubsystem,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticCategory {
    DhcpTimeout,
    DnsTimeout,
    DiskSerialMismatch,
    MirrorError404,
    SignatureError,
    KernelPanic,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemediationAction {
    SwitchMirror(String),
    EnableAutoLargest,
    EnableTethering,
    RetryWithDebug,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnosis {
    pub category: DiagnosticCategory,
    pub summary: String,
    pub remediation: String,
    pub action: Option<RemediationAction>,
}

fn parse_timestamp(s: &str) -> (Option<u64>, &str) {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix('[') {
        if let Some((ts_str, after)) = rest.split_once(']') {
            let ts_trimmed = ts_str.trim();
            if let Ok(sec) = ts_trimmed.parse::<f64>() {
                #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "log timestamp seconds to ms")]
                let ms = (sec * 1000.0).max(0.0) as u64;
                return (Some(ms), after.trim_start());
            }
        }
    }
    (None, s)
}

fn detect_subsystem(line_lower: &str) -> LogSubsystem {
    if line_lower.contains("pci:") || line_lower.starts_with("pci ") {
        LogSubsystem::Pci
    } else if line_lower.contains("dhcp")
        || line_lower.contains("dns")
        || line_lower.contains("eth0")
        || line_lower.contains("virtio-net")
        || line_lower.contains("e1000")
        || line_lower.contains("r8169")
        || line_lower.contains("net:")
    {
        LogSubsystem::Net
    } else if line_lower.contains("disk")
        || line_lower.contains("gpt")
        || line_lower.contains("nvme")
        || line_lower.contains("ahci")
        || line_lower.contains("virtio-blk")
        || line_lower.contains("ext4")
        || line_lower.contains("btrfs")
        || line_lower.contains("xfs")
        || line_lower.contains("f2fs")
    {
        LogSubsystem::Disk
    } else if line_lower.contains("pkg:")
        || line_lower.contains("pacman")
        || line_lower.contains("mirror")
        || line_lower.contains(".db")
        || line_lower.contains("signature")
        || line_lower.contains("keyring")
    {
        LogSubsystem::Pkg
    } else if line_lower.contains("usb:") || line_lower.contains("xhci") || line_lower.contains("tether") {
        LogSubsystem::Usb
    } else if line_lower.contains("boot:") || line_lower.contains("init:") || line_lower.contains("kernel:") {
        LogSubsystem::Init
    } else {
        LogSubsystem::General
    }
}

fn detect_level(line_lower: &str) -> LogLevel {
    if line_lower.contains("error:") || line_lower.contains("fatal:") || line_lower.contains("panic:") {
        LogLevel::Error
    } else if line_lower.contains("warning:") || line_lower.contains("warn:") {
        LogLevel::Warn
    } else if line_lower.contains("debug:") {
        LogLevel::Debug
    } else if line_lower.contains("trace:") {
        LogLevel::Trace
    } else {
        LogLevel::Info
    }
}

pub fn parse_log(raw_text: &str) -> Vec<LogEntry> {
    let mut entries = Vec::new();
    for line in raw_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (ts, rest) = parse_timestamp(trimmed);
        let line_lower = rest.to_ascii_lowercase();
        let level = detect_level(&line_lower);
        let subsystem = detect_subsystem(&line_lower);
        entries.push(LogEntry {
            raw: trimmed.to_string(),
            timestamp_ms: ts,
            level,
            subsystem,
            message: rest.to_string(),
        });
    }
    entries
}

pub fn analyze_log(entries: &[LogEntry]) -> Vec<Diagnosis> {
    let mut diagnoses = Vec::new();
    let mut seen_dhcp = false;
    let mut seen_dns = false;
    let mut seen_disk = false;
    let mut seen_404 = false;
    let mut seen_sig = false;

    for entry in entries {
        let msg = entry.message.to_ascii_lowercase();

        if !seen_dhcp && (msg.contains("dhcp timeout") || msg.contains("dhcp failed")) {
            seen_dhcp = true;
            diagnoses.push(Diagnosis {
                category: DiagnosticCategory::DhcpTimeout,
                summary: "DHCP lease acquisition timed out on candidate network interface.".into(),
                remediation: "Verify Ethernet cable link and router DHCP service. Enable USB tethering to use a mobile hotspot if wired network requires authentication.".into(),
                action: Some(RemediationAction::EnableTethering),
            });
        }

        if !seen_dns && (msg.contains("dns timeout") || msg.contains("dns lookup failed")) {
            seen_dns = true;
            diagnoses.push(Diagnosis {
                category: DiagnosticCategory::DnsTimeout,
                summary: "DNS resolution timed out for mirror hostnames.".into(),
                remediation: "Check upstream gateway DNS availability or configure static fallback resolvers.".into(),
                action: None,
            });
        }

        if !seen_disk && (msg.contains("disk serial mismatch") || msg.contains("refusing disk")) {
            seen_disk = true;
            diagnoses.push(Diagnosis {
                category: DiagnosticCategory::DiskSerialMismatch,
                summary: "Target disk serial did not match configured confirmation serial.".into(),
                remediation: "Verify disk serial in Disk settings, or enable 'Erase and install onto the largest disk' (auto_largest) to bypass manual serial confirmation.".into(),
                action: Some(RemediationAction::EnableAutoLargest),
            });
        }

        if !seen_404 && (msg.contains("404") || msg.contains("not found")) && (msg.contains("mirror") || msg.contains(".db")) {
            seen_404 = true;
            diagnoses.push(Diagnosis {
                category: DiagnosticCategory::MirrorError404,
                summary: "Pacman mirror returned HTTP 404 Not Found for core or extra package database.".into(),
                remediation: "The primary mirror may be stale or out of sync. Switch primary mirror to https://geo.mirror.pkgbuild.com/$repo/os/$arch.".into(),
                action: Some(RemediationAction::SwitchMirror("https://geo.mirror.pkgbuild.com/$repo/os/$arch".into())),
            });
        }

        if !seen_sig && (msg.contains("signature verification failed") || msg.contains("bad signature")) {
            seen_sig = true;
            diagnoses.push(Diagnosis {
                category: DiagnosticCategory::SignatureError,
                summary: "Cryptographic signature verification failed against Arch master keyring.".into(),
                remediation: "Update the installer master keyring or check system hardware clock/RTC synchronization.".into(),
                action: None,
            });
        }
    }

    diagnoses
}

pub fn filter_entries<'a>(
    entries: &'a [LogEntry],
    min_level: LogLevel,
    query: Option<&str>,
) -> Vec<&'a LogEntry> {
    let q_lower = query.map(|q| q.trim().to_ascii_lowercase());
    entries
        .iter()
        .filter(|e| {
            if e.level < min_level {
                return false;
            }
            if let Some(ref q) = q_lower {
                if !q.is_empty() && !e.message.to_ascii_lowercase().contains(q) && !e.raw.to_ascii_lowercase().contains(q) {
                    return false;
                }
            }
            true
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_LOG: &str = r#"
[   0.010] boot: entering kernel with 4096 MiB RAM
[   0.050] pci: 00:03.0 8086:100e network (Intel 82540EM)
[   0.060] pci: 00:04.0 1af4:1001 storage (virtio-blk)
[   0.500] dhcp: sending discover on eth0
[   5.500] warning: dhcp timeout waiting for lease on eth0
[   5.520] error: disk serial mismatch: expected 'S123', found 'TESTDISK0'
[   6.000] pkg: downloading core.db from https://geo.mirror.pkgbuild.com
[   6.100] error: mirror HTTP 404: https://geo.mirror.pkgbuild.com/core.db
[   6.200] error: signature verification failed for extra.db
"#;

    #[test]
    fn parses_plain_and_tagged_log_lines() {
        let entries = parse_log(SAMPLE_LOG);
        assert!(!entries.is_empty());
        assert_eq!(entries[0].timestamp_ms, Some(10));
        assert_eq!(entries[0].subsystem, LogSubsystem::Init);
        assert_eq!(entries[0].level, LogLevel::Info);

        let pci = &entries[1];
        assert_eq!(pci.timestamp_ms, Some(50));
        assert_eq!(pci.subsystem, LogSubsystem::Pci);

        let dhcp_warn = entries.iter().find(|e| e.message.contains("dhcp timeout")).unwrap();
        assert_eq!(dhcp_warn.level, LogLevel::Warn);
        assert_eq!(dhcp_warn.subsystem, LogSubsystem::Net);

        let disk_err = entries.iter().find(|e| e.message.contains("disk serial mismatch")).unwrap();
        assert_eq!(disk_err.level, LogLevel::Error);
        assert_eq!(disk_err.subsystem, LogSubsystem::Disk);
    }

    #[test]
    fn diagnoses_common_installer_errors() {
        let entries = parse_log(SAMPLE_LOG);
        let diagnoses = analyze_log(&entries);
        assert!(diagnoses.len() >= 3);

        let dhcp_diag = diagnoses.iter().find(|d| d.category == DiagnosticCategory::DhcpTimeout).unwrap();
        assert!(dhcp_diag.remediation.contains("tethering") || dhcp_diag.remediation.contains("network"));

        let disk_diag = diagnoses.iter().find(|d| d.category == DiagnosticCategory::DiskSerialMismatch).unwrap();
        assert_eq!(disk_diag.action, Some(RemediationAction::EnableAutoLargest));

        let mirror_diag = diagnoses.iter().find(|d| d.category == DiagnosticCategory::MirrorError404).unwrap();
        assert!(mirror_diag.summary.contains("404"));

        let sig_diag = diagnoses.iter().find(|d| d.category == DiagnosticCategory::SignatureError).unwrap();
        assert!(sig_diag.summary.contains("signature"));
    }

    #[test]
    fn filters_entries_by_level_and_query() {
        let entries = parse_log(SAMPLE_LOG);
        let only_errors = filter_entries(&entries, LogLevel::Error, None);
        assert_eq!(only_errors.len(), 3);
        assert!(only_errors.iter().all(|e| e.level == LogLevel::Error));

        let pci_search = filter_entries(&entries, LogLevel::Info, Some("pci"));
        assert_eq!(pci_search.len(), 2);
    }
}
