//! Unified block device abstraction.
//!
//! All storage devices (SD card, memory card, DVD) implement [`BlockDevice`],
//! allowing higher-level code (filesystem layers, raw dump tools) to work
//! with any storage backend interchangeably.
//!
//! ## Sector sizes
//!
//! Different devices use different native sector sizes:
//!
//! | Device          | Logical sector | Notes                          |
//! |-----------------|----------------|--------------------------------|
//! | SD card         | 512 bytes      | Standard SD / SDHC             |
//! | Memory card     | 512 bytes      | Read segment (CARD_READSIZE)   |
//! | DVD disc        | 2048 bytes     | Standard GC disc sector        |
//!
//! ## Device discovery
//!
//! [`scan`] probes all known EXI slots and the DVD drive, returning a list
//! of detected devices with their type and geometry.

use crate::exi::{self, Channel, Device, DeviceType};

pub use crate::sd::{SdCard, Slot as SdSlot, SdError};
pub use crate::memcard::{MemCard, CardSlot, CardError};
pub use crate::dvd::{self, DvdError};

// ─── BlockDevice trait ────────────────────────────────────────────────────────

/// Error type for block device operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    NoDevice,
    IoError,
    Timeout,
    BadAddress,
    WriteProtected,
    ReadOnly,
}

/// A trait representing a readable block storage device.
///
/// All sector addresses are logical block addresses (LBA), zero-indexed.
/// Callers must ensure buffers are large enough for `sector_size()` bytes.
pub trait BlockDevice {
    /// Human-readable device name.
    fn name(&self) -> &'static str;
    /// Size of one logical sector in bytes.
    fn sector_size(&self) -> usize;
    /// Total number of logical sectors.
    fn sector_count(&self) -> u64;
    /// Total capacity in bytes.
    fn capacity_bytes(&self) -> u64 {
        self.sector_count() * self.sector_size() as u64
    }
    /// Read one sector into `buf`. `buf` must be ≥ `sector_size()` bytes.
    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError>;
    /// Write one sector from `buf`. Returns `Err(ReadOnly)` for read-only devices.
    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError>;
    /// `true` for read-only devices (DVD).
    fn is_read_only(&self) -> bool { false }
}

// ─── SD card BlockDevice impl ─────────────────────────────────────────────────

impl BlockDevice for SdCard {
    fn name(&self) -> &'static str { "SD Card" }
    fn sector_size(&self) -> usize { 512 }
    fn sector_count(&self) -> u64 { self.sectors() as u64 }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let arr: &mut [u8; 512] =
            unsafe { &mut *(buf[..512].as_mut_ptr() as *mut [u8; 512]) };
        unsafe {
            self.read_sector(lba as u32, arr)
                .map_err(|_| BlockError::IoError)
        }
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        let arr: &[u8; 512] = buf[..512].try_into()
            .map_err(|_| BlockError::BadAddress)?;
        unsafe {
            self.write_sector(lba as u32, arr)
                .map_err(|_| BlockError::IoError)
        }
    }
}

// ─── Memory card BlockDevice impl ─────────────────────────────────────────────

impl BlockDevice for MemCard {
    fn name(&self) -> &'static str { self.device_type().name() }
    fn sector_size(&self) -> usize { 512 }
    fn sector_count(&self) -> u64 { self.total_bytes as u64 / 512 }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let arr: &mut [u8; 512] =
            unsafe { &mut *(buf[..512].as_mut_ptr() as *mut [u8; 512]) };
        let addr = (lba * 512) as u32;
        unsafe {
            self.read_segment(addr, arr)
                .map_err(|_| BlockError::IoError)
        }
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        // Memory card write granularity is 128 bytes (PAGE_SIZE).
        // Writing one 512-byte "sector" = 4 page writes at consecutive addresses.
        let base_addr = (lba * 512) as u32;
        for page in 0u32..4 {
            let addr = base_addr + page * 128;
            let arr: &[u8; 128] = buf[page as usize * 128..][..128].try_into()
                .map_err(|_| BlockError::BadAddress)?;
            unsafe {
                self.write_page(addr, arr)
                    .map_err(|_| BlockError::IoError)?;
            }
        }
        Ok(())
    }
}

// ─── Device detection / inventory ────────────────────────────────────────────

/// Information about a detected storage device.
#[derive(Debug, Clone, Copy)]
pub struct StorageInfo {
    pub kind:         StorageKind,
    pub dev_type:     DeviceType,
    pub sector_size:  usize,
    pub sector_count: u64,
    pub read_only:    bool,
}

/// Category of storage device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind {
    SdCardSlotA,
    SdCardSlotB,
    SdCardSp2,
    MemCardSlotA,
    MemCardSlotB,
    DvdDisc,
}

impl StorageKind {
    pub fn name(self) -> &'static str {
        match self {
            StorageKind::SdCardSlotA  => "SD Card (Slot A)",
            StorageKind::SdCardSlotB  => "SD Card (Slot B)",
            StorageKind::SdCardSp2    => "SD Card (SP2)",
            StorageKind::MemCardSlotA => "Memory Card (Slot A)",
            StorageKind::MemCardSlotB => "Memory Card (Slot B)",
            StorageKind::DvdDisc      => "DVD Disc",
        }
    }
}

/// Scan all storage devices and return the count found.
///
/// `results` receives one entry per detected device. The function returns
/// the number of entries written.
///
/// Probes:
/// - EXI Ch0 Dev0: SD Gecko Slot A or Memory Card Slot A
/// - EXI Ch1 Dev0: SD Gecko Slot B or Memory Card Slot B
/// - EXI Ch2 Dev0: SD2SP2 (always probed — Ch2 has no EXT detection)
/// - DVD drive: checks cover state
///
/// # Safety
/// EXI and DVD must not be in active use.
pub unsafe fn scan(results: &mut [StorageInfo]) -> usize {
    let mut count = 0;

    // ── EXI slots ─────────────────────────────────────────────────────────
    let slots = [
        (Channel::Ch0, StorageKind::SdCardSlotA, StorageKind::MemCardSlotA),
        (Channel::Ch1, StorageKind::SdCardSlotB, StorageKind::MemCardSlotB),
    ];

    for (ch, sd_kind, mc_kind) in &slots {
        if count >= results.len() { break; }
        if !exi::probe(*ch) { continue; }

        let dev_type = exi::get_id(*ch, Device::Dev0);

        match dev_type {
            DeviceType::None => {
                // No card present, or might be an SD Gecko with no card —
                // attempt a quick SD init to check.
            }
            dt if dt.is_memory_card() => {
                if let Ok(card) = MemCard::probe(
                    if *ch == Channel::Ch0 { CardSlot::A } else { CardSlot::B }
                ) {
                    results[count] = StorageInfo {
                        kind:         *mc_kind,
                        dev_type,
                        sector_size:  512,
                        sector_count: card.total_bytes as u64 / 512,
                        read_only:    false,
                    };
                    count += 1;
                }
            }
            _ => {
                // Unknown or SD Gecko — try SD init
                let sd_slot = if *ch == Channel::Ch0 { SdSlot::A } else { SdSlot::B };
                let mut card = SdCard::new(sd_slot);
                if card.init().is_ok() {
                    results[count] = StorageInfo {
                        kind:         *sd_kind,
                        dev_type:     DeviceType::SdCard,
                        sector_size:  512,
                        sector_count: card.sectors() as u64,
                        read_only:    false,
                    };
                    count += 1;
                }
            }
        }
    }

    // ── SD2SP2 (EXI Ch2) ─────────────────────────────────────────────────
    if count < results.len() {
        let mut card = SdCard::new(SdSlot::Sp2);
        if card.init().is_ok() {
            results[count] = StorageInfo {
                kind:         StorageKind::SdCardSp2,
                dev_type:     DeviceType::SdCard,
                sector_size:  512,
                sector_count: card.sectors() as u64,
                read_only:    false,
            };
            count += 1;
        }
    }

    // ── DVD drive ─────────────────────────────────────────────────────────
    if count < results.len() {
        dvd::init();
        if !dvd::cover_open() {
            // Try reading the disc ID to confirm a disc is present
            if dvd::read_disc_id().is_ok() {
                results[count] = StorageInfo {
                    kind:         StorageKind::DvdDisc,
                    dev_type:     DeviceType::None,
                    sector_size:  2048,
                    sector_count: 0, // GC disc capacity is fixed at ~1.4 GB but we don't read it here
                    read_only:    true,
                };
                count += 1;
            }
        }
    }

    count
}
