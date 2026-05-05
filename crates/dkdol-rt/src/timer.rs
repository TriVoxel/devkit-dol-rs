//! Decrementer-based system timer.
//!
//! The Gekko's Decrementer (DEC, SPR 22) counts down from a loaded value at
//! the bus clock / 4 rate. When it crosses zero, exception vector `0x0900`
//! fires (if MSR[EE] is set).
//!
//! ## Timebase frequency
//!
//! | Platform | CPU      | Bus       | TBR / DEC tick rate |
//! |----------|----------|-----------|---------------------|
//! | GameCube | 486 MHz  | 162 MHz   | 40.5 MHz            |
//! | Wii      | 729 MHz  | 243 MHz   | 60.75 MHz           |
//!
//! A 60 Hz decrementer interrupt on GameCube: `40_500_000 / 60 = 675_000` ticks.
//!
//! ## Usage
//!
//! Call [`init`] once before enabling interrupts, then register a decrementer
//! handler via [`crate::exception::register`] if you need a callback.
//!
//! The global tick counter is always available via [`ticks`] and [`millis`].

use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};

/// Decrementer reload value for 60 Hz on GameCube.
pub const DEC_60HZ_GC: u32 = 675_000;
/// Decrementer reload value for 50 Hz (PAL) on GameCube.
pub const DEC_50HZ_GC: u32 = 810_000;

/// Current decrementer reload value. Set by [`init`].
static DEC_RELOAD: AtomicU32 = AtomicU32::new(DEC_60HZ_GC);

/// Global tick counter. Incremented by the decrementer handler.
/// At 60 Hz this overflows after ~828 days of uptime.
static TICK_COUNT: AtomicU32 = AtomicU32::new(0);

/// Initialise the decrementer timer.
///
/// Loads the DEC register with `reload_value` and stores it for future
/// reloads by the exception handler. Interrupts must be enabled (MSR[EE]=1)
/// for the decrementer exception to fire.
///
/// # Safety
///
/// Should be called once during startup, before enabling interrupts.
/// The exception handler for the Decrementer vector (0x0900) must be
/// installed via [`crate::exception::init`] before interrupts are enabled.
pub unsafe fn init(reload_value: u32) {
    DEC_RELOAD.store(reload_value, Ordering::Relaxed);
    TICK_COUNT.store(0, Ordering::Relaxed);
    set_dec(reload_value);
}

/// Called from the decrementer exception handler.
///
/// Reloads DEC and increments the tick counter. This is an internal function
/// called from the exception dispatch table — do not call it directly.
#[no_mangle]
pub unsafe extern "C" fn __timer_dec_handler() {
    set_dec(DEC_RELOAD.load(Ordering::Relaxed));
    TICK_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Return the current tick count.
///
/// Each tick is one decrementer interrupt period (e.g. 1/60 s at 60 Hz).
#[inline]
pub fn ticks() -> u32 {
    TICK_COUNT.load(Ordering::Relaxed)
}

/// Return approximate elapsed time in milliseconds since [`init`] was called.
///
/// Accuracy depends on the reload value. At 60 Hz each tick ≈ 16.67 ms.
pub fn millis() -> u32 {
    let t = TICK_COUNT.load(Ordering::Relaxed);
    let reload = DEC_RELOAD.load(Ordering::Relaxed);
    // millis = ticks * (reload / tick_hz * 1000)
    // tick_hz ≈ 40_500_000
    // simplified: millis = ticks * reload * 1000 / 40_500_000
    // Use 64-bit intermediate to avoid overflow.
    let numer = (t as u64) * (reload as u64) * 1000;
    (numer / 40_500_000) as u32
}

/// Read the raw Time Base Register (lower 32 bits).
///
/// The TBR ticks at bus_clock/4 (40.5 MHz on GC). Use this for fine-grained
/// profiling. Wraps approximately every 106 seconds.
#[inline]
pub fn tbr() -> u32 {
    let val: u32;
    unsafe {
        asm!("mftb {v}", v = out(reg) val, options(nostack, nomem));
    }
    val
}

/// Read the full 64-bit Time Base Register.
///
/// Reads TBU (upper) twice, bracketing a TBL read, to handle wraparound.
pub fn tbr64() -> u64 {
    loop {
        let hi0: u32;
        let lo:  u32;
        let hi1: u32;
        unsafe {
            asm!(
                "mftbu {h0}",
                "mftb  {l}",
                "mftbu {h1}",
                h0 = out(reg) hi0,
                l  = out(reg) lo,
                h1 = out(reg) hi1,
                options(nostack, nomem)
            );
        }
        if hi0 == hi1 {
            return ((hi0 as u64) << 32) | (lo as u64);
        }
        // TBU wrapped between reads — retry.
    }
}

/// Busy-wait for approximately `ms` milliseconds.
///
/// Uses the TBR, so it works without interrupts. Not suitable for precise
/// timing — use the decrementer interrupt for that.
pub fn delay_ms(ms: u32) {
    // 40_500_000 ticks/sec → 40_500 ticks/ms
    let ticks_needed = (ms as u64) * 40_500;
    let start = tbr64();
    while tbr64().wrapping_sub(start) < ticks_needed {}
}

/// Busy-wait for approximately `us` microseconds.
pub fn delay_us(us: u32) {
    let ticks_needed = (us as u64) * 41; // 40.5 rounded
    let start = tbr64();
    while tbr64().wrapping_sub(start) < ticks_needed {}
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn set_dec(val: u32) {
    asm!("mtdec {v}", v = in(reg) val, options(nostack, nomem));
}
