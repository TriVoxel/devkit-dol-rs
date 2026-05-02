# DevKit DOL RS — Work In Progress

## Milestone 0 — Scaffold + hello_world ✅
- [x] Workspace layout, `powerpc-gekko-eabi.json`, `link/gcn.ld`
- [x] `_start` boot assembly, `gc-hal::vi` NTSC 480i, `gc-gfx` text console
- [x] `elf2dol`, `hello_world` example

## Milestone 1 — Runtime Foundation ✅
- [x] `gc-rt::irq` — `IrqState`, `disable/restore/enable/free`
- [x] `gc-rt::timer` — decrementer, TBR, `delay_ms/us`
- [x] `gc-rt::exception` — 15 PPC vectors, full `ExcCtx`, `HANDLERS[15]`
- [x] `gc-alloc` — first-fit linked-list, 32-byte aligned, IRQ-safe
- [x] `gc-hal::pi` — all 27 IRQ masks

## Milestone 2 — Controller Input ✅
- [x] `gc-hal::si` — synchronous SICOMCSR poll, SPEC5 decode
- [x] `Port/Buttons/PadState/PadResult`, `read_pad/read_all`
- [x] `controller_test` example — 4-port live display

## Milestone 3 — GX GPU Basics ✅
- [x] `gc-hal::gx::wgpipe` — WGP init (WPAR/HID2), write primitives
- [x] `gc-hal::gx::fifo` — CP + PI FIFO registers, linked mode
- [x] `gc-hal::gx::types` — all GX enums and constants
- [x] `gc-hal::gx::state` — VCD, VAT, matrices, viewport, Z/blend, TEV, EFB→XFB
- [x] `gc-hal::gx::draw` — `begin/pos3f/color4u8/tex2f` etc.
- [x] `spinning_triangle` example — perspective, Y-rotation, double-buffer

## Milestone 4 — Audio ✅
- [x] `gc-hal::ai` — AI DMA init, `start_dma`, IRQ callback, `__ai_dma_handler`
- [x] `gc-hal::dsp` — reset, halt/unhalt, mailbox send/recv, interrupt
- [x] `gc-hal::exi` — select/deselect, probe, `imm` (≤4 B), `dma` (multi-B)
- [x] `sine_wave` example — 440 Hz double-buffered DMA, IRQ callback

## Milestone 5 — Storage (original) ✅
- [x] `gc-hal::sd` — SD/SDHC SPI init, `read/write_sector`, CRC7/CRC16
- [x] `gc-hal::dvd` — DI register DMA read, seek, disc ID, `wait_ready`
- [x] `sd_reader` example — detect, read MBR, hex dump

## Milestone 5b — Complete Storage Coverage ✅

### gc-hal::exi additions
- [x] `DeviceType` enum — `MemCard59/123/251/507/1019/2043`, `MemCardPro`,
      `BroadbandAdapter`, `IdeExi`, `SdCard`, `Unknown(u32)`, `None`
- [x] `get_id(ch, dev) → DeviceType` — send 2 zero bytes at 1 MHz, read 4 bytes
- [x] `classify_id(u32)` — maps raw ID to known device type
- [x] `DeviceType::is_memory_card()`, `card_bytes()`, `name()`

### gc-hal::sd additions
- [x] `Slot::Sp2` — EXI Ch2, Device 0 (SD2SP2 adapter on Serial Port 2)
  - `spi_clock_init`, `send_cmd_r1`, `wait_data_token`, `read/write_byte`
    all route correctly through Ch2
  - `probe()` for Ch2 always returns `true` (no EXT bit on Ch2)

### gc-hal::memcard (new)
- [x] `CardSlot` enum (A = Ch0, B = Ch1)
- [x] `MemCard::probe(slot)` — reads EXI device ID, validates against known
      memory card IDs, computes geometry from ID bits:
  - `sector_size = SECTOR_SIZES[_ROTL(id, 23) & 0x1C >> 2]` → 8 KB standard
  - `latency = LATENCIES[_ROTL(id, 26) & 0x1C >> 2]` → 4 standard
  - `total_bytes`, `sector_count` derived from card type
- [x] `MemCard::read_segment(addr, &mut [u8; 512])` — opcode 0x52, 5-byte
      address frame, `latency` dummy bytes, 512-byte DMA read
- [x] `MemCard::write_page(addr, &[u8; 128])` — opcode 0xF2, 128-byte DMA
      write, polls status until `BUSY` clears (2-second timeout)
- [x] `MemCard::erase_sector(addr)` — opcode 0xF1, 4-byte address, polls
      until `BUSY` clears (10-second timeout for flash erase)
- [x] `MemCard::read_status()` — opcode 0x83 + 0x00, read 1 byte
- [x] `MemCard::clear_status()` — opcode 0x89
- [x] `CardError` enum: `NoCard`, `Busy`, `IoError`, `Timeout`, `InvalidParam`
- [x] Address encoding: `[opcode, addr>>17, addr>>9, addr>>7, addr&0x7F]`

### gc-hal::storage (new — unified abstraction)
- [x] `BlockDevice` trait:
  - `name() → &'static str`
  - `sector_size() → usize`
  - `sector_count() → u64`
  - `capacity_bytes() → u64`
  - `read(lba, &mut [u8]) → Result<(), BlockError>`
  - `write(lba, &[u8]) → Result<(), BlockError>`
  - `is_read_only() → bool`
- [x] `BlockError` enum: `NoDevice`, `IoError`, `Timeout`, `BadAddress`,
      `WriteProtected`, `ReadOnly`
- [x] `SdCard` implements `BlockDevice` (512-byte sectors)
- [x] `MemCard` implements `BlockDevice` (512-byte read segments; writes
      fan out to 4 × 128-byte pages)
- [x] `StorageKind` enum: `SdCardSlotA/B/Sp2`, `MemCardSlotA/B`, `DvdDisc`
- [x] `StorageInfo` struct: kind, dev_type, sector_size, sector_count, read_only
- [x] `scan(&mut [StorageInfo]) → usize` — probes all slots in order:
  - Ch0: EXI ID → memory card or SD card init
  - Ch1: same
  - Ch2: SD card init (SD2SP2; always probed, no EXT detection)
  - DVD: cover + disc ID check

### examples/storage_detect (new)
- [x] Scans all storage; prints per-device: slot name, type, capacity
- [x] Reads sector 0 from each writable device, shows first 8 bytes as hex
- [x] Explains ODE transparency (CubeODE/GCLoader/Flippy → DVD driver)
- [x] Colour-coded output: green=SD, yellow=memcard, cyan=DVD

---

## Device Support Matrix

| Device                  | How it works                               | Driver        |
|-------------------------|--------------------------------------------|---------------|
| SD Gecko (Slot A)       | EXI Ch0 Dev0, SPI mode                    | `gc-hal::sd`  |
| SD Gecko (Slot B)       | EXI Ch1 Dev0, SPI mode                    | `gc-hal::sd`  |
| SD2SP2                  | EXI Ch2 Dev0, SPI mode                    | `gc-hal::sd`  |
| GC Memory Card (Slot A) | EXI Ch0 Dev0, page read/write             | `gc-hal::memcard` |
| GC Memory Card (Slot B) | EXI Ch1 Dev0, page read/write             | `gc-hal::memcard` |
| MemCard PRO GC          | EXI Ch0/1, same protocol                  | `gc-hal::memcard` |
| DVD (real drive)        | DI registers 0xCC006000                   | `gc-hal::dvd` |
| CubeODE / GCLoader      | Transparent ODE via DI registers          | `gc-hal::dvd` |
| Flippy Drive            | Transparent ODE via DI registers          | `gc-hal::dvd` |
| BBA / IDE-EXI           | Detected by `exi::get_id`, driver TODO    | `gc-hal::exi` |

---

## Milestone 6 — Wii Extensions 🔴
- [ ] Broadway CPU target (729 MHz / 243 MHz bus)
- [ ] Wii MMIO 0xCD prefix differences
- [ ] IOS/IPC (Starlet ARM coprocessor)

## Milestone 7 — cargo-gc Tooling 🔴
- [ ] `cargo gc build/run/dol` wrapping nightly `-Z build-std`
- [ ] `[package.metadata.gc]` config in project `Cargo.toml`

---

## Build Reference

```sh
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly

cargo +nightly build \
  -Z build-std=core,compiler_builtins \
  -Z build-std-features=compiler-builtins-mem \
  --target targets/powerpc-gekko-eabi.json \
  -p storage_detect --release

cargo run -p elf2dol -- \
  target/powerpc-gekko-eabi/release/storage_detect storage_detect.dol

dolphin-emu -e storage_detect.dol
```

### Available examples

| Example          | Demonstrates                                               |
|------------------|------------------------------------------------------------|
| `hello_world`    | VI init, XFB, text console                                 |
| `controller_test`| SI polling, 4 ports, buttons/axes/triggers                 |
| `spinning_triangle` | GX 3D, perspective, double-buffer                       |
| `sine_wave`      | AI DMA audio, double-buffering, 440 Hz                     |
| `sd_reader`      | SD card init, MBR read, hex dump                           |
| `storage_detect` | All storage: SD/SP2/memcard/DVD scan + sector 0 preview    |
