//! Serial Interface (SI) — GameCube controller driver.
//!
//! ## Hardware Overview
//!
//! The SI bus connects the four front controller ports. Registers are 32-bit,
//! mapped at `crate::mmio::addr(0x006400)`. Layout:
//!
//! ```text
//! Offset  Name        Description
//! 0x00    C0_OUT0     Channel 0 command word 0 (and cached input[0])
//! 0x04    C0_IN0      Channel 0 input word 0 (response bytes 0-3)
//! 0x08    C0_IN1      Channel 0 input word 1 (response bytes 4-7)
//! 0x0C    C1_OUT0     Channel 1 …  (each channel is 3 words × 4 bytes)
//! 0x18    C2_OUT0     Channel 2 …
//! 0x24    C3_OUT0     Channel 3 …
//! 0x30    SIPOLL      Polling control
//! 0x34    SICOMCSR    Communication control & status
//! 0x38    SISR        Status register (per-channel error flags + RDST bits)
//! 0x3C    SIEXILK     EXI lock
//! 0x80…  TXBUF[0..8] Shared transfer buffer (output then input, 8 words each)
//! ```
//!
//! ## Read Protocol
//!
//! This driver uses **synchronous immediate-mode transfers** via SICOMCSR
//! (no interrupts required):
//!
//! 1. Write the 3-byte read command to `TXBUF[0]` (big-endian).
//! 2. Program SICOMCSR: select channel, outlen=3, inlen=8, tstart=1.
//! 3. Busy-poll SICOMCSR until `TSTART` clears.
//! 4. Check SISR for errors (NORESPONSE = controller not plugged in).
//! 5. Read 8 bytes of response from `TXBUF[0]` and `TXBUF[1]`.
//!
//! ## GC Pad Response Format (SPEC5 / standard)
//!
//! ```text
//! Word 0 (bytes 0–3):
//!   bits 29:16  buttons (14 bits, directly map to Buttons bitfield)
//!   bits 15:8   stick X (u8, center ≈ 128)
//!   bits  7:0   stick Y (u8, center ≈ 128)
//!
//! Word 1 (bytes 4–7):
//!   bits 31:24  substick X (u8, center ≈ 128)
//!   bits 23:16  substick Y (u8, center ≈ 128)
//!   bits 15:8   trigger L  (u8, 0=released, 255=full press)
//!   bits  7:0   trigger R  (u8)
//! ```

#![allow(dead_code)]

pub const SI_BASE: usize = crate::mmio::addr(0x006400);

// Register word indices (_siReg as *mut u32)
const REG_C_OUT: [usize; 4] = [0, 3, 6, 9];   // channel N command word 0
const REG_C_IN0: [usize; 4] = [1, 4, 7, 10];  // channel N input word 0
const REG_C_IN1: [usize; 4] = [2, 5, 8, 11];  // channel N input word 1
const REG_SIPOLL:   usize = 12;
const REG_SICOMCSR: usize = 13;
const REG_SISR:     usize = 14;
const REG_TXBUF:    usize = 32;  // transfer buffer starts at _siReg[32]

// SICOMCSR bits
const SICOMCSR_TCINT:     u32 = 1 << 31;
const SICOMCSR_COMERR:    u32 = 1 << 29;
const SICOMCSR_TSTART:    u32 = 1 << 0;

// SISR per-channel error flags (each channel has 8 bits, channel 0 = bits 31:24)
const SISR_NORESPONSE: u32 = 0x08;

// GC pad command: 0x40 = read, 0x03 = mode, 0x00 = no rumble
// In big-endian u32: 0x40030000 (only 3 bytes sent)
const CMD_READ: u32 = 0x4003_0000;

#[inline(always)]
fn reg(idx: usize) -> *mut u32 {
    (SI_BASE + idx * 4) as *mut u32
}

// ─── Public types ─────────────────────────────────────────────────────────────

/// Controller port index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Port { P1 = 0, P2 = 1, P3 = 2, P4 = 3 }

/// Button bitmask constants — match `_SHIFTR(data[0], 16, 14)` directly.
#[allow(non_upper_case_globals)]
pub mod Buttons {
    pub const DLeft:   u16 = 0x0001;
    pub const DRight:  u16 = 0x0002;
    pub const DDown:   u16 = 0x0004;
    pub const DUp:     u16 = 0x0008;
    pub const Z:       u16 = 0x0010;
    pub const R:       u16 = 0x0020;  // digital R
    pub const L:       u16 = 0x0040;  // digital L
    pub const A:       u16 = 0x0100;
    pub const B:       u16 = 0x0200;
    pub const X:       u16 = 0x0400;
    pub const Y:       u16 = 0x0800;
    pub const Start:   u16 = 0x1000;
}

/// Full state of one GameCube controller.
#[derive(Debug, Clone, Copy, Default)]
pub struct PadState {
    /// Button bitmask — AND with [`Buttons`] constants.
    pub buttons:   u16,
    /// Main analog stick X (0–255, center ≈ 128).
    pub stick_x:   u8,
    /// Main analog stick Y (0–255, center ≈ 128).
    pub stick_y:   u8,
    /// C-stick X (0–255, center ≈ 128).
    pub cstick_x:  u8,
    /// C-stick Y (0–255, center ≈ 128).
    pub cstick_y:  u8,
    /// Left analog trigger (0–255).
    pub trigger_l: u8,
    /// Right analog trigger (0–255).
    pub trigger_r: u8,
}

impl PadState {
    /// Return true if `button` is pressed.
    #[inline] pub fn pressed(&self, button: u16) -> bool { self.buttons & button != 0 }

    /// Main stick X centered on zero (−128..127).
    #[inline] pub fn stick_x_centered(&self) -> i8 {
        self.stick_x.wrapping_sub(128) as i8
    }
    /// Main stick Y centered on zero.
    #[inline] pub fn stick_y_centered(&self) -> i8 {
        self.stick_y.wrapping_sub(128) as i8
    }
    /// C-stick X centered on zero.
    #[inline] pub fn cstick_x_centered(&self) -> i8 {
        self.cstick_x.wrapping_sub(128) as i8
    }
    /// C-stick Y centered on zero.
    #[inline] pub fn cstick_y_centered(&self) -> i8 {
        self.cstick_y.wrapping_sub(128) as i8
    }
}

/// Result of a pad read operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadResult {
    /// Controller read successfully.
    Ok(PadState),
    /// No controller plugged into this port.
    NoController,
    /// Communication error (collision, overrun, etc.).
    Error,
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Read the current state of one GameCube controller, blocking until complete.
///
/// This is a synchronous, interrupt-free poll. The SI transfer takes a few
/// microseconds. For vsync-aligned input, use the interrupt-driven path
/// (Milestone 2b) instead.
///
/// # Safety
/// Must be called after the SI hardware is initialised (it initialises
/// itself on first access via the boot IPL, so this is safe to call from
/// `main`).
pub unsafe fn read_pad(port: Port) -> PadResult {
    let chan = port as u32;

    // Write 3-byte command to transfer buffer
    core::ptr::write_volatile(reg(REG_TXBUF), CMD_READ);

    // Program SICOMCSR:
    //   bit 31:    TCINT = 1 (clear any pending TC interrupt)
    //   bits 22:16 outlen = 3 (bytes to send)
    //   bits 14:8  inlen  = 8 (bytes to receive)
    //   bits 2:1   channel select
    //   bit 0:     TSTART = 1 (begin transfer)
    let csr: u32 = SICOMCSR_TCINT
        | (3u32 << 16)       // outlen = 3
        | (8u32 << 8)        // inlen  = 8
        | (chan << 1)         // channel
        | SICOMCSR_TSTART;   // start
    core::ptr::write_volatile(reg(REG_SICOMCSR), csr);

    // Busy-wait for transfer to complete
    let mut timeout = 100_000u32;
    while core::ptr::read_volatile(reg(REG_SICOMCSR)) & SICOMCSR_TSTART != 0 {
        timeout -= 1;
        if timeout == 0 { return PadResult::Error; }
    }

    // Check for communication error
    if core::ptr::read_volatile(reg(REG_SICOMCSR)) & SICOMCSR_COMERR != 0 {
        // Check NORESPONSE flag for this channel in SISR
        let sisr = core::ptr::read_volatile(reg(REG_SISR));
        let ch_flags = (sisr >> ((3 - chan) * 8)) as u8;
        if ch_flags & (SISR_NORESPONSE as u8) != 0 {
            return PadResult::NoController;
        }
        return PadResult::Error;
    }

    // Read 8-byte response from the transfer buffer
    let word0 = core::ptr::read_volatile(reg(REG_TXBUF));
    let word1 = core::ptr::read_volatile(reg(REG_TXBUF + 1));

    PadResult::Ok(decode(word0, word1))
}

/// Poll all four controller ports and return an array of results.
pub unsafe fn read_all() -> [PadResult; 4] {
    [
        read_pad(Port::P1),
        read_pad(Port::P2),
        read_pad(Port::P3),
        read_pad(Port::P4),
    ]
}

// ─── Decoding ─────────────────────────────────────────────────────────────────

fn decode(word0: u32, word1: u32) -> PadState {
    // SPEC5 layout (libogc2 reference):
    //   buttons = (word0 >> 16) & 0x3FFF
    //   stickX  = (word0 >>  8) & 0xFF
    //   stickY  = (word0      ) & 0xFF
    //   cstickX = (word1 >> 24) & 0xFF
    //   cstickY = (word1 >> 16) & 0xFF
    //   triggerL= (word1 >>  8) & 0xFF
    //   triggerR= (word1      ) & 0xFF
    PadState {
        buttons:   ((word0 >> 16) & 0x3FFF) as u16,
        stick_x:   ((word0 >>  8) & 0xFF) as u8,
        stick_y:   ((word0      ) & 0xFF) as u8,
        cstick_x:  ((word1 >> 24) & 0xFF) as u8,
        cstick_y:  ((word1 >> 16) & 0xFF) as u8,
        trigger_l: ((word1 >>  8) & 0xFF) as u8,
        trigger_r: ((word1      ) & 0xFF) as u8,
    }
}
