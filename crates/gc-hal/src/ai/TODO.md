# TODO: gc-hal::ai — Audio Interface

## What This Is

The Audio Interface (AI) streams PCM audio from ARAM to the GameCube's
analog audio output at 32 kHz or 48 kHz.

**Base address:** `0xCC006C00`

## Architecture

The AI DMA reads a stereo 16-bit PCM ring buffer from ARAM at the configured
sample rate. The DSP fills this buffer via ARAM DMA; the AI drains it to the
DAC.

### Register Map

```
AICONTROL  0xCC006C00  AI control: sample rate (0=32kHz, 1=48kHz), DMA enable
AIVOLUME   0xCC006C04  Volume (left/right, 0-255)
AISTARTHI  0xCC006C08  DMA start address high
AISTARTLO  0xCC006C0A  DMA start address low
AILEN      0xCC006C0C  DMA buffer length in samples
AIPOS      0xCC006C10  Current playback position (read-only)
```

## Implementation Plan (Milestone 4)

- [ ] Define `AiRegs` volatile register struct
- [ ] Implement `Ai::init(sample_rate: SampleRate)` — enable AI DMA
- [ ] Implement `Ai::set_volume(left: u8, right: u8)`
- [ ] Implement `Ai::start_dma(aram_addr, len)` — start streaming
- [ ] AI interrupt handler: refill ARAM from a PCM source when buffer runs low
- [ ] High-level `AudioStream` type: submit stereo PCM frames

## References

- YAGCD section 9.5 (AI)
- libogc2 `audio.c` (reference only)
