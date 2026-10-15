//! End-to-end test of the exFAT appender: format a small volume in memory (bitmap, up-case table,
//! root), append files, and verify the directory entries, FAT chain, bitmap and data on disk.
use exfatw::{Appender, Error, Storage};

struct Mem(Vec<u8>);

impl Storage for Mem {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> exfatw::Result<()> {
        let o = offset as usize;
        buf.copy_from_slice(&self.0[o..o + buf.len()]);
        Ok(())
    }
    fn write_at(&mut self, offset: u64, data: &[u8]) -> exfatw::Result<()> {
        let o = offset as usize;
        self.0[o..o + data.len()].copy_from_slice(data);
        Ok(())
    }
}

const SS: u32 = 512;
const SPC: u32 = 1;
const TOTAL_SECTORS: u32 = 16;
const FAT_OFFSET: u32 = 1;
const FAT_LENGTH: u32 = 1;
const HEAP_OFFSET: u32 = 2;
const CLUSTER_COUNT: u32 = TOTAL_SECTORS - HEAP_OFFSET; // clusters 2..=15
const BITMAP_CLUSTER: u32 = 2;
const UPCASE_CLUSTER: u32 = 3;
const ROOT_CLUSTER: u32 = 4;
/// Root slots 0 and 1 hold the bitmap and up-case entries; appended sets start at slot 2.
const SET: usize = 64;

fn upcase_checksum(bytes: &[u8]) -> u32 {
    let mut s: u32 = 0;
    for &b in bytes {
        s = s.rotate_right(1).wrapping_add(b as u32);
    }
    s
}

fn cluster_off(cluster: u32) -> usize {
    (HEAP_OFFSET + (cluster - 2) * SPC) as usize * SS as usize
}

fn fmt() -> Mem {
    let mut img = vec![0u8; (TOTAL_SECTORS * SS) as usize];
    img[0..3].copy_from_slice(&[0xeb, 0x76, 0x90]);
    img[3..11].copy_from_slice(b"EXFAT   ");
    img[72..80].copy_from_slice(&(TOTAL_SECTORS as u64).to_le_bytes());
    img[80..84].copy_from_slice(&FAT_OFFSET.to_le_bytes());
    img[84..88].copy_from_slice(&FAT_LENGTH.to_le_bytes());
    img[88..92].copy_from_slice(&HEAP_OFFSET.to_le_bytes());
    img[92..96].copy_from_slice(&CLUSTER_COUNT.to_le_bytes());
    img[96..100].copy_from_slice(&ROOT_CLUSTER.to_le_bytes());
    img[104..106].copy_from_slice(&0x0100u16.to_le_bytes());
    img[108] = 9;
    img[109] = 0;
    img[110] = 1;
    img[510] = 0x55;
    img[511] = 0xaa;

    // FAT: reserved entries, then bitmap/up-case/root as single-cluster chains.
    let fat = (FAT_OFFSET * SS) as usize;
    img[fat..fat + 4].copy_from_slice(&0xfffffff8u32.to_le_bytes());
    img[fat + 4..fat + 8].copy_from_slice(&0xffffffffu32.to_le_bytes());
    for c in [BITMAP_CLUSTER, UPCASE_CLUSTER, ROOT_CLUSTER] {
        let e = fat + c as usize * 4;
        img[e..e + 4].copy_from_slice(&0xffffffffu32.to_le_bytes());
    }

    // Allocation bitmap: 14 clusters = 2 bytes; clusters 2, 3, 4 are in use.
    img[cluster_off(BITMAP_CLUSTER)] = 0b0000_0111;

    // Up-case table: ASCII a-z fold to A-Z, everything else (and everything past 128) is identity.
    let mut table = Vec::new();
    for i in 0u16..128 {
        let v = if (b'a' as u16..=b'z' as u16).contains(&i) { i - 32 } else { i };
        table.extend_from_slice(&v.to_le_bytes());
    }
    let uo = cluster_off(UPCASE_CLUSTER);
    img[uo..uo + table.len()].copy_from_slice(&table);

    // Root: bitmap entry, up-case entry, end-of-directory.
    let r = cluster_off(ROOT_CLUSTER);
    img[r] = 0x81;
    img[r + 20..r + 24].copy_from_slice(&BITMAP_CLUSTER.to_le_bytes());
    img[r + 24..r + 32].copy_from_slice(&2u64.to_le_bytes());
    img[r + 32] = 0x82;
    img[r + 36..r + 40].copy_from_slice(&upcase_checksum(&table).to_le_bytes());
    img[r + 52..r + 56].copy_from_slice(&UPCASE_CLUSTER.to_le_bytes());
    img[r + 56..r + 64].copy_from_slice(&(table.len() as u64).to_le_bytes());
    Mem(img)
}

fn read_name_entries(img: &[u8], dir_off: usize) -> String {
    let mut s = String::new();
    let set_entries = img[dir_off + 1] as usize;
    for k in 2..=set_entries {
        let o = dir_off + k * 32;
        if img[o] != 0xc1 {
            continue;
        }
        for u in 0..15 {
            let ch = u16::from_le_bytes([img[o + 2 + u * 2], img[o + 3 + u * 2]]);
            if ch == 0 || ch == 0xffff {
                break;
            }
            s.push(char::from_u32(ch as u32).unwrap());
        }
    }
    s
}

fn first_cluster_of(img: &[u8], set_off: usize) -> u32 {
    let so = set_off + 32;
    u32::from_le_bytes([img[so + 20], img[so + 21], img[so + 22], img[so + 23]])
}

fn fat_entry(img: &[u8], c: u32) -> u32 {
    let fat = (FAT_OFFSET * SS) as usize;
    let o = fat + c as usize * 4;
    u32::from_le_bytes(img[o..o + 4].try_into().unwrap())
}

#[test]
fn appends_a_file_and_the_data_lands() {
    let mut m = fmt();
    let data = b"hello exfat log";
    {
        let mut a = Appender::open(&mut m).unwrap();
        a.append("archstaller.log", data).unwrap();
    }
    let dir_off = cluster_off(ROOT_CLUSTER) + SET;
    assert_eq!(m.0[dir_off], 0x85, "file entry follows the bitmap and up-case entries");
    assert!(m.0[dir_off + 1] as usize >= 2);
    assert_eq!(read_name_entries(&m.0, dir_off), "archstaller.log");

    let so = dir_off + 32;
    assert_eq!(m.0[so], 0xc0);
    let first = first_cluster_of(&m.0, dir_off);
    let size = u64::from_le_bytes(m.0[so + 24..so + 32].try_into().unwrap());
    assert_eq!(size, data.len() as u64);
    let d = cluster_off(first);
    assert_eq!(&m.0[d..d + data.len()], data);
    assert!(fat_entry(&m.0, first) >= 0xfffffff8);

    // The bitmap on disk now marks the new cluster as used, and the dirty flag is clear.
    let idx = (first - 2) as usize;
    assert_ne!(m.0[cluster_off(BITMAP_CLUSTER) + idx / 8] & (1 << (idx % 8)), 0);
    assert_eq!(u16::from_le_bytes([m.0[106], m.0[107]]) & 0x0002, 0);
}

#[test]
fn never_hands_out_a_cluster_the_bitmap_says_is_used() {
    let mut m = fmt();
    // Cluster 5 holds data that has no FAT chain (like a NoFatChain file): FAT is 0, bitmap is 1.
    m.0[cluster_off(BITMAP_CLUSTER)] |= 1 << 3;
    m.0[cluster_off(5)..cluster_off(5) + 4].copy_from_slice(b"KEEP");
    {
        let mut a = Appender::open(&mut m).unwrap();
        a.append("x.log", b"data").unwrap();
    }
    let first = first_cluster_of(&m.0, cluster_off(ROOT_CLUSTER) + SET);
    assert_eq!(first, 6);
    assert_eq!(&m.0[cluster_off(5)..cluster_off(5) + 4], b"KEEP");
}

#[test]
fn name_hash_is_over_the_upcased_name() {
    let mut m = fmt();
    {
        let mut a = Appender::open(&mut m).unwrap();
        a.append("archstaller-log.txt", b"x").unwrap();
    }
    let so = cluster_off(ROOT_CLUSTER) + SET + 32;
    let hash = u16::from_le_bytes([m.0[so + 4], m.0[so + 5]]);
    assert_eq!(hash, 0xbadc, "value that fsck.exfat expects for this name");
}

#[test]
fn a_second_append_with_the_same_name_is_refused_case_insensitively() {
    let mut m = fmt();
    {
        let mut a = Appender::open(&mut m).unwrap();
        a.append("boot.log", b"one").unwrap();
    }
    let mut a = Appender::open(&mut m).unwrap();
    assert_eq!(a.append("BOOT.LOG", b"two"), Err(Error::Exists));
    assert_eq!(a.exists("Boot.Log"), Ok(true));
}

#[test]
fn a_long_name_spans_several_name_entries() {
    let mut m = fmt();
    let long = "a".repeat(40);
    {
        let mut a = Appender::open(&mut m).unwrap();
        a.append(&long, b"data").unwrap();
    }
    let dir_off = cluster_off(ROOT_CLUSTER) + SET;
    assert_eq!(read_name_entries(&m.0, dir_off), long);
    assert_eq!(m.0[dir_off + 1], 4); // stream + 3 name entries
}

#[test]
fn a_multi_cluster_file_chains_its_clusters() {
    let mut m = fmt();
    let data = vec![0xab; 1500]; // three 512-byte clusters
    {
        let mut a = Appender::open(&mut m).unwrap();
        a.append("big.log", &data).unwrap();
    }
    let first = first_cluster_of(&m.0, cluster_off(ROOT_CLUSTER) + SET);
    assert_eq!(fat_entry(&m.0, first), first + 1);
    let mut got = Vec::new();
    let mut c = first;
    loop {
        let o = cluster_off(c);
        got.extend_from_slice(&m.0[o..o + 512]);
        let e = fat_entry(&m.0, c);
        if e >= 0xfffffff8 {
            break;
        }
        c = e;
    }
    assert_eq!(&got[..data.len()], &data[..]);
}

#[test]
fn a_full_root_cluster_grows_the_root_chain() {
    let mut m = fmt();
    {
        let mut a = Appender::open(&mut m).unwrap();
        // 512-byte root cluster = 16 slots; 2 are metadata, each set below takes 3, so the
        // fifth append no longer fits and the root has to grow.
        for k in 0..6 {
            a.append(&format!("f{k}.log"), b"d").unwrap();
        }
    }
    assert!(fat_entry(&m.0, ROOT_CLUSTER) < 0xfffffff8, "root chain was extended");
    let mut a = Appender::open(&mut m).unwrap();
    for k in 0..6 {
        assert_eq!(a.exists(&format!("F{k}.LOG")), Ok(true));
    }
}

#[test]
fn a_non_exfat_sector_is_rejected() {
    let mut m = Mem(vec![0u8; 4096]);
    assert!(matches!(Appender::open(&mut m), Err(Error::NotExfat)));
}