//! Processor Interface (PI) — interrupt controller and reset button.
//!
//! The PI aggregates all hardware interrupt sources into the CPU's
//! external interrupt line (exception vector 0x0500).
//!
//! **Status: Stub — see TODO.md**

pub const PI_BASE: usize = 0xCC003000;

/// Enable external interrupts (sets MSR[EE]).
#[inline(always)]
pub unsafe fn enable_irq() {
    core::arch::asm!("mfmsr 3; ori 3,3,0x8000; mtmsr 3",
        out("r3") _, options(nostack));
}

/// Disable external interrupts (clears MSR[EE]).
/// Returns the previous MSR value so it can be restored.
#[inline(always)]
pub unsafe fn disable_irq() -> u32 {
    let msr: u32;
    core::arch::asm!(
        "mfmsr {0}",
        "rlwinm {1},{0},0,17,15",  // clear MSR[EE] (bit 16)
        "mtmsr {1}",
        out(reg) msr,
        out(reg) _,
        options(nostack)
    );
    msr
}
