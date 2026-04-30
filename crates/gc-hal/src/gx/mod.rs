//! Graphics Processor (GX) — FIFO command buffer interface.
//!
//! The GX is a tile-based renderer fed via a 32-byte write-gather FIFO.
//! Commands are written to the write-gather pipe at `0xCC008000`.
//!
//! **Status: Stub — see TODO.md**

// TODO (Milestone 3): Implement GX FIFO, vertex formats, matrix loads
// See crates/gc-hal/src/gx/TODO.md

pub const GX_FIFO_BASE: usize = 0xCC00_8000;
