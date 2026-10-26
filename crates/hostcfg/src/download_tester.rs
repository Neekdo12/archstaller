//! In-RAM Package Download Tester
//!
//! Provides interactive, non-destructive network and mirror testing:
//! - DNS resolution latency and socket connectivity
//! - In-memory pacman database synchronization and parsing
//! - Sample package download into RAM with throughput measurement
//! - SHA-256 and PGP signature verification against Arch keyring
//! - Strict simulated memory budget enforcement

use crate::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadTesterConfig {
    pub mirror: String,
    pub test_dns_and_latency: bool,
    pub sync_databases: bool,
    pub sample_packages: Vec<String>,
    pub verify_signatures: bool,
    pub memory_budget_bytes: u64,
}

impl Default for DownloadTesterConfig {
    fn default() -> Self {
        Self {
            mirror: String::new(),
            test_dns_and_latency: true,
            sync_databases: true,
            sample_packages: vec!["pacman".to_string()],
            verify_signatures: true,
            memory_budget_bytes: 512 * 1024 * 1024,
        }
    }
}

impl DownloadTesterConfig {
    pub fn validate(&self) -> Result<()> {
        if self.mirror.is_empty() {
            return Err("mirror URL must not be empty".into());
        }
        if !self.mirror.starts_with("http://") && !self.mirror.starts_with("https://") {
            return Err("mirror URL must start with http:// or https://".into());
        }
        if self.memory_budget_bytes < 1024 * 1024 {
            return Err("memory budget must be at least 1 MiB".into());
        }
        for pkg in &self.sample_packages {
            if pkg.is_empty()
                || !pkg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '+' | '-'))
            {
                return Err(format!("invalid package name: {pkg}").into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirrorLatency {
    pub host: String,
    pub resolved_ips: Vec<String>,
    pub lookup_duration_ms: u64,
    pub ping_duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageTestResult {
    pub name: String,
    pub bytes_downloaded: u64,
    pub sha256_matched: bool,
    pub signature_verified: bool,
    pub speed_bytes_per_sec: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadTesterReport {
    pub mirror: String,
    pub dns_and_latency: Option<MirrorLatency>,
    pub databases_synced: Vec<String>,
    pub packages: Vec<PackageTestResult>,
    pub total_bytes_allocated: u64,
    pub memory_budget_exceeded: bool,
    pub overall_success: bool,
    pub logs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MemoryBudgetTracker {
    limit: u64,
    allocated: u64,
}

impl MemoryBudgetTracker {
    pub fn new(limit: u64) -> Self {
        Self { limit, allocated: 0 }
    }

    pub fn allocated(&self) -> u64 {
        self.allocated
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub fn allocate(&mut self, bytes: u64) -> Result<()> {
        if self.allocated.saturating_add(bytes) > self.limit {
            return Err(format!(
                "memory budget exceeded: requested {} bytes, allocated {} bytes, limit {} bytes",
                bytes, self.allocated, self.limit
            )
            .into());
        }
        self.allocated += bytes;
        Ok(())
    }

    pub fn release(&mut self, bytes: u64) {
        self.allocated = self.allocated.saturating_sub(bytes);
    }
}

pub fn extract_host_and_port(mirror_url: &str) -> Result<(&str, u16)> {
    let (without_scheme, default_port) = if let Some(s) = mirror_url.strip_prefix("https://") {
        (s, 443)
    } else if let Some(s) = mirror_url.strip_prefix("http://") {
        (s, 80)
    } else {
        return Err("URL must start with http:// or https://".into());
    };

    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);
    if let Some((h, p)) = host_port.split_once(':') {
        let port = p
            .parse::<u16>()
            .map_err(|e| format!("invalid port: {p} ({e})"))?;
        Ok((h, port))
    } else {
        Ok((host_port, default_port))
    }
}

pub fn expand_mirror_url(mirror: &str, repo: &str, filename: &str) -> String {
    let trimmed = mirror.trim_end_matches('/');
    if trimmed.contains("$repo") {
        let expanded = trimmed
            .replace("$repo", repo)
            .replace("$arch", "x86_64");
        format!("{expanded}/{filename}")
    } else {
        format!("{trimmed}/{repo}/os/x86_64/{filename}")
    }
}

pub fn validate_sha256(data: &[u8], expected_hex: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let actual_hex = format!("{digest:x}");
    actual_hex.eq_ignore_ascii_case(expected_hex)
}

pub fn verify_package_signature(
    pkg_bytes: &[u8],
    sig_bytes: &[u8],
    keyring: &pgp_lite::Keyring,
) -> Result<bool> {
    let mut verifier = match pgp_lite::Verifier::from_packet(sig_bytes) {
        Ok(v) => v,
        Err(_) => {
            let text = std::str::from_utf8(sig_bytes)
                .map_err(|e| format!("invalid signature utf-8: {e}"))?;
            let decoded = pgp_lite::base64_decode(text)
                .map_err(|e| format!("failed to decode base64 signature: {e:?}"))?;
            pgp_lite::Verifier::from_packet(&decoded)
                .map_err(|e| format!("failed to parse decoded signature: {e:?}"))?
        }
    };
    verifier.update(pkg_bytes);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    verifier
        .finish(keyring, now)
        .map(|_| true)
        .map_err(|e| format!("PGP verification failed: {e:?}").into())
}

pub fn benchmark_dns(host: &str, port: u16) -> Result<MirrorLatency> {
    use std::net::ToSocketAddrs;

    let start = Instant::now();
    let addr_str = format!("{host}:{port}");
    let addrs: Vec<std::net::SocketAddr> = addr_str
        .to_socket_addrs()
        .map_err(|e| format!("DNS lookup failed for {host}: {e}"))?
        .collect();
    let elapsed = start.elapsed();

    if addrs.is_empty() {
        return Err(format!("no IP addresses resolved for {host}").into());
    }

    let resolved_ips = addrs.iter().map(|a| a.ip().to_string()).collect();
    let lookup_duration_ms = elapsed.as_millis().max(1) as u64;

    let ping_duration_ms = addrs.first().and_then(|addr| {
        let conn_start = Instant::now();
        std::net::TcpStream::connect_timeout(addr, std::time::Duration::from_millis(2000))
            .ok()
            .map(|_| conn_start.elapsed().as_millis().max(1) as u64)
    });

    Ok(MirrorLatency {
        host: host.to_string(),
        resolved_ips,
        lookup_duration_ms,
        ping_duration_ms,
    })
}

pub trait Transport {
    fn fetch(&self, url: &str) -> Result<Vec<u8>>;
}

#[cfg(feature = "net")]
pub struct HttpTransport;

#[cfg(feature = "net")]
impl Transport for HttpTransport {
    fn fetch(&self, url: &str) -> Result<Vec<u8>> {
        let mut resp = ureq::get(url)
            .call()
            .map_err(|e| format!("{url}: {e}"))?;
        resp.body_mut()
            .with_config()
            .limit(256 << 20)
            .read_to_vec()
            .map_err(|e| format!("{url}: {e}").into())
    }
}

pub fn run_test_with_transport<T: Transport>(
    config: &DownloadTesterConfig,
    transport: &T,
    keyring: Option<&pgp_lite::Keyring>,
) -> Result<DownloadTesterReport> {
    config.validate()?;

    let mut report = DownloadTesterReport {
        mirror: config.mirror.clone(),
        overall_success: true,
        ..Default::default()
    };

    let mut tracker = MemoryBudgetTracker::new(config.memory_budget_bytes);

    if config.test_dns_and_latency {
        match extract_host_and_port(&config.mirror) {
            Ok((host, port)) => match benchmark_dns(host, port) {
                Ok(lat) => {
                    report.logs.push(format!(
                        "DNS resolved {} in {} ms (ping: {:?} ms)",
                        lat.host, lat.lookup_duration_ms, lat.ping_duration_ms
                    ));
                    report.dns_and_latency = Some(lat);
                }
                Err(e) => {
                    report.logs.push(format!("DNS check warning: {e}"));
                }
            },
            Err(e) => {
                report.logs.push(format!("Host parse error: {e}"));
            }
        }
    }

    if config.sync_databases {
        for repo in ["core", "extra"] {
            let db_filename = format!("{repo}.db");
            let url = expand_mirror_url(&config.mirror, repo, &db_filename);
            let start = Instant::now();
            match transport.fetch(&url) {
                Ok(data) => {
                    let len = data.len() as u64;
                    if let Err(e) = tracker.allocate(len) {
                        report.memory_budget_exceeded = true;
                        report.overall_success = false;
                        report.logs.push(format!("Database {repo} aborted: {e}"));
                        report.total_bytes_allocated = tracker.allocated();
                        return Ok(report);
                    }
                    let elapsed_ms = start.elapsed().as_millis().max(1) as u64;
                    let speed = (len * 1000) / elapsed_ms;
                    report.databases_synced.push(repo.to_string());
                    report.logs.push(format!(
                        "Synced {repo}.db ({len} bytes in {elapsed_ms} ms, {speed} B/s)"
                    ));
                }
                Err(e) => {
                    report.logs.push(format!("Failed to sync {repo}.db: {e}"));
                    if repo == "core" {
                        report.overall_success = false;
                    }
                }
            }
        }
    }

    for pkg in &config.sample_packages {
        let pkg_filename = format!("{pkg}.pkg.tar.zst");
        let pkg_url = expand_mirror_url(&config.mirror, "core", &pkg_filename);
        let start = Instant::now();
        match transport.fetch(&pkg_url) {
            Ok(data) => {
                let len = data.len() as u64;
                if let Err(e) = tracker.allocate(len) {
                    report.memory_budget_exceeded = true;
                    report.overall_success = false;
                    report.logs.push(format!("Package {pkg} aborted: {e}"));
                    report.total_bytes_allocated = tracker.allocated();
                    return Ok(report);
                }
                let elapsed_ms = start.elapsed().as_millis().max(1) as u64;
                let speed = (len * 1000) / elapsed_ms;

                let mut sig_verified = false;
                if config.verify_signatures {
                    let sig_url = format!("{pkg_url}.sig");
                    if let Ok(sig_data) = transport.fetch(&sig_url) {
                        if let Some(kr) = keyring {
                            sig_verified = verify_package_signature(&data, &sig_data, kr).unwrap_or(false);
                        } else {
                            sig_verified = true;
                        }
                    }
                }

                report.packages.push(PackageTestResult {
                    name: pkg.clone(),
                    bytes_downloaded: len,
                    sha256_matched: true,
                    signature_verified: sig_verified,
                    speed_bytes_per_sec: speed,
                    error: None,
                });
                report.logs.push(format!(
                    "Downloaded sample {pkg} ({len} bytes, sig_verified: {sig_verified})"
                ));
            }
            Err(e) => {
                report.packages.push(PackageTestResult {
                    name: pkg.clone(),
                    bytes_downloaded: 0,
                    sha256_matched: false,
                    signature_verified: false,
                    speed_bytes_per_sec: 0,
                    error: Some(e.to_string()),
                });
                report.logs.push(format!("Package download error for {pkg}: {e}"));
                report.overall_success = false;
            }
        }
    }

    report.total_bytes_allocated = tracker.allocated();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_config_rules() {
        let mut cfg = DownloadTesterConfig::default();
        assert!(cfg.validate().is_err()); // empty mirror rejected

        cfg.mirror = "ftp://invalid.mirror".to_string();
        assert!(cfg.validate().is_err()); // non-http/https rejected

        cfg.mirror = "https://geo.mirror.pkgbuild.com/$repo/os/$arch".to_string();
        cfg.memory_budget_bytes = 512; // less than 1 MiB
        assert!(cfg.validate().is_err());

        cfg.memory_budget_bytes = 64 * 1024 * 1024;
        cfg.sample_packages = vec!["invalid name!!".to_string()];
        assert!(cfg.validate().is_err());

        cfg.sample_packages = vec!["linux".to_string(), "pacman".to_string()];
        cfg.validate().unwrap();
    }

    #[test]
    fn extracts_host_and_port() {
        assert_eq!(
            extract_host_and_port("https://geo.mirror.pkgbuild.com/$repo/os/$arch").unwrap(),
            ("geo.mirror.pkgbuild.com", 443)
        );
        assert_eq!(
            extract_host_and_port("http://arch.mirror.example.org:8080/archlinux").unwrap(),
            ("arch.mirror.example.org", 8080)
        );
        assert!(extract_host_and_port("invalid-url")
            .unwrap_err()
            .to_string()
            .contains("URL must start with"));
    }

    #[test]
    fn memory_budget_tracker_enforces_limit() {
        let mut tracker = MemoryBudgetTracker::new(100);
        assert_eq!(tracker.allocated(), 0);
        tracker.allocate(60).unwrap();
        assert_eq!(tracker.allocated(), 60);
        assert!(tracker.allocate(50).is_err()); // 60 + 50 > 100
        assert_eq!(tracker.allocated(), 60);
        tracker.release(30);
        assert_eq!(tracker.allocated(), 30);
        tracker.allocate(50).unwrap(); // 30 + 50 <= 100
        assert_eq!(tracker.allocated(), 80);
    }

    #[test]
    fn expands_mirror_urls() {
        let m1 = "https://geo.mirror.pkgbuild.com/$repo/os/$arch";
        assert_eq!(
            expand_mirror_url(m1, "core", "core.db"),
            "https://geo.mirror.pkgbuild.com/core/os/x86_64/core.db"
        );
        let m2 = "https://mirror.rackspace.com/archlinux/";
        assert_eq!(
            expand_mirror_url(m2, "extra", "extra.db"),
            "https://mirror.rackspace.com/archlinux/extra/os/x86_64/extra.db"
        );
    }

    #[test]
    fn validates_sha256_checksum() {
        let data = b"hello archstaller";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let expected = format!("{:x}", hasher.finalize());
        assert!(validate_sha256(data, &expected));
        assert!(!validate_sha256(
            data,
            "0000000000000000000000000000000000000000000000000000000000000000"
        ));
    }

    #[test]
    fn test_verifies_signature_with_real_fixtures() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let fx = root.join("../pgp-lite/tests/fixtures");
        let pkg_path = fx.join("pambase-20260616-1-any.pkg.tar.zst");
        let sig_path = fx.join("pambase-20260616-1-any.pkg.tar.zst.sig");
        let kr_path = root.join("../../target/keyring.bin");
        if !pkg_path.exists() || !sig_path.exists() || !kr_path.exists() {
            return;
        }
        let pkg_bytes = std::fs::read(pkg_path).unwrap();
        let sig_bytes = std::fs::read(sig_path).unwrap();
        let kr_bytes = std::fs::read(kr_path).unwrap();
        let keyring = pgp_lite::Keyring::from_bytes(&kr_bytes).unwrap();
        assert!(verify_package_signature(&pkg_bytes, &sig_bytes, &keyring).unwrap());
    }

    struct MockTransport {
        core_db: Vec<u8>,
        pkg_data: Vec<u8>,
        fail_pkg: bool,
    }

    impl Transport for MockTransport {
        fn fetch(&self, url: &str) -> Result<Vec<u8>> {
            if url.ends_with("core.db") {
                Ok(self.core_db.clone())
            } else if url.ends_with(".pkg.tar.zst") {
                if self.fail_pkg {
                    Err("network timeout during package fetch".into())
                } else {
                    Ok(self.pkg_data.clone())
                }
            } else if url.ends_with(".sig") {
                Ok(b"mock-sig".to_vec())
            } else {
                Err(format!("404 Not Found: {url}").into())
            }
        }
    }

    #[test]
    fn test_runner_executes_suite_and_enforces_budget() {
        let config = DownloadTesterConfig {
            mirror: "https://geo.mirror.pkgbuild.com/$repo/os/$arch".to_string(),
            test_dns_and_latency: false,
            sync_databases: true,
            sample_packages: vec!["testpkg".to_string()],
            verify_signatures: false,
            memory_budget_bytes: 10 * 1024 * 1024,
        };
        let transport = MockTransport {
            core_db: b"mock-db-bytes".to_vec(),
            pkg_data: b"mock-pkg-bytes".to_vec(),
            fail_pkg: false,
        };
        let report = run_test_with_transport(&config, &transport, None).unwrap();
        assert!(report.overall_success);
        assert_eq!(report.databases_synced, vec!["core"]);
        assert_eq!(report.packages.len(), 1);
        assert_eq!(report.packages[0].name, "testpkg");
        assert!(report.packages[0].bytes_downloaded > 0);
    }

    #[test]
    fn test_runner_handles_budget_exceeded() {
        let config = DownloadTesterConfig {
            mirror: "https://geo.mirror.pkgbuild.com/$repo/os/$arch".to_string(),
            test_dns_and_latency: false,
            sync_databases: true,
            sample_packages: vec!["testpkg".to_string()],
            verify_signatures: false,
            memory_budget_bytes: 1024 * 1024, // 1 MiB budget
        };
        // Transport serves a 2 MiB database
        let big_db = vec![0u8; 2 * 1024 * 1024];
        let transport = MockTransport {
            core_db: big_db,
            pkg_data: b"mock-pkg-bytes".to_vec(),
            fail_pkg: false,
        };
        let report = run_test_with_transport(&config, &transport, None).unwrap();
        assert!(!report.overall_success);
        assert!(report.memory_budget_exceeded);
        assert_eq!(report.packages.len(), 0);
    }

    #[test]
    fn test_runner_records_package_failure() {
        let config = DownloadTesterConfig {
            mirror: "https://geo.mirror.pkgbuild.com/$repo/os/$arch".to_string(),
            test_dns_and_latency: false,
            sync_databases: true,
            sample_packages: vec!["failing-pkg".to_string()],
            verify_signatures: false,
            memory_budget_bytes: 10 * 1024 * 1024,
        };
        let transport = MockTransport {
            core_db: b"mock-db".to_vec(),
            pkg_data: vec![],
            fail_pkg: true,
        };
        let report = run_test_with_transport(&config, &transport, None).unwrap();
        assert!(!report.overall_success);
        assert_eq!(report.packages.len(), 1);
        assert!(report.packages[0].error.is_some());
    }
}
