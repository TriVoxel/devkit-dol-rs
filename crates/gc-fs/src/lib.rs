//! # gc-fs — Unified GameCube/Wii Filesystem Library
//!
//! Provides read/write access to all filesystems common in GC/Wii homebrew.
//!
//! | Feature    | Filesystem              | Use case                          |
//! |------------|-------------------------|-----------------------------------|
//! | `fat`      | FAT12/16/32 + ExFAT     | SD cards (most common)            |
//! | `ext2`     | EXT2/3/4 (read-only)    | Linux-formatted storage           |
//! | `memcard`  | Nintendo MC filesystem  | GC memory card saves              |
//! | `dvd`      | GC disc filesystem      | Real discs, ODEs                  |
//! | `iso9660`  | ISO 9660 + Rock Ridge   | PS1, Neo Geo CD, generic CDs/ISOs |
//!
//! ## Unified API
//!
//! All filesystems are accessible through the [`vfs`] module:
//!
//! ```rust,no_run
//! use gc_fs::vfs;
//! use gc_fs::FsKind;
//!
//! unsafe {
//!     vfs::mount_sd(gc_hal::sd::Slot::A, "sd", FsKind::Auto).unwrap();
//!     vfs::mount_dvd("dvd").unwrap();
//!
//!     // Read from SD
//!     let mut f = vfs::open("sd:/boot.dol").unwrap();
//!
//!     // List DVD directory
//!     vfs::read_dir("dvd:/files", |m| { true }).unwrap();
//!
//!     // ISO image inside SD card — same API as a physical disc
//!     let iso_file = vfs::open("sd:/ROMS/game.iso").unwrap();
//!     // (then pass iso_file to Iso9660::mount_file for nested access)
//! }
//! ```

#![no_std]

pub mod image;
pub mod vfs;

#[cfg(feature = "fat")]     pub mod fat;
#[cfg(feature = "ext2")]    pub mod ext2;
#[cfg(feature = "memcard")] pub mod memcard;
#[cfg(feature = "dvd")]     pub mod dvd;
#[cfg(feature = "iso9660")] pub mod iso9660;

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors returned by filesystem operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    Io,             // block device I/O failed
    BadFormat,      // unrecognised or corrupt filesystem
    NotFound,       // path not found
    ReadOnly,       // write on read-only filesystem
    Eof,            // read past end of file
    BufferTooSmall, // caller's buffer too small
    InvalidArg,     // bad path, bad offset, etc.
    NoSpace,        // no free space
    NotEmpty,       // rmdir on non-empty dir
    WrongType,      // file vs directory mismatch
    Unsupported,    // operation not implemented for this fs
    TooManyMounts,  // VFS volume table full
}

pub type Result<T> = core::result::Result<T, FsError>;

// ── Filesystem kind hint ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FsKind {
    Auto,    // probe and detect
    Fat,     // FAT12/16/32
    ExFat,   // ExFAT
    Ext2,    // EXT2/3/4
    MemCard, // Nintendo MC
    GcDvd,   // GC disc filesystem (FST)
    Iso9660, // ISO 9660 / Rock Ridge
}

// ── File metadata ─────────────────────────────────────────────────────────────

/// Directory entry metadata.
#[derive(Clone, Copy)]
pub struct Metadata {
    pub size:     u64,
    pub is_dir:   bool,
    pub readonly: bool,
    pub name:     [u8; 256],
}

impl Metadata {
    pub const fn zeroed() -> Self {
        Metadata { size: 0, is_dir: false, readonly: false, name: [0u8; 256] }
    }

    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(256);
        core::str::from_utf8(&self.name[..end]).unwrap_or("<invalid>")
    }

    pub fn set_name(&mut self, s: &str) {
        let bytes = s.as_bytes();
        let len   = bytes.len().min(255);
        self.name[..len].copy_from_slice(&bytes[..len]);
        self.name[len] = 0;
    }
}

impl core::fmt::Debug for Metadata {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Metadata")
            .field("name", &self.name_str())
            .field("size", &self.size)
            .field("is_dir", &self.is_dir)
            .finish()
    }
}

// ── Block device bridge ───────────────────────────────────────────────────────

/// Trait for block-addressable storage. All filesystem drivers are generic over this.
pub trait BlockDev {
    fn sector_size(&self) -> usize;
    fn sector_count(&self) -> u64;
    /// Read one sector (exactly `sector_size()` bytes) into `buf`.
    unsafe fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<()>;
    /// Write one sector. Returns `Err(ReadOnly)` for read-only devices.
    unsafe fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<()> {
        let _ = (lba, buf);
        Err(FsError::ReadOnly)
    }
}

// ── gc-hal adapters ───────────────────────────────────────────────────────────

impl BlockDev for gc_hal::sd::SdCard {
    fn sector_size(&self)  -> usize { 512 }
    fn sector_count(&self) -> u64   { self.sectors() as u64 }

    unsafe fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<()> {
        if buf.len() < 512 { return Err(FsError::BufferTooSmall); }
        let arr: &mut [u8; 512] = (&mut buf[..512]).try_into()
            .map_err(|_| FsError::BufferTooSmall)?;
        gc_hal::sd::SdCard::read_sector(self, lba as u32, arr)
            .map_err(|_| FsError::Io)
    }

    unsafe fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<()> {
        if buf.len() < 512 { return Err(FsError::BufferTooSmall); }
        let arr: &[u8; 512] = (&buf[..512]).try_into()
            .map_err(|_| FsError::BufferTooSmall)?;
        gc_hal::sd::SdCard::write_sector(self, lba as u32, arr)
            .map_err(|_| FsError::Io)
    }
}

impl BlockDev for gc_hal::dvd::DvdDisk {
    fn sector_size(&self)  -> usize { 2048 }
    fn sector_count(&self) -> u64   { 0x0057_E000 } // ~1.35 GB GC disc
    unsafe fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<()> {
        if buf.len() < 2048 { return Err(FsError::BufferTooSmall); }
        gc_hal::dvd::read(buf.as_mut_ptr(), 2048, lba * 2048)
            .map_err(|_| FsError::Io)
    }
}

// ── Utility functions ──────────────────────────────────────────────────────────

/// Split `"prefix/rest"` at the first `/`.
pub fn path_split(path: &str) -> (&str, &str) {
    let path = path.trim_start_matches('/');
    match path.find('/') {
        Some(i) => (&path[..i], path[i+1..].trim_start_matches('/')),
        None    => (path, ""),
    }
}

/// Case-insensitive ASCII byte slice comparison.
pub fn name_eq_ci(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    a.iter().zip(b).all(|(&x,&y)| x.to_ascii_uppercase() == y.to_ascii_uppercase())
}

/// Compare a space-padded fixed-width name against a plain string.
pub fn padded_eq(padded: &[u8], name: &str) -> bool {
    let end = padded.iter().rposition(|&b| b != b' ').map(|i| i+1).unwrap_or(0);
    name_eq_ci(&padded[..end], name.as_bytes())
}
