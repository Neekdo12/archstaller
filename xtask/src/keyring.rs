//! Builds the compact signing-key blob from the pinned archlinux-keyring package.
use crate::{root, run, Result};
use pgp_lite::{KeyEntry, KeyMaterial, Keyring};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::process::Command;

const MIRROR: &str = "https://geo.mirror.pkgbuild.com";
/// Packager keys need certifications from at least this many main keys.
const MIN_MAIN_SIGS: usize = 3;

struct Pin {
    version: String,
    sha256: String,
}

fn pin_path() -> PathBuf {
    root().join("xtask/keyring.pin")
}

/// `xtask/keyring.pin` holds "<version> <sha256>" of the archlinux-keyring package.
fn read_pin() -> Result<Pin> {
    let text = std::fs::read_to_string(pin_path())?;
    let mut it = text.split_whitespace();
    match (it.next(), it.next()) {
        (Some(v), Some(h)) if h.len() == 64 => Ok(Pin { version: v.into(), sha256: h.into() }),
        _ => Err("xtask/keyring.pin must contain '<version> <sha256>'".into()),
    }
}

fn sha256_hex(path: &std::path::Path) -> Result<String> {
    Ok(Sha256::digest(std::fs::read(path)?).iter().map(|b| format!("{b:02x}")).collect())
}

/// Downloads the package for `version`: the archive keeps every version, the mirror only the newest.
fn fetch_package(version: &str, dest: &std::path::Path) -> Result<()> {
    let name = format!("archlinux-keyring-{version}-any.pkg.tar.zst");
    let urls = [
        format!("https://archive.archlinux.org/packages/a/archlinux-keyring/{name}"),
        format!("{MIRROR}/core/os/x86_64/{name}"),
    ];
    for url in &urls {
        if Command::new("curl").args(["-fsSL", "-o"]).arg(dest).arg(url).status()?.success() {
            return Ok(());
        }
    }
    Err(format!("could not download {name}").into())
}

/// Moves the pin to the newest archlinux-keyring in the mirror's core database.
pub fn update() -> Result<()> {
    use pkg::db::Db;
    let dir = root().join("target/keyring");
    std::fs::create_dir_all(&dir)?;
    let db_path = dir.join("core.db");
    run(Command::new("curl").args(["-fsSL", "-o"]).arg(&db_path).arg(format!("{MIRROR}/core/os/x86_64/core.db")))?;
    let data = std::fs::read(&db_path)?;
    let db = Db::parse("core", data.as_slice()).map_err(|e| format!("core.db: {e:?}"))?;
    let latest = db.packages.iter().find(|p| p.name == "archlinux-keyring").ok_or("archlinux-keyring not in core.db")?;
    let old = read_pin()?;
    println!("pinned keyring: {}   newest on the mirror: {}", old.version, latest.version);
    if latest.version == old.version {
        println!("already up to date");
        return Ok(());
    }

    let file = dir.join(&latest.filename);
    fetch_package(&latest.version, &file)?;
    let got = sha256_hex(&file)?;
    if got != latest.sha256 {
        let _ = std::fs::remove_file(&file);
        return Err(format!("downloaded keyring has sha256 {got}, database says {}", latest.sha256).into());
    }

    // Check the package signature against the keyring we already trust, if there is one.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    let verdict = (|| -> std::result::Result<[u8; 20], String> {
        let blob = std::fs::read(blob_path()).map_err(|_| "no existing keyring blob".to_string())?;
        let ring = Keyring::from_bytes(&blob).ok_or("unreadable keyring blob")?;
        let sig = pgp_lite::base64_decode(&latest.pgpsig).map_err(|e| format!("{e:?}"))?;
        let mut v = pgp_lite::Verifier::from_packet(&sig).map_err(|e| format!("{e:?}"))?;
        v.update(&std::fs::read(&file).map_err(|e| e.to_string())?);
        v.finish(&ring, now).map_err(|e| format!("{e:?}"))
    })();
    match verdict {
        Ok(_) => println!("signature verified with the previously pinned keyring"),
        Err(e) => println!("note: signature not verified by the old keyring ({e}); trusting the HTTPS mirror and its database checksum"),
    }

    std::fs::write(pin_path(), format!("{} {}\n", latest.version, latest.sha256))?;
    println!("pin updated: {}", latest.version);
    build()?; // fail here, not at the next ISO build, if the new keyring does not parse
    Ok(())
}

pub fn blob_path() -> PathBuf {
    root().join("target/keyring.bin")
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
}

fn fpr_bytes(s: &str) -> Option<[u8; 20]> {
    let v = unhex(s);
    v.try_into().ok()
}

#[derive(Default)]
struct Key {
    fpr: Option<[u8; 20]>,
    algo: u32,
    validity: char,
    expires: Option<u64>,
    can_sign: bool,
    pkd: Vec<String>,
}

impl Key {
    fn usable(&self) -> bool {
        !matches!(self.validity, 'r' | 'e' | 'i' | 'd' | 'n')
    }

    fn material(&self) -> Option<KeyMaterial> {
        match self.algo {
            1 if self.pkd.len() >= 2 => Some(KeyMaterial::Rsa { n: unhex(&self.pkd[0]), e: unhex(&self.pkd[1]) }),
            22 if self.pkd.len() >= 2 => {
                let oid = &self.pkd[0];
                let point = unhex(&self.pkd[1]);
                if oid == "092B06010401DA470F01" && point.len() == 33 && point[0] == 0x40 {
                    Some(KeyMaterial::Ed25519(point[1..].try_into().unwrap()))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

struct Primary {
    key: Key,
    subs: Vec<Key>,
    signers: BTreeSet<[u8; 20]>,
}

pub fn build() -> Result<Vec<u8>> {
    let dir = root().join("target/keyring");
    std::fs::create_dir_all(&dir)?;
    let pin = read_pin()?;
    let version = pin.version.as_str();
    let pkg = dir.join(format!("archlinux-keyring-{version}-any.pkg.tar.zst"));
    if !pkg.exists() {
        fetch_package(version, &pkg)?;
    }
    let hex = sha256_hex(&pkg)?;
    if hex != pin.sha256 {
        std::fs::remove_file(&pkg)?;
        return Err(format!("archlinux-keyring sha256 mismatch: {hex} (pin says {})", pin.sha256).into());
    }
    let files = dir.join("files");
    let _ = std::fs::remove_dir_all(&files);
    std::fs::create_dir_all(&files)?;
    run(Command::new("tar").args(["--zstd", "-xf"]).arg(&pkg).arg("-C").arg(&files).arg("usr/share/pacman/keyrings"))?;
    let kr = files.join("usr/share/pacman/keyrings");

    let main: Vec<[u8; 20]> = std::fs::read_to_string(kr.join("archlinux-trusted"))?
        .lines()
        .filter_map(|l| fpr_bytes(l.split(':').next()?))
        .collect();
    let revoked: HashSet<[u8; 20]> = std::fs::read_to_string(kr.join("archlinux-revoked"))?
        .lines()
        .filter_map(|l| fpr_bytes(l.trim()))
        .collect();
    if main.is_empty() {
        return Err("no main keys found".into());
    }

    let home = dir.join("gnupg");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    }
    let gpg = |args: &[&str]| -> Result<Vec<u8>> {
        let out = Command::new("gpg").args(["--homedir"]).arg(&home).args(["--batch", "--quiet", "--no-tty"]).args(args).output()?;
        if !out.status.success() {
            return Err(format!("gpg {args:?} failed: {}", String::from_utf8_lossy(&out.stderr)).into());
        }
        Ok(out.stdout)
    };
    gpg(&["--import", kr.join("archlinux.gpg").to_str().unwrap()])?;
    let listing = String::from_utf8(gpg(&["--with-colons", "--with-key-data", "--check-sigs"])?)?;

    let is_main_id = |id: &str| main.iter().any(|m| m.iter().skip(12).map(|b| format!("{b:02X}")).collect::<String>() == id);
    let mut primaries: Vec<Primary> = Vec::new();
    let mut uid_ok = true;
    let mut cur_is_sub = false;
    for line in listing.lines() {
        let f: Vec<&str> = line.split(':').collect();
        let parse_key = |f: &[&str]| Key {
            fpr: None,
            algo: f.get(3).and_then(|a| a.parse().ok()).unwrap_or(0),
            validity: f.get(1).and_then(|v| v.chars().next()).unwrap_or('-'),
            expires: f.get(6).and_then(|e| e.parse().ok()),
            can_sign: f.get(11).is_some_and(|c| c.contains('s')),
            pkd: Vec::new(),
        };
        match f[0] {
            "pub" => {
                primaries.push(Primary { key: parse_key(&f), subs: Vec::new(), signers: BTreeSet::new() });
                cur_is_sub = false;
                uid_ok = true;
            }
            "sub" => {
                if let Some(p) = primaries.last_mut() {
                    p.subs.push(parse_key(&f));
                }
                cur_is_sub = true;
            }
            "fpr" => {
                if let Some(p) = primaries.last_mut() {
                    let k = if cur_is_sub { p.subs.last_mut().unwrap() } else { &mut p.key };
                    k.fpr = f.get(9).and_then(|s| fpr_bytes(s));
                }
            }
            "pkd" => {
                if let Some(p) = primaries.last_mut() {
                    let k = if cur_is_sub { p.subs.last_mut().unwrap() } else { &mut p.key };
                    k.pkd.push(f.get(3).unwrap_or(&"").to_string());
                }
            }
            "uid" => uid_ok = !matches!(f.get(1).and_then(|v| v.chars().next()), Some('r' | 'e' | 'i' | 'd' | 'n')),
            "sig" if !cur_is_sub && uid_ok && f.get(1) == Some(&"!") => {
                let class = f.get(10).copied().unwrap_or("");
                let is_cert = ["10", "11", "12", "13"].iter().any(|c| class.starts_with(c));
                if !is_cert {
                    continue;
                }
                let Some(p) = primaries.last_mut() else { continue };
                let issuer = f.get(12).and_then(|s| fpr_bytes(s));
                let signer = match issuer {
                    Some(i) if main.contains(&i) => Some(i),
                    _ => f.get(4).and_then(|id| main.iter().find(|m| is_main_id(id) && m.iter().skip(12).map(|b| format!("{b:02X}")).collect::<String>() == *id)).copied(),
                };
                if let Some(s) = signer {
                    if Some(s) != p.key.fpr {
                        p.signers.insert(s);
                    }
                }
            }
            _ => {}
        }
    }

    let mut ring = Keyring::default();
    let mut seen = HashSet::new();
    let (mut trusted, mut rejected) = (0, 0);
    for p in &primaries {
        let Some(pfpr) = p.key.fpr else { continue };
        let ok = p.key.usable() && !revoked.contains(&pfpr) && (main.contains(&pfpr) || p.signers.len() >= MIN_MAIN_SIGS);
        if !ok {
            rejected += 1;
            continue;
        }
        trusted += 1;
        let mut candidates: Vec<&Key> = Vec::new();
        if p.key.can_sign {
            candidates.push(&p.key);
        }
        candidates.extend(p.subs.iter().filter(|s| s.can_sign && s.usable()));
        for k in candidates {
            let (Some(fpr), Some(material)) = (k.fpr, k.material()) else { continue };
            if revoked.contains(&fpr) || !seen.insert(fpr) {
                continue;
            }
            let expires = match (p.key.expires, k.expires) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            ring.keys.push(KeyEntry { fingerprint: fpr, material, expires });
        }
    }
    println!("keyring {version}: {trusted} trusted primary keys ({rejected} rejected), {} signing keys", ring.keys.len());
    let blob = ring.to_bytes();
    std::fs::write(blob_path(), &blob)?;
    println!("keyring blob: {} bytes", blob.len());
    Ok(blob)
}
