//! GameCube disc filesystem (FST — File System Table).
//!
//! GameCube discs use a proprietary Nintendo filesystem with a flat FST
//! (File System Table) at a fixed disc location.
//!
//! ## Disc layout
//!
//! ```text
//! 0x00000000  Disc header (0x440 bytes)
//!   0x0000    Game code (4 bytes), maker code (2), disc num, version
//!   0x0008    Audio streaming enable, buffer size
//!   0x001C    DVD magic: 0xC2339F3D
//!   0x0020    Game title (0x3E0 bytes)
//!   0x0400    Boot file info:
//!               0x0420  DOL offset
//!               0x0424  FST offset
//!               0x0428  FST size
//!   0x2000    Apploader (loads main DOL + game code)
//!   (DOL offset)  Main executable
//!   (FST offset)  File System Table
//! ```
//!
//! ## FST format
//!
//! ```text
//! [root entry: 12 bytes]
//! [N-1 entries: 12 bytes each]
//! [string table: variable]
//! ```
//!
//! Each FST entry is 12 bytes:
//! ```text
//! byte 0:     flags (0 = file, 1 = directory)
//! bytes 1-3:  filename offset in string table (24-bit BE)
//! bytes 4-7:  file: disc offset | dir: parent entry index
//! bytes 8-11: file: file size  | dir: next sibling entry index
//! ```

use crate::{BlockDev, FsError, Metadata, Result, path_split};

const DISC_HEADER_SIZE: usize = 0x440;
const BOOT_INFO_OFFSET: usize = 0x420;
const GC_MAGIC:         u32   = 0xC2339F3D;
const MAX_FST_ENTRIES:  usize = 4096;
const MAX_STRTAB_SIZE:  usize = 65536;

/// Parsed disc header.
#[derive(Clone, Copy, Default)]
pub struct DiscHeader {
    pub game_code:   [u8; 4],
    pub maker_code:  [u8; 2],
    pub disc_num:    u8,
    pub version:     u8,
    pub title:       [u8; 64],   // first 64 bytes of title
    pub dol_offset:  u32,
    pub fst_offset:  u32,
    pub fst_size:    u32,
}

/// A mounted GameCube disc filesystem.
pub struct GcDvd<D: BlockDev> {
    dev:         D,
    header:      DiscHeader,
    /// FST entries (12 bytes each, stored as raw bytes)
    fst_data:    [u8; MAX_FST_ENTRIES * 12],
    fst_count:   u32,
    /// String table
    strtab:      [u8; MAX_STRTAB_SIZE],
    strtab_size: u32,
}

impl<D: BlockDev> GcDvd<D> {
    pub unsafe fn mount(dev: D) -> Result<Self> {
        let ss = dev.sector_size();

        // Read header (first 2 sectors for 2048-byte disc sectors, or more for 512-byte)
        let header_sectors = (DISC_HEADER_SIZE + ss - 1) / ss;
        let mut hdr_buf = [0u8; 4096];
        for i in 0..header_sectors.min(4) {
            dev.read_sector(i as u64, &mut hdr_buf[i * ss..(i+1) * ss])?;
        }

        // Validate GC magic
        let magic = u32::from_be_bytes([hdr_buf[0x1C], hdr_buf[0x1D], hdr_buf[0x1E], hdr_buf[0x1F]]);
        if magic != GC_MAGIC { return Err(FsError::BadFormat); }

        let mut header = DiscHeader::default();
        header.game_code.copy_from_slice(&hdr_buf[0..4]);
        header.maker_code.copy_from_slice(&hdr_buf[4..6]);
        header.disc_num  = hdr_buf[6];
        header.version   = hdr_buf[7];
        header.title[..64].copy_from_slice(&hdr_buf[0x20..0x60]);
        header.dol_offset = u32::from_be_bytes([hdr_buf[0x420], hdr_buf[0x421], hdr_buf[0x422], hdr_buf[0x423]]);
        header.fst_offset = u32::from_be_bytes([hdr_buf[0x424], hdr_buf[0x425], hdr_buf[0x426], hdr_buf[0x427]]);
        header.fst_size   = u32::from_be_bytes([hdr_buf[0x428], hdr_buf[0x429], hdr_buf[0x42A], hdr_buf[0x42B]]);

        // Read FST
        let fst_lba = header.fst_offset as u64 / ss as u64;
        let fst_sectors = (header.fst_size as usize + ss - 1) / ss;
        let mut fst_raw = [0u8; MAX_FST_ENTRIES * 12 + MAX_STRTAB_SIZE];
        for i in 0..fst_sectors.min(fst_raw.len() / ss) {
            dev.read_sector(fst_lba + i as u64, &mut fst_raw[i * ss..(i+1) * ss])?;
        }

        // Root entry: entry count is at bytes 8-11 of entry 0
        let root_count = u32::from_be_bytes([fst_raw[8], fst_raw[9], fst_raw[10], fst_raw[11]]);
        let fst_count  = root_count.min(MAX_FST_ENTRIES as u32);
        let strtab_off = (fst_count as usize) * 12;
        let strtab_len = (header.fst_size as usize).saturating_sub(strtab_off);

        let mut fst = GcDvd {
            dev,
            header,
            fst_data: [0u8; MAX_FST_ENTRIES * 12],
            fst_count,
            strtab: [0u8; MAX_STRTAB_SIZE],
            strtab_size: strtab_len.min(MAX_STRTAB_SIZE) as u32,
        };

        let copy_fst  = (fst_count as usize * 12).min(MAX_FST_ENTRIES * 12);
        fst.fst_data[..copy_fst].copy_from_slice(&fst_raw[..copy_fst]);
        let copy_str = strtab_len.min(MAX_STRTAB_SIZE);
        if strtab_off + copy_str <= fst_raw.len() {
            fst.strtab[..copy_str].copy_from_slice(&fst_raw[strtab_off..strtab_off + copy_str]);
        }

        Ok(fst)
    }

    // ── Entry accessors ────────────────────────────────────────────────────

    fn entry(&self, idx: u32) -> Option<FstEntry> {
        if idx >= self.fst_count { return None; }
        let off = (idx as usize) * 12;
        let d = &self.fst_data[off..off+12];
        let is_dir   = d[0] & 1 != 0;
        let name_off = u32::from_be_bytes([0, d[1], d[2], d[3]]) as usize;
        let param1   = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
        let param2   = u32::from_be_bytes([d[8], d[9], d[10], d[11]]);
        let name = self.strtab_str(name_off);
        Some(FstEntry { idx, is_dir, name, param1, param2 })
    }

    fn strtab_str(&self, off: usize) -> &str {
        if off >= self.strtab_size as usize { return ""; }
        let end = self.strtab[off..self.strtab_size as usize]
            .iter().position(|&b| b == 0).unwrap_or(0) + off;
        core::str::from_utf8(&self.strtab[off..end]).unwrap_or("?")
    }

    // ── Path resolution ────────────────────────────────────────────────────

    /// Find an FST entry by path. Returns entry index.
    fn find_path(&self, path: &str) -> Result<u32> {
        self.find_in_dir(0, path)
    }

    fn find_in_dir(&self, dir_idx: u32, path: &str) -> Result<u32> {
        let (first, rest) = path_split(path);
        if first.is_empty() { return Ok(dir_idx); }

        let root = self.entry(dir_idx).ok_or(FsError::NotFound)?;
        // For a directory, param2 = next sibling index (scan up to there)
        let end_idx = if dir_idx == 0 { self.fst_count } else { root.param2 };

        let mut i = dir_idx + 1;
        while i < end_idx {
            let e = match self.entry(i) { Some(e) => e, None => break };
            if e.name.eq_ignore_ascii_case(first) {
                return if rest.is_empty() {
                    Ok(i)
                } else if e.is_dir {
                    self.find_in_dir(i, rest)
                } else {
                    Err(FsError::WrongType)
                };
            }
            // Skip over directory subtrees
            i = if e.is_dir { e.param2 } else { i + 1 };
        }
        Err(FsError::NotFound)
    }

    // ── Public API ─────────────────────────────────────────────────────────

    pub fn header(&self) -> &DiscHeader { &self.header }

    /// Iterate directory entries.
    pub fn read_dir<F>(&self, path: &str, mut cb: F) -> Result<()>
    where F: FnMut(&Metadata) -> bool
    {
        let dir_idx = self.find_path(path)?;
        let e = self.entry(dir_idx).ok_or(FsError::NotFound)?;
        if !e.is_dir && dir_idx != 0 { return Err(FsError::WrongType); }

        let end = if dir_idx == 0 { self.fst_count } else { e.param2 };
        let mut i = dir_idx + 1;
        while i < end {
            let child = match self.entry(i) { Some(e) => e, None => break };
            let mut meta = Metadata::zeroed();
            meta.is_dir = child.is_dir;
            meta.size   = if child.is_dir { 0 } else { child.param2 as u64 };
            meta.set_name(child.name);
            if !cb(&meta) { break; }
            i = if child.is_dir { child.param2 } else { i + 1 };
        }
        Ok(())
    }

    /// Read file data into `buf`. Reads min(buf.len(), file_size) bytes.
    pub unsafe fn read_file(&self, path: &str, buf: &mut [u8]) -> Result<usize> {
        let idx = self.find_path(path)?;
        let e   = self.entry(idx).ok_or(FsError::NotFound)?;
        if e.is_dir { return Err(FsError::WrongType); }

        let disc_offset = e.param1 as u64;
        let file_size   = e.param2 as usize;
        let to_read     = buf.len().min(file_size);

        let ss      = self.dev.sector_size();
        let lba     = disc_offset / ss as u64;
        let off     = (disc_offset % ss as u64) as usize;
        let sectors = (off + to_read + ss - 1) / ss;
        let mut tmp = [0u8; 4096];
        let mut written = 0usize;

        for s in 0..sectors {
            let chunk_ss = ss.min(tmp.len());
            self.dev.read_sector(lba + s as u64, &mut tmp[..chunk_ss])?;
            let src_off = if s == 0 { off } else { 0 };
            let take = (chunk_ss - src_off).min(to_read - written);
            buf[written..written + take].copy_from_slice(&tmp[src_off..src_off + take]);
            written += take;
        }
        Ok(written)
    }

    pub fn stat(&self, path: &str) -> Result<Metadata> {
        let idx = self.find_path(path)?;
        let e   = self.entry(idx).ok_or(FsError::NotFound)?;
        let mut meta = Metadata::zeroed();
        meta.is_dir = e.is_dir;
        meta.size   = if e.is_dir { 0 } else { e.param2 as u64 };
        meta.set_name(e.name);
        Ok(meta)
    }
}

struct FstEntry<'a> {
    idx:    u32,
    is_dir: bool,
    name:   &'a str,
    param1: u32, // file: disc offset | dir: parent idx
    param2: u32, // file: size        | dir: next sibling idx
}
