//! Host-side driver for the real-tool regression test: opens an exFAT image file, reports whether
//! the root has a `ventoy` directory, appends `archstaller-log.txt`, or refuses when it exists.
//!
//! Usage: append_raw <image> <log-file-to-read-from-stdin-or-path> [name]
//! Exit: 0 written, 3 already exists, 2 other error, 4 no ventoy dir.
use exfatw::{Appender, Error, Storage};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};

struct FileStorage(std::fs::File);

impl Storage for FileStorage {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> exfatw::Result<()> {
        self.0.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
        self.0.read_exact(buf).map_err(|_| Error::Io)
    }
    fn write_at(&mut self, offset: u64, data: &[u8]) -> exfatw::Result<()> {
        self.0.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
        self.0.write_all(data).map_err(|_| Error::Io)
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let img = args.next().expect("usage: append_raw <image> <payload-file> [name]");
    let payload_path = args.next().expect("usage: append_raw <image> <payload-file> [name]");
    let name = args.next().unwrap_or_else(|| "archstaller-log.txt".to_string());

    let mut payload = Vec::new();
    std::fs::File::open(&payload_path).expect("payload").read_to_end(&mut payload).unwrap();

    let f = OpenOptions::new().read(true).write(true).open(&img).expect("open image");
    let mut s = FileStorage(f);
    let mut a = match Appender::open(&mut s) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("open: {e:?}");
            std::process::exit(2);
        }
    };
    match a.root_has("ventoy") {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("no ventoy dir");
            std::process::exit(4);
        }
        Err(e) => {
            eprintln!("root_has: {e:?}");
            std::process::exit(2);
        }
    }
    match a.append(&name, &payload) {
        Ok(()) => {
            println!("written {name} ({} bytes)", payload.len());
        }
        Err(Error::Exists) => {
            eprintln!("exists");
            std::process::exit(3);
        }
        Err(e) => {
            eprintln!("append: {e:?}");
            std::process::exit(2);
        }
    }
}
