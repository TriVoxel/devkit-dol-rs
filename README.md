# DevKit DOL RS

**A pure-Rust devkit for Nintendo GameCube (and Wii) homebrew development.**

Write GameCube applications entirely in Rust — no `devkitPPC`, no `libogc2`,
no C toolchain required. Everything from the boot vector to hardware register
access is implemented in Rust targeting the PowerPC 750CXe (Gekko) directly.

---

## Status

| Milestone | Description                                   | Status |
|-----------|-----------------------------------------------|--------|
| 0         | Scaffold, boot, VI, text console              | ✅     |
| 1         | Exceptions, timer, IRQ, heap                  | ✅     |
| 2         | Controller input (SI/PAD)                     | ✅     |
| 3         | GX GPU pipeline, 3D rendering                 | ✅     |
| 4         | Audio (AI DMA, DSP mailbox, EXI bus)          | ✅     |
| 5         | Storage (SD card via EXI, DVD drive)          | ✅     |
| 6         | Wii extensions                                | 🔴     |
| 7         | `cargo-gc` tooling                            | 🔴     |

---

## Crates

| Crate       | Purpose                                                    | Status |
|-------------|------------------------------------------------------------|--------|
| `gc-rt`     | Boot vector, exception table, IRQ, timer, BSS init         | ✅     |
| `gc-hal`    | Hardware drivers: VI, GX, SI, PI, AI, DSP, EXI, SD, DVD   | ✅     |
| `gc-gfx`    | XFB framebuffer, YCbCr helpers, 8×8 font, text console     | ✅     |
| `gc-alloc`  | `GlobalAllocator` — first-fit linked-list over MEM1 heap   | ✅     |
| `elf2dol`   | Host tool: converts ELF binary to `.dol` format            | ✅     |
| `cargo-gc`  | `cargo gc build/run/dol` subcommand                        | 🔴     |

### gc-hal subsystem status

| Module | Hardware                         | Status                      |
|--------|----------------------------------|-----------------------------|
| `vi`   | Video Interface                  | ✅ NTSC/PAL, flush           |
| `pi`   | Processor Interface              | ✅ All 27 IRQ masks           |
| `si`   | Serial Interface (controllers)   | ✅ Sync polling, 4 ports     |
| `gx`   | Graphics Processor               | ✅ Full 3D pipeline           |
| `ai`   | Audio Interface (DMA streaming)  | ✅ DMA + IRQ callback         |
| `dsp`  | Audio DSP                        | ✅ Reset, mailbox, interrupt  |
| `exi`  | External Interface (SPI bus)     | ✅ Imm + DMA                  |
| `sd`   | SD card via EXI (SD Gecko)       | ✅ Read/write, CRC, SDHC      |
| `dvd`  | DVD drive                        | ✅ Read, seek, disc ID        |

---

## Architecture

```
Your Rust App
      │
      ├── gc-hal          Hardware abstraction layer
      │     ├── vi         Video Interface — NTSC/PAL timing, XFB pointer
      │     ├── gx         GX GPU — write-gather FIFO, matrices, TEV, draw
      │     ├── si         Serial Interface — controller polling
      │     ├── pi         Processor Interface — interrupt controller
      │     ├── ai         Audio Interface — DMA streaming, IRQ callback
      │     ├── dsp        Audio DSP — reset, mailbox, task control
      │     ├── exi        External Interface — SPI bus, imm/DMA
      │     ├── sd         SD card via EXI SD Gecko — read/write sectors
      │     └── dvd        DVD drive — read, seek, disc ID
      │
      ├── gc-gfx          CPU-side framebuffer graphics
      │     ├── Xfb        YCbCr 4:2:2 framebuffer wrapper
      │     ├── Console    Scrolling text console, 8×8 bitmap font
      │     └── color      Named YCbCr colour constants
      │
      ├── gc-alloc        Heap allocator
      │     └── GcAllocator  First-fit linked list, 32-byte aligned blocks
      │
      └── gc-rt           Bare-metal runtime
            ├── _start     Boot: BATs, FPU, cache, BSS → main
            ├── exception  15 PPC vectors, full context save/restore
            ├── irq        MSR[EE] critical sections
            └── timer      Decrementer, TBR, delay
```

### SD card pipeline

```
SD/SDHC card → SD Gecko adapter → memory card slot
  → EXI channel 0 (slot A) or 1 (slot B)
  → exi::select/imm/deselect (SPI mode, 1–8 MHz)
  → sd::SdCard::read_sector (CMD17 → 0xFE → 512 bytes + CRC16)
```

### DVD pipeline

```
GC disc → DVD drive spindle + laser
  → DI registers (0xCC006000)
  → dvd::read(buf, len, offset)   [DICMD0=0xA8, DIMAR, DICR=DMA|START]
  → DMA into MEM1 buffer
  → TC interrupt → dvd::wait_ready()
```

### Memory map

```
0x80000000  MEM1 — 24 MB, cached
0x80000100  Exception vector stubs (15 × 128 bytes)
0x80003100  DOL load address
0x817FFFFF  Stack top
0xC0000000  MEM1 uncached mirror
0xCC000000  CP   — GX command processor registers (16-bit)
0xCC002000  VI   — video interface registers (16-bit)
0xCC003000  PI   — processor interface / interrupt controller (32-bit)
0xCC005000  DSP  — audio DSP + AI DMA + ARAM DMA (16-bit)
0xCC006000  DI   — DVD interface (32-bit)
0xCC006400  SI   — serial interface (controllers) (32-bit)
0xCC006800  EXI  — external interface (32-bit, 3 channels × 5 regs)
0xCC006C00  AI   — audio interface streaming (32-bit)
0xCC008000  GX   — write-gather pipe (GPU FIFO entry)
```

---

## Quick Start

```sh
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly

# Build any example
cargo +nightly build \
  -Z build-std=core,compiler_builtins \
  -Z build-std-features=compiler-builtins-mem \
  --target targets/powerpc-gekko-eabi.json \
  -p spinning_triangle --release

cargo run -p elf2dol -- \
  target/powerpc-gekko-eabi/release/spinning_triangle \
  spinning_triangle.dol

dolphin-emu -e spinning_triangle.dol
```

### Examples

| Example              | What it shows                                                |
|----------------------|--------------------------------------------------------------|
| `hello_world`        | NTSC 480i init, XFB clear, text console with colour          |
| `controller_test`    | All 4 ports live: buttons, analog sticks, trigger fill bars  |
| `spinning_triangle`  | GX 3D pipeline, perspective projection, double-buffer        |
| `sine_wave`          | AI DMA audio, double-buffering, IRQ callback, 440 Hz tone    |
| `sd_reader`          | SD card init (SD Gecko), sector read, MBR hex dump           |

---

## Why Nightly Rust?

1. **`-Z build-std`** (Cargo unstable): required to compile `core` from source
   for a custom target JSON. No stable equivalent exists.
2. **`#![feature(asm_experimental_arch)]`**: PowerPC inline `asm!` blocks
   require this gate (PPC is not a tier-1 Rust asm target).

Same requirement as `cortex-m`, Embassy, `rp2040-hal`, and every other
bare-metal Rust project targeting a non-tier-1 architecture.

---

## References

- [YAGCD — Yet Another GameCube Documentation](https://www.gc-forever.com/yagcd/)
- [WiiBrew Wiki](https://wiibrew.org/wiki/Main_Page)
- [Dolphin Emulator](https://dolphin-emu.org/)
- libogc2 — reference implementation (not linked or included)

---

## License

MIT OR Apache-2.0.
