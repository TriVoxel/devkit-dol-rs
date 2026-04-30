# TODO: gc-hal::dvd — DVD Drive Interface

## What This Is

The DVD drive interface provides access to GameCube optical discs.
The IPL contains the low-level drive firmware; homebrew communicates
via a command/response protocol over a memory-mapped register file.

**Base address:** `0xCC006000`

## Architecture

### Register Map (partial)

```
DVDSR    0xCC006000  Status register
DVCVR    0xCC006004  Cover register (disc present / door open)
DVCMDBUF 0xCC006008  Command buffer (12 bytes, 3 × 32-bit words)
DVDDMABUF 0xCC006014 DMA start address
DVDDMALEN 0xCC006018 DMA length
DVDCR    0xCC00601C  Control register (start DMA, interrupt enable)
DVDIMMBUF 0xCC006020 Immediate data buffer (for short reads)
DVDERR   0xCC006024  Error code register
```

### Read Sequence

1. Write 12-byte command to DVCMDBUF (first word: command byte + LBA)
2. Write destination address to DVDDMABUF
3. Write byte count to DVDDMALEN
4. Write 1 to DVDCR[DMA] to start
5. Poll DVDSR until BUSY clears
6. Read data from destination buffer

## Implementation Plan (Milestone 5)

- [ ] Define `DvdRegs` volatile register struct
- [ ] Implement `Dvd::read_sector(lba: u32, buf: &mut [u8])` — 2 KB sectors
- [ ] Implement `Dvd::get_disc_id()` — read the 32-byte disc header
- [ ] Implement `Dvd::seek(lba: u32)` — pre-position for streaming reads
- [ ] Disc file system layer (GCM/ISO filesystem parser)

## References

- YAGCD section 9.6 (DVD)
- libogc2 `dvd.c`, `dvdlow.c` (reference only)
