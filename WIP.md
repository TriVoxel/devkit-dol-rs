# DevKit DOL RS — Work In Progress

## Milestone 0 — Scaffold + hello_world ✅

- [x] Workspace layout (gc-rt, gc-hal, gc-gfx, gc-alloc, elf2dol)
- [x] `powerpc-gekko-eabi.json` custom target spec
- [x] `link/gcn.ld` linker script (MEM1 layout, stack, heap symbols)
- [x] `_start` boot assembly: BATs via rfi trick, GPR clear, FPU+PS enable,
      HID0 cache enable, BSS zero → `main`
- [x] `gc-hal::vi` — NTSC 480i init, set_framebuffer, flush
- [x] `gc-gfx` — Xfb, YcbcrPair, 8×8 bitmap font, Console (scrolling)
- [x] `elf2dol` — pure Rust ELF→DOL converter
- [x] `hello_world` example — boots in Dolphin, prints colored text

---

## Milestone 1 — Runtime Foundation ✅

### gc-rt additions
- [x] `irq.rs` — `IrqState`, `disable()`, `restore()`, `enable()`, `free()`
- [x] `timer.rs` — `DEC_60HZ_GC`, `init()`, `ticks()`, `millis()`,
      `tbr()`, `tbr64()`, `delay_ms()`, `delay_us()`
- [x] `exception.rs` — Full implementation:
  - 15 exception vector stubs (6-instruction absolute-branch pattern)
  - Written at runtime via uncached BAT1 mirror (0xC0000xxx)
  - `icbi` + `isync` cache flush after installation
  - `__exc_entry` (global_asm!) — saves 192-byte `ExcCtx` (all GPRs +
    SRR0/1 + CR/LR/CTR/XER/DAR/DSISR), calls `__exc_rust_dispatch`,
    restores and `rfi`
  - `ExcCtx` struct (192 bytes, 32-byte aligned, fixed offsets)
  - `Exception` enum (15 variants, hardware vector offsets as values)
  - `HANDLERS[15]` table — `register()` / `unregister()`
  - `__EXC_STACK_TOP` — 16 KB dedicated exception stack
  - Decrementer auto-ticks timer from `__exc_rust_dispatch`

### gc-alloc
- [x] Linked-list first-fit allocator over `__heap_start`…`__heap_end`
  - 32-byte aligned headers (cache-line granularity)
  - First-fit allocation with block splitting
  - Address-sorted free list with coalescence
  - IRQ-safe via `gc_rt::irq::free`
  - `GcAllocator` implements `GlobalAlloc`; `pub static ALLOCATOR`
  - `init()` call required in `main()` before first allocation

### gc-hal::pi
- [x] All 27 interrupt source bitmasks (`IM_MEM0` … `IM_PI_HSP`)
- [x] `init()`, `pending()`, `mask()`, `unmask_irq()`, `mask_irq()`,
      `clear_irq()`, `reset_button_down()`

---

## Milestone 2 — Controller Input ✅

### gc-hal::si
- [x] Synchronous single-pad read via SICOMCSR immediate transfer
- [x] `Port` enum (P1–P4)
- [x] `Buttons` module (DLeft/Right/Up/Down, A/B/X/Y/Z, L/R, Start)
- [x] `PadState` — buttons, stick X/Y, C-stick X/Y, trigger L/R
  - `.pressed(button)`, `.stick_x_centered()` etc.
- [x] `PadResult` — `Ok(PadState)` / `NoController` / `Error`
- [x] `read_pad(port)` — single blocking poll
- [x] `read_all()` — poll all four ports at once
- [x] SPEC5 button + analog decode (matches libogc2 SPEC2_MakeStatus)

### examples/controller_test
- [x] Reads all four ports every frame
- [x] Live display: button names highlighted when pressed, analog values,
      trigger fill-bar visualization
- [x] ~60 Hz update loop via `timer::delay_ms(16)`

---

## Milestone 3 — GX GPU Basics ✅

- [ ] GX FIFO ring buffer setup (0xCC008000)
- [ ] Command processor initialization
- [ ] Basic vertex submission (position + color)
- [ ] Projection / modelview matrix upload (XF)
- [ ] Textured quad drawing
- [ ] `gc-gfx`: upgrade from CPU-drawn XFB console to GX pipeline

---

## Milestone 4 — Audio 🔴

- [ ] `gc-hal::ai` — stream 16-bit stereo PCM at 32 kHz / 48 kHz
- [ ] `gc-hal::dsp` — DSP mailbox, ARAM DMA
- [ ] Simple sine-wave test tone example

---

## Milestone 5 — Storage 🔴

- [ ] `gc-hal::exi` — EXI bus protocol (SPI-like, 3 channels)
- [ ] Memory card read/write (EXI channel 0/1)
- [ ] SD adapter read (EXI channel 0, SDIO protocol)
- [ ] Simple file abstraction

---

## Milestone 6 — DVD 🔴

- [ ] `gc-hal::dvd` — drive spin-up, seek, sector read
- [ ] Async read via DI interrupt
- [ ] Simple asset loading example

---

## Milestone 7 — Wii Extensions 🔴

- [ ] Broadway CPU target (same ISA, higher clocks)
- [ ] Wii BAT configuration differences
- [ ] `gc-hal::exi` Wii variant (IOS/AHBPROT bypass not required for homebrew)
- [ ] Wiimote via Bluetooth (HCI stack, long-term goal)

---

## Milestone 8 — cargo-gc Tooling 🔴

- [ ] `cargo gc build` — wraps the nightly invocation
- [ ] `cargo gc run` — build + elf2dol + launch Dolphin
- [ ] `cargo gc dol` — just the ELF→DOL conversion step
- [ ] Config in `Cargo.toml` `[package.metadata.gc]`

---

## Build Reference

```sh
# Build an example
cargo +nightly build \
  -Z build-std=core,compiler_builtins \
  -Z build-std-features=compiler-builtins-mem \
  --target targets/powerpc-gekko-eabi.json \
  -p hello_world --release

# Convert to DOL
cargo run -p elf2dol -- \
  target/powerpc-gekko-eabi/release/hello_world \
  hello_world.dol

# Run in Dolphin
dolphin-emu -e hello_world.dol
```
