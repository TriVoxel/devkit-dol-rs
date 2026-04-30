//! # gc-gfx — GameCube Framebuffer Graphics
//!
//! Provides:
//! - [`Xfb`]: a typed wrapper around the External Framebuffer (YCbCr 4:2:2).
//! - [`Console`]: a scrolling text console rendered directly into the XFB.
//!
//! The GameCube Video Interface reads the XFB in **YCbCr 4:2:2** format.
//! Each pair of pixels is stored as a single 32-bit word `[Y0, Cb, Y1, Cr]`.

#![no_std]

pub mod console;
pub mod font;
pub mod color;

pub use console::Console;

/// Pixel color in YCbCr 4:2:2 format (two pixels per word).
///
/// The GC XFB stores pairs of pixels as `[Y0, Cb, Y1, Cr]` (big-endian u32).
/// Both pixels in the pair share the same Cb and Cr chroma values.
///
/// Standard Y range: 16 (black) … 235 (white). Cb/Cr neutral: 128.
#[derive(Clone, Copy, Debug)]
pub struct YcbcrPair(pub u32);

impl YcbcrPair {
    /// Create a pixel pair from individual components.
    #[inline]
    pub const fn new(y0: u8, cb: u8, y1: u8, cr: u8) -> Self {
        Self(((y0 as u32) << 24) | ((cb as u32) << 16) | ((y1 as u32) << 8) | (cr as u32))
    }

    /// White pixel pair (Y=235, Cb=128, Cr=128).
    pub const WHITE: Self = Self::new(235, 128, 235, 128);
    /// Black pixel pair (Y=16, Cb=128, Cr=128).
    pub const BLACK: Self = Self::new(16, 128, 16, 128);
    /// Grey pixel pair (Y=128, Cb=128, Cr=128).
    pub const GREY:  Self = Self::new(128, 128, 128, 128);
}

/// External Framebuffer — a region of MEM1 read by the Video Interface.
///
/// The XFB stores pixels in YCbCr 4:2:2 format. Two pixels occupy one
/// 32-bit word, giving a stride of `width * 2` bytes per scanline.
///
/// # Layout
///
/// ```text
/// Word 0:  [Y0, Cb0, Y1, Cr0]   pixels (0,0) and (1,0)
/// Word 1:  [Y2, Cb1, Y3, Cr1]   pixels (2,0) and (3,0)
/// ...
/// Word W/2-1: last pair on scanline 0
/// Word W/2:   first pair on scanline 1
/// ...
/// ```
pub struct Xfb {
    /// Pointer to the framebuffer in MEM1 (cached virtual address 0x80xxxxxx).
    ptr:    *mut u32,
    /// Width in pixels (must be a multiple of 2).
    width:  u32,
    /// Height in scanlines.
    height: u32,
    /// Stride in **32-bit words** per scanline (= width / 2).
    stride: u32,
}

impl Xfb {
    /// Wrap a raw pointer as an [`Xfb`].
    ///
    /// # Safety
    ///
    /// `ptr` must be:
    /// - 32-byte aligned
    /// - Valid for `width * height * 2` bytes
    /// - Located in MEM1 (0x80000000–0x817FFFFF)
    ///
    /// The framebuffer size must not overlap the stack or any code/data sections.
    pub const unsafe fn from_raw(ptr: *mut u32, width: u32, height: u32) -> Self {
        Self { ptr, width, height, stride: width / 2 }
    }

    /// Width in pixels.
    #[inline] pub fn width(&self)  -> u32 { self.width  }
    /// Height in scanlines.
    #[inline] pub fn height(&self) -> u32 { self.height }
    /// Raw pointer to the first word of the framebuffer.
    #[inline] pub fn as_ptr(&self) -> *mut u32 { self.ptr }
    /// Size in bytes.
    #[inline] pub fn byte_len(&self) -> usize { (self.width * self.height * 2) as usize }

    /// Fill the entire framebuffer with the given pixel pair color.
    pub fn clear(&mut self, color: YcbcrPair) {
        let total_words = (self.stride * self.height) as usize;
        unsafe {
            for i in 0..total_words {
                core::ptr::write_volatile(self.ptr.add(i), color.0);
            }
        }
        // Flush the entire framebuffer out of the data cache so the VI can read it.
        unsafe {
            gc_rt::cache::dcbf_range(self.ptr as *const u8, self.byte_len());
        }
    }

    /// Write a single pixel pair at column `col` (must be even) and row `row`.
    ///
    /// # Safety
    ///
    /// `col` must be even (pairs of pixels share chroma). `row < height`, `col < width`.
    #[inline]
    pub unsafe fn write_pair(&mut self, col: u32, row: u32, color: YcbcrPair) {
        let offset = (row * self.stride + col / 2) as usize;
        core::ptr::write_volatile(self.ptr.add(offset), color.0);
    }
}
