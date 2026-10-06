//! Talking to aur.archlinux.org: the RPC interface, the git smart-HTTP ref advertisement (for a
//! repository's HEAD without a `git` binary) and the cgit snapshot of a commit.
use crate::digest::{read_snapshot, Files};
use crate::plan::{review, Review};
use crate::Result;
use serde::Deserialize;

pub const BASE: &str = "https://aur.archlinux.org";

/// One package as the RPC describes it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Info {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "PackageBase")]
    pub pkgbase: String,
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "Description")]
    pub description: Option<String>,
    #[serde(rename = "Maintainer")]
    pub maintainer: Option<String>,
    /// Unix time the package was flagged out of date, if it was.
    #[serde(rename = "OutOfDate")]
    pub out_of_date: Option<i64>,
    #[serde(rename = "LastModified")]
    pub last_modified: i64,
    #[serde(rename = "NumVotes")]
    pub votes: u32,
    #[serde(rename = "Popularity")]
    pub popularity: f64,
    #[serde(rename = "URL")]
    pub url: Option<String>,
    #[serde(rename = "Depends")]
    pub depends: Vec<String>,
    #[serde(rename = "MakeDepends")]
    pub makedepends: Vec<String>,
    #[serde(rename = "Provides")]
    pub provides: Vec<String>,
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    results: Vec<Info>,
    #[serde(default)]
    error: Option<String>,
}

fn get(url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut resp = ureq::get(url).header("User-Agent", "archstaler-gui").call().map_err(|e| format!("{url}: {e}"))?;
    resp.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| format!("{url}: {e}"))
}

fn answer(body: &[u8]) -> Result<Vec<Info>> {
    let a: Answer = serde_json::from_slice(body).map_err(|e| format!("unexpected answer from the AUR: {e}"))?;
    match a.error {
        Some(e) => Err(format!("the AUR says: {e}")),
        None => Ok(a.results),
    }
}

/// Exact lookup of up to 150 names.
pub fn info(names: &[String]) -> Result<Vec<Info>> {
    let mut out = Vec::new();
    for chunk in names.chunks(150) {
        let args: Vec<String> = chunk.iter().map(|n| format!("arg%5B%5D={}", pct(n))).collect();
        out.extend(answer(&get(&format!("{BASE}/rpc/v5/info?{}", args.join("&")), 4 << 20)?)?);
    }
    Ok(out)
}

/// Search by name and description; the RPC returns at most 5000 hits, the GUI shows the first few.
pub fn search(term: &str) -> Result<Vec<Info>> {
    if term.trim().len() < 2 {
        return Ok(vec![]);
    }
    answer(&get(&format!("{BASE}/rpc/v5/search/{}?by=name-desc", pct(term.trim())), 8 << 20)?)
}

/// The commit a repository's HEAD points at, from the git smart-HTTP ref advertisement.
pub fn head_commit(pkgbase: &str) -> Result<String> {
    let body = get(&format!("{BASE}/{}.git/info/refs?service=git-upload-pack", pct(pkgbase)), 1 << 20)?;
    parse_head(&body).ok_or_else(|| format!("{pkgbase}: no HEAD in the git answer"))
}

fn parse_head(body: &[u8]) -> Option<String> {
    // pkt-lines: 4 hex digits of length (including themselves) then the payload; 0000 is a flush.
    let mut pos = 0;
    while pos + 4 <= body.len() {
        let len = usize::from_str_radix(std::str::from_utf8(&body[pos..pos + 4]).ok()?, 16).ok()?;
        if len == 0 {
            pos += 4;
            continue;
        }
        let payload = body.get(pos + 4..pos + len)?;
        let text = String::from_utf8_lossy(payload);
        let line = text.split('\0').next()?.trim_end();
        if let Some((sha, name)) = line.split_once(' ') {
            if name == "HEAD" && sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
                return Some(sha.to_lowercase());
            }
        }
        pos += len;
    }
    None
}

/// The files of the recipe at `commit`.
pub fn snapshot(commit: &str) -> Result<Files> {
    if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{commit:?} is not a commit id"));
    }
    read_snapshot(&get(&format!("{BASE}/cgit/aur.git/snapshot/{commit}.tar.gz"), 64 << 20)?)
}

/// Downloads the recipe of `pkgbase` (HEAD, or the given commit) and reviews the package `name` in it.
pub fn fetch_review(name: &str, pkgbase: &str, commit: Option<&str>) -> Result<Review> {
    let commit = match commit {
        Some(c) => c.to_string(),
        None => head_commit(pkgbase)?,
    };
    review(name, &commit, snapshot(&commit)?)
}

fn pct(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_from_a_ref_advertisement() {
        let pkt = |t: &[u8]| {
            let mut v = format!("{:04x}", t.len() + 4).into_bytes();
            v.extend_from_slice(t);
            v
        };
        let sha = "cb43f84828ab4f9700f7c6f9c6d7a923d4cfaff0";
        let mut body = pkt(b"# service=git-upload-pack\n");
        body.extend_from_slice(b"0000");
        body.extend(pkt(format!("{sha} HEAD\0multi_ack\n").as_bytes()));
        body.extend(pkt(format!("{sha} refs/heads/master\n").as_bytes()));
        body.extend_from_slice(b"0000");
        assert_eq!(parse_head(&body).as_deref(), Some(sha));
        assert_eq!(parse_head(b"0000"), None);
        assert_eq!(parse_head(b"zzzz"), None);
    }

    #[test]
    fn answers_and_percent_encoding() {
        let ok = br#"{"resultcount":1,"results":[{"Name":"a","PackageBase":"a","Version":"1-1","Maintainer":null,"OutOfDate":null,"LastModified":5,"NumVotes":2,"Popularity":1.5,"Depends":["x"]}],"type":"search","version":5}"#;
        let r = answer(ok).unwrap();
        assert_eq!((r[0].name.as_str(), r[0].votes, r[0].depends.len()), ("a", 2, 1));
        assert!(answer(br#"{"error":"Too many package results.","results":[],"type":"error","version":5}"#).unwrap_err().contains("Too many"));
        assert_eq!(pct("a b+c"), "a%20b%2Bc");
    }

    /// Hits the real AUR; run with `ARCHSTALER_NET_TESTS=1 cargo test -p aurbuild --features net`.
    #[test]
    fn live_review_of_a_real_package() {
        if std::env::var_os("ARCHSTALER_NET_TESTS").is_none() {
            return;
        }
        let i = info(&["yay".to_string()]).unwrap();
        assert_eq!(i[0].name, "yay");
        let r = fetch_review("yay", "yay", None).unwrap();
        assert_eq!(r.commit.len(), 40);
        assert_eq!(r.tree_sha256.len(), 64);
        assert!(!search("zen-browser").unwrap().is_empty());
    }
}
