//! Cross-checks against the host's pacman: sync databases in /var/lib/pacman/sync, `vercmp`,
//! and `pacman -Sp` resolution with an empty local database.
use pkg::db::Db;
use pkg::resolve::Resolver;
use pkg::vercmp::vercmp;
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

fn load(name: &str) -> Option<Db> {
    let data = std::fs::read(format!("/var/lib/pacman/sync/{name}.db")).ok()?;
    Some(Db::parse(name, data.as_slice()).unwrap_or_else(|e| panic!("{name}.db: {e:?}")))
}

fn dbs() -> Option<Vec<Db>> {
    Some(vec![load("core")?, load("extra")?])
}

#[test]
fn parses_real_databases() {
    let Some(dbs) = dbs() else { return };
    assert!(dbs[0].packages.len() > 100, "core");
    assert!(dbs[1].packages.len() > 5000, "extra");
    let glibc = dbs[0].packages.iter().find(|p| p.name == "glibc").unwrap();
    assert_eq!(glibc.sha256.len(), 64);
    assert!(!glibc.pgpsig.is_empty());
    let openssl = dbs[0].packages.iter().find(|p| p.name == "openssl").unwrap();
    assert!(openssl.provides.iter().any(|p| p.name == "libssl.so" && p.version.contains('-')), "{openssl:?}");
}

#[test]
fn vercmp_matches_pacman() {
    let Some(dbs) = dbs() else { return };
    if !std::path::Path::new("/usr/bin/vercmp").exists() {
        return;
    }
    let versions: Vec<&str> = dbs.iter().flat_map(|d| d.packages.iter()).map(|p| p.version.as_str()).collect();
    let mut checked = 0;
    let step = versions.len() / 700 + 1;
    for i in (0..versions.len()).step_by(step) {
        for j in [i + 1, i + 37, i + 401] {
            if j >= versions.len() {
                continue;
            }
            let (a, b) = (versions[i], versions[j]);
            let out = Command::new("/usr/bin/vercmp").args([a, b]).output().unwrap();
            let want: i32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
            let got = vercmp(a, b) as i32;
            assert_eq!(got, want, "vercmp {a} {b}");
            checked += 1;
        }
    }
    assert!(checked > 1000);
}

/// Runs `pacman -Sp` against a scratch db path so no local packages influence the result.
fn pacman_targets(args: &[&str]) -> Option<BTreeSet<String>> {
    let dir = std::env::temp_dir().join(format!("pkgtest-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("db/sync")).ok()?;
    std::fs::create_dir_all(dir.join("db/local")).ok()?;
    std::fs::create_dir_all(dir.join("root")).ok()?;
    for r in ["core", "extra"] {
        std::fs::copy(format!("/var/lib/pacman/sync/{r}.db"), dir.join(format!("db/sync/{r}.db"))).ok()?;
    }
    let conf = dir.join("pacman.conf");
    std::fs::write(&conf, "[options]\nArchitecture = x86_64\nSigLevel = Never\n[core]\nServer = file:///nonexistent\n[extra]\nServer = file:///nonexistent\n").ok()?;
    let out = Command::new("pacman")
        .args(["--config"]).arg(&conf)
        .args(["--dbpath"]).arg(dir.join("db"))
        .args(["--root"]).arg(dir.join("root"))
        .args(["--noconfirm", "-Sp", "--print-format", "%n"])
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!("pacman failed: {}", String::from_utf8_lossy(&out.stderr));
        return None;
    }
    let _ = std::fs::remove_dir_all(&dir);
    Some(String::from_utf8_lossy(&out.stdout).lines().map(|s| s.to_string()).collect())
}

fn ours(dbs: &[Db], req: &[&str], providers: &[(&str, &str)]) -> BTreeSet<String> {
    let map: BTreeMap<String, String> = providers.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    let r = Resolver::new(dbs, &map);
    let req: Vec<String> = req.iter().map(|s| s.to_string()).collect();
    r.resolve(&req).unwrap().packages.into_iter().map(|s| s.pkg.name.clone()).collect()
}

#[test]
fn resolves_like_pacman() {
    let Some(dbs) = dbs() else { return };
    for (req, providers) in [
        (vec!["base"], vec![]),
        (vec!["base", "linux", "linux-firmware", "mkinitcpio"], vec![]),
        (vec!["base-devel"], vec![]),
        (vec!["openssh", "sudo", "vim", "git"], vec![]),
    ] {
        let Some(want) = pacman_targets(&req) else { return };
        let got = ours(&dbs, &req, &providers);
        assert_eq!(got, want, "request {req:?}\nonly ours: {:?}\nonly pacman: {:?}", got.difference(&want).collect::<Vec<_>>(), want.difference(&got).collect::<Vec<_>>());
    }
}

#[test]
fn provider_choice_from_config() {
    let Some(dbs) = dbs() else { return };
    // Without config the first candidate by repo priority is used and reported.
    let map = BTreeMap::new();
    let r = Resolver::new(&dbs, &map).resolve(&["linux".to_string()]).unwrap();
    let amb = r.ambiguities.iter().find(|a| a.dep == "initramfs").expect("initramfs is ambiguous");
    assert_eq!(amb.chosen, "mkinitcpio");
    assert!(amb.candidates.len() > 1);
    // Config override wins.
    let mut map = BTreeMap::new();
    map.insert("initramfs".to_string(), "dracut".to_string());
    let r = Resolver::new(&dbs, &map).resolve(&["linux".to_string()]).unwrap();
    let names: Vec<_> = r.packages.iter().map(|s| s.pkg.name.clone()).collect();
    assert!(names.contains(&"dracut".to_string()) && !names.contains(&"mkinitcpio".to_string()));
    let pos = |n: &str| names.iter().position(|x| x == n).unwrap();
    assert!(pos("dracut") < pos("linux"));
}

#[test]
fn reads_cached_packages_like_tar() {
    use pkg::tar::{Kind, TarReader};
    let Ok(rd) = std::fs::read_dir("/var/cache/pacman/pkg") else { return };
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with(".pkg.tar.zst")).collect();
    files.sort();
    let mut checked = 0;
    let mut saw_xattr = false;
    for path in files.iter().step_by(files.len() / 25 + 1) {
        let data = std::fs::read(path).unwrap();
        let mut tar = TarReader::new(pkg::compress::open(data.as_slice()).unwrap());
        let mut names = Vec::new();
        let mut total = 0u64;
        while let Some(e) = tar.next_entry().unwrap() {
            if e.kind == Kind::File {
                let d = tar.read_all().unwrap();
                assert_eq!(d.len() as u64, e.size);
                total += e.size;
            }
            saw_xattr |= !e.xattrs.is_empty();
            names.push(e.path);
        }
        let out = Command::new("tar").arg("--zstd").arg("-tf").arg(path).output().unwrap();
        let want: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(|s| s.to_string()).collect();
        assert_eq!(names, want, "{}", path.display());
        assert!(total > 0 || names.len() < 6);
        checked += 1;
    }
    eprintln!("checked {checked} packages, xattrs seen: {saw_xattr}");
}

#[test]
fn parses_pax_xattrs() {
    use pkg::tar::TarReader;
    let Ok(rd) = std::fs::read_dir("/var/cache/pacman/pkg") else { return };
    let Some(path) = rd.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("gstreamer-1") && n.to_string_lossy().ends_with(".pkg.tar.zst"))) else { return };
    let data = std::fs::read(path).unwrap();
    let mut tar = TarReader::new(pkg::compress::open(data.as_slice()).unwrap());
    let mut caps = 0;
    while let Some(e) = tar.next_entry().unwrap() {
        for (k, v) in &e.xattrs {
            if k == "security.capability" {
                assert!(v.len() >= 12 && v[0] == 1 || v[0] == 2 || v[0] == 3, "capability blob {v:?}");
                caps += 1;
            }
        }
    }
    assert!(caps > 0, "gstreamer should carry file capabilities");
}
