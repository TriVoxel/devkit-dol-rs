//! External Interface (EXI) — memory cards, SD Gecko, serial.
//!
//! The EXI bus is a SPI-like bus used for the memory card slots,
//! the RTC, SRAM, and expansion hardware like the Broadband Adapter.
//!
//! **Status: Stub — see TODO.md**

pub const EXI_BASE: usize = 0xCC006800;
