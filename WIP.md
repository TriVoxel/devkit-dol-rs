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
- [x] `hello_world` example — boots in Dolphin, prints coloured text

---

## Milestone 1 — Runtime Foundation ✅

### gc-rt additions

- [x] `irq.rs` — `IrqState`, `disable()`, `restore()`, `enable()`, `free(F)`
- [x] `timer.rs` — `DEC_60HZ_GC` (675,000 ticks), `init()`, `ticks()`,
      `millis()`, `tbr()`, `tbr64()`, `delay_ms()`, `delay_us()`
- [x] `exception.rs` — Full 15-vector implementation:
  - 6-instruction absolute-branch stubs (LIS/ORI/MTCTR/LI/BCTR pattern)
  - Written via uncached BAT1 mirror (0xC0000xxx), `icbi` + `isync` flush
  - `__exc_entry` (global_asm!): saves 192-byte `ExcCtx` on dedicated 16 KB
    stack, calls `__exc_rust_dispatch`, restores all registers, `rfi`
  - `ExcCtx` (192 bytes, 32-byte aligned): GPRs[32], SRR0/1, CR, LR, CTR,
    XER, DAR, DSISR, exc_num
  - `Exception` enum (15 variants, discriminants = hardware vector offsets)
  - `HANDLERS[15]` — `register()` / `unregister()`
  - Decrementer auto-forwards to `timer::__timer_dec_handler`

### gc-alloc

- [x] Linked-list first-fit allocator over `__heap_start`…`__heap_end`
  - 32-byte header (cache-line aligned), size + next pointer
  - First-fit with block splitting; address-sorted free list with coalescing
  - IRQ-safe via `gc_rt::irq::free`
  - `GcAllocator` implements `GlobalAlloc`; `pub static ALLOCATOR`
  - `init()` required before first allocation

### gc-hal::pi

- [x] All 27 interrupt source bitmasks (`IM_MEM0` … `IM_PI_HSP`)
- [x] `init()`, `pending()`, `mask()`, `unmask_irq()`, `mask_irq()`,
      `clear_irq()`, `reset_button_down()`

---

## Milestone 2 — Controller Input ✅

### gc-hal::si

- [x] Synchronous immediate-mode transfer via SICOMCSR (no interrupts needed)
- [x] `Port` enum (P1–P4)
- [x] `Buttons` module — all 12 buttons (DLeft/Right/Up/Down, A/B/X/Y/Z,
      L/R, Start)
- [x] `PadState` — buttons, stick X/Y (raw + centered), C-stick X/Y,
      trigger L/R
- [x] `PadResult` — `Ok(PadState)` / `NoController` / `Error`
- [x] `read_pad(port)` — single blocking poll (~5 µs)
- [x] `read_all()` — poll all four ports
- [x] SPEC5 decode: `buttons = (word0>>16)&0x3FFF`, stick from word0 low
      bytes, C-stick + triggers from word1

### examples/controller_test

- [x] Live display of all four ports every frame
- [x] Buttons highlighted in colour when pressed
- [x] Centered analog axis values + 16-char ASCII trigger fill bars
- [x] ~60 Hz loop via `timer::delay_ms(16)`

---

## Milestone 3 — GX GPU Basics ✅

### gc-hal::gx — 6 modules

- [x] `wgpipe.rs` — Write-gather pipe: `init()` (SPR 921 WPAR + SPR 920
      HID2[WPE]), `flush()`, `write8/16/32/f32()`, `load_bp_reg()`,
      `load_cp_reg()`, `load_xf_reg()`, `load_xf_regs()`, `inv_vtx_cache()`
- [x] `fifo.rs` — FIFO circular buffer: programs 14 CP registers (16-bit at
      0xCC000000) and 3 PI registers; linked CPU/GP mode; `drain()`
- [x] `types.rs` — All GX enums: `Primitive`, `VtxFmt`, `AttrType`,
      `CullMode`, `Compare`, `BlendMode/Factor`, `PixelFmt`, `ProjType`,
      `TevStage`, `cc::*`, `ca::*`
- [x] `state.rs` — VCD, VAT, matrices (`Mtx34`, `IDENTITY`,
      `load_pos_mtx_imm`), `Proj::perspective/orthographic()`, `set_viewport()`,
      `set_scissor()`, `set_z_mode()`, `set_blend_mode()`,
      `set_tev_passthrough_vtx_color()`, EFB→XFB copy functions
- [x] `draw.rs` — `begin()`, `pos3f/2f()`, `color4u8/3u8()`, `tex1f/2f()`
- [x] `mod.rs` — Init matching libogc2 `GX_Init` + `__GX_InitRevBits` +
      `__GX_InitGX`: WGP enable → FIFO setup → VAT defaults → PE defaults →
      TEV passthrough

### examples/spinning_triangle

- [x] Full 3D pipeline: perspective, Y-axis rotation, flat vertex colours
- [x] Double-buffered XFBs; EFB clear + EFB→XFB copy each frame
- [x] Pure-Rust sin/cos (Taylor series); ~60 fps via `timer::delay_ms(16)`

---

## Milestone 4 — Audio ✅

### gc-hal::ai

- [x] `init()` — resets sample counter, mutes stream, sets DSP rate to 32 kHz,
      enables AI DMA interrupt in DSPCR
- [x] `set_dsp_sample_rate(SampleRate)` — Hz32000 or Hz48000 (AI_DMAFR bit)
- [x] `set_volume(left, right)` — 0–255 per channel (AI_STREAM_VOL)
- [x] `register_dma_callback(fn())` — IRQ-safe callback registration
- [x] `start_dma(ptr, len_bytes)` — programs DSP regs 24/25 (address) and
      27 (length + enable); physical address stripping; 32-byte alignment enforcement
- [x] `stop_dma()` — clears DMA enable bit
- [x] `dma_bytes_left()` — reads DSP reg 29 (remaining blocks × 32)
- [x] `__ai_dma_handler()` — clears DSPCR[AIINT], dispatches callback
- [x] `DmaCallback` type — `fn()` called from IRQ context

### gc-hal::dsp

- [x] `init()` — DSP reset sequence (DSPCR_DSPRESET), enables DSPINTMSK
- [x] `halt()` / `unhalt()` — pause/resume DSP execution
- [x] `has_mail_from_dsp()` — checks DSP→CPU mailbox bit 15
- [x] `read_mail_from_dsp()` — blocking read from dspReg[2/3]
- [x] `mail_to_dsp_busy()` — checks CPU→DSP mailbox busy bit
- [x] `send_mail_to_dsp(u32)` — blocking write to dspReg[0/1]
- [x] `aram_dma_busy()` — checks DSPCR[DSPDMA]
- [x] `register_callback(fn())` — user handler for DSP→CPU interrupts
- [x] `__dsp_int_handler()` — clears DSPCR[DSPINT], dispatches callback
- [x] Safe W1C bit handling throughout (DSPINT/ARINT/AIINT never
      accidentally cleared by unrelated writes)

### gc-hal::exi (Milestone 5 advanced start)

- [x] `Channel` enum (Ch0/Ch1/Ch2), `Device` enum, `Freq` enum, `Mode` enum
- [x] `select(ch, dev, freq)` — chip-select + clock rate in CSR
- [x] `deselect(ch)` — releases chip select
- [x] `probe(ch)` — checks EXT bit for device presence
- [x] `imm(ch, buf, len, mode)` — ≤4-byte synchronous transfer via DATA reg
- [x] `dma(ch, buf, len, mode)` — multi-byte DMA via MAR/LEN/CR
- [x] `read_u32(ch)` / `write_u32(ch, val)` — convenience big-endian helpers

### examples/sine_wave

- [x] 440 Hz (A4 / concert pitch) stereo sine wave at 32 kHz
- [x] Double-buffered: BUF_A and BUF_B alternate; DMA callback fills the
      idle buffer while the other plays
- [x] Phase-continuous: sine phase accumulator tracks across buffer boundaries
- [x] Pure-Rust Q16.16 fixed-point sine via Taylor series (no libm)
- [x] On-screen status: DMA completion count, active buffer, frame counter

---

## Milestone 5 — Storage 🔴

- [x] EXI bus core (implemented in Milestone 4 above)
- [ ] Memory card read/write — EXI protocol over Ch0/Ch1 Dev0
  - [ ] Card identification (EXI_GetID)
  - [ ] Sector read (EXI_Imm: 0x52 cmd + address + data)
  - [ ] Sector write
- [ ] SD card adapter read (EXI Ch0, 4-wire SPI / SDIO-over-EXI)
  - [ ] CMD0 (reset), CMD8, CMD55+ACMD41 (init), CMD9 (CSD), CMD17/18 (read)
- [ ] Simple raw sector abstraction (no filesystem)

---

## Milestone 6 — DVD 🔴

- [ ] `gc-hal::dvd` — drive spin-up, seek, sector read
- [ ] DI interrupt for async sector completion
- [ ] Simple asset loading example (binary blob from disc)

---

## Milestone 7 — Wii Extensions 🔴

- [ ] Broadway CPU target (same ISA: 729 MHz / 243 MHz bus)
- [ ] Wii MMIO differences (0xCD prefix)
- [ ] IPC / IOS communication (Starlet ARM coprocessor)

---

## Milestone 8 — cargo-gc Tooling 🔴

- [ ] `cargo gc build` — wraps `+nightly -Z build-std` invocation
- [ ] `cargo gc run`   — build + elf2dol + launch Dolphin
- [ ] `cargo gc dol`   — ELF→DOL only
- [ ] `[package.metadata.gc]` in project `Cargo.toml`

---

## Build Reference

```sh
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly

# Build an example (replace hello_world with any example name)
cargo +nightly build \
  -Z build-std=core,compiler_builtins \
  -Z build-std-features=compiler-builtins-mem \
  --target targets/powerpc-gekko-eabi.json \
  -p hello_world --release

# Convert ELF → DOL
cargo run -p elf2dol -- \
  target/powerpc-gekko-eabi/release/hello_world \
  hello_world.dol

# Launch in Dolphin
dolphin-emu -e hello_world.dol
```

### Available examples

| Example              | Demonstrates                                              |
|----------------------|-----------------------------------------------------------|
| `hello_world`        | VI init, XFB clear, text console with colour              |
| `controller_test`    | SI pad polling, live button/axis/trigger display          |
| `spinning_triangle`  | GX 3D pipeline, double-buffer, EFB→XFB copy              |
| `sine_wave`          | AI DMA audio, double-buffering, IRQ callback, 440 Hz tone |
