//! SD/SDHC card driver over EXI (SD Gecko / SD adapter).
//!
//! Reads from SD or SDHC cards plugged into a "SD Gecko" adapter in the
//! GameCube memory card slots. Uses the EXI bus in SPI mode.
//!
//! ## Hardware setup
//!
//! An SD Gecko adapter connects an SD card to the EXI bus via the memory
//! card slot. The card's SPI interface is wired to EXI channel 0 (slot A)
//! or channel 1 (slot B), device 0.
//!
//! ## Protocol (SPI mode)
//!
//! All SD commands are 6 bytes: `[0x40|cmd, arg[3], arg[2], arg[1], arg[0], crc7|0x01]`
//!
//! Init sequence:
//! 1. Clock out ≥74 cycles with CS high (10 × 0xFF bytes)
//! 2. CMD0 — reset to SPI mode; expect R1 = 0x01
//! 3. CMD8 — interface condition (3.3V, check pattern 0xAA)
//! 4. ACMD41 — init, set HCS bit for SDHC; poll until idle bit clears
//! 5. CMD58 — read OCR; check CCS bit for block vs byte addressing
//! 6. CMD16 — set block length to 512 (not needed for SDHC)
//!
//! Read sequence:
//! 1. CMD17 (single) or CMD18 (multi) with sector address
//! 2. Wait for 0xFE data token
//! 3. Read 512 bytes + 2 bytes CRC16
//!
//! ## Usage
//!
//! ```rust,no_run
//! use gc_hal::sd::{self, Slot};
//!
//! static mut BUF: [u8; 512] = [0; 512];
//!
//! unsafe {
//!     let mut card = sd::SdCard::new(Slot::A);
//!     card.init().expect("SD card not found");
//!     card.read_sector(0, &mut BUF).expect("read failed");
//! }
//! ```

use crate::exi::{self, Channel, Device, Freq, Mode};
use gc_rt::timer;

// ─── Error type ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdError {
    /// No card detected in the slot.
    NoCard,
    /// Card did not respond in time.
    Timeout,
    /// CRC mismatch on received data.
    CrcError,
    /// Card returned an unexpected or illegal command response.
    BadResponse,
    /// Card is busy (previous write still in progress).
    Busy,
    /// Sector address out of range.
    OutOfRange,
}

pub type Result<T> = core::result::Result<T, SdError>;

// ─── Slot selection ───────────────────────────────────────────────────────────

/// Which slot the SD Gecko / SD2SP2 adapter is connected to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    /// Memory card slot A — EXI channel 0, device 0 (SD Gecko)
    A,
    /// Memory card slot B — EXI channel 1, device 0 (SD Gecko)
    B,
    /// Serial Port 2 (bottom of console) — EXI channel 2, device 0 (SD2SP2)
    Sp2,
}

impl Slot {
    fn channel(self) -> Channel {
        match self {
            Slot::A   => Channel::Ch0,
            Slot::B   => Channel::Ch1,
            Slot::Sp2 => Channel::Ch2,
        }
    }
}

// ─── Card type ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CardType {
    /// Standard SD (byte addressing; address = sector × 512)
    Sd,
    /// SDHC/SDXC (block addressing; address = sector number)
    Sdhc,
}

// ─── SdCard ──────────────────────────────────────────────────────────────────

/// An SD/SDHC card connected via the SD Gecko adapter.
pub struct SdCard {
    slot: Slot,
    card_type: CardType,
    sectors: u32,
    initialized: bool,
}

impl SdCard {
    /// Create a new (uninitialized) card handle for the given slot.
    pub const fn new(slot: Slot) -> Self {
        SdCard {
            slot,
            card_type: CardType::Sd,
            sectors: 0,
            initialized: false,
        }
    }

    /// Initialize the SD card. Must be called before any read/write.
    ///
    /// Returns `Err(SdError::NoCard)` if no card is detected.
    ///
    /// # Safety
    /// EXI must not be in use by another driver on the same channel.
    pub unsafe fn init(&mut self) -> Result<()> {
        let ch = self.slot.channel();

        // Check card present (EXT bit low = card present)
        if !exi::probe(ch) { return Err(SdError::NoCard); }

        // ── 1. Power-up: ≥74 clock cycles with CS deasserted ─────────────
        // Send 10 × 0xFF = 80 clock edges while card is not selected.
        // We do this by selecting at 400 kHz (closest available = 1 MHz) then
        // sending 0xFF bytes, but SPI mode requires CS high during init clocks.
        // We set freq but don't actually assert CS — EXI handles this via
        // write_deselected helper below.
        spi_clock_init(ch);

        // ── 2. CMD0 — GO_IDLE_STATE ───────────────────────────────────────
        exi::select(ch, Device::Dev0, Freq::Mhz1);
        let r1 = send_cmd_r1(ch, 0, 0)?;
        exi::deselect(ch);
        send_ff(ch); // clock out idle byte
        if r1 != 0x01 { return Err(SdError::BadResponse); }

        // ── 3. CMD8 — SEND_IF_COND (check 3.3V + pattern 0xAA) ──────────
        // Arg: VHS=0001 (3.3V), check pattern=0xAA → 0x000001AA
        let mut is_v2 = false;
        exi::select(ch, Device::Dev0, Freq::Mhz1);
        if let Ok(r1) = send_cmd_r1(ch, 8, 0x000001AA) {
            if r1 == 0x01 {
                // Read 4 additional bytes of R7 response
                let mut resp = [0xFFu8; 4];
                for b in resp.iter_mut() {
                    *b = read_byte(ch)?;
                }
                // Check voltage accepted and echo pattern
                if resp[2] == 0x01 && resp[3] == 0xAA { is_v2 = true; }
            }
        }
        exi::deselect(ch);
        send_ff(ch);

        // ── 4. ACMD41 — SD_SEND_OP_COND (poll until card ready) ──────────
        // HCS bit (bit 30) set for SDHC support in CMD8-aware cards
        let acmd41_arg = if is_v2 { 0x4000_0000u32 } else { 0 };
        let deadline = timer::tbr64() + 40_500_000; // ~1 second timeout at 40.5 MHz TBR
        loop {
            // ACMD41 = CMD55 + CMD41
            exi::select(ch, Device::Dev0, Freq::Mhz1);
            let r55 = send_cmd_r1(ch, 55, 0)?;
            exi::deselect(ch);
            send_ff(ch);
            if r55 & 0xFE != 0 { return Err(SdError::BadResponse); }

            exi::select(ch, Device::Dev0, Freq::Mhz1);
            let r41 = send_cmd_r1(ch, 41, acmd41_arg)?;
            exi::deselect(ch);
            send_ff(ch);

            if r41 == 0x00 { break; } // ready
            if r41 & 0xFE != 0 { return Err(SdError::BadResponse); }
            if timer::tbr64() > deadline { return Err(SdError::Timeout); }
        }

        // ── 5. CMD58 — READ_OCR (check CCS for SDHC) ─────────────────────
        let mut card_type = CardType::Sd;
        exi::select(ch, Device::Dev0, Freq::Mhz1);
        let r1 = send_cmd_r1(ch, 58, 0)?;
        if r1 == 0x00 {
            let mut ocr = [0xFFu8; 4];
            for b in ocr.iter_mut() { *b = read_byte(ch)?; }
            if ocr[0] & 0x40 != 0 { card_type = CardType::Sdhc; }
        }
        exi::deselect(ch);
        send_ff(ch);

        // ── 6. CMD16 — SET_BLOCKLEN to 512 (SD only, SDHC ignores) ───────
        if card_type == CardType::Sd {
            exi::select(ch, Device::Dev0, Freq::Mhz1);
            let r = send_cmd_r1(ch, 16, 512)?;
            exi::deselect(ch);
            send_ff(ch);
            if r != 0x00 { return Err(SdError::BadResponse); }
        }

        // ── 7. CMD9 — SEND_CSD (read card capacity) ───────────────────────
        exi::select(ch, Device::Dev0, Freq::Mhz1);
        let r = send_cmd_r1(ch, 9, 0)?;
        exi::deselect(ch);
        send_ff(ch);
        if r != 0x00 { return Err(SdError::BadResponse); }

        // Read 16-byte CSD register via data block
        let mut csd = [0u8; 16];
        exi::select(ch, Device::Dev0, Freq::Mhz1);
        wait_data_token(ch)?;
        for b in csd.iter_mut() { *b = read_byte(ch)?; }
        let _crc = [read_byte(ch)?, read_byte(ch)?]; // discard CRC
        exi::deselect(ch);
        send_ff(ch);

        self.sectors = csd_to_sectors(&csd, card_type);

        // Switch to higher clock speed (8 MHz) for data transfers
        // The EXI imm/DMA functions already handle the freq parameter;
        // just store it — we'll use Mhz8 for all data transfers now.

        self.card_type = card_type;
        self.initialized = true;
        Ok(())
    }

    /// Return the total number of 512-byte sectors on the card.
    pub fn sectors(&self) -> u32 { self.sectors }

    /// Return true if an SD card is initialized and ready.
    pub fn is_ready(&self) -> bool { self.initialized }

    /// Read one 512-byte sector into `buf`.
    ///
    /// `buf` must be exactly 512 bytes. For DMA mode it must also be
    /// 32-byte aligned; `imm` mode works with any alignment.
    pub unsafe fn read_sector(&self, sector: u32, buf: &mut [u8; 512]) -> Result<()> {
        if !self.initialized { return Err(SdError::NoCard); }
        let addr = if self.card_type == CardType::Sdhc { sector } else { sector * 512 };
        let ch = self.slot.channel();

        exi::select(ch, Device::Dev0, Freq::Mhz8);
        let r = send_cmd_r1(ch, 17, addr)?; // CMD17 = READ_SINGLE_BLOCK
        if r != 0x00 { exi::deselect(ch); return Err(SdError::BadResponse); }

        // Wait for data start token 0xFE
        wait_data_token(ch)?;

        // Read 512 bytes
        for b in buf.iter_mut() { *b = read_byte(ch)?; }

        // Read and verify CRC16
        let crc_hi = read_byte(ch)? as u16;
        let crc_lo = read_byte(ch)? as u16;
        let received_crc = (crc_hi << 8) | crc_lo;
        let computed_crc = crc16(buf);
        exi::deselect(ch);
        send_ff(ch);

        if received_crc != computed_crc { return Err(SdError::CrcError); }
        Ok(())
    }

    /// Write one 512-byte sector from `buf`.
    pub unsafe fn write_sector(&self, sector: u32, buf: &[u8; 512]) -> Result<()> {
        if !self.initialized { return Err(SdError::NoCard); }
        let addr = if self.card_type == CardType::Sdhc { sector } else { sector * 512 };
        let ch = self.slot.channel();

        exi::select(ch, Device::Dev0, Freq::Mhz8);
        let r = send_cmd_r1(ch, 24, addr)?; // CMD24 = WRITE_BLOCK
        if r != 0x00 { exi::deselect(ch); return Err(SdError::BadResponse); }

        // Send data start token
        write_byte(ch, 0xFE)?;

        // Write 512 bytes
        for &b in buf.iter() { write_byte(ch, b)?; }

        // Send CRC16
        let crc = crc16(buf);
        write_byte(ch, (crc >> 8) as u8)?;
        write_byte(ch, (crc & 0xFF) as u8)?;

        // Read data response token (bits 4:1 = status, 5 = accepted)
        let resp = read_byte(ch)?;
        if (resp & 0x1F) != 0x05 {
            exi::deselect(ch);
            return Err(SdError::CrcError); // data rejected
        }

        // Wait for card to finish writing (busy = 0x00)
        let deadline = timer::tbr64() + 40_500_000; // ~1 second
        loop {
            let b = read_byte(ch)?;
            if b != 0x00 { break; }
            if timer::tbr64() > deadline {
                exi::deselect(ch);
                return Err(SdError::Timeout);
            }
        }

        exi::deselect(ch);
        send_ff(ch);
        Ok(())
    }

    /// Read multiple consecutive sectors.
    pub unsafe fn read_sectors(&self, start: u32, count: u32, buf: &mut [u8]) -> Result<()> {
        if !self.initialized { return Err(SdError::NoCard); }
        if buf.len() < (count as usize) * 512 { return Err(SdError::OutOfRange); }
        for i in 0..count {
            let sector_buf: &mut [u8; 512] = buf[i as usize * 512..][..512].try_into()
                .map_err(|_| SdError::OutOfRange)?;
            self.read_sector(start + i, sector_buf)?;
        }
        Ok(())
    }
}

// ─── Low-level SPI helpers ────────────────────────────────────────────────────

/// Send 10 × 0xFF (80 clock pulses) without selecting the card.
unsafe fn spi_clock_init(ch: Channel) {
    // To generate clocks without CS, select momentarily at very low speed
    // and send dummy bytes. Workaround: select, send, deselect in quick succession.
    // Some adapters require CS=high; for EXI we can only generate clocks while selected.
    // The card will ignore commands while not in SPI mode yet; it just needs clocks.
    exi::select(ch, Device::Dev0, Freq::Mhz1);
    let mut ff = [0xFFu8; 1];
    for _ in 0..10 {
        exi::imm(ch, ff.as_mut_ptr(), 1, Mode::ReadWrite);
    }
    exi::deselect(ch);
}

/// Send an SD SPI command and read the R1 response byte.
unsafe fn send_cmd_r1(ch: Channel, cmd: u8, arg: u32) -> Result<u8> {
    let mut frame = [
        0x40 | cmd,
        ((arg >> 24) & 0xFF) as u8,
        ((arg >> 16) & 0xFF) as u8,
        ((arg >>  8) & 0xFF) as u8,
        ( arg        & 0xFF) as u8,
        crc7_cmd(cmd, arg),
    ];
    for b in frame.iter_mut() {
        write_byte(ch, *b)?;
    }

    // Poll for response (bit 7 = 0 means valid)
    for _ in 0..8 {
        let r = read_byte(ch)?;
        if r & 0x80 == 0 { return Ok(r); }
    }
    Err(SdError::Timeout)
}

/// Wait for the 0xFE data start token.
unsafe fn wait_data_token(ch: Channel) -> Result<()> {
    let deadline = timer::tbr64() + 40_500_000;
    loop {
        let b = read_byte(ch)?;
        if b == 0xFE { return Ok(()); }
        if b != 0xFF { return Err(SdError::BadResponse); }
        if timer::tbr64() > deadline { return Err(SdError::Timeout); }
    }
}

/// Read one byte from the EXI bus (sends 0xFF as the MOSI byte in SPI).
#[inline(always)]
unsafe fn read_byte(ch: Channel) -> Result<u8> {
    let mut b = 0xFFu8;
    exi::imm(ch, &mut b as *mut u8, 1, Mode::Read);
    Ok(b)
}

/// Write one byte to the EXI bus.
#[inline(always)]
unsafe fn write_byte(ch: Channel, val: u8) -> Result<()> {
    let mut b = val;
    exi::imm(ch, &mut b as *mut u8, 1, Mode::Write);
    Ok(())
}

/// Send one 0xFF idle byte (deselected clock pulse).
unsafe fn send_ff(ch: Channel) {
    exi::select(ch, Device::Dev0, Freq::Mhz1);
    let _ = read_byte(ch);
    exi::deselect(ch);
}

// ─── CRC helpers ─────────────────────────────────────────────────────────────

/// Compute the CRC7 for an SD command frame.
/// The command-specific values are precomputed for the common cases.
fn crc7_cmd(cmd: u8, arg: u32) -> u8 {
    let frame = [
        0x40 | cmd,
        ((arg >> 24) & 0xFF) as u8,
        ((arg >> 16) & 0xFF) as u8,
        ((arg >>  8) & 0xFF) as u8,
        ( arg        & 0xFF) as u8,
    ];
    let mut crc: u8 = 0;
    for &byte in &frame {
        crc ^= byte;
        for _ in 0..8 {
            if crc & 0x80 != 0 { crc = (crc << 1) ^ 0x09; }
            else { crc <<= 1; }
        }
    }
    (crc << 1) | 1 // end bit
}

/// Compute CRC16 (CCITT) over a 512-byte sector buffer.
fn crc16(buf: &[u8; 512]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in buf.iter() {
        let pos = (((crc >> 8) as u8) ^ byte) as usize;
        // CRC16-CCITT polynomial 0x1021 — compute table on the fly
        let mut entry = (pos as u16) << 8;
        for _ in 0..8 {
            if entry & 0x8000 != 0 { entry = (entry << 1) ^ 0x1021; }
            else { entry <<= 1; }
        }
        crc = entry ^ (crc << 8);
    }
    crc
}

// ─── CSD parsing ─────────────────────────────────────────────────────────────

/// Extract the total sector count from a 16-byte CSD register.
fn csd_to_sectors(csd: &[u8; 16], card_type: CardType) -> u32 {
    let csd_structure = (csd[0] >> 6) & 0x03;
    match (csd_structure, card_type) {
        // CSD v1 (SD cards ≤2 GB)
        (0, CardType::Sd) => {
            let c_size = (((csd[6] & 0x03) as u32) << 10)
                       | ((csd[7] as u32) << 2)
                       | (((csd[8] >> 6) & 0x03) as u32);
            let c_size_mult = (((csd[9] & 0x03) as u32) << 1)
                            | (((csd[10] >> 7) & 0x01) as u32);
            let read_bl_len = (csd[5] & 0x0F) as u32;
            let block_len = 1u32 << read_bl_len;
            let mult = 1u32 << (c_size_mult + 2);
            let block_count = (c_size + 1) * mult;
            block_count * (block_len / 512)
        }
        // CSD v2 (SDHC/SDXC)
        (1, CardType::Sdhc) | (_, CardType::Sdhc) => {
            let c_size = (((csd[7] & 0x3F) as u32) << 16)
                       | ((csd[8] as u32) << 8)
                       | (csd[9] as u32);
            (c_size + 1) * 1024 // each unit = 512 KB = 1024 sectors
        }
        _ => 0,
    }
}
