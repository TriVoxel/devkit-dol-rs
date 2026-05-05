# TODO: targets/

## What This Directory Is

Custom Rust target specification files (`.json`) used with `rustc --target`.
These tell `rustc` and LLVM everything about the target CPU, ABI, and linker.

## Current Files

- `powerpc-gekko-eabi.json` — GameCube Gekko (PowerPC 750CXe), hard-float EABI, no OS.

## TODO

### powerpc-gekko-eabi.json

- [ ] **Verify `cpu` field** — LLVM's `"750"` maps to the MPC750 which is close to
  Gekko but missing the Paired Singles extension. Confirm codegen is correct.
  If LLVM misses Gekko-specific codegen, switch to `"generic"` with explicit features.

- [ ] **Paired Singles (PS) support** — The Gekko's PS instructions (`psq_l`, `psq_st`,
  `ps_add`, etc.) are not in upstream LLVM's PowerPC backend. Options:
  1. Emit PS via `global_asm!` / `asm!` inline for hot paths.
  2. Patch LLVM to recognize `-mgekko`.
  3. Use a pre-built LLVM with Gekko support (devkitPPC's LLVM fork).

- [ ] **Verify `data-layout`** — Cross-check against GCC's `-dumpmachine` output
  and `powerpc-unknown-none-eabi` data layout in LLVM source.

- [ ] **`disable-redzone`** — Currently `true`. Verify that interrupt handlers and
  exception vectors don't clobber the redzone. On PPC the redzone is 224 bytes below SP.

### powerpc-broadway-eabi.json (not yet created)

- [ ] Create a second target spec for the Wii's Broadway CPU.
  Broadway is Gekko + higher clock + minor extensions. The target JSON will be nearly
  identical but gated behind the `wii` feature in `dkdol-hal`.
