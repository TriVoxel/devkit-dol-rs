# TODO: dkdol-hal::exi — External Interface

## What This Is

The EXI bus is a synchronous serial bus connecting:
- Memory Card slot A (EXI channel 0)
- Memory Card slot B (EXI channel 1)  
- IPL ROM / SRAM / RTC (EXI channel 0, device 1)
- Broadband Adapter / Modem / SD Gecko (EXI channel 0, device 0)

**Base address:** `0xCC006800`

## Architecture

### Transfer Protocol

```
1. Assert CS (chip select) for the target device
2. Send command bytes (1–4 bytes, depending on device)
3. Transfer data (read or write) in 4-byte chunks
4. Deassert CS
```

### Register Map (per channel, stride 0x14)

```
EXI0CR  0xCC006800  Channel 0 control register
EXI0MAR 0xCC006804  Channel 0 memory address register (for DMA)
EXI0LEN 0xCC006808  Channel 0 DMA length
EXI0CR2 0xCC00680C  Channel 0 control register 2 (device select, speed)
EXI0DAT 0xCC006810  Channel 0 data register (immediate mode)
```

## Implementation Plan (Milestone 5)

- [ ] Define `EXIRegs` volatile register struct
- [ ] Implement `EXI::transfer(channel, device, data, direction)` — immediate mode
- [ ] Implement `EXI::dma_write(channel, device, src, len)` — DMA mode
- [ ] Implement Memory Card low-level protocol (Milestone 5a)
- [ ] Implement SD Gecko adapter (SPI over EXI) (Milestone 5b)
- [ ] Implement RTC read (wall clock) (Milestone 5c)

## References

- YAGCD section 9.4 (EXI)
- libogc2 `exi.c`, `gcsd.c`, `sdgecko_io.c` (reference only)
