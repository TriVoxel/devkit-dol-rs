//! Audio Interface (AI) — PCM streaming DMA driver.
//!
//! The AI feeds 16-bit signed stereo PCM samples directly from MEM1 to the
//! DAC via DMA. The DMA engine lives in the DSP register block (crate::mmio::addr(0x005000)),
//! not in the AI register block (crate::mmio::addr(0x006C00)); the AI block only controls
//! sample rate, volume, and streaming play state.
//!
//! ## Two audio paths
//!
//! **AI DMA** (this module's primary focus): CPU programs a MEM1 buffer
//! into DSP registers 24–29. Hardware plays it straight to the DAC.
//! When the buffer drains, it fires `IRQ_DSP_AI` (PI interrupt bit 5,
//! DSPCR bit 3). The driver reloads the next buffer in the callback.
//!
//! **AI Streaming**: plays a separate I2S stream from EXI/DVD. Less
//! commonly used for homebrew; stub only.
//!
//! ## DMA format
//!
//! - Interleaved L/R pairs of `i16` (big-endian)
//! - Buffer address must be 32-byte aligned (physical address)
//! - Buffer length must be a multiple of 32 bytes
//! - Sample rate: 32 kHz (default) or 48 kHz
//!
//! ## Usage
//!
//! ```rust,no_run
//! use gc_hal::ai::{self, SampleRate};
//!
//! static mut BUF_A: [i16; 1024] = [0; 1024]; // 512 stereo frames
//! static mut BUF_B: [i16; 1024] = [0; 1024];
//!
//! unsafe {
//!     ai::init();
//!     ai::set_dsp_sample_rate(SampleRate::Hz32000);
//!     ai::set_volume(255, 255); // max volume
//!     fill_sine(&mut BUF_A);
//!     fill_sine(&mut BUF_B);
//!     ai::register_dma_callback(my_callback);
//!     ai::start_dma(BUF_A.as_ptr(), BUF_A.len() * 2); // bytes
//! }
//! ```

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, Ordering};

// ── Register bases ─────────────────────────────────────────────────────────

const AI_BASE:  usize = crate::mmio::addr(0x006C00); // AI control registers (32-bit)
const DSP_BASE: usize = crate::mmio::addr(0x005000); // DSP registers (16-bit), shared with ARAM

// AI register indices (u32)
const AI_CONTROL:   usize = 0;
const AI_STREAM_VOL:usize = 1;
const AI_SAMPLE_CNT:usize = 2;
const AI_INT_TIMING:usize = 3;

// AI control bits
const AI_PSTAT:    u32 = 0x01; // streaming play state
const AI_AISFR:    u32 = 0x02; // stream freq (0=32kHz, 1=48kHz)
const AI_AIINTMSK: u32 = 0x04; // stream interrupt mask
const AI_AIINT:    u32 = 0x08; // stream interrupt status (W1C)
const AI_AIINTVLD: u32 = 0x10;
const AI_SCRESET:  u32 = 0x20; // sample counter reset
const AI_DMAFR:    u32 = 0x40; // DMA freq (0=48kHz, 1=32kHz)

// DSP control register bits (dspReg[5])
const DSPCR_AIINTMSK: u16 = 0x0010;
const DSPCR_AIINT:    u16 = 0x0008;

// Interrupt flag bits we must not accidentally clear
const DSPCR_PRESERVE: u16 = !(0x0080 | 0x0020 | 0x0008); // don't clear DSPINT, ARINT, AIINT

#[inline(always)]
fn ai(idx: usize) -> *mut u32 { (AI_BASE + idx * 4) as *mut u32 }
#[inline(always)]
fn dsp(idx: usize) -> *mut u16 { (DSP_BASE + idx * 2) as *mut u16 }

// ── Sample rate ────────────────────────────────────────────────────────────

/// Audio DMA sample rate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SampleRate {
    /// 32 kHz (standard GC audio rate, matches DVD audio)
    Hz32000 = 0,
    /// 48 kHz
    Hz48000 = 1,
}

// ── DMA callback ───────────────────────────────────────────────────────────

/// Called from the AI DMA interrupt when the current buffer finishes.
///
/// The callback must call [`start_dma`] to queue the next buffer,
/// otherwise audio output will stop.
pub type DmaCallback = fn();

static mut DMA_CALLBACK: Option<DmaCallback> = None;
static INITED: AtomicBool = AtomicBool::new(false);

// ── Public API ─────────────────────────────────────────────────────────────

/// Initialise the Audio Interface.
///
/// Resets the sample counter, mutes the stream output, sets the DSP
/// sample rate to 32 kHz, and enables the AI DMA interrupt.
///
/// # Safety
/// Must be called once during startup, after `gc_rt::exception::init()`.
pub unsafe fn init() {
    if INITED.swap(true, Ordering::SeqCst) { return; }

    // Reset sample counter, mute streaming, clear stream interrupt
    let ctrl = core::ptr::read_volatile(ai(AI_CONTROL));
    core::ptr::write_volatile(ai(AI_CONTROL),
        (ctrl & !(AI_AIINTVLD | AI_AIINTMSK | AI_PSTAT)) | AI_AIINT);

    // Mute stream volume
    core::ptr::write_volatile(ai(AI_STREAM_VOL), 0);
    core::ptr::write_volatile(ai(AI_INT_TIMING), 0);

    // Reset sample counter
    core::ptr::write_volatile(ai(AI_CONTROL),
        core::ptr::read_volatile(ai(AI_CONTROL)) | AI_SCRESET);

    // Default: DSP at 32 kHz
    set_dsp_sample_rate(SampleRate::Hz32000);

    // Enable AI DMA interrupt in DSPCR
    let dspcr = core::ptr::read_volatile(dsp(5));
    core::ptr::write_volatile(dsp(5), (dspcr & 0xF0C7) | DSPCR_AIINTMSK);
}

/// Set the DSP (AI DMA) sample rate.
///
/// Changes take effect on the next [`start_dma`] call.
pub unsafe fn set_dsp_sample_rate(rate: SampleRate) {
    let ctrl = core::ptr::read_volatile(ai(AI_CONTROL));
    match rate {
        SampleRate::Hz32000 => {
            // AI_DMAFR = 1 for 32 kHz
            core::ptr::write_volatile(ai(AI_CONTROL), ctrl | AI_DMAFR);
        }
        SampleRate::Hz48000 => {
            core::ptr::write_volatile(ai(AI_CONTROL), ctrl & !AI_DMAFR);
        }
    }
}

/// Set streaming output volume (0 = mute, 255 = max).
pub unsafe fn set_volume(left: u8, right: u8) {
    core::ptr::write_volatile(ai(AI_STREAM_VOL),
        (left as u32) | ((right as u32) << 8));
}

/// Register the DMA completion callback.
///
/// The callback fires from the AI DMA interrupt handler when the current
/// DMA buffer has played through. It must call [`start_dma`] to continue
/// audio output.
pub unsafe fn register_dma_callback(cb: DmaCallback) {
    let _guard = gc_rt::irq::disable();
    DMA_CALLBACK = Some(cb);
}

/// Start an AI DMA transfer.
///
/// `buf` must point to 16-bit stereo PCM samples in MEM1, 32-byte aligned.
/// `len_bytes` must be a multiple of 32.
///
/// The DMA plays `buf` and fires the DMA callback when done.
///
/// # Safety
/// - `buf` must remain valid and unmodified for the entire transfer.
/// - Must be 32-byte aligned.
/// - `len_bytes` must be a multiple of 32.
pub unsafe fn start_dma(buf: *const i16, len_bytes: usize) {
    debug_assert!(buf as usize % 32 == 0, "AI DMA buffer not 32-byte aligned");
    debug_assert!(len_bytes % 32 == 0, "AI DMA length not multiple of 32");

    // Physical address (strip cached virtual offset)
    let phys = (buf as usize) & 0x1FFF_FFFF;

    let _guard = gc_rt::irq::disable();

    // Program DMA address: dspReg[24] = high 13 bits (>>16), dspReg[25] = low 16 bits
    let hi = ((phys >> 16) & 0x1FFF) as u16;
    let lo = (phys & 0xFFE0) as u16; // already 32-byte aligned, keep bits 15:5
    let cur24 = core::ptr::read_volatile(dsp(24));
    let cur25 = core::ptr::read_volatile(dsp(25));
    core::ptr::write_volatile(dsp(24), (cur24 & !0x1FFF) | hi);
    core::ptr::write_volatile(dsp(25), (cur25 & !0xFFE0) | lo);

    // DMA length in 32-byte blocks: dspReg[27] bits 14:0 = len>>5
    let blocks = (len_bytes >> 5) as u16;
    let cur27 = core::ptr::read_volatile(dsp(27));
    // Write length (bits 14:0) and set enable bit (bit 15)
    core::ptr::write_volatile(dsp(27), (cur27 & !0x7FFF) | blocks | 0x8000);
}

/// Stop the AI DMA immediately.
pub unsafe fn stop_dma() {
    let cur27 = core::ptr::read_volatile(dsp(27));
    core::ptr::write_volatile(dsp(27), cur27 & !0x8000);
}

/// Return the number of bytes remaining in the current DMA transfer.
pub unsafe fn dma_bytes_left() -> usize {
    let remaining_blocks = core::ptr::read_volatile(dsp(29)) & 0x7FFF;
    (remaining_blocks as usize) << 5
}

/// Called from the DSP/AI interrupt handler in gc-rt.
///
/// Clears the AI interrupt flag in DSPCR and invokes the user callback.
/// This is not part of the public API — it is called by the exception system.
#[no_mangle]
pub unsafe extern "C" fn __ai_dma_handler() {
    // Clear AI interrupt in DSPCR (bit 3), preserving DSP and ARAM interrupt bits
    let dspcr = core::ptr::read_volatile(dsp(5));
    core::ptr::write_volatile(dsp(5), (dspcr & 0xF0C7) | DSPCR_AIINT);

    if let Some(cb) = DMA_CALLBACK {
        cb();
    }
}
