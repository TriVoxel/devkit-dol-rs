# DevKit DOL

**A pure-Rust devkit for Nintendo GameCube and Wii homebrew development.**

No `devkitPPC`. No `libogc2`. No C toolchain. Everything from the boot vector
to hardware register access is written in Rust targeting the PowerPC 750CXe
(Gekko/Broadway) directly.

---

## Status

| Milestone | Description                                   | Status |
|-----------|-----------------------------------------------|--------|
| 0         | Scaffold, boot, VI, text console              | ✅     |
| 1         | Exceptions, timer, IRQ, heap allocator        | ✅     |
| 2         | Controller input (SI/PAD)                     | ✅     |
| 3         | GX GPU pipeline, 3D rendering                 | ✅     |
| 4         | Audio (AI DMA, DSP mailbox, EXI bus)          | ✅     |
| 5         | All storage (SD/SP2/MemCard/DVD/ODE)          | ✅     |
| 6         | Wii extensions (Broadway, MEM2, MMIO switch)  | ✅     |
| 7         | `cargo-dkdol` tooling                            | ✅     |

---

## Quick Start

```sh
# Install the build tool
cargo install --path tools/cargo-dkdol

# Run an example on GameCube
cargo dkdol run --release --example spinning_triangle

# Run an example on Wii
cargo dkdol run --release --example hello_world --wii

# Create a new project
cargo dkdol new my_game && cd my_game
cargo dkdol run --release --example hello
```

---

## Crates

| Crate       | Purpose                                                    | Status |
|-------------|------------------------------------------------------------|--------|
| `gc-rt`     | Boot, exception table, IRQ, timer, BSS init                | ✅     |
| `gc-hal`    | Hardware drivers for every GC/Wii subsystem                | ✅     |
| `gc-gfx`    | XFB framebuffer, YCbCr helpers, 8×8 font, text console     | ✅     |
| `gc-alloc`  | `GlobalAllocator` — first-fit linked-list over MEM1/MEM2   | ✅     |
| `elf2dol`   | Host tool: ELF → DOL converter                             | ✅     |
| `cargo-dkdol`  | `cargo dkdol build/run/dol/new` subcommand                    | ✅     |

---

## gc-hal subsystems

| Module    | Hardware                          | GC  | Wii  |
|-----------|-----------------------------------|-----|------|
| `vi`      | Video Interface                   | ✅  | ✅   |
| `pi`      | Interrupt controller              | ✅  | ✅   |
| `si`      | Serial Interface (controllers)    | ✅  | ✅   |
| `gx`      | Graphics Processor (FIFO)         | ✅  | ✅   |
| `ai`      | Audio DMA streaming               | ✅  | ✅   |
| `dsp`     | Audio DSP mailbox                 | ✅  | ✅   |
| `exi`     | External Interface (SPI bus)      | ✅  | ✅   |
| `sd`      | SD card — Slot A, B, SP2         | ✅  | ✅   |
| `memcard` | GC Memory Card — Slot A, B       | ✅  | ✅   |
| `dvd`     | DVD drive / ODE                   | ✅  | ✅   |
| `storage` | `BlockDevice` trait + auto-scan   | ✅  | ✅   |
| `mmio`    | MMIO base: 0xCC (GC) / 0xCD (Wii)| ✅  | ✅   |
| `mem2`    | Wii MEM2 (64 MB extended RAM)     | —   | ✅   |

All hardware modules switch MMIO prefix automatically when built
with `--features gc-hal/wii` (or `cargo dkdol build --wii`).

---

## Platform differences

| Feature           | GameCube (Gekko)    | Wii (Broadway)          |
|-------------------|---------------------|-------------------------|
| CPU clock         | 486 MHz             | 729 MHz                 |
| Bus clock         | 162 MHz             | 243 MHz                 |
| TBR/DEC tick rate | 40.5 MHz            | 60.75 MHz               |
| MEM1              | 24 MB @ 0x80000000  | 24 MB @ 0x80000000      |
| MEM2              | —                   | 64 MB @ 0x90000000      |
| MMIO prefix       | 0xCC000000          | 0xCD000000              |
| Extra coprocessor | —                   | Starlet (ARM926EJ-S)    |
| Target spec       | powerpc-gekko-eabi  | powerpc-broadway-eabi   |
| Linker script     | link/gcn.ld         | link/wii.ld             |

---

## Storage device support

| Device            | Port              | Driver            | Notes                  |
|-------------------|-------------------|-------------------|------------------------|
| SD Gecko          | Slot A (EXI 0)    | `gc-hal::sd`      |                        |
| SD Gecko          | Slot B (EXI 1)    | `gc-hal::sd`      |                        |
| SD2SP2            | SP2 (EXI 2)       | `gc-hal::sd`      | Serial Port 2 adapter  |
| GC Memory Card    | Slot A/B          | `gc-hal::memcard` |                        |
| MemCard PRO GC    | Slot A/B          | `gc-hal::memcard` |                        |
| DVD drive         | DI registers      | `gc-hal::dvd`     |                        |
| **CubeODE**       | DI registers      | `gc-hal::dvd` ✓   | Transparent ODE        |
| **GCLoader**      | DI registers      | `gc-hal::dvd` ✓   | Transparent ODE        |
| **Flippy Drive**  | DI registers      | `gc-hal::dvd` ✓   | Transparent ODE        |

---

## cargo-dkdol subcommands

```
cargo dkdol build [--release] [-p <pkg>] [--example <name>] [--wii]
    Cross-compile and convert ELF → DOL.

cargo dkdol dol <elf> [output.dol]
    Convert an existing ELF binary to DOL format.

cargo dkdol run [--release] [-p <pkg>] [--example <name>]
            [--dolphin <path>] [--wii]
    Build, convert, then launch in Dolphin Emulator.

cargo dkdol new <project-name>
    Scaffold a complete new GC/Wii project.

cargo dkdol help [<subcommand>]
    Detailed help.
```

### Project config (`Cargo.toml`)

```toml
[package.metadata.gc]
dolphin     = "dolphin-emu"    # Dolphin executable (default: dolphin-emu)
dol_out_dir = "."              # Where to write .dol files
target_gc   = "targets/powerpc-gekko-eabi.json"
target_wii  = "targets/powerpc-broadway-eabi.json"
```

---

## Architecture

```
Your Rust App
      │
      ├── gc-hal::mmio        MMIO base (0xCC or 0xCD, compile-time feature)
      │
      ├── gc-hal              Hardware abstraction layer
      │     ├── vi            VI — NTSC/PAL timing, XFB pointer
      │     ├── gx            GPU — FIFO, matrices, TEV, draw calls
      │     ├── si            Controllers — sync poll, 4 ports
      │     ├── pi            Interrupt controller — 27 IRQ sources
      │     ├── ai            Audio DMA — stereo PCM streaming
      │     ├── dsp           Audio DSP — mailbox, task control
      │     ├── exi           SPI bus — imm/DMA, device ID detection
      │     ├── sd            SD card — Slot A/B/SP2, SDHC, CRC
      │     ├── memcard       Memory card — page read/write/erase
      │     ├── dvd           DVD/ODE — sector DMA, disc ID
      │     ├── storage       BlockDevice trait + auto-scan all slots
      │     └── mem2          Wii MEM2 constants (wii feature only)
      │
      ├── gc-gfx              CPU framebuffer: XFB, Console, color
      ├── gc-alloc            MEM1 heap (first-fit, 32-byte aligned)
      └── gc-rt               Bare-metal runtime
            ├── _start        Boot: BATs (+ MEM2 BATs on Wii), FPU, BSS
            ├── exception     15 PPC vectors, full ExcCtx save/restore
            ├── irq           MSR[EE] critical sections
            └── timer         Decrementer, TBR, delay
```

---

## Memory maps

### GameCube

```
0x80000000  MEM1 start (24 MB, cached)
0x80000100  Exception stubs (15 × 128 B, installed at boot)
0x80003100  DOL load address
0x817FFFF0  Stack top
0xC0000000  MEM1 uncached mirror
0xCC000000  Hardware registers (MMIO)
  0xCC002000  VI — video interface
  0xCC003000  PI — interrupt controller
  0xCC005000  DSP — audio DSP + AI/ARAM DMA
  0xCC006000  DI — DVD interface
  0xCC006400  SI — serial interface (controllers)
  0xCC006800  EXI — external interface (SPI bus)
  0xCC006C00  AI — audio streaming
  0xCC008000  GX — write-gather pipe (GPU FIFO)
```

### Wii (additional)

```
0x90000000  MEM2 start (64 MB, cached)
0x90400000  IOS reservation end (~4 MB); homebrew heap starts here
0x93FFFFFF  MEM2 end
0xCD000000  Wii MMIO (same registers, 0xCD prefix instead of 0xCC)
0xD0000000  MEM2 uncached mirror
```

---

## Why Nightly Rust?

1. **`-Z build-std`** — required to compile `core` for a custom target JSON.
2. **`#![feature(asm_experimental_arch)]`** — PowerPC inline `asm!` requires this gate.

Same requirement as `cortex-m`, Embassy, `rp2040-hal`, and all other
bare-metal Rust projects on non-tier-1 architectures.

---

## References

- [YAGCD — Yet Another GameCube Documentation](https://www.gc-forever.com/yagcd/)
- [WiiBrew Wiki](https://wiibrew.org/wiki/Main_Page)
- [Dolphin Emulator](https://dolphin-emu.org/)
- libogc2 — reference implementation (not linked or included)

---

## License

MIT OR Apache-2.0.
