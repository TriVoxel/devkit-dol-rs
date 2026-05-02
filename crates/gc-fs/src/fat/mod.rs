//! FAT12 / FAT16 / FAT32 / ExFAT filesystem driver (read + write).
//!
//! Supports all FAT variants. ExFAT supports read; write is FAT32 only for now.
//!
//! ## On-disk layout
//!
//! ```text
//! LBA 0          Boot sector (BPB)
//! LBA 1..R-1     Reserved sectors
//! LBA R          FAT #0 (F sectors)
//! LBA R+F        FAT #1 (mirror)
//! LBA R+F*N      Root dir (FAT12/16 only; fixed size)
//! LBA data       Cluster 2, 3, …
//! ```
//!
//! All multi-byte integers are little-endian.

use crate::{BlockDev, FsError, Metadata, Result, path_split, name_eq_ci};

// ─────────────────────────────────────────────────────────────────────────────
// BPB parsing
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FatKind { Fat12, Fat16, Fat32, ExFat }

#[derive(Clone, Copy)]
pub struct FatGeom {
    pub kind:            FatKind,
    pub bytes_per_sec:   u32,
    pub secs_per_clus:   u32,
    pub fat_lba:         u64,   // LBA of FAT #0
    pub data_lba:        u64,   // LBA of first data cluster (cluster 2)
    pub root_lba:        u64,   // FAT12/16 fixed root dir LBA (0 for FAT32)
    pub root_entries:    u32,   // FAT12/16 root dir entries (0 for FAT32)
    pub root_cluster:    u32,   // FAT32 root dir cluster (0 for FAT12/16)
    pub fat_size_secs:   u32,
    pub fat_count:       u8,
    pub total_clusters:  u32,
}

impl FatGeom {
    pub fn parse(s: &[u8; 512]) -> Result<Self> {
        if &s[3..11] == b"EXFAT   " { return Self::parse_exfat(s); }

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

        let fat_sz = if fs16 > 0 { fs16 } else { fs32 };
        let tot    = if ts16 > 0 { ts16 } else { ts32 };

        let fat_lba   = rsvd as u64;
        let rde_secs  = (rde * 32 + bps - 1) / bps;
        let root_lba  = fat_lba + (nfat as u64) * fat_sz as u64;
        let data_lba  = root_lba + rde_secs as u64;
        let data_secs = tot.saturating_sub(data_lba as u32);
        let clusters  = data_secs / spc;

        let kind = if clusters < 4085 { FatKind::Fat12 }
                   else if clusters < 65525 { FatKind::Fat16 }
                   else { FatKind::Fat32 };

        Ok(FatGeom {
            kind, bytes_per_sec: bps, secs_per_clus: spc,
            fat_lba,
            data_lba,
            root_lba:     if kind == FatKind::Fat32 { 0 } else { root_lba },
            root_entries: if kind == FatKind::Fat32 { 0 } else { rde },
            root_cluster: if kind == FatKind::Fat32 { rc } else { 0 },
            fat_size_secs: fat_sz,
            fat_count: nfat,
            total_clusters: clusters,
        })
    }

    fn parse_exfat(s: &[u8; 512]) -> Result<Self> {
        // ExFAT BPB (Microsoft exFAT specification)
        let part_off  = u64::from_le_bytes(s[64..72].try_into().unwrap());
        let _ = part_off;
        let fat_off   = u32::from_le_bytes([s[80],  s[81],  s[82],  s[83]]);
        let fat_len   = u32::from_le_bytes([s[84],  s[85],  s[86],  s[87]]);
        let data_off  = u32::from_le_bytes([s[88],  s[89],  s[90],  s[91]]);
        let root_clus = u32::from_le_bytes([s[96],  s[97],  s[98],  s[99]]);
        let sec_shift = s[108];
        let spc_shift = s[109];
        let n_fats    = s[110];
        let bps       = 1u32 << sec_shift;
        let spc       = 1u32 << spc_shift;
        Ok(FatGeom {
            kind: FatKind::ExFat,
            bytes_per_sec: bps, secs_per_clus: spc,
            fat_lba:     fat_off as u64,
            data_lba:    data_off as u64,
            root_lba:    0,
            root_entries: 0,
            root_cluster: root_clus,
            fat_size_secs: fat_len,
            fat_count: n_fats,
            total_clusters: 0,
        })
    }

    pub fn cluster_lba(&self, c: u32) -> u64 {
        self.data_lba + (c as u64 - 2) * self.secs_per_clus as u64
    }

    pub fn bytes_per_cluster(&self) -> u32 {
        self.bytes_per_sec * self.secs_per_clus
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FAT volume
// ─────────────────────────────────────────────────────────────────────────────

pub struct FatVolume<D: BlockDev> {
    dev:  D,
    geom: FatGeom,
}

impl<D: BlockDev> FatVolume<D> {
    /// Mount from any block device. Reads boot sector and validates BPB.
    pub unsafe fn mount(dev: D) -> Result<Self> {
        let mut sec = [0u8; 512];
        dev.read_sector(0, &mut sec)?;
        if sec[510] != 0x55 || sec[511] != 0xAA {
            return Err(FsError::BadFormat);
        }
        let geom = FatGeom::parse(&sec)?;
        Ok(FatVolume { dev, geom })
    }

    pub fn kind(&self) -> FatKind { self.geom.kind }
    pub fn kind_str(&self) -> &'static str {
        match self.geom.kind {
            FatKind::Fat12 => "FAT12",
            FatKind::Fat16 => "FAT16",
            FatKind::Fat32 => "FAT32",
            FatKind::ExFat => "ExFAT",
        }
    }

    // ── Low-level I/O ─────────────────────────────────────────────────────

    unsafe fn read_sector(&self, lba: u64, buf: &mut [u8; 512]) -> Result<()> {
        self.dev.read_sector(lba, buf)
    }

    // ── FAT chain ─────────────────────────────────────────────────────────

    pub unsafe fn fat_next(&self, cluster: u32) -> Result<u32> {
        let bps = self.geom.bytes_per_sec as u64;
        let fat = self.geom.fat_lba;
        let mut sec = [0u8; 512];
        match self.geom.kind {
            FatKind::Fat12 => {
                let byte = cluster as u64 + cluster as u64 / 2;
                self.read_sector(fat + byte / bps, &mut sec)?;
                let lo = sec[(byte % bps) as usize];
                let hi = if (byte % bps) + 1 < bps {
                    sec[(byte % bps + 1) as usize]
                } else {
                    self.read_sector(fat + byte / bps + 1, &mut sec)?;
                    sec[0]
                };
                let val = u16::from_le_bytes([lo, hi]) as u32;
                let e   = if cluster & 1 != 0 { val >> 4 } else { val & 0xFFF };
                Ok(if e >= 0xFF8 { 0x0FFF_FFFF } else { e })
            }
            FatKind::Fat16 => {
                let byte = cluster as u64 * 2;
                self.read_sector(fat + byte / bps, &mut sec)?;
                let e = u16::from_le_bytes([sec[(byte%bps) as usize], sec[(byte%bps+1) as usize]]) as u32;
                Ok(if e >= 0xFFF8 { 0x0FFF_FFFF } else { e })
            }
            FatKind::Fat32 | FatKind::ExFat => {
                let byte = cluster as u64 * 4;
                self.read_sector(fat + byte / bps, &mut sec)?;
                let o = (byte % bps) as usize;
                let e = u32::from_le_bytes([sec[o],sec[o+1],sec[o+2],sec[o+3]]) & 0x0FFF_FFFF;
                Ok(if e >= 0x0FFF_FFF8 { 0x0FFF_FFFF } else { e })
            }
        }
    }

    pub unsafe fn fat_set(&self, cluster: u32, value: u32) -> Result<()> {
        let bps = self.geom.bytes_per_sec as u64;
        for copy in 0..self.geom.fat_count as u64 {
            let fat = self.geom.fat_lba + copy * self.geom.fat_size_secs as u64;
            match self.geom.kind {
                FatKind::Fat32 | FatKind::ExFat => {
                    let byte = cluster as u64 * 4;
                    let lba  = fat + byte / bps;
                    let off  = (byte % bps) as usize;
                    let mut sec = [0u8; 512];
                    self.dev.read_sector(lba, &mut sec)?;
                    let old = u32::from_le_bytes([sec[off],sec[off+1],sec[off+2],sec[off+3]]);
                    let new = (old & 0xF000_0000) | (value & 0x0FFF_FFFF);
                    sec[off..off+4].copy_from_slice(&new.to_le_bytes());
                    self.dev.write_sector(lba, &sec)?;
                }
                _ => return Err(FsError::Unsupported),
            }
        }
        Ok(())
    }

    // ── Directory walking ─────────────────────────────────────────────────

    /// Walk all 32-byte directory entries in a cluster chain (or fixed root).
    /// Callback returns false to stop early.
    unsafe fn walk_dir<F>(&self, start: u32, fixed_root: bool, mut f: F) -> Result<()>
    where F: FnMut(&[u8; 32]) -> bool
    {
        let bps = self.geom.bytes_per_sec as usize;
        let spc = self.geom.secs_per_clus as usize;
        let mut sec = [0u8; 512];

        if fixed_root {
            let secs = (self.geom.root_entries * 32 + bps as u32 - 1) / bps as u32;
            for s in 0..secs as u64 {
                self.read_sector(self.geom.root_lba + s, &mut sec)?;
                for e in 0..(bps / 32) {
                    let entry = &sec[e*32..e*32+32];
                    let arr: &[u8;32] = entry.try_into().unwrap();
                    if arr[0] == 0x00 { return Ok(()); }
                    if arr[0] == 0xE5 || arr[11] & 0x0F == 0x0F { continue; }
                    if !f(arr) { return Ok(()); }
                }
            }
            return Ok(());
        }

        let mut clus = start;
        while clus < 0x0FFF_FFF8 && clus >= 2 {
            let clba = self.geom.cluster_lba(clus);
            for s in 0..spc as u64 {
                self.read_sector(clba + s, &mut sec)?;
                for e in 0..(bps / 32) {
                    let entry = &sec[e*32..e*32+32];
                    let arr: &[u8;32] = entry.try_into().unwrap();
                    if arr[0] == 0x00 { return Ok(()); }
                    if arr[0] == 0xE5 || arr[11] & 0x0F == 0x0F { continue; }
                    if !f(arr) { return Ok(()); }
                }
            }
            clus = self.fat_next(clus)?;
        }
        Ok(())
    }

    // ── Name matching ─────────────────────────────────────────────────────

    fn entry_name_matches(entry: &[u8; 32], target: &[u8]) -> bool {
        let name = trim_spaces(&entry[0..8]);
        let ext  = trim_spaces(&entry[8..11]);
        // Try "NAME.EXT" and "NAME" variants
        let mut full = [0u8; 12];
        let mut n = 0;
        for &b in name { full[n] = b; n += 1; }
        if !ext.is_empty() {
            full[n] = b'.'; n += 1;
            for &b in ext { full[n] = b; n += 1; }
        }
        name_eq_ci(&full[..n], target)
    }

    fn entry_to_cluster(entry: &[u8; 32]) -> u32 {
        let hi = u16::from_le_bytes([entry[20], entry[21]]) as u32;
        let lo = u16::from_le_bytes([entry[26], entry[27]]) as u32;
        (hi << 16) | lo
    }

    fn entry_size(entry: &[u8; 32]) -> u64 {
        u32::from_le_bytes([entry[28], entry[29], entry[30], entry[31]]) as u64
    }

    fn entry_is_dir(entry: &[u8; 32]) -> bool { entry[11] & 0x10 != 0 }

    // ── Path resolution ───────────────────────────────────────────────────

    unsafe fn find_in(&self, dir_clus: u32, fixed: bool, path: &str)
        -> Result<(u32, u64, bool)>
    {
        let (first, rest) = path_split(path);
        if first.is_empty() { return Ok((dir_clus, 0, true)); }

        let target = first.as_bytes();
        let mut found: Option<(u32, u64, bool)> = None;

        self.walk_dir(dir_clus, fixed, |e| {
            if Self::entry_name_matches(e, target) {
                found = Some((
                    Self::entry_to_cluster(e),
                    Self::entry_size(e),
                    Self::entry_is_dir(e),
                ));
                false
            } else { true }
        })?;

        match found {
            Some((c, _, true))  if !rest.is_empty() => self.find_in(c, false, rest),
            Some((c, sz, d))    if rest.is_empty()  => Ok((c, sz, d)),
            Some(_)                                  => Err(FsError::WrongType),
            None                                     => Err(FsError::NotFound),
        }
    }

    unsafe fn resolve(&self, path: &str) -> Result<(u32, u64, bool)> {
        let (root, fixed) = match self.geom.kind {
            FatKind::Fat12 | FatKind::Fat16 => (0u32, true),
            _                               => (self.geom.root_cluster, false),
        };
        let (first, _) = path_split(path);
        if first.is_empty() { return Ok((root, 0, true)); }
        self.find_in(root, fixed, path)
    }

    // ── Public API ────────────────────────────────────────────────────────

    pub unsafe fn open<'s>(&'s self, path: &str) -> Result<FatFile<'s, D>> {
        let (c, sz, dir) = self.resolve(path)?;
        if dir { return Err(FsError::WrongType); }
        Ok(FatFile { vol: self, start: c, cur: c, size: sz, pos: 0, clus_pos: 0 })
    }

    pub unsafe fn read_dir<F>(&self, path: &str, mut cb: F) -> Result<()>
    where F: FnMut(&Metadata) -> bool
    {
        let (c, _, dir) = self.resolve(path)?;
        if !dir { return Err(FsError::WrongType); }
        let fixed = matches!(self.geom.kind, FatKind::Fat12|FatKind::Fat16) && c == 0;
        self.walk_dir(c, fixed, |e| {
            if e[0] == b'.' || e[11] & 0x08 != 0 { return true; } // skip . .. and volume label
            let mut meta = Metadata::zeroed();
            meta.is_dir  = Self::entry_is_dir(e);
            meta.readonly= e[11] & 0x01 != 0;
            meta.size    = Self::entry_size(e);
            // Build 8.3 name
            let name = trim_spaces(&e[0..8]);
            let ext  = trim_spaces(&e[8..11]);
            let mut n = 0;
            for &b in name { meta.name[n] = b; n += 1; }
            if !ext.is_empty() {
                meta.name[n] = b'.'; n += 1;
                for &b in ext { meta.name[n] = b; n += 1; }
            }
            !cb(&meta)
        })
    }

    pub unsafe fn stat(&self, path: &str) -> Result<Metadata> {
        let (_, sz, dir) = self.resolve(path)?;
        let mut meta = Metadata::zeroed();
        meta.is_dir = dir;
        meta.size   = sz;
        let name = path.rsplit('/').next().unwrap_or(path);
        meta.set_name(name);
        Ok(meta)
    }

    pub unsafe fn create<'s>(&'s mut self, _path: &str) -> Result<FatFile<'s, D>> {
        Err(FsError::Unsupported) // TODO
    }
}

fn trim_spaces(b: &[u8]) -> &[u8] {
    let end = b.iter().rposition(|&x| x != b' ').map(|i| i+1).unwrap_or(0);
    &b[..end]
}

// ─────────────────────────────────────────────────────────────────────────────
// Open file handle
// ─────────────────────────────────────────────────────────────────────────────

pub struct FatFile<'a, D: BlockDev> {
    vol:      &'a FatVolume<D>,
    start:    u32,
    cur:      u32,   // current cluster
    size:     u64,
    pos:      u64,
    clus_pos: u64,   // byte offset within current cluster
}

impl<'a, D: BlockDev> FatFile<'a, D> {
    pub fn size(&self) -> u64 { self.size }
    pub fn pos(&self)  -> u64 { self.pos  }

    pub unsafe fn seek(&mut self, target: u64) -> Result<()> {
        if target > self.size { return Err(FsError::InvalidArg); }
        if target < self.pos { // rewind
            self.cur = self.start; self.pos = 0; self.clus_pos = 0;
        }
        let bpc = self.vol.geom.bytes_per_cluster() as u64;
        while self.pos < target {
            let rem_in_clus = bpc - self.clus_pos;
            let need = target - self.pos;
            if need < rem_in_clus {
                self.clus_pos += need; self.pos += need;
            } else {
                self.pos += rem_in_clus; self.clus_pos = 0;
                self.cur = self.vol.fat_next(self.cur)?;
            }
        }
        Ok(())
    }

    pub unsafe fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.pos >= self.size { return Ok(0); }
        let want = ((self.size - self.pos) as usize).min(buf.len());
        let bps  = self.vol.geom.bytes_per_sec as usize;
        let spc  = self.vol.geom.secs_per_clus as usize;
        let bpc  = bps * spc;
        let mut done = 0usize;
        let mut tmp  = [0u8; 512];

        while done < want && self.cur < 0x0FFF_FFF8 {
            let clba    = self.vol.geom.cluster_lba(self.cur);
            let off     = self.clus_pos as usize;
            let avail   = bpc - off;
            let take    = (want - done).min(avail);

            let s_start = off / bps;
            let s_end   = (off + take + bps - 1) / bps;
            let mut copied = 0usize;

            for s in s_start..s_end {
                self.vol.dev.read_sector(clba + s as u64, &mut tmp)?;
                let s_off  = if s == s_start { off % bps } else { 0 };
                let s_take = (bps - s_off).min(take - copied);
                buf[done + copied..done + copied + s_take]
                    .copy_from_slice(&tmp[s_off..s_off + s_take]);
                copied += s_take;
            }

            done += take;
            self.pos      += take as u64;
            self.clus_pos += take as u64;
            if self.clus_pos as usize >= bpc {
                self.clus_pos = 0;
                self.cur = self.vol.fat_next(self.cur)?;
            }
        }
        Ok(done)
    }

    pub unsafe fn write(&mut self, _buf: &[u8]) -> Result<usize> {
        Err(FsError::Unsupported) // TODO: write support
    }
}
