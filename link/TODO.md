# TODO: link/

## What This Directory Is

Linker scripts for supported platforms. Each script defines the memory map,
section layout, and special symbols used by `dkdol-rt` and the hardware drivers.

## Current Files

- `gcn.ld` — GameCube (DOL format). Tested with Dolphin emulator.

## TODO

- [ ] **Validate section alignment** — GX DMA transfers require 32-byte alignment
  on source buffers. Confirm that `.data` and `.rodata` sections meet this.

- [ ] **ARAM segment** — GameCube has 16 MB of Audio RAM (ARAM) accessible via DMA.
  Consider adding an `ARAM` memory region so audio buffers can be placed there
  explicitly via `#[link_section = ".aram"]`.

- [ ] **wii.ld** — Wii DOL linker script. Nearly identical to `gcn.ld` but:
  - MEM2 region: `0x90000000`, 64 MB
  - Stack can be placed in MEM2 for large apps
  - IPC buffer region must be reserved at specific addresses

- [ ] **`.got` / `.plt` sections** — Currently discarded. If position-independent
  code is ever needed (unlikely for DOLs, but possible for modules), these will
  need proper handling.

- [ ] **Exception vectors** — The GC exception table lives at `0x80000100`.
  The current design has `dkdol-rt` install handler stubs at runtime by writing
  to that address. An alternative is to place them in the linker script as
  `AT(0x80000100)`. The runtime approach is currently preferred for simplicity.

- [ ] **Map file output** — Add `--Map=output.map` to linker flags in `cargo-dkdol`
  so developers can inspect section sizes and symbol addresses.
