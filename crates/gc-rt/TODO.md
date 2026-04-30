# TODO: gc-rt — Runtime (Remaining Work)

## Completed (Milestone 0)

- [x] Boot assembly: BAT init, GPR clear, FPU enable, cache enable, BSS zero, branch to main
- [x] Panic handler (halt loop)
- [x] Cache operations: dcbf, dcbf_range, dcbi, icbi, sync, isync
- [x] Exception enum definitions and stub init

## Remaining (Milestone 1)

- [ ] **Full exception vector stubs** — install 128-byte stubs at 0x80000100 for all 16
  exception types. Each stub must: save all GPRs to an ExceptionContext on the stack,
  call the Rust handler dispatch function, restore GPRs, and `rfi`.

- [ ] **ExceptionContext struct** — hold all 32 GPRs, FPRs (optional), SRR0, SRR1, CR,
  XER, CTR, LR, and the exception cause code.

- [ ] **Handler registration API** — `gc_rt::exception::register(Exception::Dsi, my_handler)`
  with a static function pointer table.

- [ ] **Decrementer timer** — The decrementer register (SPR 22) counts down and triggers
  exception 0x0900. Set it up for a configurable tick rate (60 Hz default).

- [ ] **Critical sections** — `with_irq_disabled(|| { ... })` helper that saves and
  restores MSR[EE] around a closure. Used internally by gc-alloc and gc-hal drivers.

- [ ] **BSS fill optimisation** — The current BSS zeroing loop in start.rs uses `stb`
  (store byte). Replace with word-aligned `stw` or `dcbz` for speed. On a 24 MB BSS
  this is noticeable during boot.

- [ ] **sbrk / newlib integration** (optional) — If a newlib-based libc is ever desired,
  provide `_sbrk` by adjusting the heap pointer upward. Currently not needed since we
  don't use newlib.
