//! Real-tool check: format with mkfs.exfat, append many files (forces root growth), then require
//! fsck.exfat to accept the image. Skipped when exfatprogs is not installed.
use exfatw::{Appender, Storage};
use std::fs::OpenOptions;
use std::os::unix::fs::FileExt;
use std::process::Command;

struct FileStore(std::fs::File);

impl Storage for FileStore {
    fn read_at(&mut self, o: u64, b: &mut [u8]) -> exfatw::Result<()> {
        self.0.read_exact_at(b, o).map_err(|_| exfatw::Error::Io)
    }
    fn write_at(&mut self, o: u64, d: &[u8]) -> exfatw::Result<()> {
        self.0.write_all_at(d, o).map_err(|_| exfatw::Error::Io)
    }
}

fn have(tool: &str) -> bool {
    Command::new(tool).arg("-V").output().is_ok()
}

fn run_case(cluster: u32, files: usize) {
    if !have("mkfs.exfat") || !have("fsck.exfat") {
        eprintln!("skipping: exfatprogs not installed");
        return;
    }
    let path = std::env::temp_dir().join(format!("exfatw-{}-{}.img", std::process::id(), cluster));
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    f.set_len(64 << 20).unwrap();
    let c = cluster.to_string();
    let st = Command::new("mkfs.exfat")
        .args(["-c", c.as_str()])
        .arg(&path)
        .status()
        .unwrap();
    assert!(st.success(), "mkfs.exfat failed");
    {
        let mut s = FileStore(f);
        let mut a = Appender::open(&mut s).unwrap();
        for k in 0..files {
            let name = format!("archstaller-log-{k}.txt");
            let data = vec![b'x'; 1000 + k * 37];
            a.append(&name, &data).unwrap();
        }
    }
    let st = Command::new("fsck.exfat")
        .arg("-n")
        .arg(&path)
        .status()
        .unwrap();
    std::fs::remove_file(&path).ok();
    assert!(
        st.success(),
        "fsck.exfat rejected the image (cluster size {cluster})"
    );
}

#[test]
fn fsck_accepts_512_byte_clusters() {
    run_case(512, 40);
}

#[test]
fn fsck_accepts_4096_byte_clusters() {
    run_case(4096, 200);
}

#[test]
fn fsck_accepts_32768_byte_clusters() {
    run_case(32768, 400);
}
