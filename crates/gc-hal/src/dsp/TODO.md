# TODO: gc-hal::dsp — Audio DSP

## What This Is

The Gekko's DSP coprocessor handles:
- Audio mixing/decoding via a custom DSP instruction set
- ARAM (Audio RAM) DMA — 16 MB of dedicated audio RAM
- Mailbox-based IPC with the main CPU

**DSP registers:** `0xCC005000`
**ARAM:** Accessible only via DMA (not memory-mapped)

## Architecture

### Mailbox Protocol

```
CPU → DSP: write to DSP_MAILBOX_HI (0xCC005004), then DSP_MAILBOX_LO (0xCC005006)
DSP → CPU: read DSP_INBOX_HI (0xCC005000), then DSP_INBOX_LO (0xCC005002)
```

### Bootstrap Sequence

1. Assert DSP reset (DSPCR[RES])
2. Upload DSP microcode (DROM) via ARAM DMA
3. Release reset
4. Handshake via mailbox

The GC IPL includes a default audio ucode (AX audio engine). For custom
audio, you supply your own DSP ucode.

### ARAM DMA

```
Write ARAM address to ARAM_AR_DMA_MMADDR_H/L
Write MEM address  to ARAM_AR_DMA_ARADDR_H/L
Write count        to ARAM_AR_DMA_CNT_H/L
Write control      to ARAM_AR_DMA_CNT_L (bit 0 = direction: 0=ARAM→MEM, 1=MEM→ARAM)
Poll ARAM_AR_DMA_CNT_L until transfer complete
```

## Implementation Plan (Milestone 4)

- [ ] Define DSP register set with volatile reads/writes
- [ ] Implement DSP reset sequence
- [ ] Implement ARAM DMA (MEM↔ARAM transfer)
- [ ] Implement mailbox send/receive
- [ ] Bootstrap with a simple audio ucode (sine wave generator)

## References

- YAGCD section 9.3 (DSP / ARAM)
- libogc2 `dsp.c`, `audio.c` (reference only)
