//! # Video Interface (VI) Driver
//!
//! The Video Interface is the GameCube/Wii hardware block that:
//! 1. Programs the DAC with timing signals for the TV standard (NTSC/PAL).
//! 2. DMA-reads the XFB (external framebuffer) from MEM1/MEM2.
//! 3. Converts the XFB from YCbCr 4:2:2 to analog RGB/composite/component.
//!
//! ## Register Map
//!
//! All VI registers are 16-bit, memory-mapped starting at `0xCC002000`.
//!
//! ```text
//! Offset  Name   Description
//! ------  -----  ----------------------------------------------------------
//! 0x00    VTR    Vertical timing / equalization pulse count
//! 0x02    DCR    Display Configuration: TV mode, interlace, enable bit
//! 0x04    HTR0   Horizontal timing 0: HCS/HCE (sync pulse positions)
//! 0x06    HTR1   Horizontal timing 1: half-line width (HLW)
//! 0x08    HBE640 Horizontal blanking end (640px mode)
//! 0x0A    HBS640 Horizontal blanking start (640px mode)
//! 0x0C    VTO    Vertical timing odd field: PSB/PRB
//! 0x0E    VTO2   Vertical timing odd field line count
//! 0x10    VTE    Vertical timing even field: PSB/PRB
//! 0x12    VTE2   Vertical timing even field line count
//! 0x14    BE3/BS3, 0x16 BE1/BS1, 0x18 BE4/BS4, 0x1A BE2/BS2 (burst/blanking)
//! 0x1C    TFBL_H Top field base addr [high byte]
//! 0x1E    TFBL_L Top field base addr [low word]
//! 0x20    TFBR_H Top field base addr right [high byte] (3D mode)
//! 0x22    TFBR_L Top field base addr right [low word]  (3D mode)
//! 0x24    BFBL_H Bottom field base addr [high byte]
//! 0x26    BFBL_L Bottom field base addr [low word]
//! 0x28    BFBR_H Bottom field base addr right [high byte] (3D mode)
//! 0x2A    BFBR_L Bottom field base addr right [low word]  (3D mode)
//! 0x30    DI0    Display interrupt 0
//! 0x32    DI0_L
//! 0x34    DI1    Display interrupt 1
//! 0x36    DI1_L
//! 0x38    DI2    Display interrupt 2
//! 0x3A    DI2_L
//! 0x3C    DI3    Display interrupt 3
//! 0x3E    DI3_L
//! 0x48    HSR    Horizontal scaling register
//! 0x4C-0x7A     Filter coefficient registers
//! 0x70    FBW    Framebuffer width
//! ```
//!
//! ## XFB Format
//!
//! The VI reads the framebuffer in **YCbCr 4:2:2** format:
//! - Two pixels are stored per 32-bit word: `[Y0, Cb, Y1, Cr]`
//! - `Y` (luma): 16 (black) … 235 (white)
//! - `Cb`, `Cr` (chroma): 128 is neutral grey
//!
//! White pixel pair:  `0xEB80_EB80`
//! Black pixel pair:  `0x1080_1080`

pub mod regs;
pub mod timing;

use regs::VI_REGS;
use timing::{Timing, NTSC_480I};

// ──────────────────────────────────────────────────────────────────────────────
// Public types
// ──────────────────────────────────────────────────────────────────────────────

/// Supported TV standards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TvMode {
    /// NTSC (North America, Japan) — 480 lines, 60 Hz
    Ntsc,
    /// PAL (Europe) — 576 lines, 50 Hz
    Pal,
    /// MPAL (Brazil) — NTSC lines, PAL subcarrier
    Mpal,
}

/// Scan mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// Interlaced: two fields per frame, alternating odd/even lines.
    Interlaced,
    /// Progressive: full frame each vsync.
    Progressive,
}

/// Video mode descriptor — fully describes a display configuration.
#[derive(Debug, Clone, Copy)]
pub struct VideoMode {
    pub tv:          TvMode,
    pub scan:        ScanMode,
    /// Active display width in pixels (multiple of 16).
    pub fb_width:    u32,
    /// Active display height in lines.
    pub fb_height:   u32,
    /// X origin in VI pixel units (centres the image).
    pub vi_x_origin: u16,
    /// Y origin in VI line units.
    pub vi_y_origin: u16,
    /// Width of the VI output (normally 640 or 720).
    pub vi_width:    u16,
    /// Height of the VI output in lines.
    pub vi_height:   u16,
}

/// NTSC 480i — the standard GameCube video mode.
pub const NTSC_480I_MODE: VideoMode = VideoMode {
    tv:          TvMode::Ntsc,
    scan:        ScanMode::Interlaced,
    fb_width:    640,
    fb_height:   480,
    vi_x_origin: 40,   // (720-640)/2
    vi_y_origin: 0,
    vi_width:    640,
    vi_height:   480,
};

// ──────────────────────────────────────────────────────────────────────────────
// Public API
// ──────────────────────────────────────────────────────────────────────────────

/// Initialise the Video Interface for NTSC 480i.
///
/// This programs all VI timing registers, sets up the scan mode, and enables
/// the display. Call `set_framebuffer` to point the VI at an XFB before
/// calling `flush`.
///
/// # Safety
///
/// Must be called from a single context (no concurrent VI access).
/// Should be called early in `main` before drawing anything.
pub unsafe fn init_ntsc_480i() {
    configure(&NTSC_480I_MODE, &NTSC_480I);
}

/// Point the Video Interface at the given framebuffer.
///
/// The `xfb` pointer must be 32-byte aligned and point to a region of
/// `fb_width * fb_height * 2` bytes in MEM1.
///
/// For NTSC 480i (single-field mode), the top-field and bottom-field
/// pointers both point to the same buffer with stride between them.
///
/// # Safety
///
/// The caller must ensure `xfb` is valid for the full framebuffer size
/// and that the cache has been flushed before this call.
pub unsafe fn set_framebuffer(xfb: *mut u32, stride_bytes: u32) {
    // Convert virtual address (0x80xxxxxx) to physical (0x00xxxxxx)
    let phys = (xfb as u32) & 0x1FFF_FFFF;

    // Address is stored right-shifted by 5 in the BAR registers.
    let tfbb = phys >> 5;
    let bfbb = (phys + stride_bytes) >> 5;

    let regs = &*VI_REGS.get();

    // TFBL: top field base address
    // reg[14] = flag(1) | xof(4) | addr_hi(8)   — flag=0 for <32MB, xof=0
    // reg[15] = addr_lo(16)
    regs.write(14, ((tfbb >> 16) & 0x00FF) as u16);
    regs.write(15, (tfbb & 0xFFFF) as u16);

    // BFBL: bottom field base address (for interlaced DF mode; SF = same as TF)
    regs.write(18, ((bfbb >> 16) & 0x00FF) as u16);
    regs.write(19, (bfbb & 0xFFFF) as u16);

    // Framebuffer width (in units of 16 pixels)
    regs.write(56, 640u16);
}

/// Flush the VI shadow registers to the hardware.
///
/// Until this is called, register writes are queued. This triggers the
/// display output to update at the next vsync.
///
/// # Safety
///
/// Must not be called concurrently with `set_framebuffer` or `init_*`.
pub unsafe fn flush() {
    // A `sync` ensures all prior writes to cached memory are visible
    // before the VI starts reading the XFB.
    dkdol_rt::cache::sync();
}

// ──────────────────────────────────────────────────────────────────────────────
// Internal: configure timing and scan mode
// ──────────────────────────────────────────────────────────────────────────────

unsafe fn configure(mode: &VideoMode, timing: &Timing) {
    let regs = &*VI_REGS.get();

    // Reset the VI
    regs.write(1, 0x0002);
    // Short spin to let the reset settle
    for _ in 0..4000u32 { core::arch::asm!("nop", options(nomem, nostack)); }
    regs.write(1, 0x0000);

    // ── Horizontal timing ──────────────────────────────────────────────
    // HTR0 = [HCS(8) | HCE(8)]
    regs.write(2, ((timing.hcs as u16) << 8) | (timing.hce as u16));
    // HTR1 = HLW (half-line width)
    regs.write(3, timing.hlw);
    // HBE640 = HBS640 << 1
    regs.write(4, timing.hbs640 << 1);
    // HBS640 = [HBE640(7) | HSY(9)]
    regs.write(5, ((timing.hbe640 as u16) << 7) | (timing.hsy as u16));

    // ── Vertical equalization pulse count ─────────────────────────────
    regs.write(0, timing.equ as u16);

    // ── Vertical blanking: odd field ──────────────────────────────────
    let acv2m2 = (timing.acv << 1).wrapping_sub(2);
    regs.write(6, timing.psb_odd.wrapping_add(2));
    regs.write(7, timing.prb_odd.wrapping_add(acv2m2));

    // ── Vertical blanking: even field ─────────────────────────────────
    regs.write(8, timing.psb_even.wrapping_add(2));
    regs.write(9, timing.prb_even.wrapping_add(acv2m2));

    // ── Burst/blanking boundaries ─────────────────────────────────────
    regs.write(10, ((timing.be3 as u16) << 5) | (timing.bs3 as u16));
    regs.write(11, ((timing.be1 as u16) << 5) | (timing.bs1 as u16));
    regs.write(12, ((timing.be4 as u16) << 5) | (timing.bs4 as u16));
    regs.write(13, ((timing.be2 as u16) << 5) | (timing.bs2 as u16));

    // ── Display interrupts ────────────────────────────────────────────
    let nhlines_half = (timing.nhlines / 2).wrapping_add(1);
    regs.write(24, 0x1000 | nhlines_half);
    regs.write(25, timing.hlw.wrapping_add(1));
    regs.write(26, 0x1001);   // DI1 high
    regs.write(27, 0x0001);   // DI1 low
    regs.write(36, 0x2828);   // HSR

    // ── DCR: display config register ─────────────────────────────────
    // Bits [9:8] = TV mode (0=NTSC, 1=PAL, 2=MPAL)
    // Bit  [2]   = interlace
    // Bit  [0]   = enable
    let vi_mode: u16 = match mode.tv {
        TvMode::Ntsc => 0,
        TvMode::Pal  => 1,
        TvMode::Mpal => 2,
    };
    let interlace: u16 = match mode.scan {
        ScanMode::Interlaced   => 1,
        ScanMode::Progressive  => 0,
    };
    let dcr = (vi_mode << 8) | (interlace << 2) | 0x0001;
    regs.write(1, dcr);
}
