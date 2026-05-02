# DevKit DOL RS — Work In Progress

## Milestone 0 — Scaffold + hello_world ✅
## Milestone 1 — Runtime Foundation ✅
## Milestone 2 — Controller Input ✅
## Milestone 3 — GX GPU Basics ✅
## Milestone 4 — Audio ✅
## Milestone 5 — Storage (SD, SP2, MemCard, DVD, ODE) ✅

---

## Milestone 6 — Wii Extensions ✅

### targets/powerpc-broadway-eabi.json (new)
- [x] Same ISA as Gekko (PPC 750), different clock: 729 MHz CPU / 243 MHz bus
- [x] `vendor = "nintendo-wii"`, links against `link/wii.ld`

### link/wii.ld (new)
- [x] MEM1: same as GC (24 MB at 0x80000000, DOL load at 0x80003100)
- [x] Stack: 128 KB (vs 64 KB on GC) at top of MEM1
- [x] MEM2 linker region declared (0x90400000–0x93FFFFFF, 60 MB)
- [x] `__mem2_start`, `__mem2_end` symbols exported
- [x] `__heap_end` points to stack bottom (MEM2 heap via separate allocator)

### gc-rt — Wii BAT setup (`start.rs`)
- [x] Wii feature gate (`#[cfg(feature = "wii")]`) on separate `global_asm!` block
- [x] `__wii_mem2_bats` routine added to `.crt0`:
  - DBAT2: cached MEM2   — `0x90000000 → physical 0x10000000`, 64 MB, WIMG=0000
  - DBAT3: uncached MEM2 — `0xD0000000 → physical 0x10000000`, 64 MB, WIMG=0101
- [x] `[features] wii = []` in `gc-rt/Cargo.toml`

### gc-hal::mmio (new)
- [x] `mmio::BASE` — `0xCC000000` (GC) or `0xCD000000` (Wii, `--features wii`)
- [x] `mmio::addr(offset) → usize` — compile-time MMIO address computation
- [x] All hardware modules updated to use `crate::mmio::addr(...)`:
  - `vi::regs` — `mmio::addr(0x002000)`
  - `pi` — `mmio::addr(0x003000)`
  - `si` — `mmio::addr(0x006400)`
  - `ai` — `mmio::addr(0x006C00)` and `0x005000`
  - `dsp` — `mmio::addr(0x005000)`
  - `exi` — `mmio::addr(0x006800)`
  - `dvd` — `mmio::addr(0x006000)`
  - `gx::wgpipe` — `mmio::addr(0x008000)` for WGP address
  - `gx::fifo` — `mmio::addr(0x000000)` (CP) and `0x003000` (PI)
- [x] `[features] wii = ["gc-rt/wii"]` in `gc-hal/Cargo.toml`

### gc-hal::mem2 (new)
- [x] `MEM2_START = 0x9000_0000`, `MEM2_END = 0x9400_0000`
- [x] `MEM2_UNCACHED = 0xD000_0000`
- [x] `IOS_RESERVED = 4 MB`
- [x] `HEAP_START` — first byte available to homebrew
- [x] `HEAP_SIZE` — usable MEM2 (60 MB)
- [x] `to_uncached(addr)` — cached → uncached mirror conversion
- [x] `from_physical(phys)` — physical → cached virtual

### Building for Wii
```sh
cargo gc build --wii --example hello_world
# or manually:
cargo +nightly build \
  -Z build-std=core,compiler_builtins \
  -Z build-std-features=compiler-builtins-mem \
  --target targets/powerpc-broadway-eabi.json \
  --features gc-hal/wii \
  -p hello_world --release
```

---

## Milestone 7 — cargo-gc Tooling ✅

### tools/cargo-gc (full implementation)

**Subcommands:**

`cargo gc build [--release] [-p <pkg>] [--example <name>] [--wii]`
- Runs `cargo +nightly build -Z build-std=... --target ...`
- Detects ELF output automatically (newest ELF with magic bytes check)
- Converts ELF → DOL via `cargo run -p elf2dol`
- Coloured status output (green bold verbs like `cargo` itself)

`cargo gc dol <elf> [output.dol]`
- Direct ELF → DOL conversion
- Output path defaults to `<elf>.dol` in same directory

`cargo gc run [--release] [-p <pkg>] [--example <name>] [--dolphin <path>] [--wii]`
- Builds, converts, then launches `dolphin-emu -e <output.dol>`
- Dolphin path resolution: `--dolphin` flag → `[package.metadata.gc]` → `dolphin-emu` on PATH

`cargo gc new <project-name>`
- Scaffolds a complete new GC project:
  - `Cargo.toml` with gc-rt/gc-hal/gc-gfx deps and `[package.metadata.gc]`
  - `examples/hello.rs` — minimal hello world
  - `.cargo/config.toml` — target + rustflags
  - `rust-toolchain.toml` — nightly + rust-src
  - `targets/powerpc-gekko-eabi.json` — embedded GC target spec
  - `targets/powerpc-broadway-eabi.json` — embedded Wii target spec
  - `link/gcn.ld` — embedded linker script
  - `README.md`, `.gitignore`

`cargo gc help [<subcmd>]`
- Detailed help for each subcommand

**Implementation details:**
- [x] Pure `std`, no external crates (only the Rust standard library)
- [x] `[package.metadata.gc]` parsed via hand-rolled `key = "value"` scanner
- [x] `find_cargo_toml()` — walks up directory tree to locate project root
- [x] `locate_elf()` — finds the correct ELF using `--example`/`-p` hints,
      falling back to newest ELF (by mtime) with ELF magic bytes check
- [x] `is_elf()` — checks `0x7F ELF` magic bytes
- [x] `say()` — green bold status lines, degrades to plain if `TERM` is unset
- [x] Both `cargo gc <subcmd>` and `cargo-gc <subcmd>` invocation styles work

**Installation:**
```sh
cargo install --path tools/cargo-gc
```

---

## Build Reference

```sh
# Install the build tool
cargo install --path tools/cargo-gc

# Build and run (GameCube)
cargo gc run --example spinning_triangle --release

# Build and run (Wii)
cargo gc run --example hello_world --wii --release

# Scaffold a new project
cargo gc new my_game
cd my_game
cargo gc build --release --example hello
cargo gc run   --example hello
```

### Available examples

| Example            | Demonstrates                                               |
|--------------------|------------------------------------------------------------|
| `hello_world`      | VI init, XFB, text console                                 |
| `controller_test`  | SI polling, 4 ports, buttons/axes/triggers                 |
| `spinning_triangle`| GX 3D, perspective, double-buffer, EFB→XFB                |
| `sine_wave`        | AI DMA audio, double-buffering, IRQ callback               |
| `sd_reader`        | SD card init (SD Gecko), MBR read, hex dump                |
| `storage_detect`   | Scans all storage: SD/SP2/memcard/DVD + sector preview     |
