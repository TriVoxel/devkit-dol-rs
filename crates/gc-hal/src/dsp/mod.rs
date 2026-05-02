//! DSP (Digital Signal Processor) control.
//!
//! The Gekko's DSP is a 16-bit fixed-point coprocessor clocked at 101.25 MHz
//! on the GameCube. It handles audio decoding (ADPCM, PCM mixing) and
//! communicates with the CPU via a pair of 32-bit mailbox registers.
//!
//! ## Register map (16-bit, base 0xCC005000)
//!
//! | Index | Name       | Description                                    |
//! |-------|------------|------------------------------------------------|
//! | 0     | CPU→DSP Hi | High 16 bits of CPU→DSP mailbox                |
//! | 1     | CPU→DSP Lo | Low 16 bits; write triggers interrupt on DSP   |
//! | 2     | DSP→CPU Hi | High 16 bits; bit 15 = mail waiting            |
//! | 3     | DSP→CPU Lo | Low 16 bits; read clears the "mail waiting" flag|
//! | 5     | DSPCR      | Control/status register                        |
//! | 24/25 | AIDMAADDR  | AI DMA address (shared with gc-hal::ai)        |
//! | 27    | AIDMALEN   | AI DMA length + enable                         |
//! | 29    | AIDMALEFT  | AI DMA bytes remaining                         |
//!
//! ## DSPCR bit layout (register index 5)
//!
//! | Bit  | Name       | Description                            |
//! |------|------------|----------------------------------------|
//! | 0    | RES        | Reset DSP (self-clearing)              |
//! | 1    | PIINT      | Assert PI interrupt from DSP           |
//! | 2    | HALT       | Halt DSP execution                     |
//! | 3    | AIINT      | AI DMA interrupt pending (W1C)         |
//! | 4    | AIINTMSK   | AI DMA interrupt enable                |
//! | 5    | ARINT      | ARAM DMA interrupt pending (W1C)       |
//! | 6    | ARINTMSK   | ARAM DMA interrupt enable              |
//! | 7    | DSPINT     | DSP→CPU interrupt pending (W1C)        |
//! | 8    | DSPINTMSK  | DSP→CPU interrupt enable               |
//! | 9    | DSPDMA     | ARAM DMA in progress (read-only)       |
//! | 11   | DSPRESET   | Reset DSP (hold high while running)    |

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, Ordering};

const DSP_BASE: usize = 0xCC005000;

// DSPCR bits
pub const DSPCR_RES:      u16 = 0x0001;
pub const DSPCR_PIINT:    u16 = 0x0002;
pub const DSPCR_HALT:     u16 = 0x0004;
pub const DSPCR_AIINT:    u16 = 0x0008;
pub const DSPCR_AIINTMSK: u16 = 0x0010;
pub const DSPCR_ARINT:    u16 = 0x0020;
pub const DSPCR_ARINTMSK: u16 = 0x0040;
pub const DSPCR_DSPINT:   u16 = 0x0080;
pub const DSPCR_DSPINTMSK:u16 = 0x0100;
pub const DSPCR_DSPDMA:   u16 = 0x0200;
pub const DSPCR_DSPRESET: u16 = 0x0800;

/// Interrupt bits that are write-1-to-clear — must not be written inadvertently.
const DSPCR_W1C: u16 = DSPCR_DSPINT | DSPCR_ARINT | DSPCR_AIINT;

static INITED: AtomicBool = AtomicBool::new(false);

#[inline(always)]
fn dsp(idx: usize) -> *mut u16 { (DSP_BASE + idx * 2) as *mut u16 }

/// Read DSPCR safely — preserve W1C bits so we don't accidentally clear them.
#[inline(always)]
unsafe fn read_dspcr() -> u16 {
    core::ptr::read_volatile(dsp(5))
}

/// Write DSPCR, preserving W1C bits (do not accidentally clear pending interrupts).
#[inline(always)]
unsafe fn write_dspcr(val: u16) {
    // Mask out the W1C bits from the value being written unless we explicitly
    // want to clear them. Caller uses clear_dspcr_int() to clear specific flags.
    core::ptr::write_volatile(dsp(5), val & !DSPCR_W1C);
}

/// Clear specific W1C interrupt bits in DSPCR.
#[inline(always)]
unsafe fn clear_dspcr_int(bits: u16) {
    let cur = read_dspcr();
    // Write the current value back with the target bits set (they will clear)
    // but don't inadvertently set other W1C bits we're not trying to clear.
    core::ptr::write_volatile(dsp(5), (cur & !DSPCR_W1C) | (bits & DSPCR_W1C));
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Initialise the DSP subsystem.
///
/// Resets the DSP and brings it out of reset in a halted state, ready for
/// a task to be loaded via the mailbox. Enables the DSP interrupt so the
/// CPU can receive notifications from DSP programs.
///
/// # Safety
/// Call once during startup, after `gc_rt::exception::init()`.
pub unsafe fn init() {
    if INITED.swap(true, Ordering::SeqCst) { return; }

    // Assert reset + clear W1C bits (reset sequence from libogc2)
    let cur = read_dspcr();
    core::ptr::write_volatile(dsp(5),
        (cur & !(DSPCR_W1C | DSPCR_HALT)) | DSPCR_DSPRESET);
    // Release reset, leave halted
    let cur = read_dspcr();
    core::ptr::write_volatile(dsp(5), cur & !(DSPCR_HALT | DSPCR_W1C));

    // Enable DSP interrupt in DSPCR
    let cur = read_dspcr();
    core::ptr::write_volatile(dsp(5), (cur & !DSPCR_W1C) | DSPCR_DSPINTMSK);
}

/// Halt the DSP (pause execution without resetting state).
pub unsafe fn halt() {
    let cur = read_dspcr();
    core::ptr::write_volatile(dsp(5), (cur & !DSPCR_W1C) | DSPCR_HALT);
}

/// Resume DSP execution after a halt.
pub unsafe fn unhalt() {
    let cur = read_dspcr();
    core::ptr::write_volatile(dsp(5), cur & !(DSPCR_HALT | DSPCR_W1C));
}

/// Assert a PI interrupt from the DSP side (rarely needed from CPU).
pub unsafe fn assert_interrupt() {
    let cur = read_dspcr();
    core::ptr::write_volatile(dsp(5), (cur & !DSPCR_W1C) | DSPCR_PIINT);
}

/// Return true if a DSP→CPU mail message is waiting.
#[inline]
pub unsafe fn has_mail_from_dsp() -> bool {
    core::ptr::read_volatile(dsp(2)) & 0x8000 != 0
}

/// Block until DSP→CPU mail is available, then read and return it.
pub unsafe fn read_mail_from_dsp() -> u32 {
    while !has_mail_from_dsp() {}
    let hi = core::ptr::read_volatile(dsp(2)) as u32;
    let lo = core::ptr::read_volatile(dsp(3)) as u32;
    (hi << 16) | lo
}

/// Return true if the CPU→DSP mailbox is busy (previous mail not yet read).
#[inline]
pub unsafe fn mail_to_dsp_busy() -> bool {
    core::ptr::read_volatile(dsp(0)) & 0x8000 != 0
}

/// Wait until the CPU→DSP mailbox is free, then send `mail`.
pub unsafe fn send_mail_to_dsp(mail: u32) {
    while mail_to_dsp_busy() {}
    core::ptr::write_volatile(dsp(0), (mail >> 16) as u16);
    core::ptr::write_volatile(dsp(1), (mail & 0xFFFF) as u16);
}

/// Return true if an ARAM DMA transfer is in progress.
#[inline]
pub unsafe fn aram_dma_busy() -> bool {
    read_dspcr() & DSPCR_DSPDMA != 0
}

/// Called from the DSP interrupt handler.
///
/// Clears the DSP interrupt flag and invokes the registered callback.
/// Not part of the public API — called by `gc_rt::exception`.
#[no_mangle]
pub unsafe extern "C" fn __dsp_int_handler() {
    clear_dspcr_int(DSPCR_DSPINT);
    if let Some(cb) = DSP_CALLBACK {
        cb();
    }
}

/// Optional user callback for DSP interrupts.
static mut DSP_CALLBACK: Option<fn()> = None;

/// Register a callback to be invoked when the DSP sends an interrupt.
pub unsafe fn register_callback(cb: fn()) {
    let _guard = gc_rt::irq::disable();
    DSP_CALLBACK = Some(cb);
}
