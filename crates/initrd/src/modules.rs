use crate::{Error, Result};
use alloc::collections::BTreeSet;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use pkg::io::Read;

/// Module names treat `-` and `_` as the same character.
pub fn normalize(name: &str) -> String {
    name.replace('-', "_")
}

/// Strips directories and `.ko[.zst|.gz]` from a path and normalizes the result.
pub fn name_from_path(path: &str) -> Option<String> {
    let file = path.rsplit('/').next()?;
    let stem = file.strip_suffix(".zst").or_else(|| file.strip_suffix(".gz")).unwrap_or(file);
    let stem = stem.strip_suffix(".ko")?;
    Some(normalize(stem))
}

/// Returns the raw (decompressed) bytes of a module file.
pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    let mut r = pkg::compress::open(data).map_err(|e| Error::Decompress(alloc::format!("{e:?}")))?;
    let mut out = Vec::with_capacity(data.len() * 3);
    let mut buf = [0u8; 16384];
    loop {
        let n = r.read(&mut buf).map_err(|e| Error::Decompress(alloc::format!("{e:?}")))?;
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n]);
    }
}

fn le16(b: &[u8], o: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?) as usize)
}
fn le64(b: &[u8], o: usize) -> Option<usize> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?) as usize)
}
fn le32(b: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?) as usize)
}

/// Reads the `depends=` list from a raw ELF64 module's `.modinfo` section.
pub fn depends(elf: &[u8]) -> Result<Vec<String>> {
    let bad = || Error::BadModule("malformed ELF".to_string());
    if elf.get(..4) != Some(b"\x7fELF") || elf.get(4) != Some(&2) || elf.get(5) != Some(&1) {
        return Err(bad());
    }
    let shoff = le64(elf, 0x28).ok_or_else(bad)?;
    let shentsize = le16(elf, 0x3a).ok_or_else(bad)?;
    let shnum = le16(elf, 0x3c).ok_or_else(bad)?;
    let shstrndx = le16(elf, 0x3e).ok_or_else(bad)?;
    let section = |i: usize| -> Option<(usize, usize, usize)> {
        let o = shoff + i * shentsize;
        Some((le32(elf, o)?, le64(elf, o + 0x18)?, le64(elf, o + 0x20)?)) // name, offset, size
    };
    let (_, str_off, str_size) = section(shstrndx).ok_or_else(bad)?;
    let strtab = elf.get(str_off..str_off + str_size).ok_or_else(bad)?;
    for i in 0..shnum {
        let (name_off, off, size) = section(i).ok_or_else(bad)?;
        let name = strtab.get(name_off..).ok_or_else(bad)?;
        let end = name.iter().position(|&c| c == 0).ok_or_else(bad)?;
        if &name[..end] != b".modinfo" {
            continue;
        }
        let info = elf.get(off..off + size).ok_or_else(bad)?;
        for entry in info.split(|&c| c == 0) {
            if let Some(v) = entry.strip_prefix(b"depends=") {
                let v = core::str::from_utf8(v).map_err(|_| bad())?;
                return Ok(v.split(',').filter(|s| !s.is_empty()).map(normalize).collect());
            }
        }
        return Ok(Vec::new());
    }
    Err(Error::BadModule(".modinfo section missing".to_string()))
}

pub trait ModuleSource {
    /// Returns the module file (possibly compressed) for a normalized module name.
    fn find(&mut self, name: &str) -> Option<Vec<u8>>;
}

pub struct Resolved {
    /// Loading order: dependencies before dependents.
    pub order: Vec<String>,
    /// Raw module images by name, in the same order.
    pub images: Vec<Vec<u8>>,
}

/// Computes the load order for `wanted` including all dependencies. Modules listed in
/// `builtin` are skipped; missing wanted modules are skipped only if `optional`.
pub fn resolve(src: &mut dyn ModuleSource, wanted: &[&str], optional: &[&str], builtin: &BTreeSet<String>) -> Result<Resolved> {
    let mut out = Resolved { order: Vec::new(), images: Vec::new() };
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut visiting: BTreeSet<String> = BTreeSet::new();

    fn visit(
        name: &str,
        src: &mut dyn ModuleSource,
        builtin: &BTreeSet<String>,
        optional: &[&str],
        done: &mut BTreeSet<String>,
        visiting: &mut BTreeSet<String>,
        out: &mut Resolved,
        required: bool,
    ) -> Result<()> {
        let name = normalize(name);
        if done.contains(&name) || visiting.contains(&name) || builtin.contains(&name) {
            return Ok(());
        }
        let Some(file) = src.find(&name) else {
            return if required { Err(Error::Missing(name)) } else { Ok(()) };
        };
        visiting.insert(name.clone());
        let raw = decompress(&file)?;
        for dep in depends(&raw)? {
            visit(&dep, src, builtin, optional, done, visiting, out, true)?;
        }
        visiting.remove(&name);
        done.insert(name.clone());
        out.order.push(name);
        out.images.push(raw);
        Ok(())
    }

    for w in wanted {
        visit(w, src, builtin, optional, &mut done, &mut visiting, &mut out, true)?;
    }
    for w in optional {
        visit(w, src, builtin, optional, &mut done, &mut visiting, &mut out, false)?;
    }
    Ok(out)
}

/// Names from a `modules.builtin` file (`kernel/fs/ext4/ext4.ko` per line).
pub fn parse_builtin(text: &str) -> BTreeSet<String> {
    text.lines().filter_map(name_from_path).collect()
}

/// Builds the initramfs: `/init`, `/modules/*.ko` and `/modules/order`.
pub fn build_initramfs(init: &[u8], modules: &Resolved) -> Vec<u8> {
    use crate::cpio::Writer;
    let mut w = Writer::new();
    for d in ["dev", "proc", "sys", "newroot", "modules", "run", "tmp"] {
        w.dir(d, 0o755);
    }
    w.char_dev("dev/console", 0o600, 5, 1);
    w.char_dev("dev/null", 0o666, 1, 3);
    w.file("init", 0o755, init);
    let mut order = String::new();
    for (name, image) in modules.order.iter().zip(&modules.images) {
        w.file(&alloc::format!("modules/{name}.ko"), 0o644, image);
        order.push_str(name);
        order.push('\n');
    }
    w.file("modules/order", 0o644, order.as_bytes());
    w.finish()
}
