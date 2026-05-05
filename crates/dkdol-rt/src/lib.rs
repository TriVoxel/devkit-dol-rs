//! # dkdol-rt — GameCube/Wii Bare-Metal Runtime
//!
//! Provides the lowest-level runtime infrastructure for GC/Wii applications:
//!
//! - **Boot** (`_start`): BATs, FPU, cache, BSS zero, → `main`
//! - **Exceptions**: 15 exception vectors with full context save/restore
//! - **IRQ**: Critical section helpers (disable/restore MSR[EE])
//! - **Timer**: Decrementer-based tick counter and TBR access
//! - **Cache**: `dcbf`, `dcbi`, `icbi` wrappers

#![no_std]
#![feature(asm_experimental_arch)]  // required for inline PPC asm

pub mod cache;
pub mod exception;
pub mod irq;
pub mod timer;
mod start;

/// Spin forever. Terminal state for unrecoverable errors.
#[inline(always)]
pub fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)) };
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    halt()
}
