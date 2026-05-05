//! Virtual Filesystem — unified volume manager.
//!
//! ## Path format
//!
//! `"mount_point:/path/to/file"` — `"sd:/boot.dol"`, `"ext:/home/user/file"`.
//! Plain `"/path"` routes to the first mounted volume.
//!
//! ## Lifetime safety
//!
//! `FatSd` and `Ext2Sd` [`VfsFile`] variants store their state by value
//! (vol_idx + raw cluster/inode fields) and access [`VOLUMES`] by index.
//! No `'static` transmute is required. ISO and GcDvd files are read-only
//! and use a transmute that is sound because VOLUMES is `static mut` and
//! never reallocated.
//!
//! Unmount is blocked while any file on that volume reports `open_count > 0`.

use crate::{FsError, FsKind, Metadata, Result, BlockDev};

#[cfg(feature = "fat")]     use crate::fat::{FatVolume, FatKind};
#[cfg(feature = "ext2")]    use crate::ext2::{Ext2, JournalMode};
use crate::iso9660::Iso9660;
use crate::dvd::GcDvd;
use crate::memcard::MemCardFs;

use dkdol_hal::sd::{SdCard, Slot as SdSlot};
use dkdol_hal::dvd::DvdDisk;
use dkdol_hal::memcard::CardSlot;

// ─── Volume table ─────────────────────────────────────────────────────────────

pub const MAX_VOLUMES: usize = 8;
const MP_LEN: usize = 16;

pub struct Volume {
    mount:      [u8; MP_LEN],
    kind:       FsKind,
    inner:      VolumeInner,
    in_use:     bool,
    open_count: u8,
}

enum VolumeInner {
    Empty,
    #[cfg(feature = "fat")]  Fat32Sd(FatVolume<SdCard>),
    #[cfg(feature = "ext2")] Ext2Sd(Ext2<SdCard>),
    Iso9660Dvd(Iso9660<DvdDisk>),
    Iso9660Img(Iso9660<crate::image::FileImage<2048, SdCard>>),
    GcDvd(GcDvd<DvdDisk>),
    MemCard(MemCardFs),
}

static mut VOLUMES: [Volume; MAX_VOLUMES] = [
    Volume { mount:[0;MP_LEN], kind:FsKind::Auto, inner:VolumeInner::Empty, in_use:false, open_count:0 },
    Volume { mount:[0;MP_LEN], kind:FsKind::Auto, inner:VolumeInner::Empty, in_use:false, open_count:0 },
    Volume { mount:[0;MP_LEN], kind:FsKind::Auto, inner:VolumeInner::Empty, in_use:false, open_count:0 },
    Volume { mount:[0;MP_LEN], kind:FsKind::Auto, inner:VolumeInner::Empty, in_use:false, open_count:0 },
    Volume { mount:[0;MP_LEN], kind:FsKind::Auto, inner:VolumeInner::Empty, in_use:false, open_count:0 },
    Volume { mount:[0;MP_LEN], kind:FsKind::Auto, inner:VolumeInner::Empty, in_use:false, open_count:0 },
    Volume { mount:[0;MP_LEN], kind:FsKind::Auto, inner:VolumeInner::Empty, in_use:false, open_count:0 },
    Volume { mount:[0;MP_LEN], kind:FsKind::Auto, inner:VolumeInner::Empty, in_use:false, open_count:0 },
];

// ─── Path routing ─────────────────────────────────────────────────────────────

fn split_mount(path: &str) -> (&str, &str) {
    match path.find(':') {
        Some(i) => (&path[..i], &path[i+1..]),
        None    => ("", path),
    }
}

unsafe fn find_volume(mount: &str) -> Option<usize> {
    for (i, v) in VOLUMES.iter().enumerate() {
        if !v.in_use { continue; }
        let end = v.mount.iter().position(|&b| b == 0).unwrap_or(MP_LEN);
        let m   = core::str::from_utf8(&v.mount[..end]).unwrap_or("");
        if mount.is_empty() || m == mount { return Some(i); }
    }
    None
}

unsafe fn alloc_slot() -> Option<usize> {
    VOLUMES.iter().position(|v| !v.in_use)
}

fn set_mount(slot: &mut [u8; MP_LEN], name: &str) {
    let b = name.as_bytes(); let n = b.len().min(MP_LEN-1);
    slot[..n].copy_from_slice(&b[..n]); slot[n] = 0;
}

// ─── Mount functions ──────────────────────────────────────────────────────────

/// Mount an SD card. Auto-detects FAT32 or ExFAT.
pub unsafe fn mount_sd(slot: SdSlot, mount: &str, _hint: FsKind) -> Result<()> {
    let idx = alloc_slot().ok_or(FsError::TooManyMounts)?;
    let mut card = SdCard::new(slot);
    card.init().map_err(|_| FsError::Io)?;
    #[cfg(feature = "fat")] {
        let vol = FatVolume::mount(card)?;
        let v = &mut VOLUMES[idx];
        set_mount(&mut v.mount, mount);
        v.kind   = if vol.kind() == FatKind::ExFat { FsKind::ExFat } else { FsKind::Fat };
        v.inner  = VolumeInner::Fat32Sd(vol);
        v.in_use = true;
        return Ok(());
    }
    #[allow(unreachable_code)]
    Err(FsError::Unsupported)
}

/// Mount an SD card as EXT2/3/4.
///
/// `journal` controls whether a dirty filesystem is refused (`RequireClean`,
/// default) or mounted anyway (`Ignore`).
#[cfg(feature = "ext2")]
pub unsafe fn mount_ext2(slot: SdSlot, mount: &str, journal: JournalMode) -> Result<()> {
    let idx = alloc_slot().ok_or(FsError::TooManyMounts)?;
    let mut card = SdCard::new(slot);
    card.init().map_err(|_| FsError::Io)?;
    let vol = Ext2::mount_opts(card, journal)?;
    let v = &mut VOLUMES[idx];
    set_mount(&mut v.mount, mount);
    v.kind   = FsKind::Ext2;
    v.inner  = VolumeInner::Ext2Sd(vol);
    v.in_use = true;
    Ok(())
}

/// Mount the DVD drive as a GC disc (FST).
pub unsafe fn mount_dvd(mount: &str) -> Result<()> {
    let idx = alloc_slot().ok_or(FsError::TooManyMounts)?;
    dkdol_hal::dvd::init();
    let vol = GcDvd::mount(DvdDisk)?;
    let v = &mut VOLUMES[idx];
    set_mount(&mut v.mount, mount);
    v.kind = FsKind::GcDvd; v.inner = VolumeInner::GcDvd(vol); v.in_use = true;
    Ok(())
}

/// Mount the DVD drive as ISO 9660.
pub unsafe fn mount_dvd_iso(mount: &str) -> Result<()> {
    let idx = alloc_slot().ok_or(FsError::TooManyMounts)?;
    dkdol_hal::dvd::init();
    let vol = Iso9660::mount(DvdDisk)?;
    let v = &mut VOLUMES[idx];
    set_mount(&mut v.mount, mount);
    v.kind = FsKind::Iso9660; v.inner = VolumeInner::Iso9660Dvd(vol); v.in_use = true;
    Ok(())
}

/// Mount a memory card.
pub unsafe fn mount_mc(slot: CardSlot, mount: &str) -> Result<()> {
    let idx = alloc_slot().ok_or(FsError::TooManyMounts)?;
    let vol = MemCardFs::mount(slot)?;
    let v = &mut VOLUMES[idx];
    set_mount(&mut v.mount, mount);
    v.kind = FsKind::MemCard; v.inner = VolumeInner::MemCard(vol); v.in_use = true;
    Ok(())
}

/// Mount a file (e.g. a `.iso`) stored on an already-mounted FAT32 SD card
/// as a nested ISO 9660 volume.
///
/// The source file is identified by `src_path` (e.g. `"sd:/ROMS/game.iso"`).
/// The nested volume is accessible at `mount` (e.g. `"iso"`).
#[cfg(feature = "fat")]
pub unsafe fn mount_image(src_path: &str, mount: &str, hint: FsKind) -> Result<()> {
    let (mp, rest) = split_mount(src_path);
    let src_idx = find_volume(mp).ok_or(FsError::NotFound)?;

    // Only SD FAT32 sources are supported for now
    let (img_cluster, img_size) = match &VOLUMES[src_idx].inner {
        VolumeInner::Fat32Sd(fv) => {
            let info = fv.open(rest)?;
            (info.start, info.size())
        }
        _ => return Err(FsError::Unsupported),
    };

    let _ = (img_cluster, img_size, hint);

    // Build a FileImage over the SD card + file offset
    // For ISO 9660 images on FAT32:
    // The cluster chain must be contiguous for FileImage to work correctly.
    // In practice this is usually satisfied for freshly-copied ISOs.
    // A proper scatter-gather FileImage (walking the FAT chain) is a future improvement.
    let dst_idx = alloc_slot().ok_or(FsError::TooManyMounts)?;

    // Compute byte offset of first cluster on device
    let fv = match &VOLUMES[src_idx].inner {
        VolumeInner::Fat32Sd(fv) => fv,
        _ => return Err(FsError::Unsupported),
    };
    let geom    = fv.geom;
    let img_lba = geom.cluster_lba(img_cluster);
    let img_off = img_lba * geom.bytes_per_sec as u64;

    let mut card = SdCard::new(SdSlot::A);
    card.init().map_err(|_| FsError::Io)?;
    let image = crate::image::FileImage::<2048, _>::new(card, img_off, img_size);
    let vol   = Iso9660::mount(image)?;

    let v = &mut VOLUMES[dst_idx];
    set_mount(&mut v.mount, mount);
    v.kind   = FsKind::Iso9660;
    v.inner  = VolumeInner::Iso9660Img(vol);
    v.in_use = true;
    Ok(())
}

/// Unmount a volume. Returns `Err(FilesOpen)` if any files are still open.
pub unsafe fn unmount(mount: &str) -> Result<()> {
    let idx = find_volume(mount).ok_or(FsError::NotFound)?;
    if VOLUMES[idx].open_count > 0 { return Err(FsError::FilesOpen); }
    VOLUMES[idx].in_use = false;
    VOLUMES[idx].inner  = VolumeInner::Empty;
    Ok(())
}

pub unsafe fn is_mounted(mount: &str) -> bool { find_volume(mount).is_some() }

pub unsafe fn list_volumes<F>(mut cb: F) where F: FnMut(&str, &str) {
    for v in VOLUMES.iter() {
        if !v.in_use { continue; }
        let end = v.mount.iter().position(|&b|b==0).unwrap_or(MP_LEN);
        let mp  = core::str::from_utf8(&v.mount[..end]).unwrap_or("?");
        let kind = match &v.inner {
            VolumeInner::Fat32Sd(fv) => fv.kind_str(),
            #[cfg(feature = "ext2")]
            VolumeInner::Ext2Sd(ev)  => ev.kind_str(),
            VolumeInner::Iso9660Dvd(_) | VolumeInner::Iso9660Img(_) => "ISO9660",
            VolumeInner::GcDvd(_)    => "GC-DVD",
            VolumeInner::MemCard(_)  => "MemCard",
            VolumeInner::Empty       => "?",
        };
        cb(mp, kind);
    }
}

// ─── VfsFile ──────────────────────────────────────────────────────────────────

/// A file handle that is independent of the volume's borrow lifetime.
///
/// FAT and EXT2 variants store raw cluster/inode state and index into
/// the global `VOLUMES` table — no `'static` transmute required.
/// Read-only drivers (ISO, GcDvd) use a transmute that is sound because
/// `VOLUMES` is `static mut` and never moved.
pub enum VfsFile {
    /// FAT32 or ExFAT file on an SD card.
    FatSd {
        vol_idx:  usize,
        start:    u32,
        cur:      u32,
        size:     u64,
        pos:      u64,
        clus_pos: u64,
    },
    /// EXT2/3/4 file on an SD card.
    #[cfg(feature = "ext2")]
    Ext2Sd {
        vol_idx:   usize,
        ino:       u32,
        size:      u64,
        flags:     u32,
        block_raw: [u8; 60],
        blocks:    [u32; 15],
        pos:       u64,
    },
    /// ISO 9660 file (DVD or image). Read-only.
    IsoDvd(crate::iso9660::IsoFile<'static, DvdDisk>),
    IsoDvdImg(crate::iso9660::IsoFile<'static, crate::image::FileImage<2048, SdCard>>),
    /// GC memory-card raw bytes. Read-only.
    MemCardRaw { data: &'static [u8], pos: usize },
    Empty,
}

impl VfsFile {
    pub fn size(&self) -> u64 {
        match self {
            VfsFile::FatSd { size, .. }                    => *size,
            #[cfg(feature = "ext2")]
            VfsFile::Ext2Sd { size, .. }                   => *size,
            VfsFile::IsoDvd(f)                             => f.size(),
            VfsFile::IsoDvdImg(f)                          => f.size(),
            VfsFile::MemCardRaw { data, .. }               => data.len() as u64,
            VfsFile::Empty                                 => 0,
        }
    }

    pub fn pos(&self) -> u64 {
        match self {
            VfsFile::FatSd { pos, .. }                     => *pos,
            #[cfg(feature = "ext2")]
            VfsFile::Ext2Sd { pos, .. }                    => *pos,
            VfsFile::IsoDvd(f)                             => f.pos(),
            VfsFile::IsoDvdImg(f)                          => f.pos(),
            VfsFile::MemCardRaw { pos, .. }                => *pos as u64,
            VfsFile::Empty                                 => 0,
        }
    }

    pub unsafe fn seek(&mut self, target: u64) -> Result<()> {
        match self {
            VfsFile::FatSd { vol_idx, start, cur, size, pos, clus_pos } => {
                match &VOLUMES[*vol_idx].inner {
                    VolumeInner::Fat32Sd(fv) =>
                        fv.raw_seek(*start, cur, pos, clus_pos, *size, target),
                    _ => Err(FsError::Unsupported),
                }
            }
            #[cfg(feature = "ext2")]
            VfsFile::Ext2Sd { size, pos, .. } => {
                if target > *size { return Err(FsError::InvalidArg); }
                *pos = target; Ok(())
            }
            VfsFile::IsoDvd(f)      => f.seek(target),
            VfsFile::IsoDvdImg(f)   => f.seek(target),
            VfsFile::MemCardRaw { pos, data } => {
                if target > data.len() as u64 { return Err(FsError::InvalidArg); }
                *pos = target as usize; Ok(())
            }
            VfsFile::Empty => Err(FsError::NotFound),
        }
    }

    pub unsafe fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match self {
            VfsFile::FatSd { vol_idx, cur, size, pos, clus_pos, .. } => {
                match &VOLUMES[*vol_idx].inner {
                    VolumeInner::Fat32Sd(fv) =>
                        fv.raw_read(cur, pos, clus_pos, *size, buf),
                    _ => Err(FsError::Unsupported),
                }
            }
            #[cfg(feature = "ext2")]
            VfsFile::Ext2Sd { vol_idx, flags, block_raw, blocks, size, pos, .. } => {
                match &VOLUMES[*vol_idx].inner {
                    VolumeInner::Ext2Sd(ev) =>
                        ev.raw_read(*flags, block_raw, blocks, *size, pos, buf),
                    _ => Err(FsError::Unsupported),
                }
            }
            VfsFile::IsoDvd(f)      => f.read(buf),
            VfsFile::IsoDvdImg(f)   => f.read(buf),
            VfsFile::MemCardRaw { pos, data } => {
                let rem  = data.len().saturating_sub(*pos);
                let take = rem.min(buf.len());
                buf[..take].copy_from_slice(&data[*pos..*pos+take]);
                *pos += take; Ok(take)
            }
            VfsFile::Empty => Err(FsError::NotFound),
        }
    }

    pub unsafe fn write(&mut self, buf: &[u8]) -> Result<usize> {
        match self {
            VfsFile::FatSd { vol_idx, start, cur, size, pos, clus_pos } => {
                match &VOLUMES[*vol_idx].inner {
                    VolumeInner::Fat32Sd(fv) =>
                        fv.raw_write(*start, cur, pos, clus_pos, size, buf),
                    _ => Err(FsError::Unsupported),
                }
            }
            #[cfg(feature = "ext2")]
            VfsFile::Ext2Sd { vol_idx, ino, flags, block_raw, blocks, size, pos } => {
                match &mut VOLUMES[*vol_idx].inner {
                    VolumeInner::Ext2Sd(ev) =>
                        ev.raw_write(*ino, flags, block_raw, blocks, size, pos, buf),
                    _ => Err(FsError::Unsupported),
                }
            }
            _ => Err(FsError::ReadOnly),
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

impl Drop for VfsFile {
    fn drop(&mut self) {
        let idx = match self {
            VfsFile::FatSd  { vol_idx, .. } => Some(*vol_idx),
            #[cfg(feature = "ext2")]
            VfsFile::Ext2Sd { vol_idx, .. } => Some(*vol_idx),
            _ => None,
        };
        if let Some(i) = idx {
            unsafe {
                if VOLUMES[i].open_count > 0 { VOLUMES[i].open_count -= 1; }
            }
        }
    }
}

// ─── VFS operations ───────────────────────────────────────────────────────────

/// Open a file for reading (and writing if the filesystem supports it).
pub unsafe fn open(path: &str) -> Result<VfsFile> {
    let (mp, rest) = split_mount(path);
    let idx = find_volume(mp).ok_or(FsError::NotFound)?;
    let vol = &mut VOLUMES[idx];

    let file = match &mut vol.inner {
        #[cfg(feature = "fat")]
        VolumeInner::Fat32Sd(fv) => {
            let f = fv.open(rest)?;
            VfsFile::FatSd {
                vol_idx: idx, start: f.start, cur: f.start,
                size: f.size(), pos: 0, clus_pos: 0,
            }
        }
        #[cfg(feature = "ext2")]
        VolumeInner::Ext2Sd(ev) => {
            // ev.open() returns Ext2File whose accessors expose all cached inode state.
            let f = ev.open(rest)?;
            VfsFile::Ext2Sd {
                vol_idx:   idx,
                ino:       f.ino,
                size:      f.size(),
                flags:     f.flags(),
                block_raw: *f.block_raw(),
                blocks:    *f.blocks(),
                pos:       0,
            }
        }
        VolumeInner::Iso9660Dvd(iv) => {
            let iv: &'static Iso9660<DvdDisk> = core::mem::transmute(&*iv);
            VfsFile::IsoDvd(iv.open(rest)?)
        }
        VolumeInner::Iso9660Img(iv) => {
            let iv: &'static Iso9660<crate::image::FileImage<2048, SdCard>>
                = core::mem::transmute(&*iv);
            VfsFile::IsoDvdImg(iv.open(rest)?)
        }
        _ => return Err(FsError::Unsupported),
    };

    vol.open_count = vol.open_count.saturating_add(1);
    Ok(file)
}

/// Create a new file and open it for writing.
pub unsafe fn create(path: &str) -> Result<VfsFile> {
    let (mp, rest) = split_mount(path);
    let (dir, name) = {
        let rest = rest.trim_start_matches('/');
        match rest.rfind('/') {
            Some(i) => (&rest[..i], &rest[i+1..]),
            None    => ("", rest),
        }
    };
    let idx = find_volume(mp).ok_or(FsError::NotFound)?;
    let vol = &mut VOLUMES[idx];

    let file = match &mut vol.inner {
        #[cfg(feature = "fat")]
        VolumeInner::Fat32Sd(fv) => {
            let f = fv.create(dir, name)?;
            VfsFile::FatSd {
                vol_idx: idx, start: f.start, cur: f.start,
                size: 0, pos: 0, clus_pos: 0,
            }
        }
        #[cfg(feature = "ext2")]
        VolumeInner::Ext2Sd(ev) => {
            let ino = ev.create_file(rest)?;
            // inode_raw_info() returns (flags, block_raw, blocks) without
            // exposing the private Inode type.
            let (flags, block_raw, blocks) = ev.inode_raw_info(ino)?;
            VfsFile::Ext2Sd {
                vol_idx: idx, ino,
                size: 0, flags, block_raw, blocks, pos: 0,
            }
        }
        _ => return Err(FsError::Unsupported),
    };

    vol.open_count = vol.open_count.saturating_add(1);
    Ok(file)
}

pub unsafe fn mkdir(path: &str) -> Result<()> {
    let (mp, rest) = split_mount(path);
    let idx = find_volume(mp).ok_or(FsError::NotFound)?;
    match &mut VOLUMES[idx].inner {
        #[cfg(feature = "fat")]
        VolumeInner::Fat32Sd(fv) => fv.mkdir(rest),
        #[cfg(feature = "ext2")]
        VolumeInner::Ext2Sd(ev) => ev.mkdir(rest),
        _ => Err(FsError::Unsupported),
    }
}

pub unsafe fn unlink(path: &str) -> Result<()> {
    let (mp, rest) = split_mount(path);
    let idx = find_volume(mp).ok_or(FsError::NotFound)?;
    match &mut VOLUMES[idx].inner {
        #[cfg(feature = "fat")]
        VolumeInner::Fat32Sd(fv) => fv.unlink(rest),
        #[cfg(feature = "ext2")]
        VolumeInner::Ext2Sd(ev) => ev.unlink(rest),
        _ => Err(FsError::Unsupported),
    }
}

pub unsafe fn rmdir(path: &str) -> Result<()> {
    let (mp, rest) = split_mount(path);
    let idx = find_volume(mp).ok_or(FsError::NotFound)?;
    match &mut VOLUMES[idx].inner {
        #[cfg(feature = "fat")]
        VolumeInner::Fat32Sd(fv) => fv.rmdir(rest),
        #[cfg(feature = "ext2")]
        VolumeInner::Ext2Sd(ev) => ev.rmdir(rest),
        _ => Err(FsError::Unsupported),
    }
}

pub unsafe fn read_dir<F>(path: &str, cb: F) -> Result<()>
where F: FnMut(&Metadata) -> bool
{
    let (mp, rest) = split_mount(path);
    let idx = find_volume(mp).ok_or(FsError::NotFound)?;
    match &mut VOLUMES[idx].inner {
        #[cfg(feature = "fat")]
        VolumeInner::Fat32Sd(fv)    => fv.read_dir(rest, cb),
        #[cfg(feature = "ext2")]
        VolumeInner::Ext2Sd(ev)     => ev.read_dir(rest, cb),
        VolumeInner::Iso9660Dvd(iv) => iv.read_dir(rest, cb),
        VolumeInner::Iso9660Img(iv) => iv.read_dir(rest, cb),
        VolumeInner::GcDvd(dv)     => dv.read_dir(rest, cb),
        VolumeInner::MemCard(mc)    => {
            let mut f = cb;
            mc.read_dir(|meta, _| f(meta));
            Ok(())
        }
        VolumeInner::Empty => Err(FsError::NotFound),
    }
}

pub unsafe fn stat(path: &str) -> Result<Metadata> {
    let (mp, rest) = split_mount(path);
    let idx = find_volume(mp).ok_or(FsError::NotFound)?;
    match &mut VOLUMES[idx].inner {
        #[cfg(feature = "fat")]
        VolumeInner::Fat32Sd(fv)    => fv.stat(rest),
        #[cfg(feature = "ext2")]
        VolumeInner::Ext2Sd(ev)     => ev.stat(rest),
        VolumeInner::Iso9660Dvd(iv) => iv.stat(rest),
        VolumeInner::Iso9660Img(iv) => iv.stat(rest),
        VolumeInner::GcDvd(dv)     => dv.stat(rest),
        _ => Err(FsError::Unsupported),
    }
}

/// Read a file from the GC DVD filesystem directly into a buffer.
pub unsafe fn read_dvd_file(path: &str, buf: &mut [u8]) -> Result<usize> {
    let (mp, rest) = split_mount(path);
    let idx = find_volume(mp).ok_or(FsError::NotFound)?;
    match &mut VOLUMES[idx].inner {
        VolumeInner::GcDvd(dv) => dv.read_file(rest, buf),
        _ => Err(FsError::Unsupported),
    }
}

/// Read a memory card file by game code and filename.
pub unsafe fn mc_read_file(
    mount: &str, gamecode: &[u8; 4], filename: &str, buf: &mut [u8],
) -> Result<usize> {
    let idx = find_volume(mount).ok_or(FsError::NotFound)?;
    match &mut VOLUMES[idx].inner {
        VolumeInner::MemCard(mc) => {
            let entry = mc.find(gamecode, filename).ok_or(FsError::NotFound)?;
            let entry_copy = *entry;
            mc.read_file(&entry_copy, buf)
        }
        _ => Err(FsError::Unsupported),
    }
}
