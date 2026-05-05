//! Interrupt enable/disable and critical section helpers.
//!
//! The Gekko is single-core, so thread safety reduces to interrupt safety.
//! Critical sections work by saving MSR[EE] (external enable), clearing it,
//! running the protected code, then restoring MSR[EE].
//!
//! # Usage
//!
//! ```rust,no_run
//! use dkdol_rt::irq;
//!
//! let result = irq::free(|| {
//!     // interrupts disabled here
//!     42
//! });
//! ```

use core::arch::asm;

/// Saved interrupt state — returned by [`disable`], passed to [`restore`].
#[derive(Clone, Copy)]
pub struct IrqState(u32);

/// Disable external interrupts (MSR[EE] = 0).
///
/// Returns the previous MSR value. Pass it to [`restore`] to re-enable if
/// interrupts were previously enabled.
///
/// Prefer [`free`] for scoped critical sections.
#[inline(always)]
pub fn disable() -> IrqState {
    let msr: u32;
    let _new_msr: u32;
    unsafe {
        asm!(
            "mfmsr {msr}",
            "rlwinm {new}, {msr}, 0, 17, 15",   // clear bit 16 (EE)
            "mtmsr {new}",
            msr = out(reg) msr,
            new = out(reg) _new_msr,
            options(nostack, preserves_flags)
        );
    }
    IrqState(msr)
}

/// Restore external interrupt state from a previously saved [`IrqState`].
///
/// # Safety
///
/// `state` must come from a prior call to [`disable`] in the same execution
/// context. Using a stale or arbitrary state is undefined behaviour.
#[inline(always)]
pub unsafe fn restore(state: IrqState) {
    asm!(
        "mtmsr {msr}",
        msr = in(reg) state.0,
        options(nostack, preserves_flags)
    );
}

/// Enable external interrupts unconditionally (MSR[EE] = 1).
///
/// # Safety
///
/// Only safe if interrupts have been properly initialised and exception
/// vectors are installed. Calling this before [`crate::exception::init`]
/// will cause undefined behaviour if any interrupt fires.
#[inline(always)]
pub unsafe fn enable() {
    // Use a constraint-allocated register rather than hard-coding r3,
    // and use the r-prefix form for LLVM's PowerPC assembler.
    let mut msr: u32;
    asm!(
        "mfmsr {msr}",
        "ori   {msr}, {msr}, 0x8000",
        "mtmsr {msr}",
        msr = out(reg) msr,
        options(nostack, preserves_flags)
    );
    let _ = msr;
}

/// Execute `f` with external interrupts disabled, then restore the previous
/// interrupt state.
///
/// This is the idiomatic way to create a critical section:
///
/// ```rust,no_run
/// dkdol_rt::irq::free(|| {
///     // only one "thread" (interrupt level) can be here at a time
/// });
/// ```
#[inline]
pub fn free<R, F: FnOnce() -> R>(f: F) -> R {
    let state = disable();
    let result = f();
    unsafe { restore(state) };
    result
}
