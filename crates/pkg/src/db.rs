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
        let mut tar = TarReader::new(compress::open(src)?);
        let mut packages = Vec::new();
        while let Some(e) = tar.next_entry()? {
            if e.kind == Kind::File && e.path.ends_with("/desc") {
                let data = tar.read_all()?;
                let text = core::str::from_utf8(&data).map_err(|_| Error::Format("desc not utf-8"))?;
                packages.push(Package::parse(name, text)?);
            }
        }
        Ok(Db { name: name.into(), packages })
    }
}
