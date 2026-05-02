//! GameCube Memory Card driver.
//!
//! Provides raw page-level access to Nintendo-compatible memory cards
//! (59 / 123 / 251 / 507 / 1019 / 2043 block variants, plus MemCard PRO GC).
//!
//! ## Hardware overview
//!
//! Memory cards connect to EXI channel 0 (slot A) or 1 (slot B) at device 0.
//! Communication is at 16 MHz SPI. The card stores data in 128-byte pages
//! (write granularity) and 8 KB sectors (erase granularity), but reads can
//! fetch 512-byte segments.
//!
//! ## Command protocol
//!
//! All commands are sent as a sequence of EXI immediate writes, then data
//! is transferred via DMA.
//!
//! | Opcode | Command           | Payload                        |
//! |--------|-------------------|--------------------------------|
//! | 0x52   | READ_SEGMENT      | 5-byte address + latency + DMA |
//! | 0xF2   | WRITE_PAGE        | 5-byte address + 128-byte DMA  |
//! | 0xF1   | SECTOR_ERASE      | 4-byte address                 |
//! | 0x83   | READ_STATUS       | 1-byte response                |
//! | 0x89   | CLEAR_STATUS      | no response                    |
//! | 0x81   | ENABLE_INTERRUPT  | 1-byte (0x01 enable / 0x00 disable) |
//!
//! ## Address encoding
//!
//! The 5-byte address frame for READ/WRITE commands encodes a 25-bit byte
//! address as:
//!
//! ```text
//! byte[1] = (addr >> 17) & 0x7F
//! byte[2] = (addr >>  9) & 0xFF
//! byte[3] = (addr >>  7) & 0x03
//! byte[4] = (addr      ) & 0x7F
//! ```
//!
//! ## Read geometry
//!
//! - Read unit: 512 bytes (`SEGMENT_SIZE`)
//! - Write unit: 128 bytes (`PAGE_SIZE`)
//! - Erase unit: 8,192 bytes (`SECTOR_SIZE`)
//!
//! This module exposes `read_segment` (512 B), `write_page` (128 B), and
//! `erase_sector` (8 KB) — the three fundamental operations.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use gc_hal::memcard::{self, CardSlot};
//! static mut BUF: [u8; 512] = [0; 512]; // must be 32-byte aligned
//! unsafe {
//!     let card = memcard::MemCard::probe(CardSlot::A)?;
//!     card.read_segment(0, &mut BUF)?;
//! }
//! ```

#![allow(dead_code)]

use crate::exi::{self, Channel, Device, Freq, Mode, DeviceType};
use gc_rt::timer;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Bytes per read segment.
pub const SEGMENT_SIZE: usize = 512;
/// Bytes per write page.
pub const PAGE_SIZE: usize = 128;
/// Bytes per erase sector.
pub const SECTOR_SIZE: usize = 8192;

// Card status bits
const STATUS_BUSY:     u8 = 0x80; // card is busy (erasing / writing)
const STATUS_UNLOCKED: u8 = 0x40; // card is unlocked for access

// Sector size lookup table (indexed by _ROTL(id, 23) & 0x1C >> 2)
const SECTOR_SIZES: [u32; 8] = [8192, 16384, 32768, 65536, 131072, 262144, 0, 0];
// Latency lookup table (indexed by _ROTL(id, 26) & 0x1C >> 2)
const LATENCIES: [u8; 8] = [4, 8, 16, 32, 64, 128, 0, 0];

// ─── Error type ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardError {
    /// No card in the slot, or wrong device type.
    NoCard,
    /// Card is busy (previous erase/write in progress).
    Busy,
    /// EXI communication error.
    IoError,
    /// Operation timed out.
    Timeout,
    /// Address or buffer alignment out of range.
    InvalidParam,
}

pub type Result<T> = core::result::Result<T, CardError>;

// ─── Slot ─────────────────────────────────────────────────────────────────────

/// Memory card slot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CardSlot {
    /// Slot A — EXI channel 0
    A,
    /// Slot B — EXI channel 1
    B,
}

impl CardSlot {
    fn channel(self) -> Channel {
        match self { CardSlot::A => Channel::Ch0, CardSlot::B => Channel::Ch1 }
    }
}

// ─── MemCard ─────────────────────────────────────────────────────────────────

/// A probed GameCube memory card.
pub struct MemCard {
    slot:        CardSlot,
    card_id:     u32,
    dev_type:    DeviceType,
    /// Total storage in bytes.
    pub total_bytes:  u32,
    /// Erase sector size in bytes.
    pub sector_size:  u32,
    /// Number of erase sectors.
    pub sector_count: u32,
    /// Latency bytes for read commands.
    latency:     u8,
}

impl MemCard {
    /// Probe the slot and return a `MemCard` if a compatible card is present.
    ///
    /// # Safety
    /// EXI must not be in use by another driver on this channel.
    pub unsafe fn probe(slot: CardSlot) -> Result<Self> {
        let ch = slot.channel();
        if !exi::probe(ch) { return Err(CardError::NoCard); }

        let dev_type = exi::get_id(ch, Device::Dev0);

        // Classify and extract geometry
        let (card_id, total_bytes) = match dev_type {
            DeviceType::MemCard59   => (0x04u32, 64 * 1024),
            DeviceType::MemCard123  => (0x08,    128 * 1024),
            DeviceType::MemCard251  => (0x10,    256 * 1024),
            DeviceType::MemCard507  => (0x20,    512 * 1024),
            DeviceType::MemCard1019 => (0x40,   1024 * 1024),
            DeviceType::MemCard2043 => (0x80,   2048 * 1024),
            DeviceType::MemCardPro  => (0x38,   2048 * 1024), // treat as 2043 capacity
            _ => return Err(CardError::NoCard),
        };

        // Compute sector size and latency from card ID bits
        let sector_size = {
            let idx = rotl32(card_id, 23) & 0x1C;
            SECTOR_SIZES[(idx >> 2) as usize]
        };
        let latency = {
            let idx = rotl32(card_id, 26) & 0x1C;
            LATENCIES[(idx >> 2) as usize]
        };

        let sector_count = if sector_size > 0 { total_bytes / sector_size } else { 0 };

        Ok(MemCard { slot, card_id, dev_type, total_bytes, sector_size, sector_count, latency })
    }

    /// Return the device type.
    pub fn device_type(&self) -> DeviceType { self.dev_type }

    /// Read one 512-byte segment from byte address `addr`.
    ///
    /// `buf` must be 32-byte aligned (DMA requirement).
    /// `addr` must be 512-byte aligned.
    pub unsafe fn read_segment(&self, addr: u32, buf: &mut [u8; 512]) -> Result<()> {
        if buf.as_ptr() as usize % 32 != 0 { return Err(CardError::InvalidParam); }
        let ch = self.slot.channel();

        exi::select(ch, Device::Dev0, Freq::Mhz16);

        // Send READ_SEGMENT command with address
        let cmd = encode_addr(0x52, addr);
        let mut cmd_buf = cmd;
        exi::imm(ch, cmd_buf.as_mut_ptr(), 5, Mode::Write);

        // Write `latency` dummy bytes (0xFF) to clock out card startup delay
        let mut dummy = [0xFFu8; 1];
        for _ in 0..self.latency {
            exi::imm(ch, dummy.as_mut_ptr(), 1, Mode::Write);
        }

        // DMA read 512 bytes
        exi::dma(ch, buf.as_mut_ptr(), 512, Mode::Read);
        exi::deselect(ch);
        Ok(())
    }

    /// Write one 128-byte page to byte address `addr`.
    ///
    /// `addr` must be 128-byte aligned.
    /// `buf` must be 32-byte aligned.
    ///
    /// Call [`erase_sector`] before writing to any address in a sector.
    pub unsafe fn write_page(&self, addr: u32, buf: &[u8; 128]) -> Result<()> {
        if buf.as_ptr() as usize % 32 != 0 { return Err(CardError::InvalidParam); }
        let ch = self.slot.channel();

        exi::select(ch, Device::Dev0, Freq::Mhz16);

        // Send WRITE_PAGE command
        let cmd = encode_addr(0xF2, addr);
        let mut cmd_buf = cmd;
        exi::imm(ch, cmd_buf.as_mut_ptr(), 5, Mode::Write);

        // DMA write 128 bytes
        exi::dma(ch, buf.as_ptr() as *mut u8, 128, Mode::Write);
        exi::deselect(ch);

        // Wait for write to complete (status busy bit clears)
        self.wait_not_busy()
    }

    /// Erase a sector (8 KB) at byte address `addr`.
    ///
    /// `addr` must be 8 KB aligned. Erase takes up to ~2 seconds on real hardware.
    pub unsafe fn erase_sector(&self, addr: u32) -> Result<()> {
        let ch = self.slot.channel();

        // SECTOR_ERASE: cmd + 3-byte address
        exi::select(ch, Device::Dev0, Freq::Mhz16);
        let cmd = [
            0xF1u8,
            ((addr >> 17) & 0x7F) as u8,
            ((addr >>  9) & 0xFF) as u8,
            ((addr >>  7) & 0x03) as u8,
        ];
        let mut cmd_buf = cmd;
        exi::imm(ch, cmd_buf.as_mut_ptr(), 4, Mode::Write);
        exi::deselect(ch);

        // Erase can take up to 2 seconds
        self.wait_not_busy_long()
    }

    /// Read the card status byte.
    pub unsafe fn read_status(&self) -> Result<u8> {
        let ch = self.slot.channel();
        exi::select(ch, Device::Dev0, Freq::Mhz16);
        let mut cmd = [0x83u8, 0x00u8];
        exi::imm(ch, cmd.as_mut_ptr(), 2, Mode::Write);
        let mut status = 0xFFu8;
        exi::imm(ch, &mut status as *mut u8, 1, Mode::Read);
        exi::deselect(ch);
        Ok(status)
    }

    /// Clear the card status register.
    pub unsafe fn clear_status(&self) -> Result<()> {
        let ch = self.slot.channel();
        exi::select(ch, Device::Dev0, Freq::Mhz16);
        let mut cmd = [0x89u8];
        exi::imm(ch, cmd.as_mut_ptr(), 1, Mode::Write);
        exi::deselect(ch);
        Ok(())
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    unsafe fn wait_not_busy(&self) -> Result<()> {
        let deadline = timer::tbr64() + 40_500_000u64 * 2; // 2 second
        loop {
            let status = self.read_status()?;
            if status & STATUS_BUSY == 0 { return Ok(()); }
            if timer::tbr64() > deadline { return Err(CardError::Timeout); }
        }
    }

    unsafe fn wait_not_busy_long(&self) -> Result<()> {
        let deadline = timer::tbr64() + 40_500_000u64 * 10; // 10 seconds
        loop {
            let status = self.read_status()?;
            if status & STATUS_BUSY == 0 { return Ok(()); }
            if timer::tbr64() > deadline { return Err(CardError::Timeout); }
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Build a 5-byte [opcode, addr...] command frame for READ/WRITE commands.
fn encode_addr(opcode: u8, addr: u32) -> [u8; 5] {
    [
        opcode,
        ((addr >> 17) & 0x7F) as u8,
        ((addr >>  9) & 0xFF) as u8,
        ((addr >>  7) & 0x03) as u8,
        ( addr        & 0x7F) as u8,
    ]
}

fn rotl32(val: u32, n: u32) -> u32 {
    (val << n) | (val >> (32 - n))
}

// Mhz16 = 4 in our Freq enum, but select() uses (freq as u32) << 4
// so 4 << 4 = 0x40, added to CSR → clock select bits 11:8 = 4 → 16 MHz. Correct.
