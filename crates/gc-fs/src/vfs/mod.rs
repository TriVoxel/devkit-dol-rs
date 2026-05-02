//! Virtual Filesystem — unified volume manager (libdvm equivalent).
//!
//! Manages up to [`MAX_VOLUMES`] simultaneously mounted volumes and provides
//! a single namespace for path-based file operations.
//!
//! ## Design
//!
//! Each volume is identified by a mount point prefix (e.g. `"sd:"`, `"mc:"`,
//! `"dvd:"`, `"usb:"`). Paths starting with `"sd:/ROMS/game.iso"` are routed
//! to the volume mounted at `"sd:"`. Paths starting with `"/"` go to the
//! default volume (the first mounted).
//!
//! All volume state is stored in a static table — no heap required.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use gc_fs::vfs::{self, FsKind, OpenOptions};
//!
//! unsafe {
//!     // Mount SD card (auto-detects FAT32/ExFAT)
//!     vfs::mount_sd(gc_hal::sd::Slot::A, "sd", FsKind::Auto).unwrap();
//!
//!     // Mount GC disc
//!     gc_hal::dvd::init();
//!     vfs::mount_dvd("dvd").unwrap();
//!
//!     // Mount memory card
//!     vfs::mount_mc(gc_hal::memcard::CardSlot::A, "mc").unwrap();
//!
//!     // Open a file — path is "mount_point:/rest/of/path"
//!     let mut f = vfs::open("sd:/ROMS/game.iso").unwrap();
//!
//!     // Mount the ISO image stored in that file
//!     vfs::mount_image(f, "iso", FsKind::Iso9660).unwrap();
//!     let cnf = vfs::open("iso:/SYSTEM.CNF").unwrap();
//!
//!     // List a directory
//!     vfs::read_dir("sd:/", |meta| {
//!         // meta.name_str(), meta.is_dir, meta.size
//!         true // continue
//!     }).unwrap();
//! }
//! ```

use crate::{FsError, FsKind, Metadata, Result, BlockDev};
use crate::fat::FatVolume;
use crate::iso9660::Iso9660;
use crate::dvd::GcDvd;
use crate::memcard::MemCardFs;
#[cfg(feature = "ext2")]
use crate::ext2::Ext2;

use gc_hal::sd::{SdCard, Slot as SdSlot};
use gc_hal::dvd::DvdDisk;
use gc_hal::memcard::CardSlot;

// ─────────────────────────────────────────────────────────────────────────────
// Volume table (static, no heap)
// ─────────────────────────────────────────────────────────────────────────────

pub const MAX_VOLUMES: usize = 8;
pub const MAX_OPEN:    usize = 16;

/// Maximum mount point name length.
const MP_LEN: usize = 16;

/// A mounted volume entry in the VFS table.
pub struct Volume {
    /// Mount point name (without colon), e.g. "sd", "dvd", "mc", "iso".
    mount: [u8; MP_LEN],
    kind:  FsKind,
    inner: VolumeInner,
    in_use: bool,
}

/// Type-erased volume storage. We use concrete types to avoid alloc.
/// The variant must match `kind` in the enclosing `Volume`.
enum VolumeInner {
    Empty,
    Fat32Sd(FatVolume<SdCard>),
    Fat32Img(FatVolume<crate::image::FileImage<512, SdCard>>),
    Iso9660Dvd(Iso9660<DvdDisk>),
    Iso9660Img(Iso9660<crate::image::FileImage<2048, SdCard>>),
    GcDvd(GcDvd<DvdDisk>),
    MemCard(MemCardFs),
}

/// Global VFS volume table.
static mut VOLUMES: [Volume; MAX_VOLUMES] = [
    Volume { mount: [0;MP_LEN], kind: FsKind::Auto, inner: VolumeInner::Empty, in_use: false },
    Volume { mount: [0;MP_LEN], kind: FsKind::Auto, inner: VolumeInner::Empty, in_use: false },
    Volume { mount: [0;MP_LEN], kind: FsKind::Auto, inner: VolumeInner::Empty, in_use: false },
    Volume { mount: [0;MP_LEN], kind: FsKind::Auto, inner: VolumeInner::Empty, in_use: false },
    Volume { mount: [0;MP_LEN], kind: FsKind::Auto, inner: VolumeInner::Empty, in_use: false },
    Volume { mount: [0;MP_LEN], kind: FsKind::Auto, inner: VolumeInner::Empty, in_use: false },
    Volume { mount: [0;MP_LEN], kind: FsKind::Auto, inner: VolumeInner::Empty, in_use: false },
    Volume { mount: [0;MP_LEN], kind: FsKind::Auto, inner: VolumeInner::Empty, in_use: false },
];

// ─────────────────────────────────────────────────────────────────────────────
// Path routing
// ─────────────────────────────────────────────────────────────────────────────

/// Split `"sd:/foo/bar"` into `("sd", "/foo/bar")`.
/// Plain `"/foo/bar"` or `"foo/bar"` gets mount point `""` (default volume).
fn split_mount<'a>(path: &'a str) -> (&'a str, &'a str) {
    if let Some(colon) = path.find(':') {
        (&path[..colon], &path[colon+1..])
    } else {
        ("", path)
    }
}

unsafe fn find_volume(mount: &str) -> Option<usize> {
    for (i, v) in VOLUMES.iter().enumerate() {
        if !v.in_use { continue; }
        let m_bytes = &v.mount[..];
        let end = m_bytes.iter().position(|&b| b == 0).unwrap_or(MP_LEN);
        let m_str = core::str::from_utf8(&m_bytes[..end]).unwrap_or("");
        if mount.is_empty() || m_str == mount { return Some(i); }
    }
    None
}

unsafe fn alloc_slot() -> Option<usize> {
    VOLUMES.iter().position(|v| !v.in_use)
}

fn set_mount(slot: &mut [u8; MP_LEN], name: &str) {
    let bytes = name.as_bytes();
    let len   = bytes.len().min(MP_LEN - 1);
    slot[..len].copy_from_slice(&bytes[..len]);
    slot[len] = 0;
}

// ─────────────────────────────────────────────────────────────────────────────
// Mount functions
// ─────────────────────────────────────────────────────────────────────────────

/// Mount an SD card. Detects FAT12/16/32/ExFAT automatically.
///
/// `mount` is the mount point name (e.g. `"sd"` → paths like `"sd:/foo"`).
pub unsafe fn mount_sd(slot: SdSlot, mount: &str, _hint: FsKind) -> Result<()> {
    let idx = alloc_slot().ok_or(FsError::TooManyMounts)?;
    let mut card = SdCard::new(slot);
    card.init().map_err(|_| FsError::Io)?;
    let vol = FatVolume::mount(card)?;
    let v = &mut VOLUMES[idx];
    set_mount(&mut v.mount, mount);
    v.kind   = FsKind::Fat;
    v.inner  = VolumeInner::Fat32Sd(vol);
    v.in_use = true;
    Ok(())
}

/// Mount the DVD drive's GC disc filesystem.
pub unsafe fn mount_dvd(mount: &str) -> Result<()> {
    let idx = alloc_slot().ok_or(FsError::TooManyMounts)?;
    gc_hal::dvd::init();
    let vol = GcDvd::mount(DvdDisk)?;
    let v = &mut VOLUMES[idx];
    set_mount(&mut v.mount, mount);
    v.kind   = FsKind::GcDvd;
    v.inner  = VolumeInner::GcDvd(vol);
    v.in_use = true;
    Ok(())
}

/// Mount a GC disc as ISO 9660 (useful for non-GC discs in an ODE).
pub unsafe fn mount_dvd_iso(mount: &str) -> Result<()> {
    let idx = alloc_slot().ok_or(FsError::TooManyMounts)?;
    gc_hal::dvd::init();
    let vol = Iso9660::mount(DvdDisk)?;
    let v = &mut VOLUMES[idx];
    set_mount(&mut v.mount, mount);
    v.kind   = FsKind::Iso9660;
    v.inner  = VolumeInner::Iso9660Dvd(vol);
    v.in_use = true;
    Ok(())
}

/// Mount a memory card.
pub unsafe fn mount_mc(slot: CardSlot, mount: &str) -> Result<()> {
    let idx = alloc_slot().ok_or(FsError::TooManyMounts)?;
    let vol = MemCardFs::mount(slot)?;
    let v = &mut VOLUMES[idx];
    set_mount(&mut v.mount, mount);
    v.kind   = FsKind::MemCard;
    v.inner  = VolumeInner::MemCard(vol);
    v.in_use = true;
    Ok(())
}

/// Unmount a volume by mount point name.
pub unsafe fn unmount(mount: &str) -> Result<()> {
    let idx = find_volume(mount).ok_or(FsError::NotFound)?;
    VOLUMES[idx].in_use = false;
    VOLUMES[idx].inner  = VolumeInner::Empty;
    Ok(())
}

/// Return info about mounted volumes.
pub unsafe fn list_volumes<F>(mut cb: F) where F: FnMut(&str, &str) {
    for v in VOLUMES.iter() {
        if !v.in_use { continue; }
        let end = v.mount.iter().position(|&b| b==0).unwrap_or(MP_LEN);
        let mp  = core::str::from_utf8(&v.mount[..end]).unwrap_or("?");
        let kind = match &v.inner {
            VolumeInner::Fat32Sd(_) | VolumeInner::Fat32Img(_) => "FAT32",
            VolumeInner::Iso9660Dvd(_) | VolumeInner::Iso9660Img(_) => "ISO9660",
            VolumeInner::GcDvd(_)    => "GC-DVD",
            VolumeInner::MemCard(_)  => "MemCard",
            VolumeInner::Empty       => "?",
        };
        cb(mp, kind);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// File open
// ─────────────────────────────────────────────────────────────────────────────

/// Universally storable file handle — enum over all concrete file types.
///
/// Use [`VfsFile::read`], [`VfsFile::seek`] for I/O regardless of source.
pub enum VfsFile {
    FatSd(crate::fat::FatFile<'static, SdCard>),
    IsoDvd(crate::iso9660::IsoFile<'static, DvdDisk>),
    MemCardRaw { data: &'static [u8], pos: usize },
    Empty,
}

impl VfsFile {
    pub fn size(&self) -> u64 {
        match self {
            VfsFile::FatSd(f)    => f.size(),
            VfsFile::IsoDvd(f)   => f.size(),
            VfsFile::MemCardRaw { data, .. } => data.len() as u64,
            VfsFile::Empty       => 0,
        }
    }
    pub fn pos(&self) -> u64 {
        match self {
            VfsFile::FatSd(f)    => f.pos(),
            VfsFile::IsoDvd(f)   => f.pos(),
            VfsFile::MemCardRaw { pos, .. } => *pos as u64,
            VfsFile::Empty       => 0,
        }
    }
    pub unsafe fn seek(&mut self, p: u64) -> Result<()> {
        match self {
            VfsFile::FatSd(f)  => f.seek(p),
            VfsFile::IsoDvd(f) => f.seek(p),
            VfsFile::MemCardRaw { pos, data } => {
                if p > data.len() as u64 { return Err(FsError::InvalidArg); }
                *pos = p as usize; Ok(())
            }
            VfsFile::Empty     => Err(FsError::NotFound),
        }
    }
    pub unsafe fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match self {
            VfsFile::FatSd(f)  => f.read(buf),
            VfsFile::IsoDvd(f) => f.read(buf),
            VfsFile::MemCardRaw { pos, data } => {
                let rem = data.len().saturating_sub(*pos);
                let take = rem.min(buf.len());
                buf[..take].copy_from_slice(&data[*pos..*pos + take]);
                *pos += take; Ok(take)
            }
            VfsFile::Empty     => Err(FsError::NotFound),
        }
    }
    pub unsafe fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        let mut total = 0;
        while total < buf.len() {
            let n = self.read(&mut buf[total..])?;
            if n == 0 { return Err(FsError::Eof); }
            total += n;
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// VFS operations
// ─────────────────────────────────────────────────────────────────────────────

/// Open a file. Path format: `"mount:/rest/of/path"` or `"/path"` for default volume.
///
/// Returns a [`VfsFile`] that can be read/seeked regardless of underlying filesystem.
pub unsafe fn open(path: &str) -> Result<VfsFile> {
    let (mp, rest) = split_mount(path);
    let idx = find_volume(mp).ok_or(FsError::NotFound)?;
    let vol = &mut VOLUMES[idx];

    match &mut vol.inner {
        VolumeInner::Fat32Sd(fv) => {
            // SAFETY: we transmute the lifetime — the volume lives in the static
            // VOLUMES array, so 'static is valid.
            let fv: &'static FatVolume<SdCard> = core::mem::transmute(fv as *mut _);
            let f = fv.open(rest)?;
            Ok(VfsFile::FatSd(f))
        }
        VolumeInner::Iso9660Dvd(iv) => {
            let iv: &'static Iso9660<DvdDisk> = core::mem::transmute(iv as *mut _);
            let f = iv.open(rest)?;
            Ok(VfsFile::IsoDvd(f))
        }
        VolumeInner::GcDvd(_) => {
            // GC DVD open as raw bytes read — caller can use read_dvd_file instead
            Err(FsError::Unsupported)
        }
        VolumeInner::MemCard(_mc) => {
            // MC files require game+filename, not a path — use mc_open() instead
            Err(FsError::Unsupported)
        }
        _ => Err(FsError::Unsupported),
    }
}

/// Iterate directory entries at `path`.
pub unsafe fn read_dir<F>(path: &str, cb: F) -> Result<()>
where F: FnMut(&Metadata) -> bool
{
    let (mp, rest) = split_mount(path);
    let idx = find_volume(mp).ok_or(FsError::NotFound)?;
    let vol = &mut VOLUMES[idx];

    match &mut vol.inner {
        VolumeInner::Fat32Sd(fv)    => fv.read_dir(rest, cb),
        VolumeInner::Iso9660Dvd(iv) => iv.read_dir(rest, cb),
        VolumeInner::GcDvd(dv)     => dv.read_dir(rest, cb),
        VolumeInner::MemCard(mc)    => {
            let mut f = cb; // adapt signature
            mc.read_dir(|meta, _entry| f(meta));
            Ok(())
        }
        _ => Err(FsError::Unsupported),
    }
}

/// Stat a path.
pub unsafe fn stat(path: &str) -> Result<Metadata> {
    let (mp, rest) = split_mount(path);
    let idx = find_volume(mp).ok_or(FsError::NotFound)?;
    match &mut VOLUMES[idx].inner {
        VolumeInner::Fat32Sd(fv)    => fv.stat(rest),
        VolumeInner::Iso9660Dvd(iv) => iv.stat(rest),
        VolumeInner::GcDvd(dv)     => dv.stat(rest),
        _ => Err(FsError::Unsupported),
    }
}

/// Directly read a file from the GC DVD filesystem into a buffer.
///
/// More efficient than open() for DVD reads since it avoids the VfsFile overhead.
pub unsafe fn read_dvd_file(path: &str, buf: &mut [u8]) -> Result<usize> {
    let (mp, rest) = split_mount(path);
    let idx = find_volume(mp).ok_or(FsError::NotFound)?;
    match &mut VOLUMES[idx].inner {
        VolumeInner::GcDvd(dv) => dv.read_file(rest, buf),
        _ => Err(FsError::Unsupported),
    }
}

/// Read a memory card file identified by game code and filename.
pub unsafe fn mc_read_file(
    mount:    &str,
    gamecode: &[u8; 4],
    filename: &str,
    buf:      &mut [u8],
) -> Result<usize> {
    let idx = find_volume(mount).ok_or(FsError::NotFound)?;
    match &mut VOLUMES[idx].inner {
        VolumeInner::MemCard(mc) => {
            let entry = mc.find(gamecode, filename).ok_or(FsError::NotFound)?;
            // SAFETY: entry is valid for the lifetime of mc which lives in VOLUMES
            let entry_copy = *entry;
            mc.read_file(&entry_copy, buf)
        }
        _ => Err(FsError::Unsupported),
    }
}

/// Return true if `mount` names a currently mounted volume.
pub unsafe fn is_mounted(mount: &str) -> bool {
    find_volume(mount).is_some()
}
