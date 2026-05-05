//! PowerPC exception vector table — installation and dispatch.
//!
//! [`init`] writes a 6-instruction absolute-branch stub into each of the 15
//! GC exception vectors via the uncached BAT1 mirror (0xC0000xxx), then
//! invalidates the instruction cache. Each stub saves 0, loads the absolute
//! address of `__exc_entry`, and branches there unconditionally.
//!
//! `__exc_entry` (global_asm!) saves the full register context into a 192-byte
//! [`ExcCtx`] on the dedicated exception stack, calls [`__exc_rust_dispatch`],
//! then restores and `rfi`.

use core::arch::global_asm;

// ─── ExcCtx ──────────────────────────────────────────────────────────────────

/// Full register context saved at exception time.
/// Field offsets are hardcoded in the `__exc_entry` assembly below.
#[repr(C, align(32))]
pub struct ExcCtx {
    pub gprs:    [u32; 32],   // 0–124  (0–31)
    pub srr0:    u32,          // 128
    pub srr1:    u32,          // 132
    pub cr:      u32,          // 136
    pub lr:      u32,          // 140
    pub ctr:     u32,          // 144
    pub xer:     u32,          // 148
    pub dar:     u32,          // 152
    pub dsisr:   u32,          // 156
    pub exc_num: u32,          // 160
    _pad: [u32; 7],            // 164–191 → total 192 bytes = 6×32
}
const _: () = assert!(core::mem::size_of::<ExcCtx>() == 192);

// ─── Exception enum ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Exception {
    SystemReset       = 0x0100,
    MachineCheck      = 0x0200,
    Dsi               = 0x0300,
    Isi               = 0x0400,
    ExternalInterrupt = 0x0500,
    Alignment         = 0x0600,
    Program           = 0x0700,
    FpUnavailable     = 0x0800,
    Decrementer       = 0x0900,
    SystemCall        = 0x0C00,
    Trace             = 0x0D00,
    PerformanceMon    = 0x0F00,
    InstructionDabr   = 0x1300,
    Smi               = 0x1400,
    ThermalMgmt       = 0x1700,
}

const ALL_EXCEPTIONS: &[Exception] = &[
    Exception::SystemReset, Exception::MachineCheck, Exception::Dsi,
    Exception::Isi, Exception::ExternalInterrupt, Exception::Alignment,
    Exception::Program, Exception::FpUnavailable, Exception::Decrementer,
    Exception::SystemCall, Exception::Trace, Exception::PerformanceMon,
    Exception::InstructionDabr, Exception::Smi, Exception::ThermalMgmt,
];
const NUM_EXC: usize = 15;

pub type ExcHandler = fn(Exception, &mut ExcCtx);

// ─── State ────────────────────────────────────────────────────────────────────

static mut HANDLERS: [Option<ExcHandler>; NUM_EXC] = [None; NUM_EXC];

#[repr(C, align(32))]
struct ExcStack([u8; 16384]);
static mut EXC_STACK: ExcStack = ExcStack([0; 16384]);

/// Stack-top pointer read by `__exc_entry`.  Must be `#[no_mangle]`.
#[no_mangle]
pub static mut __EXC_STACK_TOP: u32 = 0;

fn exc_to_idx(num: u32) -> Option<usize> {
    ALL_EXCEPTIONS.iter().position(|e| *e as u32 == num)
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Install all exception vector stubs and initialise the exception stack.
///
/// # Safety
/// Writes to physical 0x00000100–0x00001800 via the uncached BAT1 mirror.
/// DBAT1 must already be configured (the boot assembly does this).
pub unsafe fn init() {
    __EXC_STACK_TOP = core::ptr::addr_of!(EXC_STACK).cast::<u8>().add(16384) as u32;

    extern "C" { fn __exc_entry(); }
    let entry = __exc_entry as *const () as u32;

    for &exc in ALL_EXCEPTIONS {
        let unc = 0xC000_0000u32 | (exc as u32); // uncached write address
        let vrt = 0x8000_0000u32 | (exc as u32); // cached address for icbi
        write_stub(unc as *mut u32, entry, exc as u32);
        crate::cache::icbi(vrt as *const u8);
    }
    crate::cache::sync();
    crate::cache::isync();
}

/// Register a handler for `exc`.  Replaces any prior handler.
pub unsafe fn register(exc: Exception, handler: ExcHandler) {
    if let Some(i) = exc_to_idx(exc as u32) { HANDLERS[i] = Some(handler); }
}

/// Remove the handler for `exc` (reverts to halt).
pub unsafe fn unregister(exc: Exception) {
    if let Some(i) = exc_to_idx(exc as u32) { HANDLERS[i] = None; }
}

// ─── Rust dispatcher (called from __exc_entry) ────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn __exc_rust_dispatch(ctx: *mut ExcCtx) {
    let num = (*ctx).exc_num;
    // Decrementer: always tick the timer first.
    if num == Exception::Decrementer as u32 {
        crate::timer::__timer_dec_handler();
    }
    if let Some(i) = exc_to_idx(num) {
        if let Some(h) = HANDLERS[i] {
            h(ALL_EXCEPTIONS[i], &mut *ctx);
            return;
        }
    }
    crate::halt();
}

// ─── Stub writer ─────────────────────────────────────────────────────────────

unsafe fn write_stub(dst: *mut u32, entry: u32, exc_num: u32) {
    // 6-instruction stub (24 bytes):
    //   mtspr SPRG2, 0        0x7C1243A6
    //   lis   0, entry_hi     0x3C000000 | hi
    //   ori   0, 0, entry_lo 0x60000000 | lo
    //   mtctr 0               0x7C0903A6
    //   li    0, exc_num      0x38000000 | exc_num
    //   bctr                   0x4E800420
    // Remaining 26 words: NOP (0x60000000)
    let hi = (entry >> 16) & 0xFFFF;
    let lo = entry & 0xFFFF;
    let words: [u32; 6] = [
        0x7C1243A6,
        0x3C000000 | hi,
        0x60000000 | lo,
        0x7C0903A6,
        0x38000000 | (exc_num & 0xFFFF),
        0x4E800420,
    ];
    for (i, &w) in words.iter().enumerate() {
        core::ptr::write_volatile(dst.add(i), w);
    }
    for i in 6..32 {
        core::ptr::write_volatile(dst.add(i), 0x6000_0000);
    }
}

// ─── __exc_entry assembly ─────────────────────────────────────────────────────
//
// Context: see write_stub above for how we get here.
// 0 = exc_num, SPRG2 = original 0, SPRG3 = (used below for 1)

global_asm!(
    ".globl __exc_entry",
    ".type  __exc_entry, @function",
    "__exc_entry:",

    "   mtspr 275, 1",                   // SPRG3 = original 1

    // Switch to exception stack
    "   lis   1, __EXC_STACK_TOP@ha",
    "   addi  1, 1, __EXC_STACK_TOP@l",
    "   lwz   1, 0(1)",                 // 1 = stack top value
    "   addi  1, 1, -192",              // allocate ExcCtx frame

    // Save exc_num, then original 0/1
    "   stw   0, 160(1)",               // ctx.exc_num
    "   mfspr 0, 274",                   // 0 = original 0 (from SPRG2)
    "   stw   0, 0(1)",                 // ctx.gprs[0]
    "   mfspr 0, 275",                   // 0 = original 1 (from SPRG3)
    "   stw   0, 4(1)",                 // ctx.gprs[1]

    // Save 2–31
    "   stw   2,   8(1)",   "   stw   3,  12(1)",
    "   stw   4,  16(1)",   "   stw   5,  20(1)",
    "   stw   6,  24(1)",   "   stw   7,  28(1)",
    "   stw   8,  32(1)",   "   stw   9,  36(1)",
    "   stw   10, 40(1)",   "   stw   11, 44(1)",
    "   stw   12, 48(1)",   "   stw   13, 52(1)",
    "   stw   14, 56(1)",   "   stw   15, 60(1)",
    "   stw   16, 64(1)",   "   stw   17, 68(1)",
    "   stw   18, 72(1)",   "   stw   19, 76(1)",
    "   stw   20, 80(1)",   "   stw   21, 84(1)",
    "   stw   22, 88(1)",   "   stw   23, 92(1)",
    "   stw   24, 96(1)",   "   stw   25, 100(1)",
    "   stw   26, 104(1)",  "   stw   27, 108(1)",
    "   stw   28, 112(1)",  "   stw   29, 116(1)",
    "   stw   30, 120(1)",  "   stw   31, 124(1)",

    // Save SRR0, SRR1, CR, LR, CTR, XER, DAR, DSISR
    "   mfsrr0  0",  "   stw 0, 128(1)",
    "   mfsrr1  0",  "   stw 0, 132(1)",
    "   mfcr    0",  "   stw 0, 136(1)",
    "   mflr    0",  "   stw 0, 140(1)",
    "   mfctr   0",  "   stw 0, 144(1)",
    "   mfxer   0",  "   stw 0, 148(1)",
    "   mfspr 0, 19",  "   stw 0, 152(1)",   // DAR = SPR 19
    "   mfspr 0, 18",  "   stw 0, 156(1)",   // DSISR = SPR 18

    // Mark as recoverable, keep EE disabled
    "   mfmsr 0",
    "   ori   0, 0, 0x0002",
    "   mtmsr 0",
    "   isync",

    // Call Rust dispatcher
    "   mr  3, 1",
    "   bl  __exc_rust_dispatch",

    // Restore: disable EE before writing SRR0/SRR1
    "   mfmsr 0",
    "   rlwinm 0, 0, 0, 17, 15",    // clear EE (bit 16)
    "   mtmsr 0",
    "   isync",

    // Restore SRR0, SRR1, CR, LR, CTR, XER
    "   lwz 0, 128(1)",  "   mtsrr0 0",
    "   lwz 0, 132(1)",  "   mtsrr1 0",
    "   lwz 0, 136(1)",  "   mtcr   0",
    "   lwz 0, 140(1)",  "   mtlr   0",
    "   lwz 0, 144(1)",  "   mtctr  0",
    "   lwz 0, 148(1)",  "   mtxer  0",

    // Restore 2–31
    "   lwz   2,   8(1)",   "   lwz   3,  12(1)",
    "   lwz   4,  16(1)",   "   lwz   5,  20(1)",
    "   lwz   6,  24(1)",   "   lwz   7,  28(1)",
    "   lwz   8,  32(1)",   "   lwz   9,  36(1)",
    "   lwz   10, 40(1)",   "   lwz   11, 44(1)",
    "   lwz   12, 48(1)",   "   lwz   13, 52(1)",
    "   lwz   14, 56(1)",   "   lwz   15, 60(1)",
    "   lwz   16, 64(1)",   "   lwz   17, 68(1)",
    "   lwz   18, 72(1)",   "   lwz   19, 76(1)",
    "   lwz   20, 80(1)",   "   lwz   21, 84(1)",
    "   lwz   22, 88(1)",   "   lwz   23, 92(1)",
    "   lwz   24, 96(1)",   "   lwz   25, 100(1)",
    "   lwz   26, 104(1)",  "   lwz   27, 108(1)",
    "   lwz   28, 112(1)",  "   lwz   29, 116(1)",
    "   lwz   30, 120(1)",  "   lwz   31, 124(1)",

    // Restore 0 and 1 last
    "   lwz   0, 0(1)",     // original 0 → stash in SPRG2
    "   mtspr 274, 0",        // SPRG2 = original 0
    "   lwz   1, 4(1)",     // 1 = original stack ptr
    "   mfspr 0, 274",        // 0 = original 0
    "   rfi",

    ".size __exc_entry, . - __exc_entry",
);
