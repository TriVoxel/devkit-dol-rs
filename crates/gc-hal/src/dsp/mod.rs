//! Audio DSP — Yamaha ARAM-DMA and DSP coprocessor.
//!
//! The DSP block handles audio processing and ARAM (16 MB audio RAM) DMA.
//! It uses a mailbox protocol for CPU↔DSP communication.
//!
//! **Status: Stub — see TODO.md**

pub const DSP_BASE: usize = 0xCC005000;
pub const ARAM_BASE: usize = 0x00000000; // accessed via DMA, not directly mapped
