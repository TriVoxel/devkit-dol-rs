//! Scrolling text console for the GameCube XFB.
//!
//! [`Console`] renders 8×8 glyph characters directly into an [`Xfb`].
//! It tracks a cursor position, handles newlines, and scrolls when the
//! cursor reaches the bottom of the screen.
//!
//! ## Character cell size
//!
//! Each cell is 8×8 pixels. For a 640×480 XFB this gives 80 columns × 60 rows.
//!
//! ## Color
//!
//! The console has configurable foreground and background [`YcbcrPair`] colors.
//! Foreground applies to set bits; background to clear bits.
//!
//! ## Cache flushing
//!
//! After drawing characters the modified cache lines are flushed so the
//! VI hardware can read the updated pixels at the next vsync.

use crate::{Xfb, YcbcrPair};
use crate::font::{glyph, GLYPH_H, GLYPH_W};
use dkdol_rt::cache;

/// Scrolling text console.
pub struct Console<'a> {
    xfb:        &'a mut Xfb,
    /// Current cursor column (in character cells).
    col:        u32,
    /// Current cursor row (in character cells).
    row:        u32,
    /// Total columns that fit on screen.
    cols:       u32,
    /// Total rows that fit on screen.
    rows:       u32,
    /// Foreground color (set pixels).
    fg:         YcbcrPair,
    /// Background color (clear pixels).
    bg:         YcbcrPair,
}

impl<'a> Console<'a> {
    /// Create a new console backed by `xfb`.
    ///
    /// The console takes over the entire framebuffer. Initial colors are
    /// white-on-black.
    pub fn new(xfb: &'a mut Xfb) -> Self {
        let cols = xfb.width()  / GLYPH_W as u32;
        let rows = xfb.height() / GLYPH_H as u32;
        Console {
            xfb,
            col:  0,
            row:  0,
            cols,
            rows,
            fg:   YcbcrPair::WHITE,
            bg:   YcbcrPair::BLACK,
        }
    }

    /// Set the foreground (text) color.
    #[inline] pub fn set_fg(&mut self, color: YcbcrPair) { self.fg = color; }
    /// Set the background color.
    #[inline] pub fn set_bg(&mut self, color: YcbcrPair) { self.bg = color; }

    /// Clear the screen and move the cursor to (0, 0).
    pub fn clear(&mut self) {
        self.xfb.clear(self.bg);
        self.col = 0;
        self.row = 0;
    }

    /// Print a single character.
    ///
    /// `\n` moves to the next line. `\r` moves to column 0.
    /// Any other non-printable character is rendered as '?'.
    pub fn putchar(&mut self, c: u8) {
        match c {
            b'\n' => {
                self.col = 0;
                self.next_line();
            }
            b'\r' => {
                self.col = 0;
            }
            _ => {
                self.draw_glyph(c);
                self.col += 1;
                if self.col >= self.cols {
                    self.col = 0;
                    self.next_line();
                }
            }
        }
    }

    /// Print a byte slice as text.
    pub fn print(&mut self, s: &[u8]) {
        for &c in s { self.putchar(c); }
    }

    /// Print a `&str`.
    pub fn print_str(&mut self, s: &str) {
        self.print(s.as_bytes());
    }

    /// Flush all modified cache lines covering the console area to RAM.
    ///
    /// Must be called before the VI reads the XFB at the next vsync.
    pub fn flush(&self) {
        let ptr  = self.xfb.as_ptr() as *const u8;
        let len  = self.xfb.byte_len();
        unsafe { cache::dcbf_range(ptr, len); }
    }

    // ──────────────────────────────────────────────────────────────────────
    // Internal
    // ──────────────────────────────────────────────────────────────────────

    /// Advance to the next line, scrolling if needed.
    fn next_line(&mut self) {
        self.row += 1;
        if self.row >= self.rows {
            self.scroll_up();
            self.row = self.rows - 1;
        }
    }

    /// Scroll the framebuffer up by one character row (8 pixels).
    ///
    /// The bottom row is filled with the background color.
    fn scroll_up(&mut self) {
        let w = self.xfb.width();
        let h = self.xfb.height();
        let words_per_line = (w / 2) as usize;
        let glyph_lines    = GLYPH_H;
        let total_words    = (w * h / 2) as usize;
        let shift_words    = words_per_line * glyph_lines;

        unsafe {
            let ptr = self.xfb.as_ptr();
            // Move all rows up by GLYPH_H scanlines.
            core::ptr::copy(
                ptr.add(shift_words),
                ptr,
                total_words - shift_words,
            );
            // Fill the newly exposed bottom row with background.
            let bottom = ptr.add(total_words - shift_words);
            for i in 0..shift_words {
                core::ptr::write_volatile(bottom.add(i), self.bg.0);
            }
        }
    }

    /// Render one 8×8 glyph at the current cursor cell position.
    fn draw_glyph(&mut self, c: u8) {
        let bitmap   = glyph(c);
        let px_col   = self.col * GLYPH_W as u32;
        let px_row   = self.row * GLYPH_H as u32;
        let w        = self.xfb.width();
        let stride   = (w / 2) as usize; // words per scanline
        let fg       = self.fg.0;
        let bg       = self.bg.0;

        // XFB pixel layout: pairs of pixels share chroma.
        // For each 8-pixel row of the glyph we write 4 u32 words.
        //
        // The XFB word covering pixels (px, px+1):
        //   bits[31:24] = Y0  (luma of left pixel)
        //   bits[23:16] = Cb  (shared chroma, blue)
        //   bits[15:8]  = Y1  (luma of right pixel)
        //   bits[7:0]   = Cr  (shared chroma, red)
        //
        // We extract Y from fg/bg and blend: if bit is set → fg-Y, else → bg-Y.
        // Cb/Cr come from fg for a set bit, bg otherwise (half-pixel pair granularity).

        let fg_y0 = ((fg >> 24) & 0xFF) as u8;
        let fg_cb = ((fg >> 16) & 0xFF) as u8;
        let fg_y1 = ((fg >>  8) & 0xFF) as u8;
        let fg_cr = ( fg        & 0xFF) as u8;

        let bg_y0 = ((bg >> 24) & 0xFF) as u8;
        let bg_cb = ((bg >> 16) & 0xFF) as u8;
        let bg_y1 = ((bg >>  8) & 0xFF) as u8;
        let bg_cr = ( bg        & 0xFF) as u8;

        unsafe {
            let fb_ptr = self.xfb.as_ptr();
            for row in 0..GLYPH_H {
                let bits    = bitmap[row];
                let scanline_base = (px_row as usize + row) * stride + (px_col as usize / 2);
                // Process 4 pixel pairs (8 pixels)
                for pair in 0..4usize {
                    let bit_left  = (bits >> (7 - pair * 2))     & 1;
                    let bit_right = (bits >> (7 - pair * 2 - 1)) & 1;

                    let y0 = if bit_left  != 0 { fg_y0 } else { bg_y0 };
                    let cb = if bit_left  != 0 { fg_cb } else { bg_cb };
                    let y1 = if bit_right != 0 { fg_y1 } else { bg_y1 };
                    let cr = if bit_right != 0 { fg_cr } else { bg_cr };

                    let word = ((y0 as u32) << 24)
                             | ((cb as u32) << 16)
                             | ((y1 as u32) <<  8)
                             |  (cr as u32);

                    core::ptr::write_volatile(fb_ptr.add(scanline_base + pair), word);
                }
            }
        }
    }
}

/// Implement core::fmt::Write so `write!` / `writeln!` macros work with Console.
impl<'a> core::fmt::Write for Console<'a> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.print_str(s);
        Ok(())
    }
}
