use initrd::modules::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

fn kernel_dir() -> Option<PathBuf> {
    let rel = String::from_utf8(Command::new("uname").arg("-r").output().ok()?.stdout).ok()?;
    let d = PathBuf::from("/usr/lib/modules").join(rel.trim());
    d.join("kernel").exists().then_some(d)
}

struct DirSource(BTreeMap<String, PathBuf>);

impl DirSource {
    fn scan(dir: &Path) -> DirSource {
        let mut map = BTreeMap::new();
        let out = Command::new("find")
            .arg(dir.join("kernel"))
            .args(["-name", "*.ko*"])
            .output()
            .expect("find kernel modules");
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
        let out = Command::new("modinfo")
            .args(["-F", "depends"])
            .arg(path)
            .output()
            .unwrap();
        let want: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .trim()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(normalize)
            .collect();
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
    let pos: BTreeMap<_, _> = r
        .order
        .iter()
        .enumerate()
        .map(|(i, n)| (n.clone(), i))
        .collect();
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
    resolve(&mut src, &["no_such_module"], &[], &BTreeSet::new()).unwrap_err();
    assert!(
        resolve(&mut src, &[], &["no_such_module"], &BTreeSet::new())
            .unwrap()
            .order
            .is_empty()
    );
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
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("etc/hello")
            && text.contains("etc/link -> hello")
            && text.contains("dev/null"),
        "{text}"
    );
    let cat = Command::new("bsdtar")
        .arg("-xOf")
        .arg(&f)
        .arg("etc/hello")
        .output()
        .expect("bsdtar extract");
    assert_eq!(cat.stdout, b"hello world");
    drop(std::fs::remove_dir_all(&dir));
}

#[expect(clippy::indexing_slicing, reason = "fixed mock ELF layout")]
fn build_mock_elf(modinfo: &[u8]) -> Vec<u8> {
    let mut elf = vec![0u8; 0x200];
    elf[..4].copy_from_slice(b"\x7fELF");
    elf[4] = 2; // 64-bit
    elf[5] = 1; // little endian
    // shoff @ 0x28 = 0x40
    elf[0x28..0x30].copy_from_slice(&0x40u64.to_le_bytes());
    // shentsize @ 0x3a = 0x40
    elf[0x3a..0x3c].copy_from_slice(&0x40u16.to_le_bytes());
    // shnum @ 0x3c = 2
    elf[0x3c..0x3e].copy_from_slice(&2u16.to_le_bytes());
    // shstrndx @ 0x3e = 0
    elf[0x3e..0x40].copy_from_slice(&0u16.to_le_bytes());

    // Section 0: .shstrtab at 0xc0
    let strtab = b"\0.shstrtab\0.modinfo\0";
    elf[0xc0..0xc0 + strtab.len()].copy_from_slice(strtab);
    // section 0 entry at 0x40:
    // name_off @ 0x40 = 1 (.shstrtab)
    elf[0x40..0x44].copy_from_slice(&1u32.to_le_bytes());
    // offset @ 0x58 (0x40 + 0x18) = 0xc0
    elf[0x58..0x60].copy_from_slice(&0xc0u64.to_le_bytes());
    // size @ 0x60 (0x40 + 0x20) = strtab.len()
    elf[0x60..0x68].copy_from_slice(&(strtab.len() as u64).to_le_bytes());

    // Section 1: .modinfo at 0x100
    let modinfo_off = 0x100;
    elf[modinfo_off..modinfo_off + modinfo.len()].copy_from_slice(modinfo);
    // section 1 entry at 0x80:
    // name_off @ 0x80 = 11 (.modinfo)
    elf[0x80..0x84].copy_from_slice(&11u32.to_le_bytes());
    // offset @ 0x98 (0x80 + 0x18) = 0x100
    elf[0x98..0xa0].copy_from_slice(&(modinfo_off as u64).to_le_bytes());
    // size @ 0xa0 (0x80 + 0x20) = modinfo.len()
    elf[0xa0..0xa8].copy_from_slice(&(modinfo.len() as u64).to_le_bytes());

    elf
}

struct MockSource(BTreeMap<String, Vec<u8>>);

impl ModuleSource for MockSource {
    fn find(&mut self, name: &str) -> Option<Vec<u8>> {
        self.0.get(name).cloned()
    }
}

#[test]
fn malformed_elf_does_not_panic() {
    depends(&[]).unwrap_err();
    depends(b"\x7fELF\x02\x01").unwrap_err();

    // shstrndx out of range
    let mut elf = build_mock_elf(b"depends=\0");
    elf[0x3e..0x40].copy_from_slice(&99u16.to_le_bytes());
    depends(&elf).unwrap_err();

    // Arithmetic overflow in section headers
    let mut bad_shoff = build_mock_elf(b"depends=\0");
    bad_shoff[0x28..0x30].copy_from_slice(&usize::MAX.to_le_bytes());
    depends(&bad_shoff).unwrap_err();
}

#[test]
fn optional_module_with_missing_dependency_is_skipped() {
    let elf_a = build_mock_elf(b"depends=missing_b\0");
    let mut map = BTreeMap::new();
    map.insert("mod_a".to_string(), elf_a);
    let mut src = MockSource(map);

    // If mod_a is wanted (required), missing missing_b is an error
    let err = resolve(&mut src, &["mod_a"], &[], &BTreeSet::new());
    err.unwrap_err();

    // If mod_a is optional, missing missing_b should skip mod_a without failing resolve
    let ok = resolve(&mut src, &[], &["mod_a"], &BTreeSet::new()).expect("should not fail for optional module");
    assert!(ok.order.is_empty(), "mod_a with missing dependency must be skipped");
}

