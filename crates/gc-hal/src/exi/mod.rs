//! External Interface (EXI) — SPI-like serial bus.
//!
//! ## Hardware
//!
//! 3 channels, each with up to 3 device slots:
//!
//! | Channel | Device 0       | Device 1       | Device 2    |
//! |---------|----------------|----------------|-------------|
//! | 0       | Memory Card A  | Mask ROM / BBA | Serial Port |
//! | 1       | Memory Card B  | (expansion)    | —           |
//! | 2       | RTC / IPL      | —              | —           |
//!
//! ## Register layout (32-bit at `0xCC006800`)
//!
//! Each channel occupies 5 × u32 registers:
//!
//! ```text
//! Channel N base = 0xCC006800 + N * 20
//!   +0  EXIxCSR   — Control/Status (device select, clock, interrupts)
//!   +4  EXIxMAR   — DMA address (physical, 32-byte aligned)
//!   +8  EXIxLEN   — DMA length (bytes)
//!   +12 EXIxCR    — Transfer: len(5:4), mode(3:2), start(0)
//!   +16 EXIxDATA  — Immediate data register (≤4 bytes)
//! ```
//!
//! ## CSR fields
//!
//! | Bits  | Name    | Description                                |
//! |-------|---------|---------------------------------------------|
//! | 1     | EXIINT  | EXI interrupt pending (W1C)                |
//! | 3     | TCINT   | Transfer complete (W1C)                    |
//! | 4     | EXTINT  | External insert (W1C)                      |
//! | 7     | DEV0    | Select device 0                            |
//! | 8     | DEV1    | Select device 1                            |
//! | 9     | DEV2    | Select device 2                            |
//! | 7:4   | CLKSEL  | SPI clock: see [`Freq`]                    |
//! | 12    | EXTBIT  | Device present (read-only)                 |

#![allow(dead_code)]

const EXI_BASE: usize = 0xCC006800;
const CH_STRIDE: usize = 20;

const REG_CSR:  usize = 0;
const REG_MAR:  usize = 4;
const REG_LEN:  usize = 8;
const REG_CR:   usize = 12;
const REG_DATA: usize = 16;

const CSR_EXIINT: u32 = 0x0002;
const CSR_TCINT:  u32 = 0x0008;
const CSR_EXTINT: u32 = 0x0800;
const CSR_EXTBIT: u32 = 0x1000;
const CSR_W1C:    u32 = CSR_EXIINT | CSR_TCINT | CSR_EXTINT;

// ─── Public types ─────────────────────────────────────────────────────────────

/// EXI channel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Channel { Ch0 = 0, Ch1 = 1, Ch2 = 2 }

/// EXI device (chip select) within a channel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Device { Dev0 = 0, Dev1 = 1, Dev2 = 2 }

/// SPI clock frequency.
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
pub enum Mode { Write = 0, Read = 1, ReadWrite = 2 }

/// Known device types, identified by the 4-byte EXI device ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    /// Nintendo/3rd-party memory card, 59 blocks (64 KB)
    MemCard59,
    /// Memory card, 123 blocks (128 KB)
    MemCard123,
    /// Memory card, 251 blocks (256 KB)
    MemCard251,
    /// Memory card, 507 blocks (512 KB)
    MemCard507,
    /// Memory card, 1019 blocks (1 MB)
    MemCard1019,
    /// Memory card, 2043 blocks (2 MB)
    MemCard2043,
    /// MemCard PRO GC
    MemCardPro,
    /// Nintendo Broadband Adapter (BBA)
    BroadbandAdapter,
    /// IDE-EXI hard drive adapter
    IdeExi,
    /// SD card via SD Gecko (detected by card init, not EXI ID)
    SdCard,
    /// Device present but unrecognised; contains raw ID.
    Unknown(u32),
    /// No device (ID read as 0x00000000 or 0xFFFFFFFF).
    None,
}

impl DeviceType {
    /// Return `true` if this is a GC memory card type.
    pub fn is_memory_card(self) -> bool {
        matches!(self,
            DeviceType::MemCard59 | DeviceType::MemCard123 |
            DeviceType::MemCard251 | DeviceType::MemCard507 |
            DeviceType::MemCard1019 | DeviceType::MemCard2043 |
            DeviceType::MemCardPro
        )
    }

    /// Return the memory card raw size in bytes, or 0 for non-card devices.
    pub fn card_bytes(self) -> u32 {
        match self {
            DeviceType::MemCard59   => 512 * 1024 / 8,   // 64 KB
            DeviceType::MemCard123  => 128 * 1024,
            DeviceType::MemCard251  => 256 * 1024,
            DeviceType::MemCard507  => 512 * 1024,
            DeviceType::MemCard1019 => 1024 * 1024,
            DeviceType::MemCard2043 => 2048 * 1024,
            _ => 0,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DeviceType::MemCard59   => "Memory Card 59",
            DeviceType::MemCard123  => "Memory Card 123",
            DeviceType::MemCard251  => "Memory Card 251",
            DeviceType::MemCard507  => "Memory Card 507",
            DeviceType::MemCard1019 => "Memory Card 1019",
            DeviceType::MemCard2043 => "Memory Card 2043",
            DeviceType::MemCardPro  => "MemCard PRO GC",
            DeviceType::BroadbandAdapter => "Broadband Adapter",
            DeviceType::IdeExi      => "IDE-EXI",
            DeviceType::SdCard      => "SD Card (SD Gecko)",
            DeviceType::Unknown(_)  => "Unknown Device",
            DeviceType::None        => "None",
        }
    }
}

// ─── Register accessors ───────────────────────────────────────────────────────

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

// ─── Public API ───────────────────────────────────────────────────────────────

/// Select a device on an EXI channel (assert CS, set clock).
pub unsafe fn select(ch: Channel, dev: Device, freq: Freq) {
    let mut val = core::ptr::read_volatile(csr(ch));
    val &= 0x405;
    val |= (0x80u32 << dev as u32) | ((freq as u32) << 4);
    core::ptr::write_volatile(csr(ch), val);
}

/// Deselect all devices on a channel.
pub unsafe fn deselect(ch: Channel) {
    let val = core::ptr::read_volatile(csr(ch));
    core::ptr::write_volatile(csr(ch), val & 0x405);
}

/// Return `true` if a device is present on the slot.
///
/// For Ch2 (RTC/IPL) this always returns `true`.
pub unsafe fn probe(ch: Channel) -> bool {
    if ch == Channel::Ch2 { return true; }
    // EXT bit = 1 means NO card (active-low insertion detect)
    core::ptr::read_volatile(csr(ch)) & CSR_EXTBIT == 0
}

/// Read the 32-bit device ID.
///
/// Protocol: select at 1 MHz, write 2 zero bytes, read 4 bytes, deselect.
/// Returns [`DeviceType::None`] if the slot is empty or returns 0/0xFFFFFFFF.
///
/// # Safety
/// The channel must not already be in use.
pub unsafe fn get_id(ch: Channel, dev: Device) -> DeviceType {
    select(ch, dev, Freq::Mhz1);
    let mut cmd = [0u8; 2];
    imm(ch, cmd.as_mut_ptr(), 2, Mode::Write);
    let mut buf = [0u8; 4];
    imm(ch, buf.as_mut_ptr(), 4, Mode::Read);
    deselect(ch);
    classify_id(u32::from_be_bytes(buf))
}

fn classify_id(id: u32) -> DeviceType {
    if id == 0 || id == 0xFFFF_FFFF { return DeviceType::None; }
    // Memory cards: id & ~0xFF == 0 and id & 0x03 == 0
    if id & !0xFF == 0 && id & 0x03 == 0 {
        match id & 0xFC {
            0x04 => return DeviceType::MemCard59,
            0x08 => return DeviceType::MemCard123,
            0x10 => return DeviceType::MemCard251,
            0x20 => return DeviceType::MemCard507,
            0x40 => return DeviceType::MemCard1019,
            0x80 => return DeviceType::MemCard2043,
            _ => {}
        }
    }
    match id & !0xFFFF {
        0x3842_0000 => return DeviceType::MemCardPro,
        _ => {}
    }
    match id & !0xFF {
        0x0402_0000 | 0x0402_0100 | 0x0402_0200 | 0x0402_0300 => {
            return DeviceType::BroadbandAdapter;
        }
        0x4944_4500 => return DeviceType::IdeExi,
        _ => {}
    }
    DeviceType::Unknown(id)
}

/// Immediate transfer: read/write up to 4 bytes (blocking).
///
/// # Safety
/// `select()` must have been called first.
pub unsafe fn imm(ch: Channel, buf: *mut u8, len: usize, mode: Mode) {
    debug_assert!(len >= 1 && len <= 4, "EXI imm: len must be 1–4");

    if mode != Mode::Read {
        let mut val = 0u32;
        for i in 0..len {
            val |= (*buf.add(i) as u32) << ((3 - i) * 8);
        }
        core::ptr::write_volatile(data(ch), val);
    }

    let cr_val = (((len - 1) as u32 & 0x3) << 4)
               | ((mode as u32 & 0x3) << 2)
               | 0x1;
    core::ptr::write_volatile(cr(ch), cr_val);
    while core::ptr::read_volatile(cr(ch)) & 0x1 != 0 {}

    if mode != Mode::Write {
        let val = core::ptr::read_volatile(data(ch));
        for i in 0..len {
            *buf.add(i) = ((val >> ((3 - i) * 8)) & 0xFF) as u8;
        }
    }

    let csr_val = core::ptr::read_volatile(csr(ch));
    core::ptr::write_volatile(csr(ch), (csr_val & !CSR_W1C) | CSR_TCINT);
}

/// DMA transfer: read/write `len` bytes (blocking).
///
/// `buf` must be 32-byte aligned; `len` must be a multiple of 32.
pub unsafe fn dma(ch: Channel, buf: *mut u8, len: usize, mode: Mode) {
    debug_assert!(buf as usize % 32 == 0, "EXI DMA: buffer not 32-byte aligned");
    debug_assert!(len % 32 == 0, "EXI DMA: length not multiple of 32");

    let phys = (buf as usize) & 0x1FFF_FFFF;
    core::ptr::write_volatile(mar(ch), phys as u32);
    core::ptr::write_volatile(len_reg(ch), len as u32);
    core::ptr::write_volatile(cr(ch), ((mode as u32 & 0x3) << 2) | 0x3);
    while core::ptr::read_volatile(cr(ch)) & 0x1 != 0 {}

    let csr_val = core::ptr::read_volatile(csr(ch));
    core::ptr::write_volatile(csr(ch), (csr_val & !CSR_W1C) | CSR_TCINT);
}

/// Read a 32-bit big-endian word from the selected device.
pub unsafe fn read_u32(ch: Channel) -> u32 {
    let mut buf = [0u8; 4];
    imm(ch, buf.as_mut_ptr(), 4, Mode::Read);
    u32::from_be_bytes(buf)
}

/// Write a 32-bit big-endian word to the selected device.
pub unsafe fn write_u32(ch: Channel, val: u32) {
    let mut buf = val.to_be_bytes();
    imm(ch, buf.as_mut_ptr(), 4, Mode::Write);
}
