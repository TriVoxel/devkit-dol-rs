//! # gc-rt — GameCube/Wii Bare-Metal Runtime
//!
//! This crate provides the lowest-level runtime infrastructure for GameCube
//! (and optionally Wii) applications:
//!
//! - **Boot vector** (`_start`): PowerPC assembly entry point. Sets up BATs,
//!   initialises the FPU and cache, zeroes BSS, then calls the Rust `main`.
//! - **Exception handlers**: Installs stubs for all 16 PPC exception vectors
//!   at `0x80000100`. (Milestone 1 — currently minimal.)
//! - **Cache operations**: `dcbi`, `dcbf`, `dcbst`, `icbi` wrappers.
//! - **Panic handler**: Infinite halt loop on bare metal.
//!
//! ## Usage
//!
//! Add `gc-rt` as a dependency of your binary crate. The `_start` symbol is
//! exported automatically and the linker script (`link/gcn.ld`) is wired up
//! via `build.rs`.
//!
//! Your application entry point must be:
//!
//! ```rust,no_run
//! #![no_std]
//! #![no_main]
//!
//! #[no_mangle]
//! pub extern "C" fn main() -> ! {
//!     loop {}
//! }
//! ```
//!
//! ## Safety
//!
//! The boot code runs before Rust memory safety guarantees exist (BSS is not
//! yet zeroed, the heap allocator is not yet initialised). All code in this
//! crate is `unsafe` by nature of being bare-metal startup.

#![no_std]
#![feature(asm_experimental_arch)]  // needed for PowerPC-specific asm

// Public modules
pub mod cache;
pub mod exception;

// Internal boot-sequence module — not public API.
// The entry point `_start` is emitted here via global_asm!.
mod start;

/// Spin forever. Used as the terminal state for unrecoverable errors.
#[inline(always)]
pub fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)) };
    }
}

/// Panic handler for bare-metal targets.
///
/// On a real GC there's nowhere to print, so we just halt.
/// In Dolphin you can attach a debugger and inspect the panic location.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // TODO (Milestone 1): Write panic message to the framebuffer / EXI debug port
    halt()
}
