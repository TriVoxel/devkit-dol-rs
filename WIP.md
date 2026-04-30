# DevKit DOL RS — Work In Progress

This document is the living tracker for devkit-dol-rs. It records what is done,
what is in progress, what is stubbed, and what the next priorities are.

---

## ✅ Milestone 0 — Project Scaffold (CURRENT)

- [x] Repository structure and workspace `Cargo.toml`
- [x] Custom Rust target spec (`powerpc-gekko-eabi.json`)
- [x] Linker script (`link/gcn.ld`) — memory layout for GameCube DOLs
- [x] `gc-rt`: boot assembly (`_start`, BAT init, cache init, BSS clear)
- [x] `gc-rt`: panic handler
- [x] `gc-hal::vi`: VI register definitions and NTSC 480i init sequence
- [x] `gc-hal::vi`: framebuffer address programming
- [x] `gc-gfx`: XFB framebuffer abstraction (YCbCr 4:2:2)
- [x] `gc-gfx`: 8×8 bitmap font (full printable ASCII)
- [x] `gc-gfx`: text console (putchar, print_str, newline, scroll)
- [x] `elf2dol`: ELF → DOL format conversion tool
- [x] `examples/hello_world`: boots, prints text to screen, loops
- [x] README.md, WIP.md, per-crate TODO.md files

**Goal:** A DOL that boots in Dolphin and prints "Hello, GameCube!" to the screen.

---

## 🟡 Milestone 1 — Solid Runtime

- [ ] `gc-rt`: Full exception vector table (DSI, ISI, EXT, ALIGN, PROG, FP, DEC, SYS, TRACE, PERF, IABR, SMI, ThermalMgmt)
- [ ] `gc-rt`: Exception handler dispatch to Rust closures / function pointers
- [ ] `gc-rt`: Decrementer interrupt (timer tick)
- [ ] `gc-rt`: Thread-safe critical sections (IRQ mask/restore)
- [ ] `gc-alloc`: Linked-list allocator over MEM1 heap region
- [ ] `gc-alloc`: `GlobalAllocator` impl + `#[global_allocator]` export
- [ ] `gc-hal::pi`: Processor Interface — interrupt enable/disable, reset button
- [ ] `.cargo/config.toml`: Finalize build flags and target path resolution
- [ ] Verify zero-BSS init is correct (check in Dolphin memory viewer)
- [ ] Verify cache flush before/after framebuffer write

---

## 🔴 Milestone 2 — Input (Controllers)

- [ ] `gc-hal::si`: Serial Interface register map
- [ ] `gc-hal::si`: SI poll — read 4-button GameCube controller state
- [ ] `gc-hal::si`: SI command/response protocol (GC pad, keyboard, steering wheel)
- [ ] High-level `Pad` type: buttons, sticks, triggers, rumble
- [ ] Example: `controller_test` — display pad state on screen

---

## 🔴 Milestone 3 — Video (GX GPU)

- [ ] `gc-hal::gx`: GX FIFO setup (CPU FIFO, write-gather pipe)
- [ ] `gc-hal::gx`: GX state initialization (`GXInit`-equivalent)
- [ ] `gc-hal::gx`: Vertex format definitions (VTXFMT registers)
- [ ] `gc-hal::gx`: Load projection / model-view matrices via GX registers
- [ ] `gc-hal::gx`: Draw quads, tris, line strips via FIFO commands
- [ ] `gc-hal::gx`: Texture object upload (TMEM)
- [ ] `gc-hal::gx`: EFB → XFB copy (`GXCopyDisp`)
- [ ] `gc-gfx`: GPU-accelerated 2D drawing (rects, sprites) via GX
- [ ] Example: `triangle` — rotating colored triangle via GX

---

## 🔴 Milestone 4 — Audio

- [ ] `gc-hal::dsp`: DSP bootstrap / DROM upload
- [ ] `gc-hal::dsp`: DSP mailbox protocol (read/write)
- [ ] `gc-hal::ai`: Audio Interface register map (sample rate, DMA)
- [ ] `gc-hal::ai`: Streaming audio via ARAM DMA
- [ ] High-level `AudioBuffer` type (stereo 16-bit PCM)
- [ ] Example: `sine_wave` — generate and output a 440Hz sine tone

---

## 🔴 Milestone 5 — Storage

- [ ] `gc-hal::exi`: EXI bus register map and transfer protocol
- [ ] `gc-hal::exi`: Memory card (slot A/B) low-level I/O
- [ ] `gc-hal::exi`: SD Gecko adapter support
- [ ] `gc-hal::dvd`: DVD drive command interface (read sectors)
- [ ] FAT filesystem layer (via `embedded-sdmmc` or custom)
- [ ] Example: `file_browser` — list files on SD card

---

## 🔴 Milestone 6 — Networking (GameCube)

- [ ] `gc-hal::exi`: Broadband Adapter (BBA) detection
- [ ] TCP/IP stack integration (port `smoltcp`)
- [ ] UDP socket example: `wiiload`-compatible listener
- [ ] `cargo-gc run --net` — push DOL over network to running GC

---

## 🔴 Milestone 7 — Wii Support

- [ ] Feature flag `wii` to gate Broadway-specific code
- [ ] `targets/powerpc-broadway-eabi.json` target spec
- [ ] Wii-specific BAT config (MEM2 mapping)
- [ ] IPC (inter-processor communication) with Starlet (ARM9)
- [ ] `gc-hal::wpad`: Wii Remote (via BT stack)
- [ ] `gc-hal::ios`: IOS ioctl interface (Wii system services)
- [ ] Example: `wii_hello` — runs on Wii hardware

---

## 🔴 Milestone 8 — cargo-gc Tooling

- [ ] `cargo gc build` — wraps `cargo build` with correct flags
- [ ] `cargo gc run --dolphin` — build + convert + launch Dolphin
- [ ] `cargo gc run --net` — build + push over network (wiiload protocol)
- [ ] `cargo gc new` — project template generator
- [ ] `cargo gc check` — lint + size report

---

## Notes & Known Issues

- **Target spec `cpu` field**: LLVM uses `"750"` for the Gekko. Paired Singles (PS)
  instructions are a Gekko extension not in upstream LLVM; if PS intrinsics are needed,
  they'll be emitted via `global_asm!` or a custom LLVM patch.

- **Cache coherency**: The XFB is accessed by the VI hardware (which reads from physical
  RAM). After writing pixels via the CPU cache, `dcbf` (data cache block flush) must be
  called on each modified cache line before the VI reads it. This is currently done with
  a bulk flush in `gc-gfx` but should be tracked more carefully.

- **Linker script**: `link/gcn.ld` places `.crt0` first so `_start` is at the DOL entry
  point. The DOL header stores this entry address and the IPL jumps to it.

- **No `std` / no allocator yet**: The hello_world example is fully `no_std` with no
  dynamic allocation. Milestone 1 adds the heap allocator, which will unblock `alloc`
  usage.

- **`elf2dol` maturity**: The current implementation handles simple DOLs (one text
  section, one data section). Complex memory layouts with multiple BSS sections or
  large TLS regions may need further work.
