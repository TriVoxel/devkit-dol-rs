/* wii.ld — Wii (Broadway) DOL/ELF linker script
 *
 * Memory map differences from GameCube:
 *   MEM1: 24 MB at 0x80000000 (same as GC)
 *   MEM2: 64 MB at 0x90000000 (Wii-only; MEM1 uncached = 0xC0000000,
 *                               MEM2 uncached = 0xD0000000)
 *   MMIO: 0xCD000000 prefix (vs 0xCC000000 on GC)
 *   IPC:  0xCD000000 (Starlet ARM coprocessor comms)
 *
 * The Wii IPL loads homebrew at 0x80004000 (HBC entrypoint) or
 * 0x80003100 if launched via standard DOL loader.
 *
 * This script targets the HBC/standard loader address.
 */

OUTPUT_FORMAT("elf32-powerpc", "elf32-powerpc", "elf32-powerpc")
OUTPUT_ARCH(powerpc)
ENTRY(_start)

/* ── Memory Regions ─────────────────────────────────────────────────────── */
MEMORY {
    /* 24 MB MEM1, same as GC. DOL loads at 0x80003100. */
    MEM1 (rwx) : ORIGIN = 0x80003100, LENGTH = (0x01800000 - 0x3100 - 0x20000)

    /* Stack: 128 KB at top of MEM1 (Wii has the same MEM1 top) */
    STACK (rw) : ORIGIN = 0x817E0000, LENGTH = 0x00020000

    /* 64 MB MEM2 (Wii-only, available after IOS initialisation).
     * First 4 MB are used by IOS; homebrew gets the rest.
     * The MEM2 linker region is declared here but unused until the app
     * explicitly allocates from it via a MEM2 heap. */
    MEM2 (rwx) : ORIGIN = 0x90400000, LENGTH = (0x04000000 - 0x400000)
}

/* ── Symbols ─────────────────────────────────────────────────────────────── */
__mem1_start    = 0x80000000;
__mem1_end      = 0x81800000;
__mem2_start    = 0x90000000;
__mem2_end      = 0x94000000;
__stack_top     = 0x817FFFF0;
__stack_bottom  = 0x817E0000;

_SDA_BASE_      = 0;
_SDA2_BASE_     = 0;

/* ── Sections ────────────────────────────────────────────────────────────── */
SECTIONS {

    .crt0 ORIGIN(MEM1) : {
        KEEP(*(.crt0 .crt0.*))
    } > MEM1

    .text ALIGN(32) : {
        *(.text .text.*)
        *(.gnu.linkonce.t.*)
        . = ALIGN(32);
    } > MEM1

    .rodata ALIGN(32) : {
        *(.rodata .rodata.*)
        *(.gnu.linkonce.r.*)
        . = ALIGN(32);
    } > MEM1

    .sdata2 ALIGN(32) : {
        _SDA2_BASE_ = . + 0x8000;
        *(.sdata2 .sdata2.*)
        *(.sbss2  .sbss2.*)
        . = ALIGN(32);
    } > MEM1

    .data ALIGN(32) : {
        *(.data .data.*)
        *(.gnu.linkonce.d.*)
        . = ALIGN(32);
    } > MEM1

    .sdata ALIGN(32) : {
        _SDA_BASE_ = . + 0x8000;
        *(.sdata .sdata.*)
        . = ALIGN(32);
    } > MEM1

    .bss ALIGN(32) (NOLOAD) : {
        __bss_start = .;
        *(.bss .bss.*)
        *(.gnu.linkonce.b.*)
        . = ALIGN(32);
        __bss_end = .;
    } > MEM1

    .sbss ALIGN(32) (NOLOAD) : {
        __sbss_start = .;
        *(.sbss .sbss.*)
        . = ALIGN(32);
        __sbss_end = .;
    } > MEM1

    _end         = .;
    __heap_start = _end;
    __heap_end   = __stack_bottom;

    /DISCARD/ : {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
        *(.ARM.exidx*)
    }
}
