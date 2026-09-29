use initrd::modules::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

fn kernel_dir() -> Option<PathBuf> {
    let rel = String::from_utf8(Command::new("uname").arg("-r").output().ok()?.stdout).ok()?;
    let d = PathBuf::from("/usr/lib/modules").join(rel.trim());
    d.join("kernel").exists().then_some(d)
}

struct DirSource(BTreeMap<String, PathBuf>);

impl DirSource {
    fn scan(dir: &PathBuf) -> DirSource {
        let mut map = BTreeMap::new();
        let out = Command::new("find").arg(dir.join("kernel")).args(["-name", "*.ko*"]).output().unwrap();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some(n) = name_from_path(line) {
                map.insert(n, PathBuf::from(line));
            }
        }
        DirSource(map)
    }
}

impl ModuleSource for DirSource {
    fn find(&mut self, name: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.get(name)?).ok()
    }
}

#[test]
fn depends_match_modinfo() {
    let Some(dir) = kernel_dir() else { return };
    let src = DirSource::scan(&dir);
    let mut checked = 0;
    for (name, path) in src.0.iter().step_by(src.0.len() / 60 + 1) {
        let raw = decompress(&std::fs::read(path).unwrap()).unwrap();
        let got = depends(&raw).unwrap();
        let out = Command::new("modinfo").args(["-F", "depends"]).arg(path).output().unwrap();
        let want: Vec<String> = String::from_utf8_lossy(&out.stdout).trim().split(',').filter(|s| !s.is_empty()).map(normalize).collect();
        assert_eq!(got, want, "{name}");
        checked += 1;
    }
    assert!(checked > 20);
}

#[test]
fn resolution_orders_dependencies_first() {
    let Some(dir) = kernel_dir() else { return };
    let mut src = DirSource::scan(&dir);
    // A module with a dependency chain that is not built in on any config: snd_hda_intel.
    let r = resolve(&mut src, &["snd_hda_intel"], &[], &BTreeSet::new()).unwrap();
    assert!(r.order.len() > 3, "{:?}", r.order);
    let pos: BTreeMap<_, _> = r.order.iter().enumerate().map(|(i, n)| (n.clone(), i)).collect();
    for (name, image) in r.order.iter().zip(&r.images) {
        for dep in depends(image).unwrap() {
            assert!(pos[&dep] < pos[name], "{dep} must load before {name}");
        }
    }
    assert_eq!(r.order.last().unwrap(), "snd_hda_intel");
    // Built-in modules are skipped; unknown required modules are an error.
    let builtin: BTreeSet<String> = r.order[..2].iter().cloned().collect();
    let r2 = resolve(&mut src, &["snd_hda_intel"], &[], &builtin).unwrap();
    assert!(r2.order.iter().all(|n| !builtin.contains(n)));
    assert!(resolve(&mut src, &["no_such_module"], &[], &BTreeSet::new()).is_err());
    assert!(resolve(&mut src, &[], &["no_such_module"], &BTreeSet::new()).unwrap().order.is_empty());
}

#[test]
fn cpio_is_readable() {
    let mut w = initrd::cpio::Writer::new();
    w.dir("etc", 0o755);
    w.file("etc/hello", 0o644, b"hello world");
    w.symlink("etc/link", "hello");
    w.char_dev("dev/null", 0o666, 1, 3);
    let bytes = w.finish();
    assert_eq!(bytes.len() % 512, 0);
    let dir = std::env::temp_dir().join(format!("cpio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("t.cpio");
    std::fs::write(&f, bytes).unwrap();
    let out = Command::new("bsdtar").arg("-tvf").arg(&f).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("etc/hello") && text.contains("etc/link -> hello") && text.contains("dev/null"), "{text}");
    let cat = Command::new("bsdtar").arg("-xOf").arg(&f).arg("etc/hello").output().unwrap();
    assert_eq!(cat.stdout, b"hello world");
    let _ = std::fs::remove_dir_all(&dir);
}
