//! Block-letter banners ("ARCHSTALLER" at start, "INSTALLED" when the install is done, and on the
//! installed system's login prompt and message of the day).
use crate::println;
use alloc::string::String;

/// 5x5 glyphs, one string of five rows per letter.
fn glyph(c: u8) -> [&'static str; 5] {
    match c {
        b'A' => [" ### ", "#   #", "#####", "#   #", "#   #"],
        b'R' => ["#### ", "#   #", "#### ", "#  # ", "#   #"],
        b'C' => [" ####", "#    ", "#    ", "#    ", " ####"],
        b'H' => ["#   #", "#   #", "#####", "#   #", "#   #"],
        b'S' => [" ####", "#    ", " ### ", "    #", "#### "],
        b'T' => ["#####", "  #  ", "  #  ", "  #  ", "  #  "],
        b'L' => ["#    ", "#    ", "#    ", "#    ", "#####"],
        b'E' => ["#####", "#    ", "#### ", "#    ", "#####"],
        b'I' => ["#####", "  #  ", "  #  ", "  #  ", "#####"],
        b'N' => ["#   #", "##  #", "# # #", "#  ##", "#   #"],
        b'D' => ["#### ", "#   #", "#   #", "#   #", "#### "],
        _ => ["     "; 5],
    }
}

/// The word as five lines of text.
pub fn render(word: &str) -> String {
    let mut out = String::new();
    for row in 0..5 {
        for c in word.bytes() {
            out.push_str(glyph(c).get(row).copied().unwrap_or(""));
            out.push_str("  ");
        }
        out.push('\n');
    }
    out
}

/// Prints the word without allocating (it is shown before the heap exists).
pub fn show(word: &str) {
    println!();
    for row in 0..5 {
        for c in word.bytes() {
            crate::print!("{}  ", glyph(c).get(row).copied().unwrap_or(""));
        }
        println!();
    }
    println!();
}

/// The word as a gzip-compressed DurDraw movie (`.dur`), which the `ly` login manager can draw as
/// its background (`animation = dur_file`). The gzip is "stored" (uncompressed): it is tiny.
pub fn dur(word: &str) -> alloc::vec::Vec<u8> {
    use alloc::format;
    let art = render(word);
    let rows: alloc::vec::Vec<&str> = art.lines().collect();
    let width = rows.iter().map(|r| r.trim_end().len()).max().unwrap_or(0);
    let mut contents = String::new();
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            contents.push(',');
        }
        contents.push_str(&format!("\"{:<width$}\"", r.trim_end()));
    }
    // colorMap[x][y] = [foreground, background] of the cell in column x, row y.
    let mut cmap = String::new();
    for x in 0..width {
        if x > 0 {
            cmap.push(',');
        }
        cmap.push('[');
        for (y, r) in rows.iter().enumerate() {
            if y > 0 {
                cmap.push(',');
            }
            let lit = r.as_bytes().get(x).is_some_and(|&c| c != b' ');
            cmap.push_str(if lit { "[7,0]" } else { "[8,0]" });
        }
        cmap.push(']');
    }
    let json = format!(
        "{{\"DurMovie\":{{\"formatVersion\":7,\"colorFormat\":\"16\",\"preferredFont\":\"fixed\",\"encoding\":\"utf-8\",\"name\":\"\",\"artist\":\"\",\"framerate\":6.0,\"sizeX\":{width},\"sizeY\":{},\"extra\":null,\"frames\":[{{\"frameNumber\":1,\"delay\":0,\"contents\":[{contents}],\"colorMap\":[{cmap}]}}]}}}}",
        rows.len()
    );
    let data = json.as_bytes();
    let mut gz = alloc::vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
    let mut chunks = data.chunks(65535).peekable();
    while let Some(c) = chunks.next() {
        gz.push(chunks.peek().is_none() as u8); // BFINAL, BTYPE = stored
        gz.extend_from_slice(&(c.len() as u16).to_le_bytes());
        gz.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        gz.extend_from_slice(c);
    }
    gz.extend_from_slice(&disk::crc32::crc32(data).to_le_bytes());
    gz.extend_from_slice(&(data.len() as u32).to_le_bytes());
    gz
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::vec::Vec;

    const SUPPORTED: &[u8] = b"ARCHSTLEIND";

    fn expected_glyph(c: u8) -> [&'static str; 5] {
        match c {
            b'A' => [" ### ", "#   #", "#####", "#   #", "#   #"],
            b'R' => ["#### ", "#   #", "#### ", "#  # ", "#   #"],
            b'C' => [" ####", "#    ", "#    ", "#    ", " ####"],
            b'H' => ["#   #", "#   #", "#####", "#   #", "#   #"],
            b'S' => [" ####", "#    ", " ### ", "    #", "#### "] ,
            b'T' => ["#####", "  #  ", "  #  ", "  #  ", "  #  "],
            b'L' => ["#    ", "#    ", "#    ", "#    ", "#####"],
            b'E' => ["#####", "#    ", "#### ", "#    ", "#####"],
            b'I' => ["#####", "  #  ", "  #  ", "  #  ", "#####"],
            b'N' => ["#   #", "##  #", "# # #", "#  ##", "#   #"],
            b'D' => ["#### ", "#   #", "#   #", "#   #", "#### "],
            _ => ["     "; 5],
        }
    }

    #[test]
    fn test_render_line_count_and_empty() {
        let empty = render("");
        let lines: Vec<&str> = empty.lines().collect();
        assert_eq!(lines.len(), 5);
        for l in lines {
            assert!(l.is_empty());
        }
    }

    #[test]
    fn test_render_all_supported_glyphs() {
        for &c in SUPPORTED {
            let bytes = [c];
            let s = core::str::from_utf8(&bytes).expect("ascii byte");
            let rendered = render(s);
            let lines: Vec<&str> = rendered.lines().collect();
            assert_eq!(lines.len(), 5);
            let exp = expected_glyph(c);
            for row in 0..5 {
                assert_eq!(lines[row], format!("{}  ", exp[row]));
            }
        }
    }

    #[test]
    fn test_render_fallback_and_multiword() {
        let unk = render("?!");
        let lines: Vec<&str> = unk.lines().collect();
        assert_eq!(lines.len(), 5);
        for l in lines {
            assert_eq!(l, "              ");
        }

        let mw = render("ARCH STALLER");
        let mw_lines: Vec<&str> = mw.lines().collect();
        assert_eq!(mw_lines.len(), 5);
        for l in mw_lines {
            assert_eq!(l.len(), 12 * 7);
        }
    }

    fn decompress_stored_gz(gz: &[u8]) -> Vec<u8> {
        assert!(gz.len() >= 18);
        assert_eq!(gz[0], 0x1f);
        assert_eq!(gz[1], 0x8b);
        assert_eq!(gz[2], 8);
        assert_eq!(gz[3], 0);
        assert_eq!(gz[8], 0);
        assert_eq!(gz[9], 0xff);

        let mut offset = 10;
        let mut out = Vec::new();
        let payload_end = gz.len() - 8;

        while offset < payload_end {
            let b = gz[offset];
            let bfinal = (b & 1) != 0;
            let btype = (b >> 1) & 0x03;
            assert_eq!(btype, 0, "stored deflate block");
            offset += 1;

            let len = u16::from_le_bytes(gz[offset..offset + 2].try_into().expect("slice")) as usize;
            let nlen = u16::from_le_bytes(gz[offset + 2..offset + 4].try_into().expect("slice"));
            assert_eq!(nlen, !(len as u16));
            offset += 4;

            out.extend_from_slice(&gz[offset..offset + len]);
            offset += len;

            if bfinal {
                break;
            }
        }
        assert_eq!(offset, payload_end);

        let crc = u32::from_le_bytes(gz[payload_end..payload_end + 4].try_into().expect("crc"));
        let isize = u32::from_le_bytes(gz[payload_end + 4..].try_into().expect("isize"));
        assert_eq!(disk::crc32::crc32(&out), crc);
        assert_eq!(out.len() as u32, isize);
        out
    }

    #[test]
    #[expect(
        clippy::string_slice,
        reason = "test verifies known ASCII JSON structure"
    )]
    fn test_dur_decompression_and_schema() {
        for word in ["", "A", "ARCHSTALLER", "INSTALLED", "ARCH 2026"] {
            let gz = dur(word);
            let payload = decompress_stored_gz(&gz);
            let json = core::str::from_utf8(&payload).expect("utf8 json");

            assert!(json.starts_with("{\"DurMovie\":{"));
            assert!(json.contains("\"formatVersion\":7"));
            assert!(json.contains("\"colorFormat\":\"16\""));
            assert!(json.contains("\"framerate\":6.0"));

            let art = render(word);
            let rows: Vec<&str> = art.lines().collect();
            let width = rows.iter().map(|r| r.trim_end().len()).max().unwrap_or(0);

            assert!(json.contains(&format!("\"sizeX\":{width}")));
            assert!(json.contains("\"sizeY\":5"));
            assert!(json.contains("\"frames\":[{\"frameNumber\":1,\"delay\":0,"));

            let contents_tag = "\"contents\":[";
            let c_pos = json.find(contents_tag).expect("contents");
            let c_end = json[c_pos..].find(']').expect("contents end");
            let c_body = &json[c_pos + contents_tag.len()..c_pos + c_end];
            let content_items: Vec<&str> = if c_body.is_empty() {
                Vec::new()
            } else {
                c_body.split(',').collect()
            };
            assert_eq!(content_items.len(), 5);

            let cmap_tag = "\"colorMap\":[";
            let m_pos = json.find(cmap_tag).expect("colorMap");
            assert!(json.ends_with("]}]}}"));
            let m_body = &json[m_pos + cmap_tag.len()..json.len() - 5];
            if width == 0 {
                assert_eq!(m_body, "");
            } else {
                let inner = &m_body[1..m_body.len() - 1];
                let cols: Vec<&str> = inner.split("]],[[").collect();
                assert_eq!(cols.len(), width);
                let total_cells = m_body.matches("[7,0]").count() + m_body.matches("[8,0]").count();
                assert_eq!(total_cells, width * 5);
            }
        }
    }

    #[test]
    fn test_issue_and_ly_config_durability() {
        let art = render("ARCHSTALLER");
        let issue = format!("\n{art}\nArch Linux \\r (\\l)\n\n");
        assert!(issue.starts_with('\n'));
        assert!(issue.contains("Arch Linux \\r (\\l)\n\n"));
        assert_eq!(issue.lines().filter(|l| l.contains('#')).count(), 5);

        let motd = format!("\n{art}\nInstalled by archstaller.\n\n");
        assert!(motd.contains("Installed by archstaller."));

        let ly_conf = "animation = dur_file\ndur_file_path = /etc/ly/archstaller.dur\ndur_offset_alignment = topcenter\ndur_y_offset = 2\n";
        assert!(ly_conf.contains("animation = dur_file"));
        assert!(ly_conf.contains("/etc/ly/archstaller.dur"));
    }
}
