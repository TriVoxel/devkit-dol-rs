# DevKit DOL RS

**A pure-Rust devkit for Nintendo GameCube (and Wii) homebrew development.**

No `devkitPPC`. No `libogc2`. No C toolchain. Everything from the boot vector
to hardware register access is written in Rust, targeting the PowerPC 750CXe
(Gekko) directly.

---

## Status

| Milestone | Description                                   | Status |
|-----------|-----------------------------------------------|--------|
| 0         | Scaffold, boot, VI, text console              | ✅     |
| 1         | Exceptions, timer, IRQ, heap allocator        | ✅     |
| 2         | Controller input (SI/PAD)                     | ✅     |
| 3         | GX GPU pipeline, 3D rendering                 | ✅     |
| 4         | Audio (AI DMA, DSP mailbox, EXI bus)          | ✅     |
| 5         | All storage devices (SD, SP2, MC, DVD, ODE)   | ✅     |
| 6         | Wii extensions                                | 🔴     |
| 7         | `cargo-gc` tooling                            | 🔴     |

---

## Crates

| Crate       | Purpose                                                    | Status |
|-------------|------------------------------------------------------------|--------|
| `gc-rt`     | Boot vector, exception table, IRQ, timer, BSS init         | ✅     |
| `gc-hal`    | Hardware drivers for every GC subsystem                    | ✅     |
| `gc-gfx`    | XFB framebuffer, YCbCr helpers, 8×8 font, text console     | ✅     |
| `gc-alloc`  | `GlobalAllocator` — first-fit linked-list over MEM1 heap   | ✅     |
| `elf2dol`   | Host tool: converts ELF binary to `.dol` format            | ✅     |
| `cargo-gc`  | `cargo gc build/run/dol` subcommand                        | 🔴     |

### gc-hal subsystem status

| Module      | Hardware                            | Status                      |
|-------------|-------------------------------------|-----------------------------|
| `vi`        | Video Interface                     | ✅ NTSC/PAL, flush           |
| `pi`        | Processor Interface (interrupts)    | ✅ All 27 IRQ masks           |
| `si`        | Serial Interface (controllers)      | ✅ Sync poll, 4 ports         |
| `gx`        | Graphics Processor                  | ✅ Full 3D pipeline           |
| `ai`        | Audio Interface (DMA streaming)     | ✅ DMA + IRQ callback         |
| `dsp`       | Audio DSP                           | ✅ Reset, mailbox, interrupt  |
| `exi`       | External Interface + device ID      | ✅ Imm + DMA + `get_id()`     |
| `sd`        | SD card (Slot A, Slot B, SP2)       | ✅ Read/write, CRC, SDHC      |
| `memcard`   | GC Memory Card (Slot A, Slot B)     | ✅ Read/write/erase           |
| `dvd`       | DVD drive / ODE                     | ✅ Read, seek, disc ID        |
| `storage`   | Unified `BlockDevice` + scanner     | ✅ All devices                |

---

## Storage Device Support

| Device                  | Slot / Port    | Driver            |
|-------------------------|----------------|-------------------|
| SD Gecko SD card        | Slot A (EXI 0) | `gc-hal::sd`      |
| SD Gecko SD card        | Slot B (EXI 1) | `gc-hal::sd`      |
| SD2SP2 SD card          | SP2 (EXI 2)    | `gc-hal::sd`      |
| GC Memory Card          | Slot A (EXI 0) | `gc-hal::memcard` |
| GC Memory Card          | Slot B (EXI 1) | `gc-hal::memcard` |
| MemCard PRO GC          | Slot A or B    | `gc-hal::memcard` |
| DVD (real drive)        | DI registers   | `gc-hal::dvd`     |
| **CubeODE**             | DI registers   | `gc-hal::dvd` ✓   |
| **GCLoader**            | DI registers   | `gc-hal::dvd` ✓   |
| **Flippy Drive**        | DI registers   | `gc-hal::dvd` ✓   |
| BBA / IDE-EXI           | Detected by ID | stub              |

ODEs (CubeODE, GCLoader, Flippy) impersonate the DVD drive at the hardware
register level. Our `dvd` driver is completely transparent to them — no special
handling needed.

---

## Architecture

```
Your Rust App
      │
      ├── gc-hal            Hardware abstraction layer
      │     ├── vi           Video Interface — NTSC/PAL, XFB
      │     ├── gx           GX GPU — FIFO, matrices, TEV, draw calls
      │     ├── si           Controllers — sync poll, 4 ports
      │     ├── pi           Interrupt controller — 27 IRQ sources
      │     ├── ai           Audio DMA — stereo PCM streaming
      │     ├── dsp          Audio DSP — reset, mailbox
      │     ├── exi          SPI bus — imm/DMA + device ID
      │     ├── sd           SD card — Slot A/B/SP2, SDHC
      │     ├── memcard      Memory card — page read/write/erase
      │     ├── dvd          DVD drive — sector DMA, disc ID
      │     └── storage      BlockDevice trait + auto-scan
      │
      ├── gc-gfx            CPU framebuffer graphics
      │     ├── Xfb          YCbCr 4:2:2 wrapper
      │     ├── Console      Scrolling text, 8×8 bitmap font
      │     └── color        Named YCbCr constants
      │
      ├── gc-alloc          Heap allocator (first-fit, 32-byte aligned)
      └── gc-rt             Bare-metal runtime
            ├── _start       Boot: BATs, FPU, cache, BSS → main
            ├── exception    15 PPC vectors, full context save/restore
            ├── irq          MSR[EE] critical sections
            └── timer        Decrementer, TBR, delay
```

### Storage scan flow

```
gc_hal::storage::scan()
  ├── EXI Ch0 (Slot A): get_id() → memcard? → MemCard::probe()
  │                              → other?   → SdCard::init()
  ├── EXI Ch1 (Slot B): same
  ├── EXI Ch2 (SP2):    SdCard::init() (no EXT bit, always attempt)
  └── DVD drive:        dvd::init() → cover_open()? → read_disc_id()
```

### Memory map

```
0x80000000  MEM1 — 24 MB, cached
0x80000100  Exception vector stubs (15 × 128 bytes, installed at boot)
0x80003100  DOL load address
0x817FFFFF  Stack top
0xC0000000  MEM1 uncached mirror
0xCC000000  CP   — GX command processor (16-bit)
0xCC002000  VI   — video interface (16-bit)
0xCC003000  PI   — interrupt controller (32-bit)
0xCC005000  DSP  — audio DSP + AI DMA + ARAM DMA (16-bit)
0xCC006000  DI   — DVD interface (32-bit)
0xCC006400  SI   — serial interface / controllers (32-bit)
0xCC006800  EXI  — external interface, 3 channels (32-bit)
0xCC006C00  AI   — audio interface streaming (32-bit)
0xCC008000  GX   — write-gather pipe / GPU FIFO entry
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
  -p storage_detect --release

# Convert to DOL
cargo run -p elf2dol -- \
  target/powerpc-gekko-eabi/release/storage_detect \
  storage_detect.dol

# Run in Dolphin
dolphin-emu -e storage_detect.dol
```

### Examples

| Example            | What it shows                                              |
|--------------------|------------------------------------------------------------|
| `hello_world`      | NTSC 480i, XFB clear, coloured text console                |
| `controller_test`  | All 4 ports: buttons, analog sticks, trigger fill bars     |
| `spinning_triangle`| GX 3D pipeline, perspective, Y-rotation, double-buffer     |
| `sine_wave`        | AI DMA audio, double-buffering, IRQ callback, 440 Hz tone  |
| `sd_reader`        | SD card init, read sector 0, MBR check, hex dump           |
| `storage_detect`   | Scans all storage (SD/SP2/memcard/DVD) + sector 0 preview  |

---

## Writing Your Own Application

### Minimum skeleton

```rust
#![no_std]
#![no_main]

use gc_hal::vi;

#[no_mangle]
pub extern "C" fn main() -> ! {
    unsafe {
        vi::init_ntsc_480i();
        loop {}
    }
}
```

### Using the storage scanner

```rust
use gc_hal::storage::{self, BlockDevice, StorageInfo};

static mut DEVICES: [StorageInfo; 8] = [/* zero init */];

unsafe fn find_storage() {
    let count = storage::scan(&mut DEVICES);
    for i in 0..count {
        let dev = &DEVICES[i];
        // dev.kind, dev.sector_count, dev.sector_size, dev.read_only
    }
}
```

### Reading a specific device

```rust
// SD card (any slot)
let mut card = gc_hal::sd::SdCard::new(gc_hal::sd::Slot::Sp2);
unsafe { card.init()?; }
unsafe { card.read_sector(0, &mut buf)?; }

// Memory card
let card = gc_hal::memcard::MemCard::probe(gc_hal::memcard::CardSlot::A)?;
unsafe { card.read_segment(0, &mut buf)?; }

// DVD disc
unsafe {
    gc_hal::dvd::init();
    gc_hal::dvd::read(buf.as_mut_ptr(), 2048, 0x20000)?;
}
```

---

## Why Nightly Rust?

1. **`-Z build-std`** (Cargo unstable): required to compile `core` for a
   custom target JSON. No stable equivalent.
2. **`#![feature(asm_experimental_arch)]`**: PowerPC inline `asm!` requires
   this gate (PPC is not a tier-1 Rust asm target).

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
