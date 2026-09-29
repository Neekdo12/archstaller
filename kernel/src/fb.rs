use core::fmt;

static FONT: &[u8] = include_bytes!("../font/default8x16.psf");

const FG: u32 = 0xd0d0d0;
const BG: u32 = 0x000000;

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
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]) as usize
}

impl FbConsole {
    /// Only 32 bpp RGB framebuffers are supported.
    pub fn new(fb: &limine::framebuffer::Framebuffer) -> Option<Self> {
        if fb.bpp != 32 || fb.memory_model != limine::framebuffer::FRAMEBUFFER_RGB {
            return None;
        }
        if FONT.len() < 32 || FONT[..4] != [0x72, 0xb5, 0x4a, 0x86] {
            return None;
        }
        let header = le32(FONT, 8);
        let char_size = le32(FONT, 20);
        let glyph_h = le32(FONT, 24);
        let glyph_w = le32(FONT, 28);
        if glyph_w != 8 {
            return None;
        }
        let pack = |rgb: u32| -> u32 {
            let r = (rgb >> 16) & 0xff;
            let g = (rgb >> 8) & 0xff;
            let b = rgb & 0xff;
            (r << fb.red_mask_shift) | (g << fb.green_mask_shift) | (b << fb.blue_mask_shift)
        };
        let mut c = FbConsole {
            base: fb.address() as *mut u8,
            pitch: fb.pitch as usize,
            cols: fb.width as usize / 8,
            rows: fb.height as usize / glyph_h,
            fg: pack(FG),
            bg: pack(BG),
            x: 0,
            y: 0,
            glyphs: &FONT[header..],
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

    fn clear(&mut self) {
        self.fill_rows(0, self.rows * self.glyph_h);
        self.x = 0;
        self.y = 0;
    }

    fn scroll(&mut self) {
        let text_h = self.glyph_h;
        for row in text_h..self.rows * text_h {
            unsafe {
                core::ptr::copy(
                    self.base.add(row * self.pitch),
                    self.base.add((row - text_h) * self.pitch),
                    self.cols * 8 * 4,
                );
            }
        }
        self.fill_rows((self.rows - 1) * text_h, text_h);
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
        let glyph = &self.glyphs[ch as usize * self.char_size..][..self.char_size];
        for (r, bits) in glyph.iter().enumerate().take(self.glyph_h) {
            let line = unsafe { self.base.add((self.y * self.glyph_h + r) * self.pitch) } as *mut u32;
            for c in 0..8 {
                let on = bits & (0x80 >> c) != 0;
                unsafe { line.add(self.x * 8 + c).write_volatile(if on { self.fg } else { self.bg }) };
            }
        }
        self.x += 1;
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
