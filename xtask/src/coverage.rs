//! `cargo xtask coverage [--pci FILE] [--usb FILE] [--missing N] [--era FROM-TO]`: wired-network coverage of the
//! installer drivers against the corpora in `coverage/` (see `docs/network-driver-coverage.md`).
//!
//! A corpus is a tab-separated file, one hardware id per row, with a `weight` (how often the id turns up in
//! the real world, 0 = unknown). "Recognized" is computed from the real
//! drivers (`drivers::net_driver_for`, `usbnet::recognizes`); the other stages come from the `test`
//! column, which holds the highest validation level that has actually passed for that id. Each stage is
//! reported by entries (unweighted) and, when the corpus has weights, by weight, which counts a model that
//! many people own for more than a rare one. Weights are a sample, not a market share.
use crate::{root, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Validation ladder, lowest first (`docs/network-driver-coverage.md`, "Validation ladder").
const LEVELS: [&str; 6] = ["none", "decode", "probe", "link", "network", "e2e"];
const COLUMNS: usize = 10;
const CONFIDENCE: [&str; 3] = ["low", "medium", "high"];

pub const PCI_TARGET: f64 = 95.0;
pub const USB_TARGET: f64 = 70.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub vendor: u16,
    pub device: u16,
    pub name: String,
    pub family: String,
    /// PCI only: first and last year systems with this NIC were commonly built (a hand estimate), if known.
    pub era: Option<(u16, u16)>,
    /// USB only: `class/subclass/protocol` of the interfaces, when the source recorded them.
    pub interfaces: Vec<(u8, u8, u8)>,
    pub source: String,
    pub confidence: String,
    pub level: usize,
    /// Popularity count from the source (probes, units); 0 when unknown.
    pub weight: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    Pci,
    Usb,
}

fn hex16(s: &str, what: &str, line: usize) -> Result<u16> {
    if s.len() != 4 || !s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
        return Err(format!("line {line}: {what} {s:?} must be 4 lower-case hex digits").into());
    }
    Ok(u16::from_str_radix(s, 16)?)
}

fn parse_era(s: &str, line: usize) -> Result<Option<(u16, u16)>> {
    if s == "-" || s.is_empty() {
        return Ok(None);
    }
    let bad = || format!("line {line}: era {s:?} must be - or FIRST-LAST years like 2013-2019");
    let (a, b) = s.split_once('-').ok_or_else(bad)?;
    let (a, b): (u16, u16) = (a.parse().map_err(|_| bad())?, b.parse().map_err(|_| bad())?);
    if !(1990..=2100).contains(&a) || b < a || b > 2100 {
        return Err(bad().into());
    }
    Ok(Some((a, b)))
}

fn triple(s: &str, line: usize) -> Result<(u8, u8, u8)> {
    let p: Vec<&str> = s.split('/').collect();
    let ok = p.len() == 3 && p.iter().all(|x| x.len() == 2 && x.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    if !ok {
        return Err(format!("line {line}: interface {s:?} must look like ff/ff/00").into());
    }
    Ok((u8::from_str_radix(p[0], 16)?, u8::from_str_radix(p[1], 16)?, u8::from_str_radix(p[2], 16)?))
}

/// Parses a corpus. Columns (header row required, `#` lines ignored):
/// PCI `vendor device name family era source confidence test weight notes`,
/// USB `vid pid name family interfaces source confidence test weight notes`.
pub fn parse(text: &str, bus: Bus) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut header = false;
    for (n, line) in text.lines().enumerate() {
        let n = n + 1;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let c: Vec<&str> = line.split('\t').collect();
        if !header {
            header = true;
            let want = if bus == Bus::Pci { "vendor" } else { "vid" };
            if c.first() != Some(&want) || c.len() != COLUMNS {
                return Err(format!("line {n}: the header row must have {COLUMNS} columns and start with {want:?}").into());
            }
            continue;
        }
        if c.len() != COLUMNS {
            return Err(format!("line {n}: expected {COLUMNS} tab-separated columns, found {}", c.len()).into());
        }
        let (vendor, device) = (hex16(c[0], "vendor id", n)?, hex16(c[1], "device id", n)?);
        if !seen.insert((vendor, device)) {
            return Err(format!("line {n}: {:04x}:{:04x} appears twice", vendor, device).into());
        }
        let interfaces = if bus == Bus::Usb && !c[4].is_empty() && c[4] != "-" { c[4].split(';').map(|t| triple(t, n)).collect::<Result<Vec<_>>>()? } else { vec![] };
        let era = if bus == Bus::Pci { parse_era(c[4], n)? } else { None };
        if c[2].is_empty() || c[3].is_empty() || c[5].is_empty() {
            return Err(format!("line {n}: name, family and source must not be empty").into());
        }
        if !CONFIDENCE.contains(&c[6]) {
            return Err(format!("line {n}: confidence {:?} must be one of {CONFIDENCE:?}", c[6]).into());
        }
        let level = LEVELS.iter().position(|l| *l == c[7]).ok_or_else(|| format!("line {n}: test {:?} must be one of {LEVELS:?}", c[7]))?;
        let weight = c[8].parse::<u64>().map_err(|_| format!("line {n}: weight {:?} must be a whole number (0 when unknown)", c[8]))?;
        out.push(Entry { vendor, device, name: c[2].into(), family: c[3].into(), era, interfaces, source: c[5].into(), confidence: c[6].into(), level, weight });
    }
    if !header {
        return Err("the corpus has no header row".into());
    }
    Ok(out)
}

/// The driver that recognizes this entry, if any.
pub fn recognized(bus: Bus, e: &Entry) -> Option<&'static str> {
    match bus {
        Bus::Pci => drivers::net_driver_for(e.vendor, e.device),
        Bus::Usb => usbnet::recognizes(e.vendor, e.device, &e.interfaces),
    }
}

/// Rejects a corpus that claims testing for an id no driver recognizes.
pub fn check(bus: Bus, entries: &[Entry]) -> Result<()> {
    for e in entries {
        if e.level >= 2 && recognized(bus, e).is_none() {
            return Err(format!("{:04x}:{:04x} ({}) says test={} but no driver recognizes it", e.vendor, e.device, e.name, LEVELS[e.level]).into());
        }
    }
    Ok(())
}

/// Entries and weight at each stage.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub total: u64,
    pub recognized: u64,
    pub initialized: u64,
    pub link: u64,
    pub network: u64,
    pub e2e: u64,
}

pub struct Report {
    /// By entry: every row counts once.
    pub entries: Counts,
    /// By weight: every row counts as often as the source saw it. All zero without weights.
    pub weighted: Counts,
    pub by_driver: BTreeMap<&'static str, usize>,
    /// Unrecognized entries per vendor id and family: (vendor, family, entries, weight), heaviest first.
    pub missing: Vec<(u16, String, usize, u64)>,
    /// The unrecognized single ids with the most weight: (vendor, device, name, weight).
    pub top_missing: Vec<(u16, u16, String, u64)>,
}

impl Report {
    pub fn has_weights(&self) -> bool {
        self.weighted.total > 0
    }
}

fn tally(entries: &[Entry], bus: Bus, weighted: bool) -> Counts {
    let mut c = Counts::default();
    for e in entries {
        let w = if weighted { e.weight } else { 1 };
        c.total += w;
        if recognized(bus, e).is_some() {
            c.recognized += w;
        }
        for (min, slot) in [(2, &mut c.initialized), (3, &mut c.link), (4, &mut c.network), (5, &mut c.e2e)] {
            if e.level >= min {
                *slot += w;
            }
        }
    }
    c
}

pub fn report(bus: Bus, entries: &[Entry]) -> Report {
    let mut by_driver = BTreeMap::new();
    let mut groups: BTreeMap<(u16, String), (usize, u64)> = BTreeMap::new();
    let mut top_missing = Vec::new();
    for e in entries {
        match recognized(bus, e) {
            Some(d) => *by_driver.entry(d).or_insert(0) += 1,
            None => {
                let g = groups.entry((e.vendor, e.family.clone())).or_insert((0, 0));
                g.0 += 1;
                g.1 += e.weight;
                top_missing.push((e.vendor, e.device, e.name.clone(), e.weight));
            }
        }
    }
    let mut missing: Vec<(u16, String, usize, u64)> = groups.into_iter().map(|((v, f), (n, w))| (v, f, n, w)).collect();
    missing.sort_by(|a, b| b.3.cmp(&a.3).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
    top_missing.sort_by(|a, b| b.3.cmp(&a.3).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
    Report { entries: tally(entries, bus, false), weighted: tally(entries, bus, true), by_driver, missing, top_missing }
}

fn pct(n: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        n as f64 * 100.0 / total as f64
    }
}

fn print(label: &str, path: &Path, target: f64, r: &Report, missing: usize) {
    let weighted = r.has_weights();
    println!("{label} corpus {} ({} entries{})", path.display(), r.entries.total, if weighted { format!(", weight {} in total", r.weighted.total) } else { ", unweighted: no weights in this corpus".into() });
    println!("  {:<30} {:>6} {:>7}{}", "", "entries", "", if weighted { "   by weight" } else { "" });
    let rows = |c: &Counts| [("recognized ids", c.recognized), ("initialized (probe or better)", c.initialized), ("link-tested", c.link), ("network-tested", c.network), ("installer e2e-tested", c.e2e)];
    for ((name, n), (_, w)) in rows(&r.entries).into_iter().zip(rows(&r.weighted)) {
        let by_weight = if weighted { format!("   {:>5.1}%", pct(w, r.weighted.total)) } else { String::new() };
        println!("  {name:<30} {n:>6} {:>6.1}%{by_weight}", pct(n, r.entries.total));
    }
    let (p, basis) = if weighted { (pct(r.weighted.recognized, r.weighted.total), "by weight") } else { (pct(r.entries.recognized, r.entries.total), "unweighted") };
    println!("  target {target:.0}% recognized: {} ({p:.1}%, {basis})", if p >= target { "reached on this corpus" } else { "not reached" });
    if !r.by_driver.is_empty() {
        let parts: Vec<String> = r.by_driver.iter().map(|(d, n)| format!("{d} {n}")).collect();
        println!("  recognized by: {}", parts.join(", "));
    }
    if missing > 0 && !r.missing.is_empty() {
        println!("  largest unrecognized groups (vendor id, family, entries{}):", if weighted { ", weight" } else { "" });
        for (v, f, n, w) in r.missing.iter().take(missing) {
            if weighted {
                println!("    {v:04x}  {f:<28} {n:>4} {w:>8}");
            } else {
                println!("    {v:04x}  {f:<28} {n:>4}");
            }
        }
        if weighted {
            println!("  most used unrecognized ids (id, weight, name):");
            for (v, d, n, w) in r.top_missing.iter().take(missing) {
                println!("    {v:04x}:{d:04x} {w:>8}  {n}");
            }
        }
    }
}

/// Entries whose era range overlaps `from..=to` (`contained` = lies wholly inside it) and those with no era.
pub fn by_era(entries: &[Entry], from: u16, to: u16, contained: bool) -> Vec<Entry> {
    entries
        .iter()
        .filter(|e| e.era.is_some_and(|(a, b)| if contained { a >= from && b <= to } else { a <= to && b >= from }))
        .cloned()
        .collect()
}

/// Entries launched in `from..=to` (first year inside it), however long they stayed in use.
pub fn launched_in(entries: &[Entry], from: u16, to: u16) -> Vec<Entry> {
    entries
        .iter()
        .filter(|e| e.era.is_some_and(|(a, _)| a >= from && a <= to))
        .cloned()
        .collect()
}

fn print_era(entries: &[Entry], bus: Bus, from: u16, to: u16) {
    let no_era: Vec<Entry> = entries.iter().filter(|e| e.era.is_none()).cloned().collect();
    println!("  by hardware era {from}-{to} (hand-estimated year ranges; a chip built for years counts in every era it spans):");
    println!("  {:<42} {:>8} {:>9} {:>9} {:>9}", "", "entries", "weight", "recognized", "e2e-tested");
    let sets = [("ids in use during the era (overlap)", by_era(entries, from, to, false)), ("ids launched in the era (first year)", launched_in(entries, from, to)), ("ids only from the era (contained)", by_era(entries, from, to, true)), ("no era known", no_era)];
    for (name, set) in sets {
        let r = report(bus, &set);
        let (rec, e2e) = if r.has_weights() { (pct(r.weighted.recognized, r.weighted.total), pct(r.weighted.e2e, r.weighted.total)) } else { (pct(r.entries.recognized, r.entries.total), pct(r.entries.e2e, r.entries.total)) };
        println!("  {name:<42} {:>8} {:>9} {:>8.1}% {:>8.1}%", r.entries.total, r.weighted.total, rec, e2e);
    }
    println!("  (recognized and e2e-tested are by weight where the set has weights)");
}

pub fn run(args: &[String]) -> Result<()> {
    let dir = root().join("coverage");
    let (mut pci, mut usb, mut missing) = (dir.join("pci.tsv"), dir.join("usb.tsv"), 10usize);
    let mut era: Option<(u16, u16)> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--pci" => pci = PathBuf::from(it.next().ok_or("--pci needs a file")?),
            "--usb" => usb = PathBuf::from(it.next().ok_or("--usb needs a file")?),
            "--era" => {
                let v = it.next().ok_or("--era needs FROM-TO years")?;
                let (a, b) = v.split_once('-').ok_or("--era needs FROM-TO years like 2010-2019")?;
                let (a, b): (u16, u16) = (a.parse()?, b.parse()?);
                if b < a {
                    return Err("--era: TO is before FROM".into());
                }
                era = Some((a, b));
            }
            "--missing" => missing = it.next().ok_or("--missing needs a number")?.parse()?,
            other => return Err(format!("unknown option {other} (usage: cargo xtask coverage [--pci FILE] [--usb FILE] [--missing N] [--era FROM-TO])").into()),
        }
    }
    for (bus, label, path, target) in [(Bus::Pci, "PCI Ethernet", &pci, PCI_TARGET), (Bus::Usb, "USB Ethernet/tethering", &usb, USB_TARGET)] {
        let entries = parse(&std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?, bus).map_err(|e| format!("{}: {e}", path.display()))?;
        check(bus, &entries).map_err(|e| format!("{}: {e}", path.display()))?;
        print(label, path, target, &report(bus, &entries), missing);
        if let (Some((a, b)), Bus::Pci) = (era, bus) {
            print_era(&entries, bus, a, b);
        }
        println!();
    }
    println!("Weights are probe counts from a sample of Linux machines, not market share, and rows without a weight count");
    println!("only in the entry columns. Recognized is not the same as working; only the tested rows are evidence, and a");
    println!("tested id does not prove every chip revision behind it. USB rows without interface data can only match the");
    println!("vendor-specific id table, so class-compliant adapters (CDC-ECM/NCM, RNDIS) are undercounted there.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "vendor\tdevice\tname\tfamily\tera\tsource\tconfidence\ttest\tweight\tnotes\n";

    fn pci(rows: &[&str]) -> String {
        format!("{HEAD}{}", rows.join("\n"))
    }

    #[test]
    fn parses_and_counts_recognized_ids_with_the_real_drivers() {
        let text = pci(&[
            "10ec\t8168\tRTL8111/8168\tRealtek\t-\ttest\thigh\te2e\t700\t",
            "8086\t100e\t82540EM\tIntel\t-\ttest\thigh\tnetwork\t100\t",
            "8086\t10fb\t82599 10GbE\tIntel\t-\ttest\tlow\tnone\t150\t",
            "14e4\t1648\tNetXtreme\tBroadcom\t-\ttest\tlow\tnone\t50\t",
        ]);
        let e = parse(&text, Bus::Pci).unwrap();
        check(Bus::Pci, &e).unwrap();
        let r = report(Bus::Pci, &e);
        assert_eq!(r.entries, Counts { total: 4, recognized: 2, initialized: 2, link: 2, network: 2, e2e: 1 });
        // A heavily used id moves the weighted figures far more than the entry figures.
        assert_eq!(r.weighted, Counts { total: 1000, recognized: 800, initialized: 800, link: 800, network: 800, e2e: 700 });
        assert!(r.has_weights());
        assert_eq!(r.top_missing[0].3, 150);
        assert_eq!(r.missing[0].1, "Intel");
        assert_eq!(r.by_driver.get("r8169"), Some(&1));
        assert_eq!(r.by_driver.get("e1000"), Some(&1));
        assert_eq!(r.missing.len(), 2);
    }

    #[test]
    fn rejects_bad_rows() {
        let bad = |row: &str| parse(&pci(&[row]), Bus::Pci).unwrap_err().to_string();
        assert!(bad("10EC\t8168\tn\tf\t-\ts\thigh\tnone\t0\t").contains("lower-case hex"));
        assert!(bad("10ec\t816\tn\tf\t-\ts\thigh\tnone\t0\t").contains("4 lower-case hex"));
        assert!(bad("10ec\t8168\tn\tf\t-\ts\thigh\tgreat\t0\t").contains("test"));
        assert!(bad("10ec\t8168\tn\tf\t-\ts\tcertain\tnone\t0\t").contains("confidence"));
        assert!(bad("10ec\t8168\tn\tf\t-\ts\thigh\tnone").contains("10 tab-separated"));
        assert!(bad("10ec\t8168\tn\tf\t-\ts\thigh\tnone\tmany\t").contains("weight"));
        assert!(bad("10ec\t8168\t\tf\t-\ts\thigh\tnone\t0\t").contains("must not be empty"));
        let dup = parse(&pci(&["10ec\t8168\tn\tf\t-\ts\thigh\tnone\t0\t", "10ec\t8168\tn\tf\t-\ts\thigh\tnone\t0\t"]), Bus::Pci).unwrap_err().to_string();
        assert!(dup.contains("twice"));
        assert!(parse("", Bus::Pci).is_err());
        assert!(parse("name\tx\n", Bus::Pci).is_err());
    }

    #[test]
    fn tested_but_unrecognized_ids_are_refused() {
        let e = parse(&pci(&["14e4\t1648\tNetXtreme\tBroadcom\t-\ts\tlow\tlink\t0\t"]), Bus::Pci).unwrap();
        assert!(check(Bus::Pci, &e).unwrap_err().to_string().contains("no driver recognizes"));
    }

    #[test]
    fn usb_rows_match_by_id_table_or_interface_class() {
        let head = "vid\tpid\tname\tfamily\tinterfaces\tsource\tconfidence\ttest\tweight\tnotes\n";
        let text = format!("{head}0b95\t1790\tAX88179\tASIX\tff/ff/00\ts\thigh\tnetwork\t0\t");
        let e = parse(&text, Bus::Usb).unwrap();
        assert_eq!(recognized(Bus::Usb, &e[0]), Some("ax88179"));
        let cdc = Entry { vendor: 0x1234, device: 0x5678, name: "x".into(), family: "x".into(), era: None, interfaces: vec![(0x02, 0x06, 0x00), (0x0a, 0x00, 0x00)], source: "s".into(), confidence: "low".into(), level: 0, weight: 0 };
        assert_eq!(recognized(Bus::Usb, &cdc), Some("cdc-ecm"));
        let unknown = Entry { interfaces: vec![], ..cdc.clone() };
        assert_eq!(recognized(Bus::Usb, &unknown), None);
        let apple = Entry { vendor: 0x05ac, interfaces: vec![(0x02, 0x06, 0x00)], ..cdc };
        assert_eq!(recognized(Bus::Usb, &apple), None);
        assert!(parse(&format!("{head}0b95\t1790\tn\tf\tff-ff-00\ts\thigh\tnone\t0\t"), Bus::Usb).is_err());
    }

    #[test]
    fn era_filters_overlap_and_contained() {
        let text = pci(&[
            "10ec\t8168\tRTL8111\tRealtek\t2007-2023\ts\thigh\te2e\t700\t",
            "8086\t100e\t82540EM\tIntel\t1998-2010\ts\thigh\tnetwork\t100\t",
            "8086\t153a\tI217\tIntel\t2013-2016\ts\tlow\tnone\t200\t",
            "14e4\t1648\tNetXtreme\tBroadcom\t-\ts\tlow\tnone\t50\t",
        ]);
        let e = parse(&text, Bus::Pci).unwrap();
        assert_eq!(by_era(&e, 2010, 2019, false).len(), 3); // RTL8111, 82540EM (ends 2010), I217
        assert_eq!(by_era(&e, 2010, 2019, true).len(), 1); // only I217 lies wholly inside
        assert_eq!(launched_in(&e, 2010, 2019).len(), 1); // I217 launched 2013; RTL8111 launched 2007
        assert_eq!(e.iter().filter(|x| x.era.is_none()).count(), 1);
        let bad = |era: &str| parse(&pci(&[&format!("10ec\t8168\tn\tf\t{era}\ts\thigh\tnone\t0\t")]), Bus::Pci).unwrap_err().to_string();
        assert!(bad("2019-2010").contains("era"));
        assert!(bad("20x-2019").contains("era"));
        assert!(bad("2013").contains("era"));
    }

    #[test]
    fn the_repository_corpora_parse_and_are_consistent() {
        for (bus, f) in [(Bus::Pci, "pci.tsv"), (Bus::Usb, "usb.tsv")] {
            let path = root().join("coverage").join(f);
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let e = parse(&text, bus).unwrap_or_else(|e| panic!("{f}: {e}"));
            check(bus, &e).unwrap_or_else(|e| panic!("{f}: {e}"));
            assert!(!e.is_empty(), "{f} is empty");
        }
    }
}
