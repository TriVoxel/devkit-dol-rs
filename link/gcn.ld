/* gcn.ld — GameCube (Gekko) DOL linker script
 *
 * Memory map:
 *   0x80000000 - 0x80002FFF : OS globals, exception vector stubs (reserved)
 *   0x80003100 - 0x817FFFFF : Application code, data, BSS, heap
 *   0x817FFFFF              : Stack top (grows downward from here)
 *   0xC0000000              : MEM1 uncached mirror (same physical RAM)
 *   0xCC002000              : Video Interface (VI) registers
 *   0xCC003000              : Processor Interface (PI) registers
 *   ... (see WIP.md for full hardware register map)
 *
 * DOL format notes:
 *   A .dol file has up to 7 text sections and 11 data sections, each with a
 *   load address, file offset, and size. The IPL/loader copies each section
 *   to its physical address and then jumps to the entry point.
 *   The entry point is _start (defined in dkdol-rt).
 */

OUTPUT_FORMAT("elf32-powerpc", "elf32-powerpc", "elf32-powerpc")
OUTPUT_ARCH(powerpc)
ENTRY(_start)

/* ── Memory Regions ────────────────────────────────────────────────────── */
MEMORY {
    /* 24 MB MEM1 (cached). First 0x3100 bytes are OS-reserved.
     * We load our DOL starting at 0x80003100.
     * The stack lives at the very top of MEM1 and grows downward. */
    MEM1 (rwx) : ORIGIN = 0x80003100, LENGTH = (0x01800000 - 0x3100 - 0x10000)

    /* 64 KB stack reservation at the top of MEM1 */
    STACK (rw) : ORIGIN = 0x817F0000, LENGTH = 0x00010000
}

/* ── Symbols referenced by dkdol-rt boot code ─────────────────────────────── */
__mem1_start    = 0x80000000;
__mem1_end      = 0x81800000;
__stack_top     = 0x817FFFF0;   /* aligned to 16 bytes; stack grows down */
__stack_bottom  = 0x817F0000;

_SDA_BASE_      = 0;            /* small data area base (r13); patched by runtime */
_SDA2_BASE_     = 0;            /* read-only small data area base (r2) */

/* ── Sections ───────────────────────────────────────────────────────────── */
SECTIONS {

    /* ── .crt0: Entry point — must be first ── */
    .crt0 ORIGIN(MEM1) : {
        KEEP(*(.crt0 .crt0.*))
    } > MEM1

    /* ── .text: Code ── */
    .text ALIGN(32) : {
        *(.text .text.*)
        *(.gnu.linkonce.t.*)
        . = ALIGN(32);
    } > MEM1

    /* ── .rodata: Read-only data ── */
    .rodata ALIGN(32) : {
        *(.rodata .rodata.*)
        *(.gnu.linkonce.r.*)
        . = ALIGN(32);
    } > MEM1

    /* ── Small read-only data (r2-relative) ── */
    .sdata2 ALIGN(32) : {
        _SDA2_BASE_ = . + 0x8000;
        *(.sdata2 .sdata2.*)
        *(.sbss2 .sbss2.*)
        . = ALIGN(32);
    } > MEM1

    /* ── .data: Initialised read-write data ── */
    .data ALIGN(32) : {
        *(.data .data.*)
        *(.gnu.linkonce.d.*)
        . = ALIGN(32);
    } > MEM1

    /* ── Small read-write data (r13-relative) ── */
    .sdata ALIGN(32) : {
        _SDA_BASE_ = . + 0x8000;
        *(.sdata .sdata.*)
        . = ALIGN(32);
    } > MEM1

    /* ── .bss: Zeroed at startup by dkdol-rt ── */
    .bss ALIGN(32) : {
        __bss_start = .;
        *(.bss .bss.*)
        *(.gnu.linkonce.b.*)
        . = ALIGN(32);
        __bss_end = .;
    } > MEM1

    /* ── Small BSS ── */
    .sbss ALIGN(32) : {
        __sbss_start = .;
        *(.sbss .sbss.*)
        . = ALIGN(32);
        __sbss_end = .;
    } > MEM1

    /* ── End of used memory ── */
    _end = .;
    __heap_start = _end;
    __heap_end   = __stack_bottom;

    /* ── Discard noise ── */
    /DISCARD/ : {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
        *(.ARM.exidx*)
    }
}
