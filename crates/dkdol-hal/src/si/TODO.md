# TODO: dkdol-hal::si — Serial Interface (Controllers)

## What This Is

The Serial Interface (SI) bus connects the four controller ports on the front
of the GameCube. It handles:
- Standard GCN pads (buttons, analog sticks, triggers, rumble)
- GCN keyboard
- Steering wheel / DK Bongos
- GBA link cable

**Base address:** `0xCC006400`

## Architecture

### Register Map (partial)

```
0xCC006400  C0    Channel 0 output buffer (command sent to controller)
0xCC006404  C0    Channel 0 input buffer  (response from controller)
0xCC006408  C0    Channel 0 input buffer  (continued)
...         (each channel is +0x0C bytes apart)
0xCC006430  SIPOLL   Poll control register
0xCC006434  SICOMCSR Communication control & status
0xCC006438  SISR     Status register
0xCC00643C  SIEXILK  EXI lock
```

### SI Protocol

Each poll cycle:
1. Write command (1–3 bytes) to the output buffer for each channel
2. Set SIPOLL to enable auto-polling at vsync
3. On SI interrupt: read 3 bytes of response from each channel
4. Decode button/axis state from the response bytes

## Implementation Plan (Milestone 2)

- [ ] Define `SiRegs` volatile register struct at `0xCC006400`
- [ ] Define `PadState` struct: buttons (u16 bitfield), sticks (i8), triggers (u8)
- [ ] Implement `Si::poll_channel(port: Port)` — reads one controller's state
- [ ] Implement `Si::init()` — configure SIPOLL for 4-channel auto-poll
- [ ] Implement SI interrupt handler (Milestone 2, depends on Milestone 1 IRQ)
- [ ] High-level `Pad::read(port)` convenience function

## References

- YAGCD section 9.2 (SI)
- libogc2 `si.c` / `pad.c` (reference only, not linked)
