# DevKit DOL RS — Work In Progress

## Milestone 0 — Scaffold + hello_world ✅

- [x] Workspace layout (gc-rt, gc-hal, gc-gfx, gc-alloc, elf2dol)
- [x] `powerpc-gekko-eabi.json` custom target spec
- [x] `link/gcn.ld` linker script (MEM1 layout, stack, heap symbols)
- [x] `_start` boot assembly: BATs, FPU, cache, BSS zero → `main`
- [x] `gc-hal::vi` — NTSC 480i init, set_framebuffer, flush
- [x] `gc-gfx` — Xfb, YcbcrPair, 8×8 bitmap font, Console (scrolling)
- [x] `elf2dol` — pure Rust ELF→DOL converter
- [x] `hello_world` — boots in Dolphin, prints coloured text

---

## Milestone 1 — Runtime Foundation ✅

- [x] `gc-rt::irq` — `IrqState`, `disable()`, `restore()`, `enable()`, `free(F)`
- [x] `gc-rt::timer` — `DEC_60HZ_GC`, `init()`, `ticks()`, `millis()`,
      `tbr()`, `tbr64()`, `delay_ms()`, `delay_us()`
- [x] `gc-rt::exception` — 15 PPC exception vectors:
  - 6-instruction stubs via uncached mirror, `icbi` + `isync` flush
  - `__exc_entry`: saves 192-byte `ExcCtx`, calls `__exc_rust_dispatch`, `rfi`
  - `Exception` enum (15 variants), `HANDLERS[15]`, `register/unregister`
  - Decrementer auto-forwards to timer
- [x] `gc-alloc` — first-fit linked-list allocator, 32-byte aligned,
      IRQ-safe, block splitting + coalescing
- [x] `gc-hal::pi` — all 27 IRQ masks, `init/pending/unmask/mask/clear/reset_button_down`

---

## Milestone 2 — Controller Input ✅

- [x] `gc-hal::si` — synchronous SICOMCSR immediate-mode transfer
  - `Port` (P1–P4), `Buttons`, `PadState`, `PadResult`
  - `read_pad(port)`, `read_all()`; SPEC5 decode
- [x] `controller_test` example — live 4-port display, buttons + axes + trigger bars

---

## Milestone 3 — GX GPU Basics ✅

- [x] `gc-hal::gx::wgpipe` — WGP init (WPAR SPR 921, HID2 SPR 920), write primitives,
      `load_bp/cp/xf_reg`, `inv_vtx_cache`
- [x] `gc-hal::gx::fifo` — CP + PI FIFO registers, linked mode, `drain()`
- [x] `gc-hal::gx::types` — all enums: `Primitive`, `VtxFmt`, `Compare`,
      `BlendMode/Factor`, `ProjType`, `TevStage`, `cc::*`, `ca::*`
- [x] `gc-hal::gx::state` — VCD, VAT, matrices (`Mtx34`, `IDENTITY`,
      `load_pos_mtx_imm`), `Proj::perspective/orthographic()`, `set_viewport()`,
      `set_scissor()`, `set_z_mode()`, `set_blend_mode()`, TEV passthrough,
      EFB→XFB copy functions
- [x] `gc-hal::gx::draw` — `begin()`, `pos3f/2f()`, `color4u8/3u8()`, `tex1f/2f()`
- [x] `gc-hal::gx::mod` — init matching `GX_Init` + `__GX_InitRevBits` + `__GX_InitGX`
- [x] `spinning_triangle` example — perspective, Y-rotation, double-buffer, ~60 fps

---

## Milestone 4 — Audio ✅

- [x] `gc-hal::ai` — AI DMA streaming:
  - `init()`, `set_dsp_sample_rate()`, `set_volume()`, `register_dma_callback()`
  - `start_dma(ptr, len_bytes)` — DSP regs 24/25/27; physical addr stripping
  - `stop_dma()`, `dma_bytes_left()`
  - `__ai_dma_handler()` — clears DSPCR[AIINT], dispatches callback
- [x] `gc-hal::dsp` — DSP control:
  - `init()` — reset sequence, enable DSPINTMSK
  - `halt()`/`unhalt()`, mailbox: `has_mail_from_dsp()`, `read_mail_from_dsp()`,
    `mail_to_dsp_busy()`, `send_mail_to_dsp()`
  - `aram_dma_busy()`, `register_callback()`
  - Safe W1C bit handling throughout
- [x] `gc-hal::exi` — EXI SPI bus:
  - `select/deselect`, `probe`, `imm` (≤4 bytes), `dma` (multi-byte)
  - `read_u32/write_u32` convenience helpers
- [x] `sine_wave` example — 440 Hz A4, double-buffered DMA, phase-continuous,
      pure-Rust Q16.16 fixed-point sine, IRQ callback

---

## Milestone 5 — Storage ✅

- [x] `gc-hal::sd` — SD/SDHC card driver over EXI (SD Gecko adapter):
  - `SdCard::new(Slot)`, `init()`, `sectors()`, `is_ready()`
  - Full SPI init sequence: CMD0 → CMD8 → ACMD41 → CMD58 → CMD16 → CMD9
  - SDHC detection via CMD8 + CMD58 CCS bit (block vs byte addressing)
  - CSD parsing: CSD v1 (SD ≤2 GB) and CSD v2 (SDHC/SDXC)
  - `read_sector(n, &mut [u8; 512])` — CMD17 + 0xFE token + CRC16 verify
  - `write_sector(n, &[u8; 512])` — CMD24 + 0xFE token + CRC16 + busy wait
  - `read_sectors(start, count, buf)` — multi-sector convenience wrapper
  - CRC7 for command frames (polynomial 0x09), CRC16 (CCITT 0x1021)
  - `SdError` enum: `NoCard`, `Timeout`, `CrcError`, `BadResponse`,
    `Busy`, `OutOfRange`
- [x] `gc-hal::dvd` — DVD drive interface:
  - `init()` — clears DI interrupt flags, enables TC interrupt
  - `read(buf, len, offset)` — DMA read from disc (blocking)
  - `read_disc_id()` — reads 32-byte disc header (`DiscId` struct)
  - `seek(offset)` — drive seek command
  - `spin_up()`, `stop_motor()`
  - `wait_ready()` — polls DISR[TC] with 10-second timeout
  - `register_callback(fn)` — async TC interrupt support
  - `DvdError` enum: `NoDisk`, `CoverOpen`, `Timeout`, `DriveError`,
    `AlignmentError`
- [x] `sd_reader` example:
  - Detects SD card in Slot A (EXI Ch0 Dev0 via SD Gecko)
  - Displays card capacity in MB
  - Reads sector 0 (MBR), checks 0x55AA signature
  - Colour-coded hex dump of first 128 bytes (8 rows × 16 bytes)

---

## Milestone 6 — Wii Extensions 🔴

- [ ] Broadway CPU target (729 MHz / 243 MHz bus)
- [ ] Wii MMIO prefix (0xCD instead of 0xCC)
- [ ] IOS/IPC communication (Starlet ARM coprocessor)
- [ ] Wii feature flag throughout gc-hal

---

## Milestone 7 — cargo-gc Tooling 🔴

- [ ] `cargo gc build` — wraps `+nightly -Z build-std` invocation
- [ ] `cargo gc run`   — build + elf2dol + launch Dolphin
- [ ] `cargo gc dol`   — ELF→DOL conversion only
- [ ] `[package.metadata.gc]` in project `Cargo.toml`

---

## Build Reference

```sh
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly

cargo +nightly build \
  -Z build-std=core,compiler_builtins \
  -Z build-std-features=compiler-builtins-mem \
  --target targets/powerpc-gekko-eabi.json \
  -p sd_reader --release

cargo run -p elf2dol -- \
  target/powerpc-gekko-eabi/release/sd_reader sd_reader.dol

dolphin-emu -e sd_reader.dol
```

### Available examples

| Example              | Demonstrates                                              |
|----------------------|-----------------------------------------------------------|
| `hello_world`        | VI init, XFB text console, colour output                  |
| `controller_test`    | SI pad polling, all 4 ports, buttons/axes/triggers        |
| `spinning_triangle`  | GX 3D pipeline, double-buffer, EFB→XFB copy              |
| `sine_wave`          | AI DMA audio, double-buffering, IRQ callback, 440 Hz tone |
| `sd_reader`          | SD card init over EXI, sector read, hex dump of MBR       |
