//! The reviewed-tree digest and a reader for the AUR snapshot tarballs.
//!
//! The digest is what the installed system recomputes before it builds anything (`tree_digest` in
//! `firstboot/archstaler-aur.sh` implements the same rule): SHA-256 over the lines
//! `<sha256 of the file>  <path>\n` for every regular file but `.git`, sorted by path bytes.
use crate::Result;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;

pub type Files = BTreeMap<String, Vec<u8>>;

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// The digest of a tree given as relative path -> contents.
pub fn tree_digest(files: &Files) -> String {
    let mut h = Sha256::new();
    // BTreeMap<String, _> iterates in byte order of the path, like `LC_ALL=C sort`.
    for (path, data) in files {
        h.update(format!("{}  {}\n", sha256_hex(data), path).as_bytes());
    }
    hex(&h.finalize())
}

fn octal(field: &[u8]) -> Option<u64> {
    let s: String = field.iter().take_while(|b| **b != 0 && **b != b' ').map(|b| *b as char).collect();
    if s.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(s.trim(), 8).ok()
}

fn cstr(field: &[u8]) -> String {
    String::from_utf8_lossy(&field[..field.iter().position(|b| *b == 0).unwrap_or(field.len())]).into_owned()
}

/// Reads the regular files of a ustar/pax tar. The first path component (the snapshot's top directory)
/// is removed. Links, devices and directories are ignored.
pub fn read_tar(data: &[u8]) -> Result<Files> {
    let mut files = Files::new();
    let mut pos = 0usize;
    let mut next_path: Option<String> = None;
    let mut next_size: Option<u64> = None;
    while pos + 512 <= data.len() {
        let h = &data[pos..pos + 512];
        if h.iter().all(|b| *b == 0) {
            break;
        }
        let mut size = octal(&h[124..136]).ok_or("tar: bad size field")?;
        let flag = h[156];
        let mut name = cstr(&h[0..100]);
        if &h[257..262] == b"ustar" {
            let prefix = cstr(&h[345..500]);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
        }
        if let Some(p) = next_path.take() {
            name = p;
        }
        if let Some(s) = next_size.take() {
            size = s;
        }
        let start = pos + 512;
        let end = start.checked_add(size as usize).filter(|e| *e <= data.len()).ok_or("tar: truncated")?;
        let body = &data[start..end];
        pos = start + (size as usize).div_ceil(512) * 512;
        match flag {
            b'0' | 0 => {
                let rel = name.split_once('/').map(|(_, r)| r).unwrap_or("");
                if rel.is_empty() || rel.ends_with('/') {
                    continue;
                }
                if rel.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
                    return Err(format!("tar: unsafe path {rel:?}"));
                }
                files.insert(rel.to_string(), body.to_vec());
            }
            b'x' => {
                // pax records: "<len> key=value\n"
                let mut rest = body;
                while !rest.is_empty() {
                    let sp = rest.iter().position(|b| *b == b' ').ok_or("tar: bad pax record")?;
                    let len: usize = std::str::from_utf8(&rest[..sp]).ok().and_then(|s| s.parse().ok()).ok_or("tar: bad pax length")?;
                    if len == 0 || len > rest.len() {
                        return Err("tar: bad pax record".into());
                    }
                    let rec = String::from_utf8_lossy(&rest[sp + 1..len]).trim_end_matches('\n').to_string();
                    if let Some(p) = rec.strip_prefix("path=") {
                        next_path = Some(p.to_string());
                    } else if let Some(s) = rec.strip_prefix("size=") {
                        next_size = s.parse().ok();
                    }
                    rest = &rest[len..];
                }
            }
            b'L' => next_path = Some(cstr(body)),
            _ => {} // 'g' global header, directories, symlinks, ...
        }
    }
    Ok(files)
}

/// Reads a `.tar.gz` snapshot (at most 64 MiB unpacked) into its files.
pub fn read_snapshot(gz: &[u8]) -> Result<Files> {
    let mut raw = Vec::new();
    GzDecoder::new(gz).take(64 << 20).read_to_end(&mut raw).map_err(|e| format!("snapshot: {e}"))?;
    read_tar(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tar entry with a ustar header.
    pub fn entry(name: &str, flag: u8, body: &[u8]) -> Vec<u8> {
        let mut h = vec![0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..107].copy_from_slice(b"0000644");
        h[124..135].copy_from_slice(format!("{:011o}", body.len()).as_bytes());
        h[156] = flag;
        h[257..262].copy_from_slice(b"ustar");
        let mut out = h;
        out.extend_from_slice(body);
        out.resize(512 + body.len().div_ceil(512) * 512, 0);
        out
    }

    fn tar(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut t: Vec<u8> = entries.concat();
        t.extend_from_slice(&[0u8; 1024]);
        t
    }

    #[test]
    fn reads_files_and_skips_the_rest() {
        let t = tar(&[entry("pkg/", b'5', b""), entry("pkg/PKGBUILD", b'0', b"abc"), entry("pkg/.SRCINFO", 0, b"x"), entry("pkg/link", b'2', b""), entry("pkg/sub/p.patch", b'0', &[7u8; 700])]);
        let f = read_tar(&t).unwrap();
        assert_eq!(f.keys().collect::<Vec<_>>(), [".SRCINFO", "PKGBUILD", "sub/p.patch"]);
        assert_eq!(f["sub/p.patch"].len(), 700);
    }

    #[test]
    fn pax_paths_and_global_headers() {
        let rec = |k: &str, v: &str| {
            let body = format!(" {k}={v}\n");
            let mut len = body.len() + 1;
            while len.to_string().len() + body.len() != len {
                len = len.to_string().len() + body.len(); // the length counts its own digits
            }
            format!("{len}{body}")
        };
        let t = tar(&[entry("pax_global_header", b'g', rec("comment", "0123").as_bytes()), entry("PaxHeader/x", b'x', rec("path", "pkg/a/long.txt").as_bytes()), entry("pkg/short", b'0', b"data")]);
        let f = read_tar(&t).unwrap();
        assert_eq!(f.keys().collect::<Vec<_>>(), ["a/long.txt"]);
    }

    #[test]
    fn refuses_unsafe_and_truncated_input() {
        assert!(read_tar(&tar(&[entry("pkg/../etc/x", b'0', b"a")])).is_err());
        let mut t = tar(&[entry("pkg/f", b'0', &[1u8; 600])]);
        t.truncate(700);
        assert!(read_tar(&t).is_err());
    }

    #[test]
    fn digest_is_order_independent_and_content_sensitive() {
        let mut a = Files::new();
        a.insert("PKGBUILD".into(), b"x".to_vec());
        a.insert(".SRCINFO".into(), b"y".to_vec());
        let d = tree_digest(&a);
        let mut b = Files::new();
        b.insert(".SRCINFO".into(), b"y".to_vec());
        b.insert("PKGBUILD".into(), b"x".to_vec());
        assert_eq!(d, tree_digest(&b));
        b.insert("PKGBUILD".into(), b"z".to_vec());
        assert_ne!(d, tree_digest(&b));
        assert_eq!(d.len(), 64);
    }

    /// The same rule, evaluated by the shell function of the first-boot script.
    #[test]
    fn matches_the_shell_implementation() {
        let dir = std::env::temp_dir().join(format!("aurbuild-digest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/config"), b"ignored").unwrap();
        let mut f = Files::new();
        for (p, c) in [("PKGBUILD", "pkgname=a\n"), (".SRCINFO", "pkgbase = a\n"), ("sub/fix.patch", "--- a\n"), ("B", "upper"), ("a-b", "x"), ("a.b", "y")] {
            std::fs::write(dir.join(p), c).unwrap();
            f.insert(p.to_string(), c.as_bytes().to_vec());
        }
        let script = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../firstboot/archstaler-aur.sh")).unwrap();
        let func = script.split("tree_digest() {").nth(1).unwrap().split("\n}\n").next().unwrap();
        let sh = format!("tree_digest() {{{func}\n}}\ntree_digest \"{}\"", dir.display());
        let out = std::process::Command::new("bash").arg("-c").arg(sh).output();
        let _ = std::fs::remove_dir_all(&dir);
        if let Ok(o) = out {
            assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), tree_digest(&f), "stderr: {}", String::from_utf8_lossy(&o.stderr));
        }
    }
}
