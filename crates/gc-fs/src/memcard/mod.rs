//! Nintendo GameCube Memory Card filesystem driver.
//!
//! The GC memory card uses a proprietary Nintendo filesystem on top of the
//! raw flash storage (8 KB erase sectors, 128-byte write pages).
//!
//! ## On-card layout
//!
//! Block 0 (0x0000–0x1FFF): Card header — serial number, device ID, capacity.
//! Block 1 (0x2000–0x3FFF): Directory (copy A).
//! Block 2 (0x4000–0x5FFF): Directory (copy B) — mirror of block 1.
//! Block 3 (0x6000–0x7FFF): Block allocation table (BAT, copy A).
//! Block 4 (0x8000–0x9FFF): BAT (copy B).
//! Block 5+ (0xA000+):     User data (files stored in 8 KB blocks).
//!
//! The active directory and BAT copies are identified by the highest
//! `updated` sequence number.
//!
//! ## Files
//!
//! Each of the 127 directory entries contains:
//! - Game code (4 bytes), company code (2 bytes)
//! - Filename (32 bytes, space-padded)
//! - First block, length in blocks, file size
//! - Comment address (for Dolphin-style memory card manager)
//! - Icon/banner metadata
//!
//! To read a file: look up its first block, then follow the BAT chain
//! (FAT-style linked list). Each BAT entry is a u16 pointing to the
//! next block, or 0xFFFF for end-of-file.

use crate::{BlockDev, FsError, Metadata, Result, name_eq_ci};
use gc_hal::memcard::{MemCard, CardSlot};
use gc_hal::memcard::SEGMENT_SIZE;

// ── Constants ─────────────────────────────────────────────────────────────────

const BLOCK_SIZE:    usize = 8192; // bytes per logical block (= erase sector)
const MAX_FILES:     usize = 127;
const SYSAREA_BLKS:  u32   = 5;   // blocks 0-4 are system area
const DIR_BLOCK_A:   u32   = 1;   // block number of directory copy A
const DIR_BLOCK_B:   u32   = 2;   // block number of directory copy B
const BAT_BLOCK_A:   u32   = 3;
const BAT_BLOCK_B:   u32   = 4;

// ── Directory entry (64 bytes on-card) ────────────────────────────────────────

#[derive(Clone, Copy, Default)]
struct DirEntry {
    gamecode:  [u8; 4],
    company:   [u8; 2],
    /// Banner format flags
    bannerfmt: u8,
    /// Null-terminated filename (32 bytes)
    filename:  [u8; 32],
    /// First block in data area (block 5 = address 0xA000)
    first_blk: u16,
    /// Length in blocks
    blk_count: u16,
    _pad:      u16,
    /// File size hint (not always accurate)
    file_len:  u16,
    /// Permissions
    permission: u8,
    copy_times: u8,
    /// Comment address (within the file data, for card manager)
    comment_addr: u32,
    _pad2: [u8; 4],
}

impl DirEntry {
    fn is_valid(&self) -> bool {
        // A valid entry has a non-0xFF gamecode and a non-zero filename
        self.gamecode[0] != 0xFF && self.filename[0] != 0
    }

    fn filename_str(&self) -> &str {
        let end = self.filename.iter().position(|&b| b == 0).unwrap_or(32);
        core::str::from_utf8(&self.filename[..end]).unwrap_or("???")
    }
}

fn parse_dir_entry(data: &[u8; 64]) -> DirEntry {
    let mut e = DirEntry::default();
    e.gamecode.copy_from_slice(&data[0..4]);
    e.company.copy_from_slice(&data[4..6]);
    e.bannerfmt = data[7];
    e.filename.copy_from_slice(&data[8..40]);
    e.first_blk = u16::from_be_bytes([data[42], data[43]]);
    e.blk_count = u16::from_be_bytes([data[44], data[45]]);
    e.file_len  = u16::from_be_bytes([data[48], data[49]]);
    e.permission = data[50];
    e.copy_times = data[51];
    e.comment_addr = u32::from_be_bytes([data[52], data[53], data[54], data[55]]);
    e
}

// ── Memory card filesystem ─────────────────────────────────────────────────────

/// A mounted GameCube memory card filesystem.
pub struct MemCardFs {
    card:    MemCard,
    /// Parsed directory entries (max 127)
    dir:     [DirEntry; MAX_FILES],
    /// Block allocation table (BAT). `bat[n]` = next block after data block n,
    /// or 0xFFFF = end of chain. Index 0 corresponds to data block 5.
    bat:     [u16; 0xFFB], // max BAT entries for a 2043-block card
    /// Total data blocks on card
    total_blocks: u32,
}

impl MemCardFs {
    /// Mount a memory card and parse its directory and BAT.
    ///
    /// # Safety
    /// EXI must not be in use on the channel used by `slot`.
    pub unsafe fn mount(slot: CardSlot) -> Result<Self> {
        let card = MemCard::probe(slot).map_err(|_| FsError::Io)?;
        let total_blocks = card.total_bytes / BLOCK_SIZE as u32;

        let mut fs = MemCardFs {
            card,
            dir: [DirEntry::default(); MAX_FILES],
            bat: [0xFFFF; 0xFFB],
            total_blocks,
        };
        fs.load_directory()?;
        fs.load_bat()?;
        Ok(fs)
    }

    // ── Load directory ────────────────────────────────────────────────────

    unsafe fn load_directory(&mut self) -> Result<()> {
        // Read both directory copies and pick the one with the highest updated counter
        let dir_a = self.read_block_raw(DIR_BLOCK_A)?;
        let dir_b = self.read_block_raw(DIR_BLOCK_B)?;

        // Updated counter is at offset 0x1F8A–0x1F8B (bytes 8074–8075) in each copy
        let updated_a = u16::from_be_bytes([dir_a[8074], dir_a[8075]]);
        let updated_b = u16::from_be_bytes([dir_b[8074], dir_b[8075]]);
        let dir = if updated_a >= updated_b { &dir_a } else { &dir_b };

        // Each directory entry is 64 bytes; 127 entries start at offset 0
        for i in 0..MAX_FILES {
            let off = i * 64;
            let entry_bytes: &[u8; 64] = &dir[off..off+64].try_into()
                .map_err(|_| FsError::BadFormat)?;
            self.dir[i] = parse_dir_entry(entry_bytes);
        }
        Ok(())
    }

    unsafe fn load_bat(&mut self) -> Result<()> {
        let bat_a = self.read_block_raw(BAT_BLOCK_A)?;
        let bat_b = self.read_block_raw(BAT_BLOCK_B)?;

        let updated_a = u16::from_be_bytes([bat_a[4], bat_a[5]]);
        let updated_b = u16::from_be_bytes([bat_b[4], bat_b[5]]);
        let bat = if updated_a >= updated_b { &bat_a } else { &bat_b };

        // BAT starts at byte 4 (after 2-byte checksum and 2-byte updated counter)
        // Actually: [0..2] = chksum1, [2..4] = chksum2, [4..6] = updated, [6..8] = freeblocks
        // [8..10] = lastalloc, then [10..] = BAT entries (u16 each)
        let bat_data_off = 10;
        let entries = (BLOCK_SIZE - bat_data_off) / 2;
        for i in 0..entries.min(0xFFB) {
            let off = bat_data_off + i * 2;
            self.bat[i] = u16::from_be_bytes([bat[off], bat[off+1]]);
        }
        Ok(())
    }

    // ── Block I/O ─────────────────────────────────────────────────────────

    /// Read a raw 8 KB block from the card.
    unsafe fn read_block_raw(&self, block: u32) -> Result<[u8; BLOCK_SIZE]> {
        let mut data = [0u8; BLOCK_SIZE];
        // Each block = 16 × 512-byte segments
        let segs_per_block = BLOCK_SIZE / SEGMENT_SIZE;
        for s in 0..segs_per_block {
            let seg_addr = block * BLOCK_SIZE as u32 + s as u32 * SEGMENT_SIZE as u32;
            let seg = &mut data[s * SEGMENT_SIZE..(s+1) * SEGMENT_SIZE];
            let seg_arr: &mut [u8; SEGMENT_SIZE] = seg.try_into()
                .map_err(|_| FsError::BadFormat)?;
            self.card.read_segment(seg_addr, seg_arr)
                .map_err(|_| FsError::Io)?;
        }
        Ok(data)
    }

    /// Write a raw 8 KB block to the card (erase + write pages).
    unsafe fn write_block_raw(&self, block: u32, data: &[u8; BLOCK_SIZE]) -> Result<()> {
        let base_addr = block * BLOCK_SIZE as u32;
        // Erase the sector first
        self.card.erase_sector(base_addr).map_err(|_| FsError::Io)?;
        // Write 64 pages of 128 bytes each
        for p in 0..(BLOCK_SIZE / 128) {
            let page_addr = base_addr + p as u32 * 128;
            let page: &[u8; 128] = data[p*128..(p+1)*128].try_into()
                .map_err(|_| FsError::BadFormat)?;
            self.card.write_page(page_addr, page).map_err(|_| FsError::Io)?;
        }
        Ok(())
    }

    // ── BAT chain following ───────────────────────────────────────────────

    /// Return next block in chain. Block numbering: data block 5 = BAT index 0.
    fn bat_next(&self, block: u32) -> Option<u32> {
        if block < SYSAREA_BLKS { return None; }
        let idx = (block - SYSAREA_BLKS) as usize;
        if idx >= self.bat.len() { return None; }
        let next = self.bat[idx];
        if next >= 0xFFFC { None } else { Some(next as u32) }
    }

    // ── Public API ────────────────────────────────────────────────────────

    /// Iterate all files on the card, calling `cb` for each.
    pub fn read_dir<F>(&self, mut cb: F) where F: FnMut(&Metadata, &DirEntry) -> bool {
        for entry in &self.dir {
            if !entry.is_valid() { continue; }
            let mut meta = Metadata::zeroed();
            meta.is_dir = false;
            meta.size   = (entry.blk_count as u64) * BLOCK_SIZE as u64;
            meta.set_name(entry.filename_str());
            if !cb(&meta, entry) { break; }
        }
    }

    /// Find a file by game code + filename.
    ///
    /// `game` is the 4-byte game code (e.g. "GALE" for Melee).
    /// `filename` is the 32-byte name as stored on the card.
    pub fn find(&self, game: &[u8; 4], filename: &str) -> Option<&DirEntry> {
        for e in &self.dir {
            if !e.is_valid() { continue; }
            if &e.gamecode != game { continue; }
            if name_eq_ci(e.filename_str().as_bytes(), filename.as_bytes()) {
                return Some(e);
            }
        }
        None
    }

    /// Read an entire file into `buf`. Returns bytes written.
    ///
    /// `buf` must be large enough for the file (`entry.blk_count * 8192` bytes).
    pub unsafe fn read_file(&self, entry: &DirEntry, buf: &mut [u8]) -> Result<usize> {
        let needed = entry.blk_count as usize * BLOCK_SIZE;
        if buf.len() < needed { return Err(FsError::BufferTooSmall); }

        let mut block = entry.first_blk as u32;
        let mut written = 0usize;

        loop {
            let blk_data = self.read_block_raw(block)?;
            buf[written..written + BLOCK_SIZE].copy_from_slice(&blk_data);
            written += BLOCK_SIZE;
            match self.bat_next(block) {
                Some(next) => block = next,
                None       => break,
            }
        }
        Ok(written)
    }

    /// Write a complete file. The file must already exist (this overwrites it).
    ///
    /// For creating new files, format operations are needed — not yet implemented.
    pub unsafe fn write_file(&self, entry: &DirEntry, data: &[u8]) -> Result<()> {
        let blocks = entry.blk_count as usize;
        if data.len() < blocks * BLOCK_SIZE { return Err(FsError::InvalidArg); }

        let mut block = entry.first_blk as u32;
        for i in 0..blocks {
            let slice = &data[i * BLOCK_SIZE..(i+1) * BLOCK_SIZE];
            let arr: &[u8; BLOCK_SIZE] = slice.try_into().unwrap();
            self.write_block_raw(block, arr)?;
            match self.bat_next(block) {
                Some(next) => block = next,
                None if i + 1 < blocks => return Err(FsError::BadFormat),
                None => {}
            }
        }
        Ok(())
    }

    /// Total data blocks on this card.
    pub fn total_blocks(&self) -> u32 { self.total_blocks }

    /// Free blocks (counts BAT entries equal to 0x0000 = free).
    pub fn free_blocks(&self) -> u32 {
        let data_blocks = self.total_blocks.saturating_sub(SYSAREA_BLKS);
        self.bat[..data_blocks as usize].iter().filter(|&&b| b == 0).count() as u32
    }
}
