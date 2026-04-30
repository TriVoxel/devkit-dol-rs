# TODO: gc-hal::pi — Processor Interface

## What This Is

The Processor Interface (PI) is the interrupt controller for the GameCube.
It aggregates all hardware interrupt sources into the CPU's single external
interrupt line (exception vector `0x80000500`).

**Base address:** `0xCC003000`

## Architecture

### Register Map

```
PIINTSR    0xCC003000  Interrupt source register (read: pending, write: clear)
PIINTMR    0xCC003004  Interrupt mask register (1 = enabled)
PIRSW      0xCC003008  Reset switch status
PIDB       0xCC00300C  Debug interface (not used in homebrew)
PICP       0xCC003010  Crossbar port (DMA arbiter)
```

### Interrupt Sources (PIINTSR bits)

```
Bit 15: RSWST  — Reset button (RSW)
Bit 14: ERROR  — Crossbar error
Bit  7: HSP    — High-speed port
Bit  6: AI     — Audio interface
Bit  5: DSP    — Audio DSP
Bit  4: MEM    — Memory interface
Bit  3: VI     — Video interface (vsync / retrace)
Bit  2: PE     — Pixel engine (EFB ready)
Bit  1: DVD    — DVD interface
Bit  0: SI     — Serial interface (controllers)
```

## Implementation Plan (Milestone 1)

- [ ] Define `PiRegs` volatile register struct
- [ ] Implement `Pi::enable_mask(irq: IrqMask)` — set bits in PIINTMR
- [ ] Implement `Pi::disable_mask(irq: IrqMask)` — clear bits in PIINTMR
- [ ] Implement `Pi::clear_pending(irq: IrqMask)` — write to PIINTSR to ack
- [ ] Implement `Pi::pending()` — read PIINTSR
- [ ] Register a VI retrace handler for vsync-based game loops
- [ ] Register a SI handler for auto-polled controller input

## References

- YAGCD section 9.1 (PI)
- libogc2 `irq.c` (reference only)
