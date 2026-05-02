//! EXT2 / EXT3 / EXT4 filesystem driver (read-only).
//!
//! EXT3 and EXT4 are backward-compatible supersets of EXT2 for read
//! operations — we can read all three using the same driver by ignoring
//! journal and extended-feature fields.
//!
//! ## Layout
//!
//! ```text
//! Byte 0–1023:   Boot block (unused, padding)
//! Byte 1024+:    Superblock (1024 bytes)
//! Block 2+:      Block group descriptor table
//! (per group):   Block bitmap, inode bitmap, inode table, data blocks
//! ```
//!
//! All multi-byte integers are little-endian.

use crate::{BlockDev, FsError, Metadata, Result, path_split, name_eq_ci};

// ─────────────────────────────────────────────────────────────────────────────
// Superblock (at byte offset 1024, 1024 bytes)
// ─────────────────────────────────────────────────────────────────────────────

const EXT2_MAGIC: u16  = 0xEF53;
const ROOT_INO:   u32  = 2;       // root directory inode number

struct Superblock {
    inodes_count:       u32,
    blocks_count:       u32,
    block_size:         u32,  // in bytes (1024 << log_block_size)
    blocks_per_group:   u32,
    inodes_per_group:   u32,
    inode_size:         u16,
    groups_count:       u32,
}

fn parse_superblock(buf: &[u8]) -> Result<Superblock> {
    if buf.len() < 1024 { return Err(FsError::BadFormat); }
    let s = &buf[..];
    let magic = u16::from_le_bytes([s[56], s[57]]);
    if magic != EXT2_MAGIC { return Err(FsError::BadFormat); }

    let inodes_count     = u32::from_le_bytes([s[0],  s[1],  s[2],  s[3]]);
    let blocks_count     = u32::from_le_bytes([s[4],  s[5],  s[6],  s[7]]);
    let log_block_size   = u32::from_le_bytes([s[24], s[25], s[26], s[27]]);
    let blocks_per_group = u32::from_le_bytes([s[32], s[33], s[34], s[35]]);
    let inodes_per_group = u32::from_le_bytes([s[40], s[41], s[42], s[43]]);
    let inode_size_raw   = u16::from_le_bytes([s[88], s[89]]);
    let block_size       = 1024u32 << log_block_size;

    // EXT2 rev 0 has fixed 128-byte inodes; rev 1+ has variable
    let rev_level = u32::from_le_bytes([s[76], s[77], s[78], s[79]]);
    let inode_size = if rev_level == 0 { 128u16 } else { inode_size_raw };

    let groups_count = (blocks_count + blocks_per_group - 1) / blocks_per_group;

    Ok(Superblock { inodes_count, blocks_count, block_size, blocks_per_group,
                    inodes_per_group, inode_size, groups_count })
}

// ─────────────────────────────────────────────────────────────────────────────
// Inode
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Inode {
    mode:   u16,
    size:   u64,
    /// Direct block pointers [0..11]
    blocks: [u32; 15],
}

fn inode_is_dir(mode: u16)  -> bool { mode & 0xF000 == 0x4000 }
fn inode_is_file(mode: u16) -> bool { mode & 0xF000 == 0x8000 }

// ─────────────────────────────────────────────────────────────────────────────
// EXT2 volume
// ─────────────────────────────────────────────────────────────────────────────

pub struct Ext2<D: BlockDev> {
    dev: D,
    sb:  Superblock,
    /// LBA of superblock sector (byte 1024 of the volume)
    sb_lba: u64,
}

impl<D: BlockDev> Ext2<D> {
    pub unsafe fn mount(dev: D) -> Result<Self> {
        let ss = dev.sector_size() as u64;
        // Superblock is always at byte offset 1024
        let sb_lba = 1024 / ss;
        let sb_off = (1024 % ss) as usize;

        let mut buf = [0u8; 4096];
        // Read enough sectors to cover the 1024-byte superblock
        let sectors_needed = (sb_off + 1024 + ss as usize - 1) / ss as usize;
        for i in 0..sectors_needed.min(4) {
            dev.read_sector(sb_lba + i as u64, &mut buf[i * ss as usize..(i+1) * ss as usize])?;
        }
        let sb = parse_superblock(&buf[sb_off..])?;
        Ok(Ext2 { dev, sb, sb_lba })
    }

    pub fn kind_str(&self) -> &'static str { "EXT2/3/4" }

    // ── Block I/O ─────────────────────────────────────────────────────────

    unsafe fn read_block(&self, block: u32, buf: &mut [u8]) -> Result<()> {
        let ss  = self.dev.sector_size() as u64;
        let bs  = self.sb.block_size as u64;
        let lba = (block as u64 * bs) / ss;
        let secs = (bs / ss) as usize;
        for i in 0..secs {
            self.dev.read_sector(lba + i as u64, &mut buf[i * ss as usize..(i+1) * ss as usize])?;
        }
        Ok(())
    }

    // ── Group descriptor ──────────────────────────────────────────────────

    /// Read the block group descriptor for group `g`.
    /// Returns (inode_table_block, block_bitmap, inode_bitmap).
    unsafe fn group_desc(&self, g: u32) -> Result<(u32, u32, u32)> {
        let bs  = self.sb.block_size as u64;
        let ss  = self.dev.sector_size() as u64;
        // Group descriptor table starts at block 2 (for 1K blocks) or 1 (for 2K+)
        let gdt_block = if self.sb.block_size == 1024 { 2u32 } else { 1u32 };
        // Each group descriptor is 32 bytes (EXT2/3) or 64 bytes (EXT4 with 64-bit)
        let desc_size = 32usize;
        let desc_off  = g as usize * desc_size;
        let block_off = desc_off % self.sb.block_size as usize;
        let block_idx = gdt_block + (desc_off / self.sb.block_size as usize) as u32;

        let mut buf = [0u8; 4096];
        let block_secs = (self.sb.block_size as usize / ss as usize).max(1);
        for i in 0..block_secs.min(4) {
            self.dev.read_sector((block_idx as u64 * bs) / ss + i as u64,
                &mut buf[i * ss as usize..(i+1) * ss as usize])?;
        }

        let d = &buf[block_off..block_off + 32];
        let block_bitmap  = u32::from_le_bytes([d[0],  d[1],  d[2],  d[3]]);
        let inode_bitmap  = u32::from_le_bytes([d[4],  d[5],  d[6],  d[7]]);
        let inode_table   = u32::from_le_bytes([d[8],  d[9],  d[10], d[11]]);
        Ok((inode_table, block_bitmap, inode_bitmap))
    }

    // ── Inode reading ─────────────────────────────────────────────────────

    unsafe fn read_inode(&self, ino: u32) -> Result<Inode> {
        if ino < 1 || ino > self.sb.inodes_count { return Err(FsError::NotFound); }
        let idx   = ino - 1;
        let group = idx / self.sb.inodes_per_group;
        let local = idx % self.sb.inodes_per_group;

        let (inode_table, _, _) = self.group_desc(group)?;

        let bs       = self.sb.block_size as usize;
        let isz      = self.sb.inode_size as usize;
        let ino_off  = local as usize * isz;
        let block    = inode_table as usize + ino_off / bs;
        let off      = ino_off % bs;

        let mut buf = [0u8; 4096];
        let secs = (bs / self.dev.sector_size()).max(1);
        let ss   = self.dev.sector_size() as u64;
        let lba  = (block as u64 * self.sb.block_size as u64) / ss;
        for i in 0..secs.min(8) {
            self.dev.read_sector(lba + i as u64, &mut buf[i * ss as usize..(i+1) * ss as usize])?;
        }

        let d = &buf[off..];
        let mode = u16::from_le_bytes([d[0], d[1]]);
        let size_lo = u32::from_le_bytes([d[4], d[5], d[6], d[7]]) as u64;
        let size_hi = u32::from_le_bytes([d[108],d[109],d[110],d[111]]) as u64;
        let size = if self.sb.block_size > 1024 { (size_hi << 32) | size_lo } else { size_lo };

        let mut blocks = [0u32; 15];
        for i in 0..15 {
            blocks[i] = u32::from_le_bytes([d[40+i*4], d[41+i*4], d[42+i*4], d[43+i*4]]);
        }
        Ok(Inode { mode, size, blocks })
    }

    // ── Block pointer resolution (direct/indirect) ────────────────────────

    /// Resolve logical block N of a file to a physical block number.
    unsafe fn file_block(&self, inode: &Inode, logical: u32) -> Result<u32> {
        let bps = self.sb.block_size as usize / 4; // pointers per block
        if (logical as usize) < 12 {
            // Direct
            return Ok(inode.blocks[logical as usize]);
        }
        let logical = logical as usize - 12;
        if logical < bps {
            // Singly indirect
            return self.read_indirect(inode.blocks[12], logical as u32);
        }
        let logical = logical - bps;
        if logical < bps * bps {
            // Doubly indirect
            let l1 = self.read_indirect(inode.blocks[13], (logical / bps) as u32)?;
            return self.read_indirect(l1, (logical % bps) as u32);
        }
        // Triply indirect
        let logical = logical - bps * bps;
        let l1 = self.read_indirect(inode.blocks[14], (logical / (bps * bps)) as u32)?;
        let l2 = self.read_indirect(l1, ((logical / bps) % bps) as u32)?;
        self.read_indirect(l2, (logical % bps) as u32)
    }

    unsafe fn read_indirect(&self, block: u32, idx: u32) -> Result<u32> {
        let bs = self.sb.block_size as usize;
        let ss = self.dev.sector_size();
        let mut buf = [0u8; 4096];
        let secs = (bs / ss).max(1);
        let lba  = (block as u64 * self.sb.block_size as u64) / ss as u64;
        for i in 0..secs.min(8) {
            self.dev.read_sector(lba + i as u64, &mut buf[i*ss..(i+1)*ss])?;
        }
        let off = idx as usize * 4;
        Ok(u32::from_le_bytes([buf[off], buf[off+1], buf[off+2], buf[off+3]]))
    }

    // ── Directory walking ─────────────────────────────────────────────────

    unsafe fn walk_dir<F>(&self, ino: u32, mut cb: F) -> Result<()>
    where F: FnMut(&str, u32) -> bool   // (name, child_inode)
    {
        let inode = self.read_inode(ino)?;
        if !inode_is_dir(inode.mode) { return Err(FsError::WrongType); }

        let bs       = self.sb.block_size as usize;
        let mut buf  = [0u8; 4096];
        let blocks   = ((inode.size as usize) + bs - 1) / bs;

        'outer: for b in 0..blocks {
            let phys = self.file_block(&inode, b as u32)?;
            if phys == 0 { continue; }
            self.read_block(phys, &mut buf[..bs])?;

            let mut off = 0usize;
            while off < bs {
                let rec_len  = u16::from_le_bytes([buf[off+4], buf[off+5]]) as usize;
                let ino_num  = u32::from_le_bytes([buf[off], buf[off+1], buf[off+2], buf[off+3]]);
                let name_len = buf[off+6] as usize;
                if rec_len == 0 { break; }
                if ino_num != 0 && name_len > 0 {
                    let name_bytes = &buf[off+8..off+8+name_len];
                    if let Ok(name) = core::str::from_utf8(name_bytes) {
                        if name != "." && name != ".." {
                            if !cb(name, ino_num) { break 'outer; }
                        }
                    }
                }
                off += rec_len;
            }
        }
        Ok(())
    }

    unsafe fn lookup_in(&self, dir_ino: u32, name: &str) -> Result<u32> {
        let mut found = None;
        self.walk_dir(dir_ino, |n, ino| {
            if n == name { found = Some(ino); false } else { true }
        })?;
        found.ok_or(FsError::NotFound)
    }

    unsafe fn resolve(&self, path: &str) -> Result<u32> {
        let mut ino = ROOT_INO;
        let mut rest = path.trim_start_matches('/');
        while !rest.is_empty() {
            let (first, tail) = path_split(rest);
            ino  = self.lookup_in(ino, first)?;
            rest = tail;
        }
        Ok(ino)
    }

    // ── Public API ────────────────────────────────────────────────────────

    pub unsafe fn read_dir<F>(&self, path: &str, mut cb: F) -> Result<()>
    where F: FnMut(&Metadata) -> bool
    {
        let dir_ino = self.resolve(path)?;
        self.walk_dir(dir_ino, |name, child_ino| {
            let child = match self.read_inode(child_ino) { Ok(i) => i, Err(_) => return true };
            let mut meta = Metadata::zeroed();
            meta.is_dir  = inode_is_dir(child.mode);
            meta.size    = child.size;
            meta.readonly= child.mode & 0o200 == 0;
            meta.set_name(name);
            !cb(&meta)
        })
    }

    pub unsafe fn stat(&self, path: &str) -> Result<Metadata> {
        let ino   = self.resolve(path)?;
        let inode = self.read_inode(ino)?;
        let mut meta = Metadata::zeroed();
        meta.is_dir  = inode_is_dir(inode.mode);
        meta.size    = inode.size;
        meta.readonly= inode.mode & 0o200 == 0;
        let name = path.rsplit('/').next().unwrap_or(path);
        meta.set_name(name);
        Ok(meta)
    }

    pub unsafe fn open<'s>(&'s self, path: &str) -> Result<Ext2File<'s, D>> {
        let ino   = self.resolve(path)?;
        let inode = self.read_inode(ino)?;
        if inode_is_dir(inode.mode) { return Err(FsError::WrongType); }
        Ok(Ext2File { vol: self, inode, pos: 0 })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Open file handle
// ─────────────────────────────────────────────────────────────────────────────

pub struct Ext2File<'a, D: BlockDev> {
    vol:   &'a Ext2<D>,
    inode: Inode,
    pos:   u64,
}

impl<'a, D: BlockDev> Ext2File<'a, D> {
    pub fn size(&self) -> u64 { self.inode.size }
    pub fn pos(&self)  -> u64 { self.pos }

    pub unsafe fn seek(&mut self, p: u64) -> Result<()> {
        if p > self.inode.size { return Err(FsError::InvalidArg); }
        self.pos = p;
        Ok(())
    }

    pub unsafe fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.pos >= self.inode.size { return Ok(0); }
        let bs   = self.vol.sb.block_size as usize;
        let want = ((self.inode.size - self.pos) as usize).min(buf.len());
        let mut done = 0usize;
        let mut tmp = [0u8; 4096];

        while done < want {
            let logical  = (self.pos / bs as u64) as u32;
            let off      = (self.pos % bs as u64) as usize;
            let phys     = self.vol.file_block(&self.inode, logical)?;
            self.vol.read_block(phys, &mut tmp[..bs])?;
            let take = (bs - off).min(want - done);
            buf[done..done+take].copy_from_slice(&tmp[off..off+take]);
            done     += take;
            self.pos += take as u64;
        }
        Ok(done)
    }
}
