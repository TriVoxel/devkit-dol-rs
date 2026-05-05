//! Boot sequence for the GameCube (Gekko/Broadway).
//!
//! This module emits the `.crt0` section via `global_asm!`. The linker script
//! places `.crt0` first so `_start` is at the DOL entry point.
//!
//! ## Sequence
//!
//! 1. Save link register, jump to real-mode BAT init (`__init_bats`)
//! 2. Initialise all GPRs to zero, set up stack pointer
//! 3. Set small data area base registers (2 = .sdata2, 13 = .sdata)
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

    "   mflr    0",                        // save LR (we'll restore after BAT init)
    "   bl      1f",                        // get PC into 3 via bl trick
    "1:",
    "   mflr    3",
    "   mtlr    0",                        // restore original LR

    // Compute physical address of __init_bats_realmode
    "   lis     4, __init_bats_realmode@ha",
    "   addi    4, 4, __init_bats_realmode@l",
    "   rlwinm  4, 4, 0, 2, 31",         // strip top 2 bits (virtual→physical)
    "   mtsrr0  4",                        // SRR0 = jump target after rfi

    // Clear MSR[IR|DR] to disable address translation
    "   mfmsr   3",
    "   rlwinm  3, 3, 0, 28, 25",        // clear bits 4,5 (MSR_IR=4, MSR_DR=5)
    "   mtsrr1  3",                        // SRR1 = new MSR value
    "   rfi",                               // switch to real mode, jump to __init_bats_realmode

    // Execution continues in __init_bats_realmode (real mode, no translation)
    // That function sets up BATs and returns here (virtual mode) via rfi.

    // ── After BAT init — virtual mode restored ───────────────────────────
    ".globl __after_bat_init",
    "__after_bat_init:",

    // ── Step 2: Clear all GPRs to 0 ─────────────────────────────────────
    "   li      0,  0",
    "   li      3,  0",
    "   li      4,  0",
    "   li      5,  0",
    "   li      6,  0",
    "   li      7,  0",
    "   li      8,  0",
    "   li      9,  0",
    "   li      10, 0",
    "   li      11, 0",
    "   li      12, 0",
    "   li      14, 0",
    "   li      15, 0",
    "   li      16, 0",
    "   li      17, 0",
    "   li      18, 0",
    "   li      19, 0",
    "   li      20, 0",
    "   li      21, 0",
    "   li      22, 0",
    "   li      23, 0",
    "   li      24, 0",
    "   li      25, 0",
    "   li      26, 0",
    "   li      27, 0",
    "   li      28, 0",
    "   li      29, 0",
    "   li      30, 0",
    "   li      31, 0",

    // ── Step 3: Set up stack pointer ─────────────────────────────────────
    "   lis     1, __stack_top@ha",
    "   addi    1, 1, __stack_top@l",
    "   rlwinm  1, 1, 0, 0, 27",         // align to 16 bytes
    "   li      0, 0",
    "   stwu    0, -8(1)",               // push a null frame (ABI requirement)

    // ── Step 4: Set small data area base registers ───────────────────────
    "   lis     2,  _SDA2_BASE_@ha",
    "   addi    2,  2, _SDA2_BASE_@l",
    "   lis     13, _SDA_BASE_@ha",
    "   addi    13, 13, _SDA_BASE_@l",

    // ── Step 5: Enable FPU (MSR[FP] = bit 13) ───────────────────────────
    "   mfmsr   3",
    "   ori     3, 3, 0x2000",           // set MSR_FP
    "   mtmsr   3",
    "   isync",

    // ── Step 6: Initialise Paired Singles (HID2[PSE] = bit 0, WPE = bit 1) ──
    "   mfspr   3, 920",                  // 920 = HID2 SPR number
    "   oris    3, 3, 0xA000",           // set PSE (bit 16) and WPE (bit 17)
    "   mtspr   920, 3",
    "   isync",

    // ── Step 7: Zero all FPRs ────────────────────────────────────────────
    // Write an IEEE 0.0 to the stack below the null frame, then load it
    // into 0 and broadcast to 1-31 via fmr.
    // (Cannot use .rept/.set register arithmetic — LLVM assembler requires
    // explicit register names with the f-prefix.)
    "   li      3, 0",
    "   stw     3, -4(1)",
    "   lfs     0, -4(1)",
    "   fmr 1,  0",   "   fmr 2,  0",   "   fmr 3,  0",   "   fmr 4,  0",
    "   fmr 5,  0",   "   fmr 6,  0",   "   fmr 7,  0",   "   fmr 8,  0",
    "   fmr 9,  0",   "   fmr 10, 0",   "   fmr 11, 0",   "   fmr 12, 0",
    "   fmr 13, 0",   "   fmr 14, 0",   "   fmr 15, 0",   "   fmr 16, 0",
    "   fmr 17, 0",   "   fmr 18, 0",   "   fmr 19, 0",   "   fmr 20, 0",
    "   fmr 21, 0",   "   fmr 22, 0",   "   fmr 23, 0",   "   fmr 24, 0",
    "   fmr 25, 0",   "   fmr 26, 0",   "   fmr 27, 0",   "   fmr 28, 0",
    "   fmr 29, 0",   "   fmr 30, 0",   "   fmr 31, 0",

    // ── Step 8: Enable L1 caches ─────────────────────────────────────────
    // HID0 layout: bit 16=DCE, bit 15=ICE, bit 12=DCFI, bit 11=ICFI
    // Sequence: invalidate both caches, then enable them.
    "   mfspr   3, 1008",                 // 1008 = HID0 SPR
    "   ori     3, 3, 0x0C00",           // set DCFI (bit 12) + ICFI (bit 11)
    "   mtspr   1008, 3",
    "   isync",
    "   sync",
    "   ori     3, 3, 0xC000",           // set DCE (bit 16) + ICE (bit 15)
    "   mtspr   1008, 3",
    "   isync",

    // ── Step 9: Zero .sbss section ──────────────────────────────────────
    "   lis     3, __sbss_start@ha",
    "   addi    3, 3, __sbss_start@l",
    "   lis     5, __sbss_end@ha",
    "   addi    5, 5, __sbss_end@l",
    "   sub     5, 5, 3",               // 5 = byte count
    "   li      4, 0",
    "   bl      __bss_fill",

    // ── Step 10: Zero .bss section ──────────────────────────────────────
    "   lis     3, __bss_start@ha",
    "   addi    3, 3, __bss_start@l",
    "   lis     5, __bss_end@ha",
    "   addi    5, 5, __bss_end@l",
    "   sub     5, 5, 3",               // 5 = byte count
    "   li      4, 0",
    "   bl      __bss_fill",

    // ── Step 11: Branch to Rust main ────────────────────────────────────
    "   bl      main",
    "   b       __halt",                   // if main returns, halt

    // ──────────────────────────────────────────────────────────────────────
    // __bss_fill: memset(3, 4, 5) — fills 5 bytes at 3 with byte 4.
    // Simple word-at-a-time loop; fine for startup (not performance-critical).
    // ──────────────────────────────────────────────────────────────────────
    "__bss_fill:",
    "   cmpwi   5, 0",
    "   beqlr",                             // nothing to do
    "   mr      6, 3",
    "2:",
    "   stb     4, 0(6)",
    "   addi    6, 6, 1",
    "   subic.  5, 5, 1",
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
    "   lis     3, 0x0091",
    "   ori     3, 3, 0x0C64",
    "   mtspr   1008, 3",               // HID0 = 1008
    "   isync",

    // Clear all segment registers
    "   lis     0, 0x8000",
    "   mtsr    0,  0",
    "   mtsr    1,  0",
    "   mtsr    2,  0",
    "   mtsr    3,  0",
    "   mtsr    4,  0",
    "   mtsr    5,  0",
    "   mtsr    6,  0",
    "   mtsr    7,  0",
    "   mtsr    8,  0",
    "   mtsr    9,  0",
    "   mtsr    10, 0",
    "   mtsr    11, 0",
    "   mtsr    12, 0",
    "   mtsr    13, 0",
    "   mtsr    14, 0",
    "   mtsr    15, 0",
    "   isync",

    // Clear all BAT registers
    "   li      0, 0",
    "   mtspr   528, 0",    // IBAT0U
    "   mtspr   529, 0",    // IBAT0L
    "   mtspr   530, 0",    // IBAT1U
    "   mtspr   531, 0",    // IBAT1L
    "   mtspr   532, 0",    // IBAT2U
    "   mtspr   533, 0",    // IBAT2L
    "   mtspr   534, 0",    // IBAT3U
    "   mtspr   535, 0",    // IBAT3L
    "   mtspr   536, 0",    // DBAT0U
    "   mtspr   537, 0",    // DBAT0L
    "   mtspr   538, 0",    // DBAT1U
    "   mtspr   539, 0",    // DBAT1L
    "   mtspr   540, 0",    // DBAT2U
    "   mtspr   541, 0",    // DBAT2L
    "   mtspr   542, 0",    // DBAT3U
    "   mtspr   543, 0",    // DBAT3L
    "   isync",

    // ── IBAT0 / DBAT0: 256 MB at 0x80000000 → physical 0x00000000 ────────
    // BEPI=0x8000, BL=0x1FFF (256 MB), Vs=1, Vp=1
    // BRPN=0x0000, WIMG=0000 (cached, no guard), PP=2 (R/W)
    "   lis     3, 0x8000",
    "   ori     3, 3, 0x1FFF",         // BAT upper: BEPI|BL|Vs|Vp
    "   li      4, 0x0002",             // BAT lower: BRPN|PP=2
    "   mtspr   528, 3",               // IBAT0U
    "   mtspr   529, 4",               // IBAT0L
    "   isync",
    "   mtspr   536, 3",               // DBAT0U
    "   mtspr   537, 4",               // DBAT0L
    "   isync",

    // ── DBAT1: 256 MB at 0xC0000000 → physical 0x00000000 ───────────────
    // Covers 0xC0000000-0xCFFFFFFF — uncached mirror + hardware registers.
    // BEPI=0xC000, BL=0x1FFF, Vs=1, Vp=1
    // BRPN=0x0000, WIMG=0101 (cache-inhibited, guarded), PP=2 (R/W)
    "   lis     3, 0xC000",
    "   ori     3, 3, 0x1FFF",
    "   li      4, 0x002A",             // BRPN=0 | WIMG=0b0101 | PP=2
    "   mtspr   538, 3",               // DBAT1U
    "   mtspr   539, 4",               // DBAT1L
    "   isync",

    // ── Re-enable address translation and return ──────────────────────────
    "   mfmsr   3",
    "   ori     3, 3, 0x0030",         // set MSR_IR (bit 28) | MSR_DR (bit 27)
    "   lis     4, __after_bat_init@ha",
    "   addi    4, 4, __after_bat_init@l",
    "   mtsrr0  4",                     // SRR0 = __after_bat_init (virtual address)
    "   mtsrr1  3",                     // SRR1 = MSR with IR|DR set
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
    "   lis     3, 0x9000",
    "   ori     3, 3, 0x07FF",         // upper: BEPI|BL|Vs|Vp
    "   lis     4, 0x1000",             // lower: BRPN=0x10000000
    "   ori     4, 4, 0x0002",         // WIMG=0000 (cached), PP=2
    "   mtspr   540, 3",               // DBAT2U
    "   mtspr   541, 4",               // DBAT2L
    "   isync",

    // DBAT3: uncached MEM2 — 0xD0000000 → physical 0x10000000, 64 MB
    "   lis     3, 0xD000",
    "   ori     3, 3, 0x07FF",
    "   lis     4, 0x1000",
    "   ori     4, 4, 0x002A",         // WIMG=0101 (uncached+guarded), PP=2
    "   mtspr   542, 3",               // DBAT3U
    "   mtspr   543, 4",               // DBAT3L
    "   isync",

    "   blr",
    ".size __wii_mem2_bats, . - __wii_mem2_bats",
    ".previous",
);
