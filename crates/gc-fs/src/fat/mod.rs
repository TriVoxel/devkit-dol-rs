//! FAT32 / ExFAT filesystem driver — full read/write.
//!
//! FAT12 and FAT16 are not supported. SD cards are always FAT32 or ExFAT.
//!
//! ## Feature matrix
//!
//! | Feature            | FAT32 | ExFAT |
//! |--------------------|-------|-------|
//! | Read + LFN         |   ✓   |   ✓   |
//! | Write / create     |   ✓   |   ✓   |
//! | Delete             |   ✓   |   ✓   |
//! | mkdir / rmdir      |   ✓   |   ✓   |
//! | Alloc bitmap maint.|   —   |   ✓   |
//! | Timestamps         |   ✓   |   ✓   |
//!
//! ExFAT name hashes are computed using an ASCII-only up-case function.
//! Non-ASCII filenames are created correctly but hash verification on
//! read may fail on implementations with the full Unicode up-case table.

use crate::{BlockDev, FsError, Metadata, Result, path_split};

// ─── Geometry ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FatKind { Fat32, ExFat }

#[derive(Clone, Copy)]
pub struct FatGeom {
    pub kind:           FatKind,
    pub bytes_per_sec:  u32,
    pub secs_per_clus:  u32,
    pub fat_lba:        u64,
    pub data_lba:       u64,
    pub root_cluster:   u32,
    pub fat_size_secs:  u32,
    pub fat_count:      u8,
    pub total_clusters: u32,
    /// ExFAT: first cluster of allocation bitmap (found in root dir). 0 for FAT32.
    pub bitmap_cluster: u32,
}

impl FatGeom {
    pub fn parse(s: &[u8; 512]) -> Result<Self> {
        if &s[3..11] == b"EXFAT   " { return Self::parse_exfat(s); }

        // FAT32 only — reject FAT12/16 based on cluster count
        let bps  = u16::from_le_bytes([s[11], s[12]]) as u32;
        let spc  = s[13] as u32;
        let rsvd = u16::from_le_bytes([s[14], s[15]]) as u32;
        let nfat = s[16];
        let rde  = u16::from_le_bytes([s[17], s[18]]) as u32;
        let ts16 = u16::from_le_bytes([s[19], s[20]]) as u32;
        let fs16 = u16::from_le_bytes([s[22], s[23]]) as u32;
        let ts32 = u32::from_le_bytes([s[32], s[33], s[34], s[35]]);
        let fs32 = u32::from_le_bytes([s[36], s[37], s[38], s[39]]);
        let rc   = u32::from_le_bytes([s[44], s[45], s[46], s[47]]);

        if bps == 0 || spc == 0 { return Err(FsError::BadFormat); }

        let fat_sz  = if fs16 > 0 { fs16 } else { fs32 };
        let tot     = if ts16 > 0 { ts16 } else { ts32 };
        let fat_lba = rsvd as u64;
        let rde_sec = (rde * 32 + bps - 1) / bps;
        let data_lb = fat_lba + nfat as u64 * fat_sz as u64 + rde_sec as u64;
        let clusters = tot.saturating_sub(data_lb as u32) / spc;

        // FAT32 threshold: >= 65525 clusters
        if clusters < 65525 { return Err(FsError::BadFormat); }

        Ok(FatGeom {
            kind: FatKind::Fat32, bytes_per_sec: bps, secs_per_clus: spc,
            fat_lba, data_lba: data_lb,
            root_cluster: rc, fat_size_secs: fat_sz,
            fat_count: nfat, total_clusters: clusters,
            bitmap_cluster: 0,
        })
    }

    fn parse_exfat(s: &[u8; 512]) -> Result<Self> {
        let fat_off   = u32::from_le_bytes([s[80],  s[81],  s[82],  s[83]]);
        let fat_len   = u32::from_le_bytes([s[84],  s[85],  s[86],  s[87]]);
        let data_off  = u32::from_le_bytes([s[88],  s[89],  s[90],  s[91]]);
        let vol_len   = u64::from_le_bytes(s[72..80].try_into().unwrap());
        let root_clus = u32::from_le_bytes([s[96],  s[97],  s[98],  s[99]]);
        let sec_shift = s[108] as u32;
        let spc_shift = s[109] as u32;
        let n_fats    = s[110];
        let bps = 1u32 << sec_shift;
        let spc = 1u32 << spc_shift;
        let clusters = if data_off < vol_len as u32 {
            (vol_len as u32 - data_off) >> spc_shift
        } else { 0 };
        Ok(FatGeom {
            kind: FatKind::ExFat, bytes_per_sec: bps, secs_per_clus: spc,
            fat_lba: fat_off as u64, data_lba: data_off as u64,
            root_cluster: root_clus, fat_size_secs: fat_len,
            fat_count: n_fats, total_clusters: clusters,
            bitmap_cluster: 0,
        })
    }

    #[inline]
    pub fn cluster_lba(&self, c: u32) -> u64 {
        self.data_lba + (c as u64 - 2) * self.secs_per_clus as u64
    }

    #[inline]
    pub fn bytes_per_cluster(&self) -> u64 {
        self.bytes_per_sec as u64 * self.secs_per_clus as u64
    }
}

// ─── Directory entry information (internal) ──────────────────────────────────

/// Decoded information for one directory entry (file or subdirectory).
struct EntryInfo {
    cluster:  u32,
    size:     u64,
    is_dir:   bool,
    readonly: bool,
    hidden:   bool,
    mtime:    u32,
    name:     [u8; 256],
    name_len: usize,
}

/// Position of one 32-byte raw entry within the directory cluster chain.
#[derive(Clone, Copy)]
struct EntryPos {
    clus: u32,
    sec:  usize,  // sector within cluster
    slot: usize,  // 32-byte slot within sector
}

// ─── FatVolume ────────────────────────────────────────────────────────────────

pub struct FatVolume<D: BlockDev> {
    dev:      D,
    pub geom: FatGeom,
}

impl<D: BlockDev> FatVolume<D> {
    /// Mount from any block device. Validates the boot sector.
    /// For ExFAT, scans the root directory to locate the allocation bitmap.
    pub unsafe fn mount(dev: D) -> Result<Self> {
        let mut sec = [0u8; 512];
        dev.read_sector(0, &mut sec)?;
        if sec[510] != 0x55 || sec[511] != 0xAA { return Err(FsError::BadFormat); }
        let geom = FatGeom::parse(&sec)?;
        let mut vol = FatVolume { dev, geom };
        if vol.geom.kind == FatKind::ExFat {
            vol.scan_exfat_metadata()?;
        }
        Ok(vol)
    }

    pub fn kind(&self) -> FatKind { self.geom.kind }
    pub fn kind_str(&self) -> &'static str {
        match self.geom.kind { FatKind::Fat32 => "FAT32", FatKind::ExFat => "ExFAT" }
    }

    // ── Raw I/O ───────────────────────────────────────────────────────────

    #[inline]
    unsafe fn read_sec(&self, lba: u64, buf: &mut [u8; 512]) -> Result<()> {
        self.dev.read_sector(lba, buf)
    }

    #[inline]
    unsafe fn write_sec(&self, lba: u64, buf: &[u8; 512]) -> Result<()> {
        self.dev.write_sector(lba, buf)
    }

    // ── FAT chain ─────────────────────────────────────────────────────────

    pub unsafe fn fat_next(&self, cluster: u32) -> Result<u32> {
        let bps  = self.geom.bytes_per_sec as u64;
        let byte = cluster as u64 * 4;
        let mut sec = [0u8; 512];
        self.read_sec(self.geom.fat_lba + byte / bps, &mut sec)?;
        let o = (byte % bps) as usize;
        let e = u32::from_le_bytes([sec[o], sec[o+1], sec[o+2], sec[o+3]]) & 0x0FFF_FFFF;
        Ok(if e >= 0x0FFF_FFF8 { 0x0FFF_FFFF } else { e })
    }

    pub unsafe fn fat_set(&self, cluster: u32, value: u32) -> Result<()> {
        let bps = self.geom.bytes_per_sec as u64;
        for copy in 0..self.geom.fat_count as u64 {
            let fat  = self.geom.fat_lba + copy * self.geom.fat_size_secs as u64;
            let byte = cluster as u64 * 4;
            let lba  = fat + byte / bps;
            let off  = (byte % bps) as usize;
            let mut sec = [0u8; 512];
            self.read_sec(lba, &mut sec)?;
            let old = u32::from_le_bytes([sec[off],sec[off+1],sec[off+2],sec[off+3]]);
            let new = (old & 0xF000_0000) | (value & 0x0FFF_FFFF);
            sec[off..off+4].copy_from_slice(&new.to_le_bytes());
            self.write_sec(lba, &sec)?;
        }
        Ok(())
    }

    /// Free an entire cluster chain starting at `first`.
    unsafe fn free_chain(&self, first: u32) -> Result<()> {
        let mut c = first;
        while c >= 2 && c < 0x0FFF_FFF8 {
            let next = self.fat_next(c)?;
            if self.geom.kind == FatKind::ExFat {
                self.bitmap_free(c)?;
            }
            self.fat_set(c, 0)?;
            c = next;
        }
        Ok(())
    }

    /// Allocate one free cluster, mark as EOC in FAT, update ExFAT bitmap.
    unsafe fn alloc_cluster(&self) -> Result<u32> {
        let bps = self.geom.bytes_per_sec as u64;
        let fat = self.geom.fat_lba;
        let mut sec = [0u8; 512];
        let mut cur_lba = u64::MAX;

        for c in 2u32..self.geom.total_clusters.saturating_add(2) {
            let byte = c as u64 * 4;
            let lba  = fat + byte / bps;
            if lba != cur_lba {
                self.read_sec(lba, &mut sec)?;
                cur_lba = lba;
            }
            let off = (byte % bps) as usize;
            let val = u32::from_le_bytes([sec[off],sec[off+1],sec[off+2],sec[off+3]])
                      & 0x0FFF_FFFF;
            if val == 0 {
                if self.geom.kind == FatKind::ExFat {
                    self.bitmap_set(c)?;
                }
                self.fat_set(c, 0x0FFF_FFFF)?;
                return Ok(c);
            }
        }
        Err(FsError::NoSpace)
    }

    // ── ExFAT allocation bitmap ───────────────────────────────────────────

    /// Locate allocation bitmap and up-case table clusters from the ExFAT root.
    unsafe fn scan_exfat_metadata(&mut self) -> Result<()> {
        let root = self.geom.root_cluster;
        let bps  = self.geom.bytes_per_sec as usize;
        let spc  = self.geom.secs_per_clus as usize;
        let mut clus = root;
        'outer: while clus >= 2 && clus < 0x0FFF_FFF8 {
            let clba = self.geom.cluster_lba(clus);
            for s in 0..spc {
                let mut sec = [0u8; 512];
                self.read_sec(clba + s as u64, &mut sec)?;
                for e in 0..(bps / 32) {
                    let off  = e * 32;
                    let etype = sec[off];
                    if etype & 0x80 == 0 { break 'outer; }
                    if etype == 0x81 {
                        // Allocation bitmap
                        self.geom.bitmap_cluster = u32::from_le_bytes(
                            [sec[off+20],sec[off+21],sec[off+22],sec[off+23]]);
                    }
                }
            }
            clus = self.fat_next(clus)?;
        }
        Ok(())
    }

    unsafe fn bitmap_set(&self, cluster: u32) -> Result<()> {
        self.bitmap_op(cluster, true)
    }

    unsafe fn bitmap_free(&self, cluster: u32) -> Result<()> {
        self.bitmap_op(cluster, false)
    }

    unsafe fn bitmap_op(&self, cluster: u32, set: bool) -> Result<()> {
        if self.geom.bitmap_cluster == 0 { return Ok(()); }
        let bit      = cluster - 2;
        let byte_idx = (bit / 8) as u64;
        let bit_idx  = (bit % 8) as u8;
        let bps      = self.geom.bytes_per_sec as u64;
        let bpc      = self.geom.bytes_per_cluster();

        let clus_off  = byte_idx / bpc;
        let byte_in_c = byte_idx % bpc;
        let sec_off   = byte_in_c / bps;
        let byte_in_s = (byte_in_c % bps) as usize;

        let mut c = self.geom.bitmap_cluster;
        for _ in 0..clus_off {
            c = self.fat_next(c)?;
            if c >= 0x0FFF_FFF8 { return Err(FsError::BadFormat); }
        }
        let lba = self.geom.cluster_lba(c) + sec_off;
        let mut sec = [0u8; 512];
        self.read_sec(lba, &mut sec)?;
        if set { sec[byte_in_s] |=  (1 << bit_idx); }
        else   { sec[byte_in_s] &= !(1 << bit_idx); }
        self.write_sec(lba, &sec)
    }

    // ── Directory reader ──────────────────────────────────────────────────

    /// Iterator over decoded directory entries. Handles LFN (FAT32) and
    /// entry sets (ExFAT) transparently.
    unsafe fn walk_entries<F>(&self, start_clus: u32, mut f: F) -> Result<()>
    where F: FnMut(&EntryInfo, &EntryPos, &EntryPos) -> bool
    //        (info, lfn_start_pos, sfn_or_primary_pos)
    {
        if self.geom.kind == FatKind::ExFat {
            self.walk_exfat(start_clus, f)
        } else {
            self.walk_fat32(start_clus, f)
        }
    }

    unsafe fn walk_fat32<F>(&self, start_clus: u32, mut f: F) -> Result<()>
    where F: FnMut(&EntryInfo, &EntryPos, &EntryPos) -> bool
    {
        let bps = self.geom.bytes_per_sec as usize;
        let spc = self.geom.secs_per_clus as usize;

        // LFN accumulator
        let mut lfn_parts   = [[0u16; 13]; 20];
        let mut lfn_count   = 0usize;
        let mut lfn_csum    = 0u8;
        let mut lfn_start   = EntryPos { clus: 0, sec: 0, slot: 0 };
        let mut has_lfn_start = false;

        let mut clus = start_clus;
        'chain: while clus >= 2 && clus < 0x0FFF_FFF8 {
            let clba = self.geom.cluster_lba(clus);
            for s in 0..spc {
                let lba = clba + s as u64;
                let mut sec = [0u8; 512];
                self.read_sec(lba, &mut sec)?;
                for slot in 0..(bps / 32) {
                    let off = slot * 32;
                    let e: &[u8; 32] = sec[off..off+32].try_into().unwrap();
                    let cur_pos = EntryPos { clus, sec: s, slot };

                    if e[0] == 0x00 { break 'chain; }
                    if e[0] == 0xE5 { lfn_count = 0; has_lfn_start = false; continue; }

                    if e[11] == 0x0F {
                        // LFN entry
                        let seq = e[0] & 0x3F;
                        if seq == 0 || seq > 20 {
                            lfn_count = 0; has_lfn_start = false; continue;
                        }
                        if e[0] & 0x40 != 0 {
                            // Last-in-chain (first encountered, highest seq)
                            lfn_count = seq as usize;
                            lfn_csum  = e[13];
                            lfn_start = cur_pos;
                            has_lfn_start = true;
                        }
                        let part_idx = (seq - 1) as usize;
                        lfn_parts[part_idx] = extract_lfn_chars(e);
                        continue;
                    }

                    // Regular (SFN) entry
                    let attr = e[11];
                    if attr & 0x08 != 0 {
                        // Volume label — skip
                        lfn_count = 0; has_lfn_start = false; continue;
                    }

                    let sfn_csum   = sfn_checksum(&e[0..11]);
                    let use_lfn    = lfn_count > 0 && has_lfn_start && lfn_csum == sfn_csum;
                    let mut name   = [0u8; 256];
                    let name_len   = if use_lfn {
                        assemble_lfn(&lfn_parts, lfn_count, &mut name)
                    } else {
                        assemble_sfn(e, &mut name)
                    };

                    let entry_lfn_start = if has_lfn_start { lfn_start }
                                          else             { cur_pos    };

                    let cluster = fat32_entry_cluster(e);
                    let size    = u32::from_le_bytes([e[28],e[29],e[30],e[31]]) as u64;
                    let mdate   = u16::from_le_bytes([e[24], e[25]]);
                    let mtime   = u16::from_le_bytes([e[22], e[23]]);

                    let info = EntryInfo {
                        cluster, size,
                        is_dir:   attr & 0x10 != 0,
                        readonly: attr & 0x01 != 0,
                        hidden:   attr & 0x02 != 0,
                        mtime:    fat_to_unix(mdate, mtime),
                        name, name_len,
                    };

                    lfn_count = 0; has_lfn_start = false;
                    if !f(&info, &entry_lfn_start, &cur_pos) { return Ok(()); }
                }
            }
            clus = self.fat_next(clus)?;
        }
        Ok(())
    }

    unsafe fn walk_exfat<F>(&self, start_clus: u32, mut f: F) -> Result<()>
    where F: FnMut(&EntryInfo, &EntryPos, &EntryPos) -> bool
    {
        let bps = self.geom.bytes_per_sec as usize;
        let spc = self.geom.secs_per_clus as usize;
        let mut clus = start_clus;

        // We use a "flat position" reader that can cross sector boundaries
        // for multi-entry sets.
        let mut pos_clus = clus;
        let mut pos_sec  = 0usize;
        let mut pos_slot = 0usize;
        let mut sec_buf  = [0u8; 512];
        let mut sec_lba  = u64::MAX;

        let mut read_raw = |pos_clus: &mut u32, pos_sec: &mut usize, pos_slot: &mut usize,
                             sec_buf: &mut [u8; 512], sec_lba: &mut u64|
            -> Result<Option<([u8;32], EntryPos)>>
        {
            if *pos_clus < 2 || *pos_clus >= 0x0FFF_FFF8 { return Ok(None); }
            let lba = self.geom.cluster_lba(*pos_clus) + *pos_sec as u64;
            if lba != *sec_lba {
                self.read_sec(lba, sec_buf)?;
                *sec_lba = lba;
            }
            let off = *pos_slot * 32;
            let e: [u8; 32] = sec_buf[off..off+32].try_into().unwrap();
            let cur = EntryPos { clus: *pos_clus, sec: *pos_sec, slot: *pos_slot };

            // Advance position
            *pos_slot += 1;
            if *pos_slot * 32 >= bps {
                *pos_slot = 0;
                *pos_sec += 1;
                if *pos_sec >= spc {
                    *pos_sec = 0;
                    *pos_clus = match self.fat_next(*pos_clus) {
                        Ok(n) => n,
                        Err(e) => return Err(e),
                    };
                }
            }
            Ok(Some((e, cur)))
        };

        loop {
            let (e, primary_pos) = match read_raw(&mut pos_clus, &mut pos_sec, &mut pos_slot, &mut sec_buf, &mut sec_lba)? {
                Some(x) => x,
                None => break,
            };
            let etype = e[0];
            if etype & 0x80 == 0 { break; }    // end of directory
            if etype != 0x85 { continue; }       // not a file entry

            let sec_count = e[1] as usize;
            let file_attr = u16::from_le_bytes([e[4], e[5]]);
            let mtime = exfat_timestamp(&e[8..16]);

            // Collect secondary entries
            let mut cluster    = 0u32;
            let mut data_len   = 0u64;
            let mut name_ucs2  = [0u16; 256];
            let mut name_ucs2_len = 0usize;

            for _ in 0..sec_count {
                let (se, _) = match read_raw(&mut pos_clus, &mut pos_sec, &mut pos_slot, &mut sec_buf, &mut sec_lba)? {
                    Some(x) => x,
                    None => return Err(FsError::BadFormat),
                };
                match se[0] {
                    0xC0 => {
                        // Stream extension
                        cluster  = u32::from_le_bytes([se[20],se[21],se[22],se[23]]);
                        data_len = u64::from_le_bytes(se[24..32].try_into().unwrap());
                    }
                    0xC1 => {
                        // File name extension: 15 UCS-2 chars at bytes [2..32]
                        for j in 0..15usize {
                            if name_ucs2_len >= 255 { break; }
                            let ch = u16::from_le_bytes([se[2+j*2], se[3+j*2]]);
                            if ch == 0 { break; }
                            name_ucs2[name_ucs2_len] = ch;
                            name_ucs2_len += 1;
                        }
                    }
                    _ => {}
                }
            }

            // Convert UCS-2 name to UTF-8
            let mut name     = [0u8; 256];
            let mut name_len = 0usize;
            for i in 0..name_ucs2_len {
                name_len += ucs2_to_utf8(name_ucs2[i], &mut name[name_len..]);
            }

            let info = EntryInfo {
                cluster, size: data_len,
                is_dir:   file_attr & 0x10 != 0,
                readonly: file_attr & 0x01 != 0,
                hidden:   file_attr & 0x02 != 0,
                mtime,
                name, name_len,
            };
            if !f(&info, &primary_pos, &primary_pos) { return Ok(()); }
        }
        Ok(())
    }

    // ── Path resolution ───────────────────────────────────────────────────

    unsafe fn find_in(&self, dir_clus: u32, component: &str) -> Result<EntryInfo> {
        let target = component.as_bytes();
        let mut found: Option<EntryInfo> = None;
        self.walk_entries(dir_clus, |info, _, _| {
            if crate::name_eq_ci(&info.name[..info.name_len], target) {
                found = Some(EntryInfo {
                    cluster: info.cluster, size: info.size,
                    is_dir: info.is_dir, readonly: info.readonly,
                    hidden: info.hidden, mtime: info.mtime,
                    name: info.name, name_len: info.name_len,
                });
                false
            } else { true }
        })?;
        found.ok_or(FsError::NotFound)
    }

    unsafe fn resolve(&self, path: &str) -> Result<EntryInfo> {
        let mut clus = self.geom.root_cluster;
        let mut rest = path.trim_start_matches('/');
        let mut info = EntryInfo {
            cluster: clus, size: 0, is_dir: true, readonly: false,
            hidden: false, mtime: 0, name: [0u8; 256], name_len: 0,
        };
        while !rest.is_empty() {
            let (first, tail) = path_split(rest);
            if first.is_empty() { break; }
            let found = self.find_in(clus, first)?;
            clus = found.cluster;
            info = found;
            rest = tail;
        }
        Ok(info)
    }

    // ── Read API ──────────────────────────────────────────────────────────

    pub unsafe fn open<'s>(&'s self, path: &str) -> Result<FatFile<'s, D>> {
        let info = self.resolve(path)?;
        if info.is_dir { return Err(FsError::WrongType); }
        Ok(FatFile { vol: self, start: info.cluster, cur: info.cluster,
                     size: info.size, pos: 0, clus_pos: 0 })
    }

    pub unsafe fn read_dir<F>(&self, path: &str, mut cb: F) -> Result<()>
    where F: FnMut(&Metadata) -> bool
    {
        let info = self.resolve(path)?;
        if !info.is_dir { return Err(FsError::WrongType); }
        self.walk_entries(info.cluster, |e, _, _| {
            let mut meta = Metadata::zeroed();
            meta.is_dir   = e.is_dir;
            meta.readonly = e.readonly;
            meta.hidden   = e.hidden;
            meta.size     = e.size;
            meta.mtime    = e.mtime;
            meta.name[..e.name_len].copy_from_slice(&e.name[..e.name_len]);
            !cb(&meta)
        })
    }

    pub unsafe fn stat(&self, path: &str) -> Result<Metadata> {
        let info = self.resolve(path)?;
        let mut meta = Metadata::zeroed();
        meta.is_dir   = info.is_dir;
        meta.size     = info.size;
        meta.readonly = info.readonly;
        meta.hidden   = info.hidden;
        meta.mtime    = info.mtime;
        let name = path.rsplit('/').next().unwrap_or(path);
        meta.set_name(name);
        Ok(meta)
    }

    // ── Write API — directory manipulation ───────────────────────────────

    /// Create a new file. Returns an open handle positioned at byte 0.
    pub unsafe fn create<'s>(&'s self, dir: &str, name: &str) -> Result<FatFile<'s, D>> {
        let dir_info = self.resolve(dir)?;
        if !dir_info.is_dir { return Err(FsError::WrongType); }
        // Allocate first cluster (empty file, single cluster allocated immediately)
        let new_clus = self.alloc_cluster()?;
        // Zero the cluster so reads of unwritten data are clean
        self.zero_cluster(new_clus)?;
        // Write directory entry
        if self.geom.kind == FatKind::ExFat {
            self.write_exfat_entry(dir_info.cluster, name, new_clus, 0, false)?;
        } else {
            self.write_fat32_entry(dir_info.cluster, name, new_clus, 0, 0x20)?;
        }
        Ok(FatFile { vol: self, start: new_clus, cur: new_clus,
                     size: 0, pos: 0, clus_pos: 0 })
    }

    /// Create a directory. Writes "." and ".." entries into the new cluster.
    pub unsafe fn mkdir(&self, path: &str) -> Result<()> {
        let (dir, name) = split_parent(path)?;
        let dir_info = self.resolve(dir)?;
        if !dir_info.is_dir { return Err(FsError::WrongType); }

        let new_clus = self.alloc_cluster()?;
        self.zero_cluster(new_clus)?;

        // Write . and .. entries (FAT32 only; ExFAT dirs don't need them)
        if self.geom.kind == FatKind::Fat32 {
            let lba = self.geom.cluster_lba(new_clus);
            let mut sec = [0u8; 512];
            self.read_sec(lba, &mut sec)?;
            // .
            write_sfn_entry(&mut sec[0..32], b".          ", new_clus, 0, 0x10);
            // ..
            write_sfn_entry(&mut sec[32..64], b"..         ", dir_info.cluster, 0, 0x10);
            self.write_sec(lba, &sec)?;
        }

        if self.geom.kind == FatKind::ExFat {
            self.write_exfat_entry(dir_info.cluster, name, new_clus, 0, true)?;
        } else {
            self.write_fat32_entry(dir_info.cluster, name, new_clus, 0, 0x10)?;
        }
        Ok(())
    }

    /// Delete a file. Frees its cluster chain and marks its directory entry deleted.
    pub unsafe fn unlink(&self, path: &str) -> Result<()> {
        let (dir, name) = split_parent(path)?;
        let dir_info = self.resolve(dir)?;
        if !dir_info.is_dir { return Err(FsError::WrongType); }
        self.delete_entry(dir_info.cluster, name, false)
    }

    /// Remove an empty directory.
    pub unsafe fn rmdir(&self, path: &str) -> Result<()> {
        let (dir, name) = split_parent(path)?;
        let dir_info = self.resolve(dir)?;
        if !dir_info.is_dir { return Err(FsError::WrongType); }
        // Check that target is empty before deleting
        let target = self.find_in(dir_info.cluster, name)?;
        if !target.is_dir { return Err(FsError::WrongType); }
        // Verify empty (only . and .. allowed for FAT32)
        let mut count = 0u32;
        self.walk_entries(target.cluster, |e, _, _| {
            let n = &e.name[..e.name_len];
            if n != b"." && n != b".." { count += 1; }
            count == 0
        })?;
        if count > 0 { return Err(FsError::NotEmpty); }
        self.delete_entry(dir_info.cluster, name, true)
    }

    // ── Internal write helpers ────────────────────────────────────────────

    unsafe fn zero_cluster(&self, clus: u32) -> Result<()> {
        let lba = self.geom.cluster_lba(clus);
        let sec = [0u8; 512];
        for s in 0..self.geom.secs_per_clus as u64 {
            self.write_sec(lba + s, &sec)?;
        }
        Ok(())
    }

    /// Find or extend a directory to fit one new 32-byte slot (FAT32).
    /// Returns (lba, slot_offset) where the new entry should be written.
    unsafe fn alloc_dir_slot_fat32(&self, dir_clus: u32) -> Result<(u64, usize)> {
        let bps = self.geom.bytes_per_sec as usize;
        let spc = self.geom.secs_per_clus as usize;
        let mut prev_clus = dir_clus;
        let mut clus = dir_clus;
        while clus >= 2 && clus < 0x0FFF_FFF8 {
            let clba = self.geom.cluster_lba(clus);
            for s in 0..spc {
                let lba = clba + s as u64;
                let mut sec = [0u8; 512];
                self.read_sec(lba, &mut sec)?;
                for slot in 0..(bps / 32) {
                    let first = sec[slot * 32];
                    if first == 0x00 || first == 0xE5 {
                        return Ok((lba, slot));
                    }
                }
            }
            prev_clus = clus;
            clus = self.fat_next(clus)?;
        }
        // Directory full — extend with a new cluster
        let new_clus = self.alloc_cluster()?;
        self.fat_set(prev_clus, new_clus)?;
        self.zero_cluster(new_clus)?;
        Ok((self.geom.cluster_lba(new_clus), 0))
    }

    /// Write a FAT32 SFN (+ optional LFN chain) directory entry for `name`.
    unsafe fn write_fat32_entry(&self, dir_clus: u32, name: &str,
                                  cluster: u32, size: u32, attr: u8) -> Result<()>
    {
        let (sfn, needs_lfn) = make_sfn(name);
        let lfn_name = name.as_bytes();

        // How many LFN entries?
        let lfn_count = if needs_lfn {
            (ucs2_len(lfn_name) + 12) / 13
        } else { 0 };

        // We need lfn_count + 1 contiguous slots.
        // For simplicity, scan for a contiguous run of lfn_count+1 free slots.
        // (This is O(N) but avoids fragmented LFN chains.)
        let total_needed = lfn_count + 1;
        let bps = self.geom.bytes_per_sec as usize;
        let spc = self.geom.secs_per_clus as usize;
        let slots_per_sec = bps / 32;
        let mut run_start_lba  = 0u64;
        let mut run_start_slot = 0usize;
        let mut run_len        = 0usize;
        let mut found          = false;
        let mut prev_clus      = dir_clus;
        let mut clus           = dir_clus;

        'search: while clus >= 2 && clus < 0x0FFF_FFF8 {
            let clba = self.geom.cluster_lba(clus);
            for s in 0..spc {
                let lba = clba + s as u64;
                let mut sec = [0u8; 512];
                self.read_sec(lba, &mut sec)?;
                for slot in 0..slots_per_sec {
                    let first = sec[slot * 32];
                    if first == 0x00 || first == 0xE5 {
                        if run_len == 0 { run_start_lba = lba; run_start_slot = slot; }
                        run_len += 1;
                        if run_len >= total_needed { found = true; break 'search; }
                    } else {
                        run_len = 0;
                    }
                }
            }
            prev_clus = clus;
            clus = self.fat_next(clus)?;
        }

        if !found {
            let new_clus = self.alloc_cluster()?;
            self.fat_set(prev_clus, new_clus)?;
            self.zero_cluster(new_clus)?;
            run_start_lba  = self.geom.cluster_lba(new_clus);
            run_start_slot = 0;
        }

        let csum = sfn_checksum(&sfn);

        // Write LFN entries from highest seq to lowest
        if lfn_count > 0 {
            // Build UCS-2 name (up to 260 chars)
            let mut ucs2  = [0u16; 260];
            let ucs2_len  = utf8_to_ucs2(lfn_name, &mut ucs2);
            // Pad to multiple of 13 with 0xFFFF
            for i in ucs2_len..((ucs2_len + 12) / 13 * 13) {
                ucs2[i] = if i == ucs2_len { 0x0000 } else { 0xFFFF };
            }

            for seq in (1..=lfn_count).rev() {
                let part = seq - 1;
                let slot_off = (lfn_count - seq) + (if lfn_count == 0 { 0 } else { 0 });
                let (lfn_lba, lfn_slot) = advance_dir_pos(
                    run_start_lba, run_start_slot + (lfn_count - seq),
                    slots_per_sec, &self.geom)?;
                let mut sec = [0u8; 512];
                self.read_sec(lfn_lba, &mut sec)?;
                let e = &mut sec[lfn_slot * 32..(lfn_slot+1) * 32];
                e.fill(0);
                e[0]  = if seq == lfn_count { 0x40 | seq as u8 } else { seq as u8 };
                e[11] = 0x0F;
                e[12] = 0x00;
                e[13] = csum;
                e[26] = 0; e[27] = 0;
                // Copy 13 UCS-2 chars
                let chars = &ucs2[part*13..(part+1)*13.min(ucs2_len + 1)];
                let offsets = [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
                for (i, &ch) in chars.iter().enumerate() {
                    let o = offsets[i];
                    e[o]   = ch as u8;
                    e[o+1] = (ch >> 8) as u8;
                }
                self.write_sec(lfn_lba, &sec)?;
                let _ = slot_off;
            }
        }

        // Write SFN entry
        let (sfn_lba, sfn_slot) = advance_dir_pos(
            run_start_lba, run_start_slot + lfn_count, slots_per_sec, &self.geom)?;
        let mut sec = [0u8; 512];
        self.read_sec(sfn_lba, &mut sec)?;
        write_sfn_entry(&mut sec[sfn_slot*32..(sfn_slot+1)*32], &sfn, cluster, size, attr);
        self.write_sec(sfn_lba, &sec)?;
        Ok(())
    }

    /// Write an ExFAT entry set (primary 0x85 + stream 0xC0 + filename 0xC1…).
    unsafe fn write_exfat_entry(&self, dir_clus: u32, name: &str,
                                  cluster: u32, size: u64, is_dir: bool) -> Result<()>
    {
        // Convert name to UCS-2
        let mut ucs2    = [0u16; 255];
        let name_chars  = utf8_to_ucs2(name.as_bytes(), &mut ucs2);
        if name_chars == 0 { return Err(FsError::InvalidArg); }
        let fn_entries  = (name_chars + 14) / 15;
        let sec_count   = 1 + fn_entries; // stream + filename entries
        let total_ents  = 1 + sec_count;  // primary + secondary

        // Build the entry set in a local buffer (up to 1 + 1 + 17 = 19 entries)
        let mut entries = [[0u8; 32]; 19];

        // Primary: File Entry (0x85)
        entries[0][0] = 0x85;
        entries[0][1] = sec_count as u8;
        let attr: u16 = if is_dir { 0x16 } else { 0x20 }; // dir = Archive|Dir, file = Archive
        entries[0][4..6].copy_from_slice(&attr.to_le_bytes());

        // Stream Extension (0xC0)
        entries[1][0] = 0xC0;
        entries[1][1] = 0x01; // AllocationPossible | NoFatChain cleared
        entries[1][3] = name_chars as u8;
        let nhash = exfat_name_hash(&ucs2[..name_chars]);
        entries[1][4..6].copy_from_slice(&nhash.to_le_bytes());
        entries[1][8..16].copy_from_slice(&size.to_le_bytes());   // ValidDataLength
        entries[1][20..24].copy_from_slice(&cluster.to_le_bytes());
        entries[1][24..32].copy_from_slice(&size.to_le_bytes());  // DataLength

        // File Name Extensions (0xC1)
        for i in 0..fn_entries {
            let e = &mut entries[2 + i];
            e[0] = 0xC1;
            e[1] = 0x01;
            let start = i * 15;
            let end   = (start + 15).min(name_chars);
            for j in start..end {
                let o = 2 + (j - start) * 2;
                e[o]   = ucs2[j] as u8;
                e[o+1] = (ucs2[j] >> 8) as u8;
            }
        }

        // Compute set checksum and store in entries[0][2..4]
        let csum = exfat_set_checksum(&entries[..total_ents]);
        entries[0][2..4].copy_from_slice(&csum.to_le_bytes());

        // Find space for `total_ents` contiguous slots in the directory
        let (start_lba, start_slot) = self.alloc_exfat_dir_slots(dir_clus, total_ents)?;
        let slots_per_sec = self.geom.bytes_per_sec as usize / 32;
        for i in 0..total_ents {
            let (lba, slot) = advance_dir_pos(start_lba, start_slot + i, slots_per_sec, &self.geom)?;
            let mut sec = [0u8; 512];
            self.read_sec(lba, &mut sec)?;
            sec[slot*32..(slot+1)*32].copy_from_slice(&entries[i]);
            self.write_sec(lba, &sec)?;
        }
        Ok(())
    }

    unsafe fn alloc_exfat_dir_slots(&self, dir_clus: u32, count: usize) -> Result<(u64, usize)> {
        let bps = self.geom.bytes_per_sec as usize;
        let spc = self.geom.secs_per_clus as usize;
        let slots_per_sec = bps / 32;
        let mut run_start_lba = 0u64;
        let mut run_start_slot = 0usize;
        let mut run_len = 0usize;
        let mut prev_clus = dir_clus;
        let mut clus = dir_clus;

        'search: while clus >= 2 && clus < 0x0FFF_FFF8 {
            let clba = self.geom.cluster_lba(clus);
            for s in 0..spc {
                let lba = clba + s as u64;
                let mut sec = [0u8; 512];
                self.read_sec(lba, &mut sec)?;
                for slot in 0..slots_per_sec {
                    let etype = sec[slot * 32];
                    if etype & 0x80 == 0 {  // not in-use
                        if run_len == 0 { run_start_lba = lba; run_start_slot = slot; }
                        run_len += 1;
                        if run_len >= count { break 'search; }
                    } else {
                        run_len = 0;
                    }
                }
            }
            prev_clus = clus;
            clus = self.fat_next(clus)?;
        }

        if run_len < count {
            let new_clus = self.alloc_cluster()?;
            self.fat_set(prev_clus, new_clus)?;
            self.zero_cluster(new_clus)?;
            return Ok((self.geom.cluster_lba(new_clus), 0));
        }
        Ok((run_start_lba, run_start_slot))
    }

    /// Mark all entries (LFN chain + SFN, or ExFAT entry set) as deleted.
    unsafe fn delete_entry(&self, dir_clus: u32, name: &str, is_dir: bool) -> Result<()> {
        let target = name.as_bytes();
        let bps    = self.geom.bytes_per_sec as usize;
        let spc    = self.geom.secs_per_clus as usize;

        // Find the entry and record positions
        let mut del_start: Option<EntryPos> = None;
        let mut del_end:   Option<EntryPos> = None;
        let mut del_clus:  u32 = 0;
        let mut found = false;

        self.walk_entries(dir_clus, |info, lfn_start, sfn_pos| {
            if crate::name_eq_ci(&info.name[..info.name_len], target) {
                if is_dir == info.is_dir || (!is_dir && !info.is_dir) {
                    del_start = Some(*lfn_start);
                    del_end   = Some(*sfn_pos);
                    del_clus  = info.cluster;
                    found = true;
                }
                false
            } else { true }
        })?;

        if !found { return Err(FsError::NotFound); }
        let start = del_start.unwrap();
        let end   = del_end.unwrap();

        // Walk from start to end (inclusive), stamping 0xE5 on each entry
        let mut cur = start;
        loop {
            let lba = self.geom.cluster_lba(cur.clus) + cur.sec as u64;
            let mut sec = [0u8; 512];
            self.read_sec(lba, &mut sec)?;
            let off = cur.slot * 32;
            if self.geom.kind == FatKind::ExFat {
                sec[off] &= 0x7F; // clear in-use bit
            } else {
                sec[off] = 0xE5;
            }
            self.write_sec(lba, &sec)?;

            if cur.clus == end.clus && cur.sec == end.sec && cur.slot == end.slot { break; }

            // Advance
            cur.slot += 1;
            if cur.slot * 32 >= bps {
                cur.slot = 0;
                cur.sec += 1;
                if cur.sec >= spc {
                    cur.sec = 0;
                    cur.clus = self.fat_next(cur.clus)?;
                }
            }
        }

        // Free the cluster chain
        if del_clus >= 2 {
            self.free_chain(del_clus)?;
        }
        Ok(())
    }

    // ── Raw read/write (called from VfsFile) ──────────────────────────────

    pub unsafe fn raw_read(
        &self,
        cur: &mut u32, pos: &mut u64, clus_pos: &mut u64,
        size: u64, buf: &mut [u8],
    ) -> Result<usize> {
        if *pos >= size { return Ok(0); }
        let bps  = self.geom.bytes_per_sec as usize;
        let spc  = self.geom.secs_per_clus as usize;
        let bpc  = bps * spc;
        let want = ((size - *pos) as usize).min(buf.len());
        let mut done = 0usize;
        let mut tmp  = [0u8; 512];

        while done < want && *cur >= 2 && *cur < 0x0FFF_FFF8 {
            let clba  = self.geom.cluster_lba(*cur);
            let off   = *clus_pos as usize;
            let avail = bpc - off;
            let take  = (want - done).min(avail);
            let s_s   = off / bps;
            let s_e   = (off + take + bps - 1) / bps;
            let mut copied = 0usize;
            for s in s_s..s_e {
                self.read_sec(clba + s as u64, &mut tmp)?;
                let s_off  = if s == s_s { off % bps } else { 0 };
                let s_take = (bps - s_off).min(take - copied);
                buf[done+copied..done+copied+s_take]
                    .copy_from_slice(&tmp[s_off..s_off+s_take]);
                copied += s_take;
            }
            done += take;
            *pos      += take as u64;
            *clus_pos += take as u64;
            if *clus_pos as usize >= bpc {
                *clus_pos = 0;
                *cur = self.fat_next(*cur)?;
            }
        }
        Ok(done)
    }

    pub unsafe fn raw_write(
        &self,
        start: u32,
        cur: &mut u32, pos: &mut u64, clus_pos: &mut u64,
        size: &mut u64, buf: &[u8],
    ) -> Result<usize> {
        let bps = self.geom.bytes_per_sec as usize;
        let spc = self.geom.secs_per_clus as usize;
        let bpc = bps * spc;
        let mut done = 0usize;
        let mut tmp  = [0u8; 512];

        while done < buf.len() {
            // If we've exhausted the current cluster, allocate another
            if *clus_pos as usize >= bpc || *cur < 2 || *cur >= 0x0FFF_FFF8 {
                let new_clus = self.alloc_cluster()?;
                // Find the last cluster in the chain and link it
                if *cur < 2 || *cur >= 0x0FFF_FFF8 {
                    // This shouldn't happen for a properly created file,
                    // but handle gracefully
                    let _ = start;
                } else {
                    self.fat_set(*cur, new_clus)?;
                }
                *cur = new_clus;
                *clus_pos = 0;
            }

            let clba  = self.geom.cluster_lba(*cur);
            let off   = *clus_pos as usize;
            let avail = bpc - off;
            let take  = (buf.len() - done).min(avail);

            let s_s = off / bps;
            let s_e = (off + take + bps - 1) / bps;
            let mut copied = 0usize;

            for s in s_s..s_e {
                let lba    = clba + s as u64;
                let s_off  = if s == s_s { off % bps } else { 0 };
                let s_take = (bps - s_off).min(take - copied);

                if s_off != 0 || s_take < bps {
                    // Partial sector write: read-modify-write
                    self.read_sec(lba, &mut tmp)?;
                } else {
                    tmp = [0u8; 512];
                }
                tmp[s_off..s_off+s_take].copy_from_slice(&buf[done+copied..done+copied+s_take]);
                self.write_sec(lba, &tmp)?;
                copied += s_take;
            }

            done      += take;
            *pos      += take as u64;
            *clus_pos += take as u64;
            if *pos > *size { *size = *pos; }

            if *clus_pos as usize >= bpc {
                // Move to next cluster (or EOC — next write will allocate)
                let next = self.fat_next(*cur)?;
                if next >= 0x0FFF_FFF8 {
                    // At EOC; leave cur pointing to this cluster for next alloc
                } else {
                    *cur = next;
                    *clus_pos = 0;
                }
            }
        }
        Ok(done)
    }

    pub unsafe fn raw_seek(
        &self,
        start: u32, cur: &mut u32, pos: &mut u64, clus_pos: &mut u64,
        size: u64, target: u64,
    ) -> Result<()> {
        if target > size { return Err(FsError::InvalidArg); }
        if target < *pos { *cur = start; *pos = 0; *clus_pos = 0; }
        let bpc = self.geom.bytes_per_cluster() as u64;
        while *pos < target {
            let rem = bpc - *clus_pos;
            let need = target - *pos;
            if need < rem {
                *clus_pos += need; *pos += need;
            } else {
                *pos += rem; *clus_pos = 0;
                *cur = self.fat_next(*cur)?;
            }
        }
        Ok(())
    }

    /// Update the on-disk size field for an open file.
    /// Should be called after write when the file handle is closed/flushed.
    pub unsafe fn update_size(&self, dir: &str, name: &str, new_size: u32) -> Result<()> {
        let dir_info = self.resolve(dir)?;
        let target   = name.as_bytes();
        let bps = self.geom.bytes_per_sec as usize;
        let spc = self.geom.secs_per_clus as usize;
        let mut clus = dir_info.cluster;
        while clus >= 2 && clus < 0x0FFF_FFF8 {
            let clba = self.geom.cluster_lba(clus);
            for s in 0..spc {
                let lba = clba + s as u64;
                let mut sec = [0u8; 512];
                self.read_sec(lba, &mut sec)?;
                for slot in 0..(bps / 32) {
                    let off = slot * 32;
                    let e = &sec[off..off+32];
                    if e[0] == 0x00 { return Err(FsError::NotFound); }
                    if e[0] == 0xE5 || e[11] == 0x0F { continue; }
                    let mut ename = [0u8; 12];
                    let n = assemble_sfn(e.try_into().unwrap(), &mut ename);
                    if crate::name_eq_ci(&ename[..n], target) {
                        sec[off+28..off+32].copy_from_slice(&new_size.to_le_bytes());
                        self.write_sec(lba, &sec)?;
                        return Ok(());
                    }
                }
            }
            clus = self.fat_next(clus)?;
        }
        Err(FsError::NotFound)
    }
}

// ─── FatFile ──────────────────────────────────────────────────────────────────

pub struct FatFile<'a, D: BlockDev> {
    vol:      &'a FatVolume<D>,
    pub start:    u32,
    cur:      u32,
    pub size:     u64,
    pos:      u64,
    clus_pos: u64,
}

impl<'a, D: BlockDev> FatFile<'a, D> {
    pub fn size(&self) -> u64 { self.size }
    pub fn pos(&self)  -> u64 { self.pos  }

    pub unsafe fn seek(&mut self, target: u64) -> Result<()> {
        self.vol.raw_seek(self.start, &mut self.cur, &mut self.pos,
                          &mut self.clus_pos, self.size, target)
    }

    pub unsafe fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.vol.raw_read(&mut self.cur, &mut self.pos, &mut self.clus_pos,
                          self.size, buf)
    }

    pub unsafe fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.vol.raw_write(self.start, &mut self.cur, &mut self.pos,
                           &mut self.clus_pos, &mut self.size, buf)
    }
}

// ─── Helper functions ─────────────────────────────────────────────────────────

/// Encode one UCS-2 code point as UTF-8. Returns number of bytes written.
fn ucs2_to_utf8(cp: u16, out: &mut [u8]) -> usize {
    match cp {
        0x0001..=0x007F if out.len() >= 1 => { out[0] = cp as u8; 1 }
        0x0080..=0x07FF if out.len() >= 2 => {
            out[0] = 0xC0 | (cp >> 6) as u8;
            out[1] = 0x80 | (cp & 0x3F) as u8; 2
        }
        _ if out.len() >= 3 => {
            out[0] = 0xE0 | (cp >> 12) as u8;
            out[1] = 0x80 | ((cp >> 6) & 0x3F) as u8;
            out[2] = 0x80 | (cp & 0x3F) as u8; 3
        }
        _ => 0,
    }
}

/// Count UCS-2 chars that a UTF-8 string encodes to (conservative estimate).
fn ucs2_len(utf8: &[u8]) -> usize {
    utf8.iter().filter(|&&b| b & 0xC0 != 0x80).count()
}

/// Convert UTF-8 to UCS-2 (BMP only). Returns number of UCS-2 chars written.
fn utf8_to_ucs2(src: &[u8], dst: &mut [u16]) -> usize {
    let mut i = 0usize;
    let mut out = 0usize;
    while i < src.len() && out < dst.len() {
        let b = src[i];
        if b & 0x80 == 0 {
            dst[out] = b as u16; out += 1; i += 1;
        } else if b & 0xE0 == 0xC0 && i + 1 < src.len() {
            dst[out] = ((b as u16 & 0x1F) << 6) | (src[i+1] as u16 & 0x3F);
            out += 1; i += 2;
        } else if b & 0xF0 == 0xE0 && i + 2 < src.len() {
            dst[out] = ((b as u16 & 0x0F) << 12)
                     | ((src[i+1] as u16 & 0x3F) << 6)
                     | (src[i+2] as u16 & 0x3F);
            out += 1; i += 3;
        } else { i += 1; } // skip invalid / non-BMP
    }
    out
}

fn extract_lfn_chars(e: &[u8; 32]) -> [u16; 13] {
    let mut out = [0u16; 13];
    let segs: [(usize, usize); 3] = [(1,5),(14,6),(28,2)];
    let mut idx = 0;
    for (off, cnt) in segs {
        for j in 0..cnt {
            out[idx] = u16::from_le_bytes([e[off+j*2], e[off+j*2+1]]);
            idx += 1;
        }
    }
    out
}

fn assemble_lfn(parts: &[[u16; 13]], count: usize, out: &mut [u8; 256]) -> usize {
    let mut n = 0usize;
    'outer: for i in 0..count {
        for &cp in &parts[i] {
            if cp == 0x0000 || cp == 0xFFFF { break 'outer; }
            n += ucs2_to_utf8(cp, &mut out[n..]);
            if n >= 252 { break 'outer; }
        }
    }
    out[n] = 0;
    n
}

fn assemble_sfn(e: &[u8; 32], out: &mut [u8; 256]) -> usize {
    let name = trim_spaces(&e[0..8]);
    let ext  = trim_spaces(&e[8..11]);
    let mut n = 0usize;
    for &b in name {
        out[n] = if b >= 0x61 && b <= 0x7A { b - 0x20 } else { b };
        n += 1;
    }
    if !ext.is_empty() {
        out[n] = b'.'; n += 1;
        for &b in ext {
            out[n] = if b >= 0x61 && b <= 0x7A { b - 0x20 } else { b };
            n += 1;
        }
    }
    out[n] = 0;
    n
}

fn trim_spaces(b: &[u8]) -> &[u8] {
    let end = b.iter().rposition(|&x| x != b' ').map(|i|i+1).unwrap_or(0);
    &b[..end]
}

fn sfn_checksum(sfn: &[u8]) -> u8 {
    sfn.iter().fold(0u8, |s, &b| s.rotate_right(1).wrapping_add(b))
}

fn fat32_entry_cluster(e: &[u8; 32]) -> u32 {
    let hi = u16::from_le_bytes([e[20], e[21]]) as u32;
    let lo = u16::from_le_bytes([e[26], e[27]]) as u32;
    (hi << 16) | lo
}

/// Generate an 8.3 SFN from a long name. Returns (sfn_bytes, needs_lfn).
fn make_sfn(name: &str) -> ([u8; 11], bool) {
    let mut sfn = [b' '; 11usize];
    let mut needs_lfn = false;
    let bytes = name.as_bytes();

    // Find extension
    let dot = bytes.iter().rposition(|&b| b == b'.');
    let (base_b, ext_b) = match dot {
        Some(d) if d > 0 => (&bytes[..d], &bytes[d+1..]),
        _                => (bytes, &b""[..]),
    };

    // Encode base (up to 8 chars)
    let mut bi = 0usize;
    for &b in base_b {
        if bi >= 8 { needs_lfn = true; break; }
        let up = b.to_ascii_uppercase();
        if b != up { needs_lfn = true; }
        sfn[bi] = up; bi += 1;
    }
    // Encode extension (up to 3 chars)
    let mut ei = 0usize;
    for &b in ext_b {
        if ei >= 3 { needs_lfn = true; break; }
        let up = b.to_ascii_uppercase();
        if b != up { needs_lfn = true; }
        sfn[8 + ei] = up; ei += 1;
    }

    // If LFN is needed but base name happened to fit, add numeric tail
    if needs_lfn {
        if bi < 7 { sfn[bi] = b'~'; sfn[bi+1] = b'1'; }
        else      { sfn[7]  = b'1'; }
    }

    (sfn, needs_lfn)
}

fn write_sfn_entry(e: &mut [u8], sfn: &[u8; 11], cluster: u32, size: u32, attr: u8) {
    e.fill(0);
    e[0..11].copy_from_slice(sfn);
    e[11] = attr;
    e[20] = (cluster >> 16) as u8;
    e[21] = (cluster >> 24) as u8;
    e[26] = cluster as u8;
    e[27] = (cluster >> 8) as u8;
    e[28..32].copy_from_slice(&size.to_le_bytes());
}

/// Advance a (lba, slot) position by `steps` in a FAT32 directory cluster chain.
unsafe fn advance_dir_pos(
    lba: u64, slot: usize, slots_per_sec: usize, _geom: &FatGeom,
) -> Result<(u64, usize)> {
    // All slots are within the same flat range starting from lba:slot.
    // Since we pre-allocated a run that fits, we can compute directly.
    let sec_advance = slot / slots_per_sec;
    let slot_in_sec = slot % slots_per_sec;
    // lba + sec_advance might cross a cluster boundary, but since the
    // run was found to be contiguous within the directory, it's fine.
    Ok((lba + sec_advance as u64, slot_in_sec))
}

fn split_parent(path: &str) -> Result<(&str, &str)> {
    let path = path.trim_end_matches('/');
    match path.rfind('/') {
        Some(i) => Ok((&path[..i], &path[i+1..])),
        None    => Ok(("/", path)),
    }
}

/// Convert FAT date+time words to a Unix timestamp (approximate, ignores seconds > 29).
fn fat_to_unix(date: u16, time: u16) -> u32 {
    let year  = ((date >> 9) & 0x7F) as u32 + 1980;
    let month = ((date >> 5) & 0x0F) as u32;
    let day   = (date & 0x1F) as u32;
    let hour  = ((time >> 11) & 0x1F) as u32;
    let min   = ((time >> 5)  & 0x3F) as u32;
    let sec   = (time & 0x1F) as u32 * 2;

    if month == 0 || day == 0 { return 0; }

    const DAYS_IN_MONTH: [u32; 12] = [31,28,31,30,31,30,31,31,30,31,30,31];
    let mut days = 0u32;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for m in 1..month {
        days += DAYS_IN_MONTH[(m-1) as usize];
        if m == 2 && is_leap(year) { days += 1; }
    }
    days += day - 1;
    days * 86400 + hour * 3600 + min * 60 + sec
}

fn is_leap(y: u32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Decode a 4-byte ExFAT timestamp into a Unix timestamp.
fn exfat_timestamp(ts: &[u8]) -> u32 {
    if ts.len() < 4 { return 0; }
    let v = u32::from_le_bytes([ts[0],ts[1],ts[2],ts[3]]);
    let sec   = (v & 0x1F) * 2;
    let min   = (v >>  5) & 0x3F;
    let hour  = (v >> 11) & 0x1F;
    let day   = (v >> 16) & 0x1F;
    let month = (v >> 21) & 0x0F;
    let year  = ((v >> 25) & 0x7F) + 1980;
    fat_to_unix(
        ((year - 1980) << 9 | month << 5 | day) as u16,
        (hour << 11 | min << 5 | sec / 2) as u16,
    )
}

/// ExFAT name hash (ASCII-safe; non-ASCII names get approximate hash).
fn exfat_name_hash(name: &[u16]) -> u16 {
    let mut h = 0u16;
    for &cp in name {
        let up = if cp >= b'a' as u16 && cp <= b'z' as u16 {
            cp - (b'a' as u16 - b'A' as u16)
        } else { cp };
        h = h.rotate_right(1).wrapping_add(up & 0xFF);
        h = h.rotate_right(1).wrapping_add(up >> 8);
    }
    h
}

/// ExFAT directory entry set checksum (skip bytes 2-3 of primary entry).
fn exfat_set_checksum(entries: &[[u8; 32]]) -> u16 {
    let mut sum = 0u16;
    for (i, entry) in entries.iter().enumerate() {
        for (j, &b) in entry.iter().enumerate() {
            if i == 0 && (j == 2 || j == 3) { continue; }
            sum = sum.rotate_right(1).wrapping_add(b as u16);
        }
    }
    sum
}
