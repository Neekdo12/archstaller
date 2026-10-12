use crate::compress;
use crate::desc::Package;
use crate::io::Read;
use crate::tar::{Kind, TarReader};
use crate::{Error, Result};
use alloc::string::String;
use alloc::vec::Vec;

pub struct Db {
    pub name: String,
    pub packages: Vec<Package>,
}

impl Db {
    /// Parses a (gzip/zstd compressed) sync database.
    pub fn parse<R: Read>(name: &str, src: R) -> Result<Db> {
        Db::parse_with(name, src, |_, _| {})
    }

    /// Like [`Db::parse`], and hands every package with the text of its `desc` entry to `seen`, for host
    /// tools that want fields the installer has no use for (`%DESC%`, `%ISIZE%` is parsed already).
    /// See [`crate::desc::field`].
    pub fn parse_with<R: Read>(name: &str, src: R, mut seen: impl FnMut(&Package, &str)) -> Result<Db> {
        let mut tar = TarReader::new(compress::open(src)?);
        let mut packages = Vec::new();
        while let Some(e) = tar.next_entry()? {
            if e.kind == Kind::File && e.path.ends_with("/desc") {
                let data = tar.read_all()?;
                let text = core::str::from_utf8(&data).map_err(|_| Error::Format("desc not utf-8"))?;
                let p = Package::parse(name, text)?;
                seen(&p, text);
                packages.push(p);
            }
        }
        Ok(Db { name: name.into(), packages })
    }
}
