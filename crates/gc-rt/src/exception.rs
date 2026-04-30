//! PowerPC exception vector table.
//!
//! The Gekko/Broadway CPU has 16 exception types, each handled by a 128-byte
//! stub at a fixed physical address starting at `0x80000100`.
//!
//! ## Exception Vectors (GC/DOL memory map)
//!
//! | Offset | Name              | Cause                                      |
//! |--------|-------------------|--------------------------------------------|
//! | 0x0100 | System Reset      | Hardware reset or `sc` (syscall)           |
//! | 0x0200 | Machine Check     | Bus error, ECC fault                       |
//! | 0x0300 | DSI               | Data storage interrupt (bad data access)   |
//! | 0x0400 | ISI               | Instruction storage interrupt (bad fetch)  |
//! | 0x0500 | External Interrupt| PI asserts interrupt line                  |
//! | 0x0600 | Alignment         | Misaligned load/store                      |
//! | 0x0700 | Program           | Illegal instruction, trap, FP exception    |
//! | 0x0800 | FP Unavailable    | FPU accessed with MSR[FP]=0                |
//! | 0x0900 | Decrementer       | Decrement register underflowed             |
//! | 0x0C00 | System Call       | `sc` instruction                           |
//! | 0x0D00 | Trace             | Single-step debug mode                     |
//! | 0x0F00 | Performance Mon.  | Performance counter overflow               |
//! | 0x1300 | Instruction DABR  | Data breakpoint                            |
//! | 0x1400 | SMI               | System Management Interrupt                |
//! | 0x1700 | Thermal Mgmt.     | Thermal throttle interrupt                 |
//!
//! ## Current Status
//!
//! This module is a stub. All exceptions currently halt. Milestone 1 will
//! implement proper handler dispatch with saved register contexts and
//! user-registrable callbacks.

// TODO (Milestone 1):
// - [ ] Define ExceptionContext struct (all GPRs + SRR0/SRR1 + cause)
// - [ ] Implement 128-byte exception stubs in global_asm! for each vector
// - [ ] Install stubs at 0x80000100 (must be written via physical/uncached ptr)
// - [ ] Flush instruction cache after writing stubs (icbi + isync)
// - [ ] Provide register_handler(ExceptionType, fn(&ExceptionContext) -> !) API
// - [ ] Handle decrementer interrupt for system tick (Milestone 1 timer)

/// Exception types supported by the GameCube/Wii hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Exception {
    SystemReset      = 0x0100,
    MachineCheck     = 0x0200,
    Dsi              = 0x0300,
    Isi              = 0x0400,
    ExternalInterrupt= 0x0500,
    Alignment        = 0x0600,
    Program          = 0x0700,
    FpUnavailable    = 0x0800,
    Decrementer      = 0x0900,
    SystemCall       = 0x0C00,
    Trace            = 0x0D00,
    PerformanceMon   = 0x0F00,
    InstructionDabr  = 0x1300,
    Smi              = 0x1400,
    ThermalMgmt      = 0x1700,
}

/// Initialise exception vectors.
///
/// Currently a no-op (stubs not yet installed). Any exception will cause the
/// hardware to jump to 0x80000xxx, which likely contains zeroes or IPL code.
/// This means an unhandled exception in development will typically cause a
/// Machine Check or hang rather than a clean error.
///
/// # Safety
///
/// Must be called early in the boot sequence, before any hardware is accessed
/// that could trigger an exception.
pub unsafe fn init() {
    // TODO (Milestone 1): install exception stubs
}
