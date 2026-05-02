# DevKit DOL RS

**A pure-Rust devkit for Nintendo GameCube (and Wii) homebrew development.**

Write GameCube applications entirely in Rust — no `devkitPPC`, no `libogc2`,
no C toolchain required. Everything from the boot vector to hardware register
access is implemented in Rust, targeting the GameCube's PowerPC 750CXe (Gekko)
processor directly.

---

## Status

| Milestone | Description                          | Status |
|-----------|--------------------------------------|--------|
| 0         | Scaffold, boot, VI, text console     | ✅     |
| 1         | Exceptions, timer, IRQ, heap         | ✅     |
| 2         | Controller input (SI/PAD)            | ✅     |
| 3         | GX GPU pipeline, 3D rendering        | ✅     |
| 4         | Audio (AI DMA, DSP mailbox, EXI bus) | ✅     |
| 5         | Storage (memory card, SD card)       | 🔴     |
| 6         | DVD drive                            | 🔴     |
| 7         | Wii extensions                       | 🔴     |
| 8         | `cargo-gc` tooling                   | 🔴     |

---

## Crates

| Crate       | Purpose                                                    | Status |
|-------------|------------------------------------------------------------|--------|
| `gc-rt`     | Boot vector, exception table, IRQ, timer, BSS init         | ✅     |
| `gc-hal`    | Hardware drivers: VI, GX, SI, PI, AI, DSP, EXI, DVD       | 🟡     |
| `gc-gfx`    | XFB framebuffer, YCbCr helpers, 8×8 font, text console     | ✅     |
| `gc-alloc`  | `GlobalAllocator` — first-fit linked-list over MEM1 heap   | ✅     |
| `elf2dol`   | Host tool: converts ELF binary to `.dol` format            | ✅     |
| `cargo-gc`  | `cargo gc build/run/dol` subcommand                        | 🔴     |

### gc-hal subsystem status

| Module | Hardware                         | Status                     |
|--------|----------------------------------|----------------------------|
| `vi`   | Video Interface (display)        | ✅ NTSC/PAL, flush          |
| `pi`   | Processor Interface (interrupts) | ✅ All 27 IRQ masks          |
| `si`   | Serial Interface (controllers)   | ✅ Sync polling, 4 ports     |
| `gx`   | Graphics Processor               | ✅ Full 3D pipeline          |
| `ai`   | Audio Interface (DMA streaming)  | ✅ DMA + IRQ callback        |
| `dsp`  | Audio DSP (mailbox, control)     | ✅ Reset, mailbox, interrupt |
| `exi`  | External Interface               | ✅ Imm + DMA transfers       |
| `dvd`  | DVD drive                        | 🔴 Stub                     |

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
      │     └── dvd        DVD drive — stub
      │
      ├── gc-gfx          CPU-side framebuffer graphics
      │     ├── Xfb        YCbCr 4:2:2 framebuffer wrapper
      │     ├── Console    Scrolling text console with 8×8 bitmap font
      │     └── color      Named YCbCr colour constants
      │
      ├── gc-alloc        Heap allocator
      │     └── GcAllocator  First-fit linked list, 32-byte aligned blocks
      │
      └── gc-rt           Bare-metal runtime
            ├── _start     Boot assembly: BATs, FPU, cache, BSS → main
            ├── exception  15 PPC exception vectors with full context save
            ├── irq        MSR[EE] critical sections
            └── timer      Decrementer tick counter, TBR, delay
```

### GX pipeline

```
CPU writes commands to write-gather pipe (0xCC008000)
       │  (32-byte burst when WGP buffer fills)
  FIFO ring buffer in MEM1  (≥64 KB, CP + PI registers)
       │
  CP → XF (transform) → TX (texture) → TEV (combine) → PE (Z/blend)
       │
  EFB (embedded framebuffer, up to 640×528)
       │  copy_disp()
  XFB (external framebuffer in MEM1, YCbCr 4:2:2)
       │
  VI → display / Dolphin window
```

### Audio pipeline

```
CPU fills PCM buffer (i16 stereo, 32-byte aligned)
       │
  AI DMA (dspReg[24-27]) — feeds DAC directly from MEM1
       │  (fires IRQ_DSP_AI when buffer drains)
  DMA callback → reload next buffer (double-buffered)
       │
  Analogue audio output
```

### EXI bus

```
EXI channel 0/1/2 at 0xCC006800
       │  (5 × u32 registers per channel: CSR, MAR, LEN, CR, DATA)
  select(ch, dev, freq) → imm/dma transfer → deselect(ch)
       │
  Memory card / SD card / RTC / Expansion device
```

### Memory map

```
0x80000000  MEM1 start — 24 MB, cached
0x80000100  Exception vector table (15 × 128-byte stubs, installed at runtime)
0x80003100  DOL load address — code, rodata, data, BSS
0x817F0000  Stack bottom (64 KB reserved)
0x817FFFFF  Stack top (grows downward)
0xC0000000  MEM1 uncached mirror
0xCC000000  CP registers (16-bit) — GX command processor
0xCC001000  PE registers (16-bit) — pixel engine
0xCC002000  VI registers (16-bit) — video interface
0xCC003000  PI registers (32-bit) — interrupt controller
0xCC004000  MI registers — memory interface
0xCC005000  DSP registers (16-bit) — DSP + AI DMA + ARAM DMA
0xCC006000  DVD interface registers
0xCC006400  SI registers (32-bit) — serial interface (controllers)
0xCC006800  EXI registers (32-bit) — external interface
0xCC006C00  AI registers (32-bit) — audio interface (streaming)
0xCC008000  GX write-gather pipe — GPU FIFO entry point
```

---

## Quick Start

**Prerequisites:** Rust nightly + `rust-src` component.

```sh
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
```

**Build and run an example:**

```sh
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
| `controller_test`    | All 4 controller ports, live button/axis/trigger display     |
| `spinning_triangle`  | Full GX pipeline: 3D perspective, Y-rotation, double-buffer  |
| `sine_wave`          | AI DMA audio, double-buffering, IRQ callback, 440 Hz tone    |

---

## Writing Your Own Application

### Minimal skeleton

```rust
#![no_std]
#![no_main]

use gc_hal::vi;

#[no_mangle]
pub extern "C" fn main() -> ! {
    unsafe {
        vi::init_ntsc_480i();
        // ... your code here ...
        loop {}
    }
}
```

### With audio

```rust
use gc_hal::ai::{self, SampleRate};

static mut BUF_A: [i16; 1408] = [0; 1408]; // 704 stereo frames
static mut BUF_B: [i16; 1408] = [0; 1408];

fn audio_callback() {
    unsafe {
        // Fill the idle buffer and restart DMA
        fill_audio(&mut BUF_B);
        ai::start_dma(BUF_B.as_ptr(), BUF_B.len() * 2);
    }
}

unsafe fn audio_init() {
    ai::init();
    ai::set_dsp_sample_rate(SampleRate::Hz32000);
    ai::set_volume(255, 255);
    fill_audio(&mut BUF_A);
    ai::register_dma_callback(audio_callback);
    ai::start_dma(BUF_A.as_ptr(), BUF_A.len() * 2);
}
```

### With GX 3D

```rust
use gc_hal::gx::{self, state, draw, types::*};

static mut FIFO: [u8; 256 * 1024] = [0; 256 * 1024];

unsafe fn gx_init() {
    gx::init(FIFO.as_mut_ptr(), FIFO.len());
    state::set_vtx_desc_pos_clr0();
    state::set_vtx_fmt_pos_xyz_f32_clr_rgba8(VtxFmt::Fmt0);
    let proj = state::Proj::perspective(
        60_f32 * core::f32::consts::PI / 180.0, 640.0/480.0, 0.1, 100.0);
    state::load_projection_mtx(&proj);
    state::load_pos_mtx_imm(&state::IDENTITY, 0);
    state::set_current_mtx(0);
    state::set_viewport(0.0, 0.0, 640.0, 480.0, 0.0, 1.0);
    state::set_z_mode(true, Compare::LEqual, true);
    state::set_blend_mode(BlendMode::None, BlendFactor::SrcAlpha, BlendFactor::InvSrcAlpha);
    state::set_tev_passthrough_vtx_color();
    state::set_tev_order_vtx_only();
    state::set_num_tev_stages(1);
    state::set_num_color_chans(1);
    state::set_num_tex_gens(0);
}
```

---

## Why Nightly Rust?

Two reasons, both unavoidable for bare-metal custom-target development:

1. **`-Z build-std`** (Cargo unstable): compiling `core` from source for a
   custom target JSON requires this flag. There is no stable equivalent.
2. **`#![feature(asm_experimental_arch)]`**: PowerPC inline `asm!` blocks
   require this feature gate (PPC is not a tier-1 Rust asm target).

These requirements are shared by every bare-metal Rust project
(`cortex-m`, Embassy, `rp2040-hal`, etc.).

---

## References

- [YAGCD — Yet Another GameCube Documentation](https://www.gc-forever.com/yagcd/)
- [WiiBrew Wiki](https://wiibrew.org/wiki/Main_Page)
- [Dolphin Emulator](https://dolphin-emu.org/) — primary testing target
- libogc2 — reference implementation (not linked or included)

---

## License

MIT OR Apache-2.0.
