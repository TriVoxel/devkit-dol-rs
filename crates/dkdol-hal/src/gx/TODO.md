# TODO: dkdol-hal::gx — Graphics Processor

## What This Is

The GX is a dedicated GPU connected to the CPU via a 32-byte write-gather FIFO
at `0xCC008000`. It performs:
- Vertex transformation (via the vertex cache)
- Rasterisation, lighting, texture sampling (16 MB TMEM)
- Anti-aliasing, fog, alpha blending
- EFB → XFB copy (writes rendered output to the XFB read by VI)

**Base address:** `0xCC008000` (write-gather pipe)

## Architecture

GX commands are sent as a byte stream to the FIFO. There is no read-back
from the FIFO; all communication is one-way (CPU→GPU).

### Key Concepts

- **GX FIFO**: a circular buffer (CPU writes, GPU reads). The CPU maps a
  4 KB window at `0xCC008000` and writes 32-byte aligned bursts via the
  write-gather pipe (wgpipe).
- **Display List**: a pre-recorded stream of GX commands, played back from RAM.
- **VTXFMT**: vertex format descriptor registers — define the layout of each
  vertex in the vertex stream.
- **GXSetZMode / GXSetBlendMode / GXSetTevOrder**: state registers in the
  GX register file (addressed via Index loads to the FIFO).

### FIFO Write Example (pseudo)

```rust
// Enable write-gather pipe
wgpipe::write8(GX_CMD_DRAW_QUADS | GX_VTXFMT0);
wgpipe::write16(4); // vertex count
// vertex 0
wgpipe::write_f32(x0); wgpipe::write_f32(y0); wgpipe::write_f32(0.0);
wgpipe::write_rgba(r, g, b, a);
// ... repeat for vertices 1-3
```

## Implementation Plan (Milestone 3)

- [ ] Set up the GX FIFO (memory-mapped CPU FIFO, WPAR register)
- [ ] Implement `GxInit` equivalent — clear GX state, default register values
- [ ] Define `WgPipe` write helpers (write8/16/32/f32)
- [ ] Implement vertex format (VTXFMT) register programming
- [ ] Implement matrix load (projection, modelview) via GX_CMD_LOAD_MATRIX
- [ ] Implement `GxDrawQuads`, `GxDrawTriangles`, etc.
- [ ] Implement texture object (TMEM) upload: `GxInitTexObj`, `GxLoadTexObj`
- [ ] Implement EFB→XFB copy (`GxCopyDisp` equivalent)
- [ ] Example: spinning triangle

## References

- YAGCD section 11 (GX)
- Dolphin source (gfx_vertex_loader, cp_regs, bp_regs) — very helpful
- libogc2 `gx.c` / `gx_regdef.h` (reference only)
