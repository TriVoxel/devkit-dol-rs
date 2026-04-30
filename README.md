# DevKit DOL RS

**A native Rust devkit for Nintendo GameCube (and Wii) development.**

`devkit-dol-rs` is a self-contained, pure-Rust toolchain for writing GameCube homebrew applications. It requires no C/C++ devkit, no `devkitPPC`, and no `libogc2`. Everything from the boot vector to hardware register access is written in Rust, targeting the GameCube's PowerPC 750CXe (Gekko) processor directly.

C and C++ code can still be incorporated through crates that use the `cc` crate (which invokes `clang` with a PPC target), but the devkit infrastructure itself is 100% Rust.

---

## Goals

- **Pure Rust toolchain** — No external C devkit required. `rustc` + `rust-lld` + this repo is all you need.
- **Feature parity with libogc2** — Full hardware coverage: Video (VI/GX), Audio (DSP/AI), Input (SI/PAD), Storage (DVD/EXI/SD), Networking, and more.
- **Safe, idiomatic Rust APIs** — Hardware subsystems exposed as safe Rust types where possible.
- **C/C++ interop** — Crates that bundle C code via `cc` crate work out of the box.
- **Dolphin-first iteration** — The Dolphin emulator is the primary testing target during development.
- **Wii support** — Broadway (Wii CPU) is a superset of Gekko. Wii-specific features will be gated behind a `wii` feature flag.

---

## Architecture

```
Your Rust App
      │
      ├── gc-hal          # Safe hardware abstraction layer
      │     ├── gc-hal::vi        Video Interface (display, framebuffer)
      │     ├── gc-hal::gx        Graphics Processor (GX FIFO, GPU)
      │     ├── gc-hal::si        Serial Interface (controllers)
      │     ├── gc-hal::exi       External Interface (memory card, SD)
      │     ├── gc-hal::dsp       Audio DSP
      │     ├── gc-hal::ai        Audio Interface (streaming)
      │     ├── gc-hal::dvd       DVD drive interface
      │     └── gc-hal::pi        Processor Interface (interrupts, resets)
      │
      ├── gc-gfx          # Framebuffer graphics & text console
      ├── gc-alloc        # Heap allocator (bump/linked-list over MEM1/MEM2)
      └── gc-rt           # Runtime: boot vector, exception table, BSS init
            │
            └── (links against gcn.ld linker script)
                          Memory layout for GameCube DOL executables
```

### Crate Map

| Crate | Purpose | Status |
|---|---|---|
| `gc-rt` | Boot assembly, exception vectors, BSS init, panic handler | 🟡 WIP |
| `gc-hal` | Hardware register access and subsystem drivers | 🟡 WIP |
| `gc-gfx` | Framebuffer text console, 2D drawing primitives | 🟡 WIP |
| `gc-alloc` | `GlobalAllocator` implementation over MEM1 | 🔴 Stub |
| `elf2dol` | Host tool: converts linked ELF binary to `.dol` format | 🟡 WIP |
| `cargo-gc` | `cargo gc build` subcommand: build + convert + launch Dolphin | 🔴 Stub |

### Target Spec

The Rust target is defined in `targets/powerpc-gekko-eabi.json`. It describes:

- Architecture: `powerpc` (32-bit, big-endian)
- CPU: `750` (Gekko/Broadway)
- Float ABI: hard-float (Gekko has a real IEEE 754 FPU)
- Linker: `rust-lld` (LLVM's linker, full PPC ELF support)
- Panic strategy: `abort` (no unwinding on bare metal)
- No OS, no std

### Memory Map

```
0x80000000  MEM1 start (24 MB, cached)
0x80000000  OS globals / exception vector table
0x80003100  DOL load address (code + data start here)
0x817FFFFF  MEM1 end / stack top
0xC0000000  MEM1 uncached mirror
0xCC000000  Hardware registers
  0xCC002000  VI (Video Interface)
  0xCC003000  PI (Processor Interface)
  0xCC004000  MI (Memory Interface)
  0xCC005000  DSP (Audio DSP)
  0xCC006000  DVD Interface
  0xCC006400  Serial Interface (SI)
  0xCC006800  External Interface (EXI)
  0xCC006C00  Audio Interface (AI)
  0xCC008000  GX (Graphics FIFO)
0x90000000  MEM2 start (64 MB, Wii only)
```

---

## Quick Start

> **Prerequisites:** Rust nightly, `rust-src` component, `rust-lld`.

```sh
rustup toolchain install nightly
rustup component add rust-src llvm-tools-preview

# Build the hello_world example
cargo +nightly build -Z build-std=core,compiler_builtins \
    -Z build-std-features=compiler-builtins-mem \
    --target targets/powerpc-gekko-eabi.json \
    -p hello_world --release

# Convert to .dol
cargo run -p elf2dol -- \
    target/powerpc-gekko-eabi/release/hello_world \
    hello_world.dol

# Launch in Dolphin (adjust path as needed)
dolphin-emu -e hello_world.dol
```

Or with the `cargo-gc` subcommand (once implemented):

```sh
cargo gc build --example hello_world
cargo gc run  --example hello_world --dolphin
```

---

## Progress & Roadmap

See **[WIP.md](WIP.md)** for the current work-in-progress tracker, detailed TODOs, and milestone goals.

---

## License

MIT OR Apache-2.0 (same as Rust itself).

Hardware documentation sourced from:
- [YAGCD — Yet Another GameCube Documentation](https://www.gc-forever.com/yagcd/)
- [WiiBrew Wiki](https://wiibrew.org/wiki/Main_Page)
- libogc2 (reference implementation, not linked or included)
