//! # dkdol-fs — Unified GameCube/Wii Filesystem Library
//!
//! | Feature    | Filesystem       | Read | Write | Notes                        |
//! |------------|------------------|------|-------|------------------------------|
//! | `fat`      | FAT32 + ExFAT    |  ✓   |   ✓   | LFN, ExFAT entry sets        |
//! | `ext2`     | EXT2/3/4         |  ✓   |   ✓   | Extents, inline data, no JBD |
//! | `memcard`  | Nintendo MC      |  ✓   |   ✓   |                              |
//! | `dvd`      | GC disc (FST)    |  ✓   |   —   |                              |
//! | `iso9660`  | ISO 9660         |  ✓   |   —   |                              |
//!
//! FAT12 and FAT16 are not supported.

#![no_std]

pub mod image;
pub mod vfs;

#[cfg(feature = "fat")]     pub mod fat;
#[cfg(feature = "ext2")]    pub mod ext2;
#[cfg(feature = "memcard")] pub mod memcard;
#[cfg(feature = "dvd")]     pub mod dvd;
#[cfg(feature = "iso9660")] pub mod iso9660;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    Io, BadFormat, NotFound, ReadOnly, Eof, BufferTooSmall,
    InvalidArg, NoSpace, NotEmpty, WrongType, Unsupported,
    TooManyMounts, FilesOpen,
}

pub type Result<T> = core::result::Result<T, FsError>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FsKind {
    Auto, Fat, ExFat, Ext2, MemCard, GcDvd, Iso9660,
}

#[derive(Clone, Copy)]
pub struct Metadata {
    pub size:     u64,
    pub is_dir:   bool,
    pub readonly: bool,
    pub hidden:   bool,
    /// Modification time — Unix timestamp (seconds since 1970-01-01 UTC). 0 = unknown.
    pub mtime:    u32,
    pub name:     [u8; 256],
}

impl Metadata {
    pub const fn zeroed() -> Self {
        Metadata { size: 0, is_dir: false, readonly: false, hidden: false,
                   mtime: 0, name: [0u8; 256] }
    }
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(256);
        core::str::from_utf8(&self.name[..end]).unwrap_or("<invalid>")
    }
    pub fn set_name(&mut self, s: &str) {
        let b = s.as_bytes(); let n = b.len().min(255);
        self.name[..n].copy_from_slice(&b[..n]); self.name[n] = 0;
    }
}

impl core::fmt::Debug for Metadata {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Metadata")
         .field("name",   &self.name_str())
         .field("size",   &self.size)
         .field("is_dir", &self.is_dir)
         .field("mtime",  &self.mtime)
         .finish()
    }
}

pub trait BlockDev {
    fn sector_size(&self) -> usize;
    fn sector_count(&self) -> u64;
    unsafe fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<()>;
    unsafe fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<()> {
        let _ = (lba, buf); Err(FsError::ReadOnly)
    }
}

impl BlockDev for dkdol_hal::sd::SdCard {
    fn sector_size(&self)  -> usize { 512 }
    fn sector_count(&self) -> u64   { self.sectors() as u64 }
    unsafe fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<()> {
        if buf.len() < 512 { return Err(FsError::BufferTooSmall); }
        let arr: &mut [u8; 512] = (&mut buf[..512]).try_into().map_err(|_| FsError::BufferTooSmall)?;
        dkdol_hal::sd::SdCard::read_sector(self, lba as u32, arr).map_err(|_| FsError::Io)
    }
    unsafe fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<()> {
        if buf.len() < 512 { return Err(FsError::BufferTooSmall); }
        let arr: &[u8; 512] = (&buf[..512]).try_into().map_err(|_| FsError::BufferTooSmall)?;
        dkdol_hal::sd::SdCard::write_sector(self, lba as u32, arr).map_err(|_| FsError::Io)
    }
}

impl BlockDev for dkdol_hal::dvd::DvdDisk {
    fn sector_size(&self)  -> usize { 2048 }
    fn sector_count(&self) -> u64   { 0x0057_E000 }
    unsafe fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<()> {
        if buf.len() < 2048 { return Err(FsError::BufferTooSmall); }
        dkdol_hal::dvd::read(buf.as_mut_ptr(), 2048, lba * 2048).map_err(|_| FsError::Io)
    }
}

pub fn path_split(path: &str) -> (&str, &str) {
    let path = path.trim_start_matches('/');
    match path.find('/') {
        Some(i) => (&path[..i], &path[i+1..].trim_start_matches('/')),
        None    => (path, ""),
    }
}

pub fn name_eq_ci(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    a.iter().zip(b).all(|(&x,&y)| x.to_ascii_uppercase() == y.to_ascii_uppercase())
}

pub fn padded_eq(padded: &[u8], name: &str) -> bool {
    let end = padded.iter().rposition(|&b| b != b' ').map(|i| i+1).unwrap_or(0);
    name_eq_ci(&padded[..end], name.as_bytes())
}
