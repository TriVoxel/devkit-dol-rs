//! Boot sequence for the GameCube (Gekko/Broadway).
//!
//! This module emits the `.crt0` section via `global_asm!`. The linker script
//! places `.crt0` first so `_start` is at the DOL entry point.
//!
//! ## Sequence
//!
//! 1. Save link register, jump to real-mode BAT init (`__init_bats`)
//! 2. Initialise all GPRs to zero, set up stack pointer
//! 3. Set small data area base registers (r2 = .sdata2, r13 = .sdata)
//! 4. Enable floating-point unit (MSR[FP])
//! 5. Initialise Paired Singles (HID2[PSE])
//! 6. Initialise FPRs to +0.0
//! 7. Enable L1 data cache and instruction cache (HID0)
//! 8. Zero .sbss and .bss sections
//! 9. Branch to `main` (the Rust application entry point)
//!
//! ## BAT Configuration (GC/DOL mode)
//!
//! | BAT  | Virtual          | Physical   | Size  | WIMG | Use                      |
//! |------|-----------------|------------|-------|------|--------------------------|
//! | IBAT0/DBAT0 | 0x80000000 | 0x00000000 | 256 MB | 0000 | Cached RAM               |
//! | DBAT1       | 0xC0000000 | 0x00000000 | 256 MB | 0101 | Uncached RAM mirror      |
//! | (DBAT1 also covers 0xCC000000 — hardware registers — same physical mapping) |
//!
//! References:
//! - YAGCD section 4 (CPU)
//! - libogc2 `system_asm.S` / `ogc_crt0.S` (studied for reference, not linked)

use core::arch::global_asm;

// HID0 bit definitions
// bit 17 = DCE  (data cache enable)
// bit 16 = ICE  (instruction cache enable)
// bit 11 = DLOCK (data cache lock)
// bit 10 = ILOCK (instruction cache lock)
//
// We set HID0 = 0x0091_0C64 initially (caches off, invalidate) then enable them.

global_asm!(
    // ──────────────────────────────────────────────────────────────────────
    // .crt0 section: first bytes of the DOL. The linker script ensures
    // this is placed at the very start of the binary (0x80003100).
    // ──────────────────────────────────────────────────────────────────────
    ".section .crt0, \"ax\", @progbits",
    ".globl _start",
    ".type  _start, @function",

    "_start:",

    // ── Step 1: Switch to real mode and configure BATs ──────────────────
    // We need to run __init_bats_realmode with address translation off.
    // rfi is used to atomically clear MSR[IR|DR] (instruction/data relocation).
    // We compute the physical address of __init_bats_realmode by stripping
    // the top two bits (0x80xxxxxx → 0x00xxxxxx).

    "   mflr    r0",                        // save LR (we'll restore after BAT init)
    "   bl      1f",                        // get PC into r3 via bl trick
    "1:",
    "   mflr    r3",
    "   mtlr    r0",                        // restore original LR

    // Compute physical address of __init_bats_realmode
    "   lis     r4, __init_bats_realmode@ha",
    "   addi    r4, r4, __init_bats_realmode@l",
    "   rlwinm  r4, r4, 0, 2, 31",         // strip top 2 bits (virtual→physical)
    "   mtsrr0  r4",                        // SRR0 = jump target after rfi

    // Clear MSR[IR|DR] to disable address translation
    "   mfmsr   r3",
    "   rlwinm  r3, r3, 0, 28, 25",        // clear bits 4,5 (MSR_IR=4, MSR_DR=5)
    "   mtsrr1  r3",                        // SRR1 = new MSR value
    "   rfi",                               // switch to real mode, jump to __init_bats_realmode

    // Execution continues in __init_bats_realmode (real mode, no translation)
    // That function sets up BATs and returns here (virtual mode) via rfi.

    // ── After BAT init — virtual mode restored ───────────────────────────
    ".globl __after_bat_init",
    "__after_bat_init:",

    // ── Step 2: Clear all GPRs to 0 ─────────────────────────────────────
    "   li      r0,  0",
    "   li      r3,  0",
    "   li      r4,  0",
    "   li      r5,  0",
    "   li      r6,  0",
    "   li      r7,  0",
    "   li      r8,  0",
    "   li      r9,  0",
    "   li      r10, 0",
    "   li      r11, 0",
    "   li      r12, 0",
    "   li      r14, 0",
    "   li      r15, 0",
    "   li      r16, 0",
    "   li      r17, 0",
    "   li      r18, 0",
    "   li      r19, 0",
    "   li      r20, 0",
    "   li      r21, 0",
    "   li      r22, 0",
    "   li      r23, 0",
    "   li      r24, 0",
    "   li      r25, 0",
    "   li      r26, 0",
    "   li      r27, 0",
    "   li      r28, 0",
    "   li      r29, 0",
    "   li      r30, 0",
    "   li      r31, 0",

    // ── Step 3: Set up stack pointer ─────────────────────────────────────
    "   lis     r1, __stack_top@ha",
    "   addi    r1, r1, __stack_top@l",
    "   rlwinm  r1, r1, 0, 0, 27",         // align to 16 bytes
    "   li      r0, 0",
    "   stwu    r0, -8(r1)",               // push a null frame (ABI requirement)

    // ── Step 4: Set small data area base registers ───────────────────────
    "   lis     r2,  _SDA2_BASE_@ha",
    "   addi    r2,  r2, _SDA2_BASE_@l",
    "   lis     r13, _SDA_BASE_@ha",
    "   addi    r13, r13, _SDA_BASE_@l",

    // ── Step 5: Enable FPU (MSR[FP] = bit 13) ───────────────────────────
    "   mfmsr   r3",
    "   ori     r3, r3, 0x2000",           // set MSR_FP
    "   mtmsr   r3",
    "   isync",

    // ── Step 6: Initialise Paired Singles (HID2[PSE] = bit 0, WPE = bit 1) ──
    "   mfspr   r3, 920",                  // 920 = HID2 SPR number
    "   oris    r3, r3, 0xA000",           // set PSE (bit 16) and WPE (bit 17)
    "   mtspr   920, r3",
    "   isync",

    // ── Step 7: Initialise all FPRs to +0.0 ─────────────────────────────
    "   .set    index, 0",
    "   .rept   32",
    "   fmr     0+index, 0+index",         // ensure +0.0 (reads from zeroed FPR)
    "   .set    index, index+1",
    "   .endr",

    // ── Step 8: Enable L1 caches ─────────────────────────────────────────
    // HID0 layout: bit 16=DCE, bit 15=ICE, bit 12=DCFI, bit 11=ICFI
    // Sequence: invalidate both caches, then enable them.
    "   mfspr   r3, 1008",                 // 1008 = HID0 SPR
    "   ori     r3, r3, 0x0C00",           // set DCFI (bit 12) + ICFI (bit 11)
    "   mtspr   1008, r3",
    "   isync",
    "   sync",
    "   ori     r3, r3, 0xC000",           // set DCE (bit 16) + ICE (bit 15)
    "   mtspr   1008, r3",
    "   isync",

    // ── Step 9: Zero .sbss section ──────────────────────────────────────
    "   lis     r3, __sbss_start@ha",
    "   addi    r3, r3, __sbss_start@l",
    "   lis     r5, __sbss_end@ha",
    "   addi    r5, r5, __sbss_end@l",
    "   sub     r5, r5, r3",               // r5 = byte count
    "   li      r4, 0",
    "   bl      __bss_fill",

    // ── Step 10: Zero .bss section ──────────────────────────────────────
    "   lis     r3, __bss_start@ha",
    "   addi    r3, r3, __bss_start@l",
    "   lis     r5, __bss_end@ha",
    "   addi    r5, r5, __bss_end@l",
    "   sub     r5, r5, r3",               // r5 = byte count
    "   li      r4, 0",
    "   bl      __bss_fill",

    // ── Step 11: Branch to Rust main ────────────────────────────────────
    "   bl      main",
    "   b       __halt",                   // if main returns, halt

    // ──────────────────────────────────────────────────────────────────────
    // __bss_fill: memset(r3, r4, r5) — fills r5 bytes at r3 with byte r4.
    // Simple word-at-a-time loop; fine for startup (not performance-critical).
    // ──────────────────────────────────────────────────────────────────────
    "__bss_fill:",
    "   cmpwi   r5, 0",
    "   beqlr",                             // nothing to do
    "   mr      r6, r3",
    "2:",
    "   stb     r4, 0(r6)",
    "   addi    r6, r6, 1",
    "   subic.  r5, r5, 1",
    "   bne     2b",
    "   blr",

    // ──────────────────────────────────────────────────────────────────────
    // __halt: spin forever
    // ──────────────────────────────────────────────────────────────────────
    ".globl __halt",
    "__halt:",
    "   nop",
    "   b       __halt",

    // ──────────────────────────────────────────────────────────────────────
    // __init_bats_realmode: runs with address translation disabled (real mode).
    //
    // Sets up Block Address Translation (BAT) registers so the CPU can
    // access cached RAM (0x80000000), uncached RAM (0xC0000000), and
    // hardware registers (0xCC000000, covered by DBAT1).
    //
    // After configuring BATs, re-enables MSR[IR|DR] and jumps to
    // __after_bat_init (virtual mode).
    // ──────────────────────────────────────────────────────────────────────
    "__init_bats_realmode:",

    // HID0: caches off, invalidate, stop gathering
    "   lis     r3, 0x0091",
    "   ori     r3, r3, 0x0C64",
    "   mtspr   1008, r3",               // HID0 = 1008
    "   isync",

    // Clear all segment registers
    "   lis     r0, 0x8000",
    "   mtsr    0,  r0",
    "   mtsr    1,  r0",
    "   mtsr    2,  r0",
    "   mtsr    3,  r0",
    "   mtsr    4,  r0",
    "   mtsr    5,  r0",
    "   mtsr    6,  r0",
    "   mtsr    7,  r0",
    "   mtsr    8,  r0",
    "   mtsr    9,  r0",
    "   mtsr    10, r0",
    "   mtsr    11, r0",
    "   mtsr    12, r0",
    "   mtsr    13, r0",
    "   mtsr    14, r0",
    "   mtsr    15, r0",
    "   isync",

    // Clear all BAT registers
    "   li      r0, 0",
    "   mtspr   528, r0",    // IBAT0U
    "   mtspr   529, r0",    // IBAT0L
    "   mtspr   530, r0",    // IBAT1U
    "   mtspr   531, r0",    // IBAT1L
    "   mtspr   532, r0",    // IBAT2U
    "   mtspr   533, r0",    // IBAT2L
    "   mtspr   534, r0",    // IBAT3U
    "   mtspr   535, r0",    // IBAT3L
    "   mtspr   536, r0",    // DBAT0U
    "   mtspr   537, r0",    // DBAT0L
    "   mtspr   538, r0",    // DBAT1U
    "   mtspr   539, r0",    // DBAT1L
    "   mtspr   540, r0",    // DBAT2U
    "   mtspr   541, r0",    // DBAT2L
    "   mtspr   542, r0",    // DBAT3U
    "   mtspr   543, r0",    // DBAT3L
    "   isync",

    // ── IBAT0 / DBAT0: 256 MB at 0x80000000 → physical 0x00000000 ────────
    // BEPI=0x8000, BL=0x1FFF (256 MB), Vs=1, Vp=1
    // BRPN=0x0000, WIMG=0000 (cached, no guard), PP=2 (R/W)
    "   lis     r3, 0x8000",
    "   ori     r3, r3, 0x1FFF",         // BAT upper: BEPI|BL|Vs|Vp
    "   li      r4, 0x0002",             // BAT lower: BRPN|PP=2
    "   mtspr   528, r3",               // IBAT0U
    "   mtspr   529, r4",               // IBAT0L
    "   isync",
    "   mtspr   536, r3",               // DBAT0U
    "   mtspr   537, r4",               // DBAT0L
    "   isync",

    // ── DBAT1: 256 MB at 0xC0000000 → physical 0x00000000 ───────────────
    // Covers 0xC0000000-0xCFFFFFFF — uncached mirror + hardware registers.
    // BEPI=0xC000, BL=0x1FFF, Vs=1, Vp=1
    // BRPN=0x0000, WIMG=0101 (cache-inhibited, guarded), PP=2 (R/W)
    "   lis     r3, 0xC000",
    "   ori     r3, r3, 0x1FFF",
    "   li      r4, 0x002A",             // BRPN=0 | WIMG=0b0101 | PP=2
    "   mtspr   538, r3",               // DBAT1U
    "   mtspr   539, r4",               // DBAT1L
    "   isync",

    // ── Re-enable address translation and return ──────────────────────────
    "   mfmsr   r3",
    "   ori     r3, r3, 0x0030",         // set MSR_IR (bit 28) | MSR_DR (bit 27)
    "   lis     r4, __after_bat_init@ha",
    "   addi    r4, r4, __after_bat_init@l",
    "   mtsrr0  r4",                     // SRR0 = __after_bat_init (virtual address)
    "   mtsrr1  r3",                     // SRR1 = MSR with IR|DR set
    "   rfi",                            // return to virtual mode

    // End of .crt0 section
    ".size _start, . - _start",
    ".previous",
);

// ── Wii MEM2 BAT setup ────────────────────────────────────────────────────────
//
// When compiled with `--features wii`, this block initialises DBAT2 and DBAT3
// to map the Wii's 64 MB MEM2 (physical 0x10000000):
//   DBAT2: cached   0x90000000 → 0x10000000
//   DBAT3: uncached 0xD0000000 → 0x10000000
//
// These BATs run in real mode immediately after the normal BAT init sequence,
// before address translation is re-enabled (they are placed in .crt0 after _start
// and are reached via fall-through from __init_bats_realmode).
//
// On GameCube (without the wii feature) this block compiles to nothing.

#[cfg(feature = "wii")]
use core::arch::global_asm;

#[cfg(feature = "wii")]
global_asm!(
    ".section .crt0, \"ax\", @progbits",
    ".globl __wii_mem2_bats",
    ".type  __wii_mem2_bats, @function",
    "__wii_mem2_bats:",

    // DBAT2: cached MEM2 — 0x90000000 → physical 0x10000000, 64 MB
    // BL=0x07FF (64 MB - 1 blk), Vs=1, Vp=1
    "   lis     r3, 0x9000",
    "   ori     r3, r3, 0x07FF",         // upper: BEPI|BL|Vs|Vp
    "   lis     r4, 0x1000",             // lower: BRPN=0x10000000
    "   ori     r4, r4, 0x0002",         // WIMG=0000 (cached), PP=2
    "   mtspr   540, r3",               // DBAT2U
    "   mtspr   541, r4",               // DBAT2L
    "   isync",

    // DBAT3: uncached MEM2 — 0xD0000000 → physical 0x10000000, 64 MB
    "   lis     r3, 0xD000",
    "   ori     r3, r3, 0x07FF",
    "   lis     r4, 0x1000",
    "   ori     r4, r4, 0x002A",         // WIMG=0101 (uncached+guarded), PP=2
    "   mtspr   542, r3",               // DBAT3U
    "   mtspr   543, r4",               // DBAT3L
    "   isync",

    "   blr",
    ".size __wii_mem2_bats, . - __wii_mem2_bats",
    ".previous",
);
