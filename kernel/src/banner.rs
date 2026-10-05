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
            out.push_str(glyph(c)[row]);
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
            crate::print!("{}  ", glyph(c)[row]);
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
