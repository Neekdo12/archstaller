use core::fmt;

static FONT: &[u8] = include_bytes!("../font/default8x16.psf");

const FG: u32 = 0xd0d0d0;
const BG: u32 = 0x000000;

/// Character cells for up to 4096 x 4320 pixels (larger screens use this much of themselves).
const MAX_COLS: usize = 512;
const MAX_ROWS: usize = 270;

/// What each cell on screen shows. Scrolling redraws from here instead of copying pixels inside the
/// framebuffer: video memory is uncached or write-combining, so reading it back is very slow (a whole
/// screen per line printed), while writes are cheap. Only cells whose character changes are redrawn.
static mut CELLS: [u8; MAX_COLS * MAX_ROWS] = [0; MAX_COLS * MAX_ROWS]; // `clear` fills it before use

pub struct FbConsole {
    base: *mut u8,
    pitch: usize,
    cols: usize,
    rows: usize,
    fg: u32,
    bg: u32,
    x: usize,
    y: usize,
    glyphs: &'static [u8],
    char_size: usize,
    glyph_h: usize,
}

unsafe impl Send for FbConsole {}

fn le32(b: &[u8], off: usize) -> usize {
    let mut arr = [0u8; 4];
    if let Some(src) = b.get(off..off + 4) {
        arr.copy_from_slice(src);
    }
    u32::from_le_bytes(arr) as usize
}

impl FbConsole {
    /// Only 32 bpp RGB framebuffers are supported.
    pub fn new(hhdm: u64, fb: &bootinfo::Framebuffer) -> Option<Self> {
        if fb.addr == 0 || fb.bpp != 32 {
            return None;
        }
        if FONT.len() < 32 || FONT.get(..4) != Some(&[0x72, 0xb5, 0x4a, 0x86]) {
            return None;
        }
        let header = le32(FONT, 8);
        let char_size = le32(FONT, 20);
        let glyph_h = le32(FONT, 24);
        let glyph_w = le32(FONT, 28);
        if glyph_w != 8
            || glyph_h == 0
            || (fb.width as usize) < 8
            || (fb.height as usize) < glyph_h
        {
            return None;
        }
        let pack = |rgb: u32| -> u32 {
            let r = (rgb >> 16) & 0xff;
            let g = (rgb >> 8) & 0xff;
            let b = rgb & 0xff;
            (r << fb.red_shift) | (g << fb.green_shift) | (b << fb.blue_shift)
        };
        let mut c = FbConsole {
            base: (hhdm + fb.addr) as *mut u8,
            pitch: fb.pitch as usize,
            cols: (fb.width as usize / 8).min(MAX_COLS),
            rows: (fb.height as usize / glyph_h).min(MAX_ROWS),
            fg: pack(FG),
            bg: pack(BG),
            x: 0,
            y: 0,
            glyphs: FONT.get(header..).unwrap_or(&[]),
            char_size,
            glyph_h,
        };
        c.clear();
        Some(c)
    }

    fn fill_rows(&mut self, first_px_row: usize, px_rows: usize) {
        for row in first_px_row..first_px_row + px_rows {
            let line = unsafe { self.base.add(row * self.pitch) } as *mut u32;
            for x in 0..self.cols * 8 {
                unsafe { line.add(x).write_volatile(self.bg) };
            }
        }
    }

    fn cells(&mut self) -> &mut [u8] {
        // SAFETY: there is one console, and it is only used under the console lock.
        unsafe {
            core::slice::from_raw_parts_mut((&raw mut CELLS).cast::<u8>(), self.cols * self.rows)
        }
    }

    fn clear(&mut self) {
        self.fill_rows(0, self.rows * self.glyph_h);
        self.cells().fill(b' ');
        self.x = 0;
        self.y = 0;
    }

    /// Moves every line up by one, redrawing only the cells that change.
    fn scroll(&mut self) {
        let cols = self.cols;
        let total = cols * self.rows;
        for i in 0..total {
            let below = if i + cols < total {
                self.cells().get(i + cols).copied().unwrap_or(b' ')
            } else {
                b' '
            };
            if self.cells().get(i).copied() != Some(below) {
                if let Some(slot) = self.cells().get_mut(i) {
                    *slot = below;
                }
                self.render(i % cols, i / cols, below);
            }
        }
    }

    fn newline(&mut self) {
        self.x = 0;
        if self.y + 1 >= self.rows {
            self.scroll();
        } else {
            self.y += 1;
        }
    }

    fn draw(&mut self, ch: u8) {
        let ch = if (0x20..0x7f).contains(&ch) { ch } else { b'?' };
        if self.x >= self.cols {
            self.newline();
        }
        let i = self.y * self.cols + self.x;
        if let Some(slot) = self.cells().get_mut(i) {
            *slot = ch;
        }
        self.render(self.x, self.y, ch);
        self.x += 1;
    }

    /// Paints `ch` into the cell at column `x`, row `y`.
    fn render(&mut self, x: usize, y: usize, ch: u8) {
        let off = ch as usize * self.char_size;
        let glyph = self.glyphs.get(off..off + self.char_size).unwrap_or(&[]);
        for (r, bits) in glyph.iter().enumerate().take(self.glyph_h) {
            let line = unsafe { self.base.add((y * self.glyph_h + r) * self.pitch) } as *mut u32;
            for c in 0..8 {
                let on = bits & (0x80 >> c) != 0;
                unsafe {
                    line.add(x * 8 + c)
                        .write_volatile(if on { self.fg } else { self.bg })
                };
            }
        }
    }
}

impl fmt::Write for FbConsole {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            match b {
                b'\n' => self.newline(),
                b'\r' => self.x = 0,
                _ => self.draw(b),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_or_zero_framebuffer_returns_none() {
        let mut fb = bootinfo::Framebuffer {
            addr: 0x10000,
            bpp: 32,
            pitch: 800 * 4,
            width: 4,
            height: 4,
            ..Default::default()
        };
        assert!(FbConsole::new(0, &fb).is_none());

        fb.width = 800;
        fb.height = 5;
        assert!(FbConsole::new(0, &fb).is_none());
    }
}
