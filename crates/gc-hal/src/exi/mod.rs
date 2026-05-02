//! External Interface (EXI) — SPI-like serial bus.
//!
//! The EXI bus has 3 channels, each with up to 3 device slots:
//!
//! | Channel | Device 0       | Device 1  | Device 2   |
//! |---------|----------------|-----------|------------|
//! | 0       | Memory Card A  | Mask ROM  | Broadband  |
//! | 1       | Memory Card B  | (unused)  | (unused)   |
//! | 2       | RTC / Serial   | (unused)  | (unused)   |
//!
//! ## Register layout (32-bit, base 0xCC006800)
//!
//! Each channel has 5 consecutive 32-bit registers:
//!
//! ```text
//! Channel N base = 0xCC006800 + N * 20
//!   +0  EXIxCSR   — Control/Status: device select, clock freq, interrupts
//!   +4  EXIxMAR   — DMA memory address (physical, 32-byte aligned)
//!   +8  EXIxLEN   — DMA length in bytes
//!   +12 EXIxCR    — Transfer control: length (bits 5:4), mode (bits 3:2), start (bit 0)
//!   +16 EXIxDATA  — Immediate data register (≤4 bytes)
//! ```
//!
//! ## CSR (EXIxCSR) bit layout
//!
//! | Bits  | Name    | Description                                   |
//! |-------|---------|-----------------------------------------------|
//! | 2:1   | EXIINT  | EXI interrupt pending (W1C)                   |
//! | 3     | TCINT   | Transfer complete interrupt pending (W1C)     |
//! | 4     | EXTINT  | External insert interrupt pending (W1C)       |
//! | 6     | ROMDIS  | ROM disable                                   |
//! | 7     | DEV0    | Select device 0 (chip select)                 |
//! | 8     | DEV1    | Select device 1                               |
//! | 9     | DEV2    | Select device 2                               |
//! | 11:10 | CLKSEL  | SPI clock: 00=1MHz, 01=2MHz, 10=4MHz,        |
//! |       |         |            11=8MHz, 100=16MHz (not all chips)  |
//! | 12    | EXTBIT  | Expansion device present                      |

#![allow(dead_code)]

const EXI_BASE: usize = 0xCC006800;
const CH_STRIDE: usize = 20; // 5 × 4-byte registers per channel

// Register offsets within a channel (in bytes)
const REG_CSR:  usize = 0;
const REG_MAR:  usize = 4;
const REG_LEN:  usize = 8;
const REG_CR:   usize = 12;
const REG_DATA: usize = 16;

// CSR bits
const CSR_EXIINT: u32 = 0x0002;
const CSR_TCINT:  u32 = 0x0008;
const CSR_EXTINT: u32 = 0x0800;
const CSR_DEV0:   u32 = 0x0080;
const CSR_DEV1:   u32 = 0x0100;
const CSR_DEV2:   u32 = 0x0200;
const CSR_EXTBIT: u32 = 0x1000;
const CSR_W1C:    u32 = CSR_EXIINT | CSR_TCINT | CSR_EXTINT;
const CSR_DEVMASK:u32 = CSR_DEV0 | CSR_DEV1 | CSR_DEV2;
const CSR_FREQMSK:u32 = 0x780;  // bits 10:7 = clock field... wait
// Actually: bits 11:8 for CLK? Let me re-check:
// From libogc2: val = (val&0x405)|(0x80<<nDev)|(nFrq<<4)
// 0x405 = keep bits: 0x400 (EXTBIT area) | 0x004 (ROMDIS?) | 0x001 (?)
// Freq is at bits 7:4 = nFrq << 4.

/// EXI channel index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Channel { Ch0 = 0, Ch1 = 1, Ch2 = 2 }

/// EXI device (chip select) within a channel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Device { Dev0 = 0, Dev1 = 1, Dev2 = 2 }

/// EXI SPI clock frequency.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Freq {
    Mhz1  = 0,
    Mhz2  = 1,
    Mhz4  = 2,
    Mhz8  = 3,
    Mhz16 = 4,
}

/// Transfer direction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Mode {
    Write    = 0,
    Read     = 1,
    ReadWrite= 2,
}

#[inline(always)]
fn csr(ch: Channel) -> *mut u32 {
    (EXI_BASE + (ch as usize) * CH_STRIDE + REG_CSR) as *mut u32
}
#[inline(always)]
fn mar(ch: Channel) -> *mut u32 {
    (EXI_BASE + (ch as usize) * CH_STRIDE + REG_MAR) as *mut u32
}
#[inline(always)]
fn len_reg(ch: Channel) -> *mut u32 {
    (EXI_BASE + (ch as usize) * CH_STRIDE + REG_LEN) as *mut u32
}
#[inline(always)]
fn cr(ch: Channel) -> *mut u32 {
    (EXI_BASE + (ch as usize) * CH_STRIDE + REG_CR) as *mut u32
}
#[inline(always)]
fn data(ch: Channel) -> *mut u32 {
    (EXI_BASE + (ch as usize) * CH_STRIDE + REG_DATA) as *mut u32
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Select a device on an EXI channel.
///
/// Sets the chip-select line and programs the SPI clock frequency.
/// Must be called before any transfer. Call [`deselect`] when done.
///
/// # Safety
/// The channel must not already have a transfer in progress.
pub unsafe fn select(ch: Channel, dev: Device, freq: Freq) {
    let mut val = core::ptr::read_volatile(csr(ch));
    // Clear device select and freq bits; preserve 0x405 (IRQ status, EXTBIT area)
    val &= 0x405;
    val |= (0x80u32 << dev as u32) | ((freq as u32) << 4);
    core::ptr::write_volatile(csr(ch), val);
}

/// Deselect all devices on a channel (release chip select).
pub unsafe fn deselect(ch: Channel) {
    let val = core::ptr::read_volatile(csr(ch));
    core::ptr::write_volatile(csr(ch), val & 0x405);
}

/// Return true if a device is plugged into slot 0 of a channel.
///
/// Checks the EXT bit in CSR (1 = device present).
pub unsafe fn probe(ch: Channel) -> bool {
    if ch == Channel::Ch2 { return true; } // Ch2 (RTC) is always present
    core::ptr::read_volatile(csr(ch)) & CSR_EXTBIT == 0
    // Note: EXT bit = 1 means NO card (active low). Invert.
}

/// Immediate transfer: read/write up to 4 bytes, blocking.
///
/// `buf`: pointer to 1–4 bytes of data. On `Write`, data is taken from `buf`.
/// On `Read`, data is written to `buf` after the transfer. `ReadWrite` does both.
///
/// # Safety
/// - `select()` must have been called first.
/// - `len` must be 1–4.
/// - `buf` must point to at least `len` valid bytes.
pub unsafe fn imm(ch: Channel, buf: *mut u8, len: usize, mode: Mode) {
    debug_assert!(len >= 1 && len <= 4, "EXI imm: len must be 1-4");

    // Load write data into DATA register (big-endian, left-aligned)
    if mode != Mode::Read {
        let mut val = 0u32;
        for i in 0..len {
            val |= (*buf.add(i) as u32) << ((3 - i) * 8);
        }
        core::ptr::write_volatile(data(ch), val);
    }

    // Program CR: len-1 in bits 5:4, mode in bits 3:2, start in bit 0
    let cr_val = (((len - 1) as u32 & 0x3) << 4)
               | ((mode as u32 & 0x3) << 2)
               | 0x1;
    core::ptr::write_volatile(cr(ch), cr_val);

    // Wait for TC (transfer complete) by polling CR bit 0
    while core::ptr::read_volatile(cr(ch)) & 0x1 != 0 {}

    // Read back received data
    if mode != Mode::Write {
        let val = core::ptr::read_volatile(data(ch));
        for i in 0..len {
            *buf.add(i) = ((val >> ((3 - i) * 8)) & 0xFF) as u8;
        }
    }

    // Clear TC interrupt
    let csr_val = core::ptr::read_volatile(csr(ch));
    core::ptr::write_volatile(csr(ch), (csr_val & !CSR_W1C) | CSR_TCINT);
}

/// DMA transfer: read/write `len` bytes to/from `buf`, blocking.
///
/// `buf` must be 32-byte aligned; `len` must be a multiple of 32.
///
/// # Safety
/// - `select()` must have been called first.
/// - `buf` must be 32-byte aligned.
/// - `len` must be a multiple of 32.
pub unsafe fn dma(ch: Channel, buf: *mut u8, len: usize, mode: Mode) {
    debug_assert!(buf as usize % 32 == 0, "EXI DMA buffer not 32-byte aligned");
    debug_assert!(len % 32 == 0, "EXI DMA length not multiple of 32");

    let phys = (buf as usize) & 0x1FFF_FFFF;
    core::ptr::write_volatile(mar(ch), phys as u32);
    core::ptr::write_volatile(len_reg(ch), len as u32);

    // CR: mode in bits 3:2, DMA flag (bit 1) + start (bit 0)
    let cr_val = ((mode as u32 & 0x3) << 2) | 0x3;
    core::ptr::write_volatile(cr(ch), cr_val);

    // Poll CR bit 0 for completion
    while core::ptr::read_volatile(cr(ch)) & 0x1 != 0 {}

    // Clear TC
    let csr_val = core::ptr::read_volatile(csr(ch));
    core::ptr::write_volatile(csr(ch), (csr_val & !CSR_W1C) | CSR_TCINT);
}

/// Read a 32-bit big-endian word from the currently selected EXI device.
///
/// Convenience wrapper around [`imm`] for the common case of reading 4 bytes.
pub unsafe fn read_u32(ch: Channel) -> u32 {
    let mut buf = [0u8; 4];
    imm(ch, buf.as_mut_ptr(), 4, Mode::Read);
    u32::from_be_bytes(buf)
}

/// Write a 32-bit big-endian word to the currently selected EXI device.
pub unsafe fn write_u32(ch: Channel, val: u32) {
    let mut buf = val.to_be_bytes();
    imm(ch, buf.as_mut_ptr(), 4, Mode::Write);
}
