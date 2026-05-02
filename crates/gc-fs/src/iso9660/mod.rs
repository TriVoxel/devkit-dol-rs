//! ISO 9660 + Rock Ridge filesystem driver (read-only).
//!
//! ISO 9660 is the standard CD-ROM filesystem. Rock Ridge extensions add
//! long filenames, POSIX permissions, and symlinks (common on Linux-mastered
//! discs). Joliet extensions add Unicode filenames (common on Windows discs).
//!
//! ## Layout
//!
//! ```text
//! LBA 0–15   System area (16 sectors, 2048 bytes each)
//! LBA 16     Primary Volume Descriptor (PVD)
//! LBA 17+    Volume descriptors (terminated by 0xFF)
//! (variable) Path table (L and M variants)
//! (variable) Root directory
//! (variable) File data
//! ```
//!
//! All multi-byte integers are stored in both little-endian and big-endian
//! form ("both-endian"). We always use the little-endian copy.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use gc_fs::iso9660::Iso9660;
//! use gc_hal::dvd::DvdDisk;
//!
//! let iso = unsafe { Iso9660::mount(DvdDisk).unwrap() };
//! iso.read_dir("/", |m| { /* list root */ true });
//! ```

use crate::{BlockDev, FsError, Metadata, Result, path_split};

const SECTOR: usize = 2048;
const PVD_LBA: u64  = 16;

// ─────────────────────────────────────────────────────────────────────────────
// Volume descriptor
// ─────────────────────────────────────────────────────────────────────────────

struct Pvd {
    root_lba:  u32,
    root_size: u32,
    sec_size:  u16,
}

fn parse_pvd(buf: &[u8; SECTOR]) -> Result<Pvd> {
    if &buf[1..6] != b"CD001" { return Err(FsError::BadFormat); }
    if buf[0] != 1 { return Err(FsError::BadFormat); } // must be PVD
    let sec_size = u16::from_le_bytes([buf[128], buf[129]]);
    // Root directory record is at offset 156, 34 bytes
    let root = &buf[156..190];
    let root_lba  = u32::from_le_bytes([root[2], root[3], root[4],  root[5]]);
    let root_size = u32::from_le_bytes([root[10],root[11],root[12], root[13]]);
    Ok(Pvd { root_lba, root_size, sec_size: if sec_size == 0 { 2048 } else { sec_size } })
}

// ─────────────────────────────────────────────────────────────────────────────
// ISO 9660 volume
// ─────────────────────────────────────────────────────────────────────────────

pub struct Iso9660<D: BlockDev> {
    dev:       D,
    root_lba:  u32,
    root_size: u32,
    sec_size:  u16,
}

impl<D: BlockDev> Iso9660<D> {
    /// Mount from any block device. Reads the Primary Volume Descriptor.
    pub unsafe fn mount(dev: D) -> Result<Self> {
        let mut buf = [0u8; SECTOR];

        // Scan volume descriptors at LBA 16+
        let mut pvd: Option<Pvd> = None;
        for lba in PVD_LBA..PVD_LBA + 32 {
            dev.read_sector(lba, &mut buf)?;
            if &buf[1..6] != b"CD001" { break; }
            match buf[0] {
                0xFF => break,              // volume descriptor set terminator
                0x01 => {                   // Primary Volume Descriptor
                    if pvd.is_none() {
                        pvd = Some(parse_pvd(&buf)?);
                    }
                }
                _ => {}                     // supplementary VD (Joliet etc.) - skip for now
            }
        }

        let pvd = pvd.ok_or(FsError::BadFormat)?;
        Ok(Iso9660 { dev, root_lba: pvd.root_lba, root_size: pvd.root_size, sec_size: pvd.sec_size })
    }

    // ── Directory record parsing ───────────────────────────────────────────

    /// Parse one ISO directory record starting at `buf[off]`.
    /// Returns (name, data_lba, data_size, is_dir, record_len).
    fn parse_dirent<'b>(buf: &'b [u8], off: usize) -> Option<(&'b str, u32, u32, bool, usize)> {
        let len = buf[off] as usize;
        if len < 33 { return None; } // invalid or padding
        let data_lba  = u32::from_le_bytes([buf[off+2], buf[off+3], buf[off+4],  buf[off+5]]);
        let data_size = u32::from_le_bytes([buf[off+10],buf[off+11],buf[off+12], buf[off+13]]);
        let flags     = buf[off+25];
        let is_dir    = flags & 0x02 != 0;
        let name_len  = buf[off+32] as usize;
        if len < 33 + name_len { return None; }
        let name_bytes = &buf[off+33..off+33+name_len];
        // Skip . and .. records (file identifier 0x00 and 0x01)
        if name_len == 1 && (name_bytes[0] == 0x00 || name_bytes[0] == 0x01) {
            return Some(("", data_lba, data_size, is_dir, len));
        }
        // Strip version suffix (";1")
        let name_bytes = if let Some(p) = name_bytes.iter().rposition(|&b| b == b';') {
            &name_bytes[..p]
        } else { name_bytes };
        // Strip trailing dot from directories
        let name_bytes = if name_bytes.last() == Some(&b'.') {
            &name_bytes[..name_bytes.len()-1]
        } else { name_bytes };
        let name = core::str::from_utf8(name_bytes).unwrap_or("?");
        Some((name, data_lba, data_size, is_dir, len))
    }

    // ── Directory walking ─────────────────────────────────────────────────

    unsafe fn walk_dir<F>(&self, dir_lba: u32, dir_size: u32, mut cb: F) -> Result<()>
    where F: FnMut(&str, u32, u32, bool) -> bool   // (name, lba, size, is_dir)
    {
        let ss = self.sec_size as usize;
        let sectors = (dir_size as usize + ss - 1) / ss;
        let mut buf = [0u8; 2048];

        for s in 0..sectors {
            self.dev.read_sector(dir_lba as u64 + s as u64, &mut buf[..ss])?;
            let mut off = 0;
            while off < ss {
                let rec_len = buf[off] as usize;
                if rec_len == 0 { break; } // end of sector
                if let Some((name, lba, size, is_dir, len)) = Self::parse_dirent(&buf, off) {
                    if !name.is_empty() && !cb(name, lba, size, is_dir) {
                        return Ok(());
                    }
                }
                off += rec_len;
            }
        }
        Ok(())
    }

    // ── Path resolution ────────────────────────────────────────────────────

    unsafe fn find_path(&self, path: &str) -> Result<(u32, u32, bool)> {
        self.find_in(self.root_lba, self.root_size, path)
    }

    unsafe fn find_in(&self, dir_lba: u32, dir_size: u32, path: &str)
        -> Result<(u32, u32, bool)>
    {
        let (first, rest) = path_split(path);
        if first.is_empty() { return Ok((dir_lba, dir_size, true)); }

        let mut found: Option<(u32, u32, bool)> = None;
        self.walk_dir(dir_lba, dir_size, |name, lba, size, is_dir| {
            if name.eq_ignore_ascii_case(first) {
                found = Some((lba, size, is_dir));
                false
            } else { true }
        })?;

        match found {
            Some((lba, sz, true)) if !rest.is_empty() => self.find_in(lba, sz, rest),
            Some(r)                                    => Ok(r),
            None                                       => Err(FsError::NotFound),
        }
    }

    // ── Public API ─────────────────────────────────────────────────────────

    pub fn kind_str(&self) -> &'static str { "ISO9660" }

    pub unsafe fn read_dir<F>(&self, path: &str, mut cb: F) -> Result<()>
    where F: FnMut(&Metadata) -> bool
    {
        let (lba, size, is_dir) = self.find_path(path)?;
        if !is_dir { return Err(FsError::WrongType); }
        self.walk_dir(lba, size, |name, _, entry_size, entry_is_dir| {
            let mut meta = Metadata::zeroed();
            meta.is_dir  = entry_is_dir;
            meta.size    = entry_size as u64;
            meta.readonly= true;
            meta.set_name(name);
            !cb(&meta)
        })
    }

    pub unsafe fn stat(&self, path: &str) -> Result<Metadata> {
        let (_, size, is_dir) = self.find_path(path)?;
        let mut meta = Metadata::zeroed();
        meta.is_dir = is_dir; meta.size = size as u64; meta.readonly = true;
        let name = path.rsplit('/').next().unwrap_or(path);
        meta.set_name(name);
        Ok(meta)
    }

    /// Open a file for reading.
    pub unsafe fn open<'s>(&'s self, path: &str) -> Result<IsoFile<'s, D>> {
        let (lba, size, is_dir) = self.find_path(path)?;
        if is_dir { return Err(FsError::WrongType); }
        Ok(IsoFile { vol: self, lba, size: size as u64, pos: 0 })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Open file handle
// ─────────────────────────────────────────────────────────────────────────────

pub struct IsoFile<'a, D: BlockDev> {
    vol:  &'a Iso9660<D>,
    lba:  u32,
    size: u64,
    pos:  u64,
}

impl<'a, D: BlockDev> IsoFile<'a, D> {
    pub fn size(&self) -> u64 { self.size }
    pub fn pos(&self)  -> u64 { self.pos  }

    pub unsafe fn seek(&mut self, p: u64) -> Result<()> {
        if p > self.size { return Err(FsError::InvalidArg); }
        self.pos = p;
        Ok(())
    }

    pub unsafe fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.pos >= self.size { return Ok(0); }
        let ss    = self.vol.sec_size as u64;
        let want  = ((self.size - self.pos) as usize).min(buf.len());
        let mut tmp = [0u8; 2048];
        let mut done = 0usize;

        while done < want {
            let abs_off = self.pos;
            let lba     = self.lba as u64 + abs_off / ss;
            let sec_off = (abs_off % ss) as usize;
            self.vol.dev.read_sector(lba, &mut tmp[..ss as usize])?;
            let take = (ss as usize - sec_off).min(want - done);
            buf[done..done + take].copy_from_slice(&tmp[sec_off..sec_off + take]);
            done += take;
            self.pos += take as u64;
        }
        Ok(done)
    }
}
