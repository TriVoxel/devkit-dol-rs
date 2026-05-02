//! Write-gather pipe primitives.
//!
//! The write-gather pipe is a special hardware buffer at physical address
//! `0x0C008000` (virtual `0xCC008000`). The CPU accumulates writes into a
//! 32-byte internal buffer (WGP); when full the buffer is burst-transferred
//! to the FIFO in RAM as one aligned 32-byte write.
//!
//! ## Requirements
//!
//! - WPAR (SPR 921) must be set to the physical address `0x0C008000`.
//! - HID2 bit 30 (WPE = Write-gather Pipe Enable) must be set.
//! - All writes must be `volatile` so the compiler does not reorder or
//!   eliminate them.
//! - Memory barriers between writes prevent instruction-level reordering.
//!
//! The [`init`] function handles WPAR and HID2 programming. Call it once
//! before using any other GX function.

use core::arch::asm;

pub const WGPIPE_ADDR: usize = crate::mmio::addr(0x008000);

/// Enable the write-gather pipe.
///
/// Sets WPAR to the physical pipe address and enables WPE in HID2.
///
/// # Safety
/// Must be called once during GX init, before any FIFO writes.
pub unsafe fn init() {
    // WPAR (SPR 921 = 0x399): physical address of write-gather pipe = 0x0C008000
    asm!(
        "mtspr 921, {v}",
        v = in(reg) 0x0C00_8000u32,
        options(nostack, nomem)
    );
    // HID2 (SPR 920 = 0x398): bit 30 = WPE (Write-gather Pipe Enable)
    let hid2: u32;
    asm!("mfspr {v}, 920", v = out(reg) hid2, options(nostack, nomem));
    asm!("mtspr 920, {v}", v = in(reg) hid2 | 0x4000_0000, options(nostack, nomem));
}

/// Wait for the write-gather buffer to drain (WPAR bit 0 = busy).
pub unsafe fn flush() {
    loop {
        let wpar: u32;
        asm!("mfspr {v}, 921", v = out(reg) wpar, options(nostack, nomem));
        if wpar & 1 == 0 { break; }
    }
}

// ── Raw write helpers ────────────────────────────────────────────────────────
// Each write is volatile to the WGPIPE_ADDR. The compiler memory clobber
// after each write ensures ordering (matches libogc2's `asm volatile("" ::: "memory")`).

#[inline(always)]
pub unsafe fn write8(v: u8) {
    core::ptr::write_volatile(WGPIPE_ADDR as *mut u8, v);
    asm!("", options(nostack, preserves_flags));
}

#[inline(always)]
pub unsafe fn write16(v: u16) {
    core::ptr::write_volatile(WGPIPE_ADDR as *mut u16, v);
    asm!("", options(nostack, preserves_flags));
}

#[inline(always)]
pub unsafe fn write32(v: u32) {
    core::ptr::write_volatile(WGPIPE_ADDR as *mut u32, v);
    asm!("", options(nostack, preserves_flags));
}

#[inline(always)]
pub unsafe fn writef32(v: f32) {
    // Transmute to u32 bits and write — same ABI as wgPipe->F32
    write32(v.to_bits());
}

// ── Opcode helpers ───────────────────────────────────────────────────────────

/// Load a BP (Blitting Processor) register.
///
/// `val`: high byte = BP register address, lower 24 bits = register value.
#[inline(always)]
pub unsafe fn load_bp_reg(val: u32) {
    write8(0x61);
    write32(val);
}

/// Load a CP (Command Processor) register.
///
/// `addr`: 8-bit CP register address.
/// `val`: 32-bit value.
#[inline(always)]
pub unsafe fn load_cp_reg(addr: u8, val: u32) {
    write8(0x08);
    write8(addr);
    write32(val);
}

/// Load a single XF (Transform Engine) register.
///
/// `addr`: 16-bit XF register address.
/// `val`: 32-bit value.
#[inline(always)]
pub unsafe fn load_xf_reg(addr: u16, val: u32) {
    write8(0x10);
    write32((addr as u32) & 0xFFFF);
    write32(val);
}

/// Load `count` consecutive XF registers starting at `addr`.
/// Caller must write `count` 32-bit values immediately after this call.
#[inline(always)]
pub unsafe fn load_xf_regs(addr: u16, count: u16) {
    write8(0x10);
    write32((((count - 1) as u32) << 16) | ((addr as u32) & 0xFFFF));
}

/// Invalidate the vertex cache.
#[inline(always)]
pub unsafe fn inv_vtx_cache() {
    write8(0x48);
}
