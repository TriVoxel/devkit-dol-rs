//! VI timing parameters for each TV standard.
//!
//! These values were verified against the libogc2 `video_timing[]` table
//! and YAGCD section 10.2 (Video Interface Timing).
//!
//! Each `Timing` struct contains the raw values written to the VI registers
//! during `VIDEO_Configure`. The fields correspond directly to the VI register
//! programming sequence in `vi::mod::configure`.

/// VI timing parameters for a single TV standard.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Equalization pulse count (VTR register, index 0).
    pub equ:       u8,
    /// Active color video lines per field (half the total for interlaced).
    pub acv:       u16,
    /// Pre-Blanking odd field.
    pub prb_odd:   u16,
    /// Pre-Blanking even field.
    pub prb_even:  u16,
    /// Post-Blanking odd field.
    pub psb_odd:   u16,
    /// Post-Blanking even field.
    pub psb_even:  u16,
    /// Blanking start/end positions for four burst periods (bs1..bs4, be1..be4).
    pub bs1:  u8,  pub bs2:  u8,  pub bs3:  u8,  pub bs4:  u8,
    pub be1: u16,  pub be2: u16,  pub be3: u16,  pub be4: u16,
    /// Total number of half-lines per frame.
    pub nhlines: u16,
    /// Half-line width (number of pixel-clock cycles per half-line).
    pub hlw:     u16,
    /// Horizontal sync pulse position.
    pub hsy:  u8,
    /// Horizontal colour start.
    pub hcs:  u8,
    /// Horizontal colour end.
    pub hce:  u8,
    /// Horizontal blanking end for 640-pixel mode.
    pub hbe640: u8,
    /// Horizontal blanking start for 640-pixel mode.
    pub hbs640: u16,
}

// ──────────────────────────────────────────────────────────────────────────────
// NTSC 480i — the primary GameCube mode (North America / Japan)
// ──────────────────────────────────────────────────────────────────────────────
//
// 525 total lines (262.5 per field × 2), 59.94 Hz
// Active: 480 lines (240 per field)

/// Timing for NTSC 480i (interlaced, 60 Hz).
///
/// Sourced from libogc2 `video_timing[0]` and verified against YAGCD §10.
pub const NTSC_480I: Timing = Timing {
    equ:      0x06,
    acv:      0x00F0,   // 240 active lines per field
    prb_odd:  0x0018,
    prb_even: 0x0019,
    psb_odd:  0x0003,
    psb_even: 0x0002,
    bs1: 0x0C, bs2: 0x0D, bs3: 0x0C, bs4: 0x0D,
    be1: 0x0208, be2: 0x0207, be3: 0x0208, be4: 0x0207,
    nhlines: 0x020D,    // 525 half-lines per frame
    hlw:     0x01AD,    // half-line width
    hsy:  0x40,
    hcs:  0x47,
    hce:  0x69,
    hbe640: 0xA2,
    hbs640: 0x0175,
};

// ──────────────────────────────────────────────────────────────────────────────
// NTSC 240p (non-interlaced, single-field)
// ──────────────────────────────────────────────────────────────────────────────

/// Timing for NTSC 240p (double-strike / non-interlaced, 60 Hz).
pub const NTSC_240P: Timing = Timing {
    equ:      0x06,
    acv:      0x00F0,
    prb_odd:  0x0018,
    prb_even: 0x0018,
    psb_odd:  0x0004,
    psb_even: 0x0004,
    bs1: 0x0C, bs2: 0x0C, bs3: 0x0C, bs4: 0x0C,
    be1: 0x0208, be2: 0x0208, be3: 0x0208, be4: 0x0208,
    nhlines: 0x020E,
    hlw:     0x01AD,
    hsy:  0x40,
    hcs:  0x47,
    hce:  0x69,
    hbe640: 0xA2,
    hbs640: 0x0175,
};

// ──────────────────────────────────────────────────────────────────────────────
// PAL 576i — European standard
// ──────────────────────────────────────────────────────────────────────────────

/// Timing for PAL 576i (interlaced, 50 Hz).
pub const PAL_576I: Timing = Timing {
    equ:      0x05,
    acv:      0x0120,   // 288 active lines per field
    prb_odd:  0x0021,
    prb_even: 0x0022,
    psb_odd:  0x0001,
    psb_even: 0x0000,
    bs1: 0x0D, bs2: 0x0C, bs3: 0x0B, bs4: 0x0A,
    be1: 0x026B, be2: 0x026A, be3: 0x0269, be4: 0x026C,
    nhlines: 0x0271,
    hlw:     0x01B0,
    hsy:  0x40,
    hcs:  0x4B,
    hce:  0x6A,
    hbe640: 0xAC,
    hbs640: 0x017C,
};

// ──────────────────────────────────────────────────────────────────────────────
// NTSC 480p — Progressive scan (GameCube component cable)
// ──────────────────────────────────────────────────────────────────────────────

/// Timing for NTSC 480p (progressive, 60 Hz).
pub const NTSC_480P: Timing = Timing {
    equ:      0x0C,
    acv:      0x01E0,   // 480 active lines (full progressive frame)
    prb_odd:  0x0030,
    prb_even: 0x0030,
    psb_odd:  0x0006,
    psb_even: 0x0006,
    bs1: 0x18, bs2: 0x18, bs3: 0x18, bs4: 0x18,
    be1: 0x040E, be2: 0x040E, be3: 0x040E, be4: 0x040E,
    nhlines: 0x041A,
    hlw:     0x01AD,
    hsy:  0x40,
    hcs:  0x47,
    hce:  0x69,
    hbe640: 0xA2,
    hbs640: 0x0175,
};
