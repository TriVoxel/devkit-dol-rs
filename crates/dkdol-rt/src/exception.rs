//! PowerPC exception vector table — installation and dispatch.
//!
//! [`init`] writes a 6-instruction absolute-branch stub into each of the 15
//! GC exception vectors via the uncached BAT1 mirror (0xC0000xxx), then
//! invalidates the instruction cache. Each stub saves r0, loads the absolute
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
    pub gprs:    [u32; 32],   // 0–124  (r0–r31)
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
    __EXC_STACK_TOP = EXC_STACK.0.as_ptr().add(16384) as u32;

    extern "C" { fn __exc_entry(); }
    let entry = __exc_entry as u32;

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
    //   mtspr SPRG2, r0        0x7C1243A6
    //   lis   r0, entry_hi     0x3C000000 | hi
    //   ori   r0, r0, entry_lo 0x60000000 | lo
    //   mtctr r0               0x7C0903A6
    //   li    r0, exc_num      0x38000000 | exc_num
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
// r0 = exc_num, SPRG2 = original r0, SPRG3 = (used below for r1)

global_asm!(
    ".globl __exc_entry",
    ".type  __exc_entry, @function",
    "__exc_entry:",

    "   mtspr 275, r1",                   // SPRG3 = original r1

    // Switch to exception stack
    "   lis   r1, __EXC_STACK_TOP@ha",
    "   addi  r1, r1, __EXC_STACK_TOP@l",
    "   lwz   r1, 0(r1)",                 // r1 = stack top value
    "   addi  r1, r1, -192",              // allocate ExcCtx frame

    // Save exc_num, then original r0/r1
    "   stw   r0, 160(r1)",               // ctx.exc_num
    "   mfspr r0, 274",                   // r0 = original r0 (from SPRG2)
    "   stw   r0, 0(r1)",                 // ctx.gprs[0]
    "   mfspr r0, 275",                   // r0 = original r1 (from SPRG3)
    "   stw   r0, 4(r1)",                 // ctx.gprs[1]

    // Save r2–r31
    "   stw   r2,   8(r1)",   "   stw   r3,  12(r1)",
    "   stw   r4,  16(r1)",   "   stw   r5,  20(r1)",
    "   stw   r6,  24(r1)",   "   stw   r7,  28(r1)",
    "   stw   r8,  32(r1)",   "   stw   r9,  36(r1)",
    "   stw   r10, 40(r1)",   "   stw   r11, 44(r1)",
    "   stw   r12, 48(r1)",   "   stw   r13, 52(r1)",
    "   stw   r14, 56(r1)",   "   stw   r15, 60(r1)",
    "   stw   r16, 64(r1)",   "   stw   r17, 68(r1)",
    "   stw   r18, 72(r1)",   "   stw   r19, 76(r1)",
    "   stw   r20, 80(r1)",   "   stw   r21, 84(r1)",
    "   stw   r22, 88(r1)",   "   stw   r23, 92(r1)",
    "   stw   r24, 96(r1)",   "   stw   r25, 100(r1)",
    "   stw   r26, 104(r1)",  "   stw   r27, 108(r1)",
    "   stw   r28, 112(r1)",  "   stw   r29, 116(r1)",
    "   stw   r30, 120(r1)",  "   stw   r31, 124(r1)",

    // Save SRR0, SRR1, CR, LR, CTR, XER, DAR, DSISR
    "   mfsrr0  r0",  "   stw r0, 128(r1)",
    "   mfsrr1  r0",  "   stw r0, 132(r1)",
    "   mfcr    r0",  "   stw r0, 136(r1)",
    "   mflr    r0",  "   stw r0, 140(r1)",
    "   mfctr   r0",  "   stw r0, 144(r1)",
    "   mfxer   r0",  "   stw r0, 148(r1)",
    "   mfdar   r0",  "   stw r0, 152(r1)",
    "   mfdsisr r0",  "   stw r0, 156(r1)",

    // Mark as recoverable, keep EE disabled
    "   mfmsr r0",
    "   ori   r0, r0, 0x0002",
    "   mtmsr r0",
    "   isync",

    // Call Rust dispatcher
    "   mr  r3, r1",
    "   bl  __exc_rust_dispatch",

    // Restore: disable EE before writing SRR0/SRR1
    "   mfmsr r0",
    "   rlwinm r0, r0, 0, 17, 15",    // clear EE (bit 16)
    "   mtmsr r0",
    "   isync",

    // Restore SRR0, SRR1, CR, LR, CTR, XER
    "   lwz r0, 128(r1)",  "   mtsrr0 r0",
    "   lwz r0, 132(r1)",  "   mtsrr1 r0",
    "   lwz r0, 136(r1)",  "   mtcr   r0",
    "   lwz r0, 140(r1)",  "   mtlr   r0",
    "   lwz r0, 144(r1)",  "   mtctr  r0",
    "   lwz r0, 148(r1)",  "   mtxer  r0",

    // Restore r2–r31
    "   lwz   r2,   8(r1)",   "   lwz   r3,  12(r1)",
    "   lwz   r4,  16(r1)",   "   lwz   r5,  20(r1)",
    "   lwz   r6,  24(r1)",   "   lwz   r7,  28(r1)",
    "   lwz   r8,  32(r1)",   "   lwz   r9,  36(r1)",
    "   lwz   r10, 40(r1)",   "   lwz   r11, 44(r1)",
    "   lwz   r12, 48(r1)",   "   lwz   r13, 52(r1)",
    "   lwz   r14, 56(r1)",   "   lwz   r15, 60(r1)",
    "   lwz   r16, 64(r1)",   "   lwz   r17, 68(r1)",
    "   lwz   r18, 72(r1)",   "   lwz   r19, 76(r1)",
    "   lwz   r20, 80(r1)",   "   lwz   r21, 84(r1)",
    "   lwz   r22, 88(r1)",   "   lwz   r23, 92(r1)",
    "   lwz   r24, 96(r1)",   "   lwz   r25, 100(r1)",
    "   lwz   r26, 104(r1)",  "   lwz   r27, 108(r1)",
    "   lwz   r28, 112(r1)",  "   lwz   r29, 116(r1)",
    "   lwz   r30, 120(r1)",  "   lwz   r31, 124(r1)",

    // Restore r0 and r1 last
    "   lwz   r0, 0(r1)",     // original r0 → stash in SPRG2
    "   mtspr 274, r0",        // SPRG2 = original r0
    "   lwz   r1, 4(r1)",     // r1 = original stack ptr
    "   mfspr r0, 274",        // r0 = original r0
    "   rfi",

    ".size __exc_entry, . - __exc_entry",
);
