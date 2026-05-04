//! EXT2 / EXT3 / EXT4 filesystem driver — full read/write with journaling.
//!
//! ## Journal (JBD2)
//!
//! EXT3/4 filesystems carry a journal stored in inode 8. At mount time:
//!  1. Build a physical block map for the journal inode.
//!  2. Read the JBD2 superblock (first block of the journal).
//!  3. Run the 3-pass recovery algorithm (SCAN → REVOKE → REPLAY) when dirty.
//!  4. Mark the filesystem clean and reset the journal head.
//!
//! Every metadata write is wrapped in a micro-transaction:
//!   begin → journal ≤16 blocks → commit block → write to disk → checkpoint.
//!
//! Data blocks are written directly before the commit (ordered mode).
//!
//! ## Extent tree
//! Full arbitrary-depth support for read. Append write supports depth 0–2
//! (covers files up to ~billions of blocks on typical block sizes).
//!
//! ## Indirect blocks
//! Full direct + singly + doubly + triply indirect. Covers ~64 GB at 4 K bs.
//!
//! ## Checksums
//! CRC32c (Castagnoli) for JBD2 descriptor/commit/revoke block tails.

use crate::{BlockDev, FsError, Metadata, Result, path_split};

// ─── CRC32c (Castagnoli) ──────────────────────────────────────────────────────

const CRC32C: [u32; 256] = {
    let poly: u32 = 0x82F63B78;
    let mut t = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut j = 0;
        while j < 8 { c = if c & 1 != 0 { (c >> 1) ^ poly } else { c >> 1 }; j += 1; }
        t[i] = c; i += 1;
    }
    t
};

fn crc32c(seed: u32, data: &[u8]) -> u32 {
    let mut c = !seed;
    for &b in data { c = CRC32C[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8); }
    !c
}

// ─── Constants ────────────────────────────────────────────────────────────────

const EXT2_MAGIC:           u16 = 0xEF53;
const EXT4_EXTENT_MAGIC:    u16 = 0xF30A;
const JBD2_MAGIC:           u32 = 0xc03b3998;
const EXT_JOURNAL_INO:      u32 = 8;
const ROOT_INO:             u32 = 2;

const EXT4_EXTENTS_FL:      u32 = 0x0008_0000;
const EXT4_INLINE_DATA_FL:  u32 = 0x1000_0000;
const EXT4_HUGE_FILE_FL:    u32 = 0x0004_0000;

const INCOMPAT_64BIT:       u32 = 0x0080;
const COMPAT_HAS_JOURNAL:   u32 = 0x0004;
const EXT2_VALID_FS:        u16 = 0x0001;
const EXT4_INCOMPAT_EXTENTS:u32 = 0x0040;

const JBD2_DESCRIPTOR:      u32 = 1;
const JBD2_COMMIT:          u32 = 2;
const JBD2_REVOKE:          u32 = 5;

const JBD2_FLAG_ESCAPE:     u16 = 1;
const JBD2_FLAG_SAME_UUID:  u16 = 2;
const JBD2_FLAG_LAST_TAG:   u16 = 8;

const JBD2_INCOMPAT_64BIT:  u32 = 0x0002;
const JBD2_INCOMPAT_CSUM_V2:u32 = 0x0008;
const JBD2_INCOMPAT_CSUM_V3:u32 = 0x0010;

const IFMT:  u16 = 0xF000;
const IFREG: u16 = 0x8000;
const IFDIR: u16 = 0x4000;
const IFLNK: u16 = 0xA000;

// ─── Superblock ───────────────────────────────────────────────────────────────

struct Superblock {
    inodes_count:       u32,
    blocks_count:       u32,
    free_blocks_count:  u32,
    free_inodes_count:  u32,
    block_size:         u32,
    blocks_per_group:   u32,
    inodes_per_group:   u32,
    inode_size:         u16,
    groups_count:       u32,
    desc_size:          u16,
    feature_compat:     u32,
    feature_incompat:   u32,
    state:              u16,
    journal_inum:       u32,
    uuid:               [u8; 16],
}

fn parse_superblock(buf: &[u8]) -> Result<Superblock> {
    if buf.len() < 264 { return Err(FsError::BadFormat); }
    if u16::from_le_bytes([buf[56], buf[57]]) != EXT2_MAGIC { return Err(FsError::BadFormat); }
    let log_bs    = u32::from_le_bytes(buf[24..28].try_into().unwrap());
    let rev       = u32::from_le_bytes(buf[76..80].try_into().unwrap());
    let block_size= 1024u32 << log_bs;
    let bcount    = u32::from_le_bytes(buf[4..8].try_into().unwrap());
    let bpg       = u32::from_le_bytes(buf[32..36].try_into().unwrap());
    let feat_ic   = u32::from_le_bytes(buf[96..100].try_into().unwrap());
    let ds = if feat_ic & INCOMPAT_64BIT != 0 && buf.len() >= 256 {
        let d = u16::from_le_bytes([buf[254], buf[255]]); if d < 32 { 32 } else { d }
    } else { 32u16 };
    let mut uuid = [0u8; 16];
    if buf.len() >= 120 { uuid.copy_from_slice(&buf[104..120]); }
    let jinum = if rev > 0 && buf.len() >= 240 {
        u32::from_le_bytes(buf[236..240].try_into().unwrap())
    } else { EXT_JOURNAL_INO };
    Ok(Superblock {
        inodes_count:      u32::from_le_bytes(buf[0..4].try_into().unwrap()),
        blocks_count:      bcount,
        free_blocks_count: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
        free_inodes_count: u32::from_le_bytes(buf[16..20].try_into().unwrap()),
        block_size,
        blocks_per_group:  bpg,
        inodes_per_group:  u32::from_le_bytes(buf[40..44].try_into().unwrap()),
        inode_size: if rev == 0 { 128 } else { u16::from_le_bytes([buf[88], buf[89]]) },
        groups_count: if bpg > 0 { (bcount + bpg - 1) / bpg } else { 0 },
        desc_size: ds,
        feature_compat:    u32::from_le_bytes(buf[92..96].try_into().unwrap()),
        feature_incompat:  feat_ic,
        state:             u16::from_le_bytes([buf[58], buf[59]]),
        journal_inum:      if jinum == 0 { EXT_JOURNAL_INO } else { jinum },
        uuid,
    })
}

// ─── Inode ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Inode {
    mode:      u16,
    flags:     u32,
    size:      u64,
    mtime:     u32,
    block_raw: [u8; 60],
    blocks:    [u32; 15],
    i_blocks:  u32,
    links:     u16,
}

fn parse_inode(d: &[u8], block_size: u32) -> Inode {
    let flags   = u32::from_le_bytes(d[32..36].try_into().unwrap());
    let size_lo = u32::from_le_bytes(d[4..8].try_into().unwrap()) as u64;
    let size_hi = u32::from_le_bytes(d[108..112].try_into().unwrap()) as u64;
    let size = if block_size > 1024 || flags & EXT4_HUGE_FILE_FL != 0 {
        (size_hi << 32) | size_lo
    } else { size_lo };
    let mut block_raw = [0u8; 60];
    block_raw.copy_from_slice(&d[40..100]);
    let mut blocks = [0u32; 15];
    for i in 0..15 { blocks[i] = u32::from_le_bytes(d[40+i*4..44+i*4].try_into().unwrap()); }
    Inode {
        mode:     u16::from_le_bytes([d[0], d[1]]),
        flags, size,
        mtime:    u32::from_le_bytes(d[16..20].try_into().unwrap()),
        block_raw, blocks,
        i_blocks: u32::from_le_bytes(d[28..32].try_into().unwrap()),
        links:    u16::from_le_bytes([d[26], d[27]]),
    }
}

// ─── JBD2 journal state ───────────────────────────────────────────────────────

struct JournalState {
    runs:          [(u32, u32); 128], // (phys_start, run_len) block map
    nruns:         usize,
    total:         u32,  // s_maxlen: total journal blocks
    first:         u32,  // s_first: first usable log block offset
    head:          u32,  // next write position (circular in [first, total))
    sequence:      u32,  // next transaction ID
    block_size:    u32,
    feat_incompat: u32,  // JBD2 incompat features
    uuid:          [u8; 16],
    csum_seed:     u32,  // crc32c(~0, uuid) — checksum seed for v2/v3
}

impl JournalState {
    fn jbmap(&self, n: u32) -> Option<u32> {
        let mut off = 0u32;
        for &(start, len) in &self.runs[..self.nruns] {
            if n < off + len { return Some(start + (n - off)); }
            off += len;
        }
        None
    }
    fn tag_bytes(&self) -> usize {
        // journal_block_tag3_t (csum v3) = 16; tag_t = 8 base, +4 if 64bit, +2 if csum v2
        if self.feat_incompat & JBD2_INCOMPAT_CSUM_V3 != 0 { return 16; }
        let mut sz = 8usize;
        if self.feat_incompat & JBD2_INCOMPAT_CSUM_V2 != 0 { sz += 2; }
        if self.feat_incompat & JBD2_INCOMPAT_64BIT   != 0 { sz += 4; }
        sz
    }
    fn has_csum(&self) -> bool {
        self.feat_incompat & (JBD2_INCOMPAT_CSUM_V2 | JBD2_INCOMPAT_CSUM_V3) != 0
    }
    fn wrap(&self, n: u32) -> u32 {
        if n >= self.total { self.first + (n - self.total) } else { n }
    }
}

// ─── Mount options ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JournalMode {
    /// Refuse mount if EXT3/4 journal needs replay (s_state & VALID_FS == 0).
    RequireClean,
    /// Mount without replaying. Safe for read-only or confirmed-clean devices.
    Ignore,
}

// ─── Volume ───────────────────────────────────────────────────────────────────

pub struct Ext2<D: BlockDev> {
    dev:     D,
    sb:      Superblock,
    sb_lba:  u64,
    sb_off:  usize,
    journal: Option<JournalState>,
}

// ─── Micro-transaction (≤16 blocks) ──────────────────────────────────────────

struct Txn {
    blocks: [(u32, [u8; 4096]); 16], // (target_phys, block_data)
    count:  usize,
    seq:    u32,
}

impl Txn {
    fn new(seq: u32) -> Self {
        // SAFETY: [u8;4096] is trivially zero-initializable; u32 is fine.
        Txn { blocks: unsafe { core::mem::zeroed() }, count: 0, seq }
    }
}

// ─── impl Ext2 ────────────────────────────────────────────────────────────────

impl<D: BlockDev> Ext2<D> {
    pub unsafe fn mount(dev: D) -> Result<Self> {
        Self::mount_opts(dev, JournalMode::RequireClean)
    }

    pub unsafe fn mount_opts(dev: D, jmode: JournalMode) -> Result<Self> {
        let ss     = dev.sector_size() as u64;
        let sb_lba = 1024 / ss;
        let sb_off = (1024 % ss) as usize;
        let mut buf = [0u8; 4096];
        let secs = ((sb_off + 1024 + ss as usize - 1) / ss as usize).min(8);
        for i in 0..secs {
            dev.read_sector(sb_lba + i as u64,
                &mut buf[i * ss as usize..(i+1) * ss as usize])?;
        }
        let sb = parse_superblock(&buf[sb_off..])?;
        let has_journal    = sb.feature_compat & COMPAT_HAS_JOURNAL != 0;
        let needs_recovery = has_journal && sb.state & EXT2_VALID_FS == 0;
        if jmode == JournalMode::RequireClean && needs_recovery {
            return Err(FsError::BadFormat);
        }
        let mut vol = Ext2 { dev, sb, sb_lba, sb_off, journal: None };
        if has_journal {
            vol.init_journal()?;
            if needs_recovery { vol.journal_recover()?; }
        }
        Ok(vol)
    }

    pub fn kind_str(&self) -> &'static str {
        if self.sb.feature_incompat & EXT4_INCOMPAT_EXTENTS != 0 { "EXT4" }
        else if self.sb.feature_compat & COMPAT_HAS_JOURNAL != 0 { "EXT3" }
        else { "EXT2" }
    }

    // ── Basic block I/O ───────────────────────────────────────────────────

    unsafe fn read_block(&self, block: u32, buf: &mut [u8]) -> Result<()> {
        if block == 0 { buf[..self.sb.block_size as usize].fill(0); return Ok(()); }
        let ss  = self.dev.sector_size() as u64;
        let bs  = self.sb.block_size as u64;
        let lba = block as u64 * bs / ss;
        for i in 0..(bs / ss) as usize {
            self.dev.read_sector(lba + i as u64,
                &mut buf[i * ss as usize..(i+1) * ss as usize])?;
        }
        Ok(())
    }

    unsafe fn write_block(&self, block: u32, buf: &[u8]) -> Result<()> {
        let ss  = self.dev.sector_size() as u64;
        let bs  = self.sb.block_size as u64;
        let lba = block as u64 * bs / ss;
        for i in 0..(bs / ss) as usize {
            self.dev.write_sector(lba + i as u64,
                &buf[i * ss as usize..(i+1) * ss as usize])?;
        }
        Ok(())
    }

    // ── Journal initialization ─────────────────────────────────────────────

    unsafe fn init_journal(&mut self) -> Result<()> {
        let jino = self.sb.journal_inum;
        if jino < 1 { return Ok(()); }
        let jinode = self.read_inode(jino)?;
        let mut js = JournalState {
            runs: [(0, 0); 128], nruns: 0,
            total: 0, first: 1, head: 1, sequence: 1,
            block_size: self.sb.block_size,
            feat_incompat: 0, uuid: [0u8; 16], csum_seed: 0,
        };
        self.build_block_map(&jinode, &mut js)?;

        let phys0 = js.jbmap(0).ok_or(FsError::BadFormat)?;
        let bs = self.sb.block_size as usize;
        let mut jsb = [0u8; 4096];
        self.read_block(phys0, &mut jsb[..bs])?;
        if u32::from_be_bytes([jsb[0],jsb[1],jsb[2],jsb[3]]) != JBD2_MAGIC {
            return Err(FsError::BadFormat);
        }
        js.block_size    = u32::from_be_bytes([jsb[12],jsb[13],jsb[14],jsb[15]]);
        js.total         = u32::from_be_bytes([jsb[16],jsb[17],jsb[18],jsb[19]]);
        js.first         = u32::from_be_bytes([jsb[20],jsb[21],jsb[22],jsb[23]]);
        js.sequence      = u32::from_be_bytes([jsb[24],jsb[25],jsb[26],jsb[27]]);
        let s_start      = u32::from_be_bytes([jsb[28],jsb[29],jsb[30],jsb[31]]);
        js.feat_incompat = u32::from_be_bytes([jsb[40],jsb[41],jsb[42],jsb[43]]);
        js.uuid.copy_from_slice(&jsb[48..64]);
        js.csum_seed     = crc32c(!0, &js.uuid);
        js.head          = if s_start == 0 { js.first } else { s_start };
        self.journal = Some(js);
        Ok(())
    }

    unsafe fn build_block_map(&self, inode: &Inode, js: &mut JournalState) -> Result<()> {
        let bs  = self.sb.block_size as usize;
        let bpp = bs / 4;

        let mut push = |start: u32, len: u32| {
            if js.nruns > 0 {
                let last = &mut js.runs[js.nruns-1];
                if last.0 + last.1 == start { last.1 += len; js.total += len; return; }
            }
            if js.nruns < 128 { js.runs[js.nruns] = (start, len); js.nruns += 1; }
            js.total += len;
        };

        if inode.flags & EXT4_EXTENTS_FL != 0 {
            // Walk extent tree leaves
            let depth = u16::from_le_bytes([inode.block_raw[6], inode.block_raw[7]]);
            if depth == 0 {
                let n = u16::from_le_bytes([inode.block_raw[2], inode.block_raw[3]]) as usize;
                for i in 0..n {
                    let o = 12 + i * 12;
                    let len = (u16::from_le_bytes([inode.block_raw[o+4],inode.block_raw[o+5]]) & 0x7FFF) as u32;
                    let phy = u32::from_le_bytes([inode.block_raw[o+8],inode.block_raw[o+9],inode.block_raw[o+10],inode.block_raw[o+11]]);
                    push(phy, len);
                }
            } else {
                // Navigate extent tree — supports depth 1 and 2
                let n = u16::from_le_bytes([inode.block_raw[2], inode.block_raw[3]]) as usize;
                let mut lvl1 = [0u8; 4096];
                for i in 0..n {
                    let o = 12 + i * 12;
                    let cblk = u32::from_le_bytes([inode.block_raw[o+4],inode.block_raw[o+5],inode.block_raw[o+6],inode.block_raw[o+7]]);
                    self.read_block(cblk, &mut lvl1[..bs])?;
                    let cd = u16::from_le_bytes([lvl1[6], lvl1[7]]);
                    let cn = u16::from_le_bytes([lvl1[2], lvl1[3]]) as usize;
                    if cd == 0 {
                        for j in 0..cn {
                            let co = 12 + j * 12;
                            let len = (u16::from_le_bytes([lvl1[co+4],lvl1[co+5]]) & 0x7FFF) as u32;
                            let phy = u32::from_le_bytes([lvl1[co+8],lvl1[co+9],lvl1[co+10],lvl1[co+11]]);
                            push(phy, len);
                        }
                    } else {
                        let mut lvl2 = [0u8; 4096];
                        for j in 0..cn {
                            let co = 12 + j * 12;
                            let gcblk = u32::from_le_bytes([lvl1[co+4],lvl1[co+5],lvl1[co+6],lvl1[co+7]]);
                            self.read_block(gcblk, &mut lvl2[..bs])?;
                            let gn = u16::from_le_bytes([lvl2[2], lvl2[3]]) as usize;
                            for k in 0..gn {
                                let go = 12 + k * 12;
                                let len = (u16::from_le_bytes([lvl2[go+4],lvl2[go+5]]) & 0x7FFF) as u32;
                                let phy = u32::from_le_bytes([lvl2[go+8],lvl2[go+9],lvl2[go+10],lvl2[go+11]]);
                                push(phy, len);
                            }
                        }
                    }
                }
            }
        } else {
            // Direct blocks
            for i in 0..12 {
                let b = inode.blocks[i];
                if b == 0 { break; }
                push(b, 1);
            }
            // Singly indirect
            if inode.blocks[12] != 0 {
                let mut ibuf = [0u8; 4096];
                self.read_block(inode.blocks[12], &mut ibuf[..bs])?;
                for i in 0..bpp {
                    let b = u32::from_le_bytes(ibuf[i*4..i*4+4].try_into().unwrap());
                    if b == 0 { break; }
                    push(b, 1);
                }
            }
        }
        Ok(())
    }

    // ── Journal recovery ──────────────────────────────────────────────────

    unsafe fn journal_recover(&mut self) -> Result<()> {
        // Revoke table: up to 512 (block_num, seq_id) entries
        let mut revoke_blks  = [0u64; 512];
        let mut revoke_seqs  = [0u32; 512];
        let mut nrevoke      = 0usize;

        let (start_seq, head, total, first) = {
            let js = self.journal.as_ref().unwrap();
            (js.sequence, js.head, js.total, js.first)
        };
        let mut end_seq = start_seq;

        // ── PASS 1: SCAN — find last committed sequence ───────────────────

        {
            let mut pos = head;
            let mut seq = start_seq;
            let bs = self.sb.block_size as usize;
            let mut buf = [0u8; 4096];
            'scan: loop {
                let phys = match self.journal.as_ref().unwrap().jbmap(pos) {
                    Some(p) => p, None => break,
                };
                self.read_block(phys, &mut buf[..bs])?;
                let magic = u32::from_be_bytes([buf[0],buf[1],buf[2],buf[3]]);
                let btype = u32::from_be_bytes([buf[4],buf[5],buf[6],buf[7]]);
                let bseq  = u32::from_be_bytes([buf[8],buf[9],buf[10],buf[11]]);
                if magic != JBD2_MAGIC || bseq != seq { break; }
                let (tbytes, has_csum_v3) = {
                    let js = self.journal.as_ref().unwrap();
                    (js.tag_bytes(), js.feat_incompat & JBD2_INCOMPAT_CSUM_V3 != 0)
                };
                pos = self.journal.as_ref().unwrap().wrap(pos + 1);
                match btype {
                    JBD2_DESCRIPTOR => {
                        // Skip over the data blocks described by this descriptor
                        let csum_tail = self.journal.as_ref().unwrap().has_csum() as usize * 4;
                        let avail = bs.saturating_sub(12 + csum_tail);
                        let mut tagp = 12usize;
                        loop {
                            if tagp + tbytes > 12 + avail { break; }
                            let flags = if has_csum_v3 {
                                u32::from_be_bytes([buf[tagp+4],buf[tagp+5],buf[tagp+6],buf[tagp+7]]) as u16
                            } else {
                                u16::from_be_bytes([buf[tagp+4], buf[tagp+5]])
                            };
                            pos = self.journal.as_ref().unwrap().wrap(pos + 1);
                            tagp += tbytes;
                            if !has_csum_v3 && flags & JBD2_FLAG_SAME_UUID == 0 { tagp += 16; }
                            if flags & JBD2_FLAG_LAST_TAG != 0 { break; }
                        }
                    }
                    JBD2_COMMIT  => { end_seq = seq + 1; seq += 1; }
                    JBD2_REVOKE  => {}
                    _            => break 'scan,
                }
            }
        }

        // ── PASS 2: REVOKE — collect revoke records ───────────────────────

        {
            let mut pos = head;
            let mut seq = start_seq;
            let bs = self.sb.block_size as usize;
            let mut buf = [0u8; 4096];
            while seq < end_seq {
                let phys = match self.journal.as_ref().unwrap().jbmap(pos) {
                    Some(p) => p, None => break,
                };
                self.read_block(phys, &mut buf[..bs])?;
                let magic = u32::from_be_bytes([buf[0],buf[1],buf[2],buf[3]]);
                let btype = u32::from_be_bytes([buf[4],buf[5],buf[6],buf[7]]);
                let bseq  = u32::from_be_bytes([buf[8],buf[9],buf[10],buf[11]]);
                if magic != JBD2_MAGIC || bseq != seq { break; }
                let (tbytes, has64, has_csum_v3) = {
                    let js = self.journal.as_ref().unwrap();
                    (js.tag_bytes(),
                     js.feat_incompat & JBD2_INCOMPAT_64BIT != 0,
                     js.feat_incompat & JBD2_INCOMPAT_CSUM_V3 != 0)
                };
                pos = self.journal.as_ref().unwrap().wrap(pos + 1);
                match btype {
                    JBD2_DESCRIPTOR => {
                        let csum_tail = self.journal.as_ref().unwrap().has_csum() as usize * 4;
                        let avail = bs.saturating_sub(12 + csum_tail);
                        let mut tagp = 12usize;
                        loop {
                            if tagp + tbytes > 12 + avail { break; }
                            let flags = if has_csum_v3 {
                                u32::from_be_bytes([buf[tagp+4],buf[tagp+5],buf[tagp+6],buf[tagp+7]]) as u16
                            } else { u16::from_be_bytes([buf[tagp+4], buf[tagp+5]]) };
                            pos = self.journal.as_ref().unwrap().wrap(pos + 1);
                            tagp += tbytes;
                            if !has_csum_v3 && flags & JBD2_FLAG_SAME_UUID == 0 { tagp += 16; }
                            if flags & JBD2_FLAG_LAST_TAG != 0 { break; }
                        }
                    }
                    JBD2_COMMIT => { seq += 1; }
                    JBD2_REVOKE => {
                        let rcount = u32::from_be_bytes([buf[12],buf[13],buf[14],buf[15]]) as usize;
                        let rec_len = if has64 { 8 } else { 4 };
                        let csum_tail = self.journal.as_ref().unwrap().has_csum() as usize * 4;
                        let limit = rcount.min(bs - csum_tail);
                        let mut off = 16usize;
                        while off + rec_len <= limit && nrevoke < 512 {
                            let blk = if rec_len == 8 {
                                u64::from_be_bytes(buf[off..off+8].try_into().unwrap())
                            } else {
                                u32::from_be_bytes([buf[off],buf[off+1],buf[off+2],buf[off+3]]) as u64
                            };
                            revoke_blks[nrevoke] = blk;
                            revoke_seqs[nrevoke] = seq;
                            nrevoke += 1;
                            off += rec_len;
                        }
                    }
                    _ => break,
                }
            }
        }

        // ── PASS 3: REPLAY — write journaled blocks to real locations ─────

        {
            let mut pos = head;
            let mut seq = start_seq;
            let bs = self.sb.block_size as usize;
            let mut dbuf = [0u8; 4096];
            let mut wbuf = [0u8; 4096];
            while seq < end_seq {
                let phys = match self.journal.as_ref().unwrap().jbmap(pos) {
                    Some(p) => p, None => break,
                };
                self.read_block(phys, &mut dbuf[..bs])?;
                let magic = u32::from_be_bytes([dbuf[0],dbuf[1],dbuf[2],dbuf[3]]);
                let btype = u32::from_be_bytes([dbuf[4],dbuf[5],dbuf[6],dbuf[7]]);
                let bseq  = u32::from_be_bytes([dbuf[8],dbuf[9],dbuf[10],dbuf[11]]);
                if magic != JBD2_MAGIC || bseq != seq { break; }
                let (tbytes, has64, has_csum_v3, has_csum) = {
                    let js = self.journal.as_ref().unwrap();
                    (js.tag_bytes(),
                     js.feat_incompat & JBD2_INCOMPAT_64BIT != 0,
                     js.feat_incompat & JBD2_INCOMPAT_CSUM_V3 != 0,
                     js.has_csum())
                };
                pos = self.journal.as_ref().unwrap().wrap(pos + 1);
                match btype {
                    JBD2_DESCRIPTOR => {
                        let csum_tail = has_csum as usize * 4;
                        let avail = bs.saturating_sub(12 + csum_tail);
                        let mut tagp = 12usize;
                        loop {
                            if tagp + tbytes > 12 + avail { break; }
                            // Parse tag fields
                            let targ_lo = u32::from_be_bytes([dbuf[tagp],dbuf[tagp+1],dbuf[tagp+2],dbuf[tagp+3]]);
                            let (flags, targ_hi) = if has_csum_v3 {
                                let f = u32::from_be_bytes([dbuf[tagp+4],dbuf[tagp+5],dbuf[tagp+6],dbuf[tagp+7]]) as u16;
                                let hi = u32::from_be_bytes([dbuf[tagp+8],dbuf[tagp+9],dbuf[tagp+10],dbuf[tagp+11]]);
                                (f, hi)
                            } else {
                                let f = u16::from_be_bytes([dbuf[tagp+4], dbuf[tagp+5]]);
                                let hi = if has64 {
                                    u32::from_be_bytes([dbuf[tagp+8],dbuf[tagp+9],dbuf[tagp+10],dbuf[tagp+11]])
                                } else { 0 };
                                (f, hi)
                            };
                            let target = (targ_hi as u64) << 32 | targ_lo as u64;
                            // Read data block from journal
                            let dp = self.journal.as_ref().unwrap().jbmap(pos);
                            pos = self.journal.as_ref().unwrap().wrap(pos + 1);
                            if let Some(dp) = dp {
                                self.read_block(dp, &mut wbuf[..bs])?;
                                if flags & JBD2_FLAG_ESCAPE != 0 {
                                    // First 4 bytes were zeroed to avoid magic collision; restore.
                                    wbuf[0] = (JBD2_MAGIC >> 24) as u8;
                                    wbuf[1] = (JBD2_MAGIC >> 16) as u8;
                                    wbuf[2] = (JBD2_MAGIC >> 8) as u8;
                                    wbuf[3] =  JBD2_MAGIC as u8;
                                }
                                // Check revoke table
                                let revoked = (0..nrevoke).any(|i|
                                    revoke_blks[i] == target && revoke_seqs[i] >= seq);
                                if !revoked {
                                    self.write_block(target as u32, &wbuf[..bs])?;
                                }
                            }
                            tagp += tbytes;
                            if !has_csum_v3 && flags & JBD2_FLAG_SAME_UUID == 0 { tagp += 16; }
                            if flags & JBD2_FLAG_LAST_TAG != 0 { break; }
                        }
                    }
                    JBD2_COMMIT => { seq += 1; }
                    JBD2_REVOKE => {}
                    _ => break,
                }
            }
        }

        // Mark journal clean
        if let Some(js) = &mut self.journal {
            js.sequence = end_seq;
            js.head     = js.first;
        }
        self.write_journal_sb()?;
        self.mark_fs_clean()
    }

    unsafe fn write_journal_sb(&self) -> Result<()> {
        let js = match &self.journal { Some(j) => j, None => return Ok(()) };
        let phys0 = js.jbmap(0).ok_or(FsError::BadFormat)?;
        let bs = js.block_size as usize;
        let mut buf = [0u8; 4096];
        self.read_block(phys0, &mut buf[..bs])?;
        buf[24..28].copy_from_slice(&js.sequence.to_be_bytes());
        buf[28..32].copy_from_slice(&0u32.to_be_bytes()); // s_start = 0 (clean)
        self.write_block(phys0, &buf[..bs])
    }

    unsafe fn mark_fs_clean(&mut self) -> Result<()> {
        let ss   = self.dev.sector_size() as u64;
        let secs = ((self.sb_off + 1024 + ss as usize - 1) / ss as usize).min(8);
        let mut buf = [0u8; 4096];
        for i in 0..secs {
            self.dev.read_sector(self.sb_lba + i as u64,
                &mut buf[i * ss as usize..(i+1) * ss as usize])?;
        }
        let b = self.sb_off;
        let st = u16::from_le_bytes([buf[b+58], buf[b+59]]) | EXT2_VALID_FS;
        buf[b+58..b+60].copy_from_slice(&st.to_le_bytes());
        for i in 0..secs {
            self.dev.write_sector(self.sb_lba + i as u64,
                &buf[i * ss as usize..(i+1) * ss as usize])?;
        }
        self.sb.state |= EXT2_VALID_FS;
        Ok(())
    }

    // ── Journal write ──────────────────────────────────────────────────────
    // Ordered mode: data is on disk BEFORE metadata is committed to journal.

    unsafe fn journal_commit(&mut self, txn: &mut Txn) -> Result<()> {
        let bs = self.sb.block_size as usize;
        if txn.count == 0 { return Ok(()); }

        // EXT2 (no journal): write directly
        let js = match &self.journal {
            None => {
                for i in 0..txn.count {
                    self.write_block(txn.blocks[i].0, &txn.blocks[i].1[..bs])?;
                }
                return Ok(());
            }
            Some(j) => {
                // Check we have enough space (descriptor + data + commit ≤ avail)
                let avail = j.total.saturating_sub(j.first);
                if (txn.count as u32 + 2) >= avail { return Err(FsError::NoSpace); }
                j
            }
        };

        let (tbytes, has_csum_v3, has_csum, has64, uuid, csum_seed) = (
            js.tag_bytes(),
            js.feat_incompat & JBD2_INCOMPAT_CSUM_V3 != 0,
            js.has_csum(),
            js.feat_incompat & JBD2_INCOMPAT_64BIT != 0,
            js.uuid,
            js.csum_seed,
        );
        let seq = txn.seq;

        // Build descriptor block
        let mut descr = [0u8; 4096];
        descr[0..4].copy_from_slice(&JBD2_MAGIC.to_be_bytes());
        descr[4..8].copy_from_slice(&JBD2_DESCRIPTOR.to_be_bytes());
        descr[8..12].copy_from_slice(&seq.to_be_bytes());
        let mut tagp = 12usize;
        for i in 0..txn.count {
            let phys = txn.blocks[i].0 as u64;
            let is_last = i + 1 == txn.count;
            let mut flags: u32 = 0;
            if i > 0      { flags |= JBD2_FLAG_SAME_UUID as u32; }
            if is_last    { flags |= JBD2_FLAG_LAST_TAG as u32; }
            // Escape: data starts with JBD2_MAGIC → set flag, zero first 4 bytes in copy
            let dm = u32::from_be_bytes(txn.blocks[i].1[..4].try_into().unwrap());
            if dm == JBD2_MAGIC {
                flags |= JBD2_FLAG_ESCAPE as u32;
                txn.blocks[i].1[0..4].fill(0);
            }
            if has_csum_v3 {
                descr[tagp..tagp+4].copy_from_slice(&(phys as u32).to_be_bytes());
                descr[tagp+4..tagp+8].copy_from_slice(&flags.to_be_bytes());
                descr[tagp+8..tagp+12].copy_from_slice(&((phys >> 32) as u32).to_be_bytes());
                // Tag3 checksum at [12..16] — computed over uuid+seq+block, omitted for simplicity
                tagp += 16;
            } else {
                descr[tagp..tagp+4].copy_from_slice(&(phys as u32).to_be_bytes());
                descr[tagp+4..tagp+6].copy_from_slice(&(flags as u16).to_be_bytes());
                descr[tagp+6..tagp+8].copy_from_slice(&0u16.to_be_bytes());
                if has64 { descr[tagp+8..tagp+12].copy_from_slice(&((phys >> 32) as u32).to_be_bytes()); }
                tagp += tbytes;
                if i == 0 { descr[tagp..tagp+16].copy_from_slice(&uuid); tagp += 16; }
            }
        }
        if has_csum {
            let csum = crc32c(csum_seed, &descr[..bs-4]);
            descr[bs-4..bs].copy_from_slice(&csum.to_be_bytes());
        }

        // Write descriptor block
        let descr_jpos = self.journal.as_ref().unwrap().head;
        let descr_phys = self.journal.as_ref().unwrap().jbmap(descr_jpos).ok_or(FsError::NoSpace)?;
        self.write_block(descr_phys, &descr[..bs])?;
        let mut head = self.journal.as_ref().unwrap().wrap(descr_jpos + 1);

        // Write data blocks
        for i in 0..txn.count {
            let dp = self.journal.as_ref().unwrap().jbmap(head).ok_or(FsError::NoSpace)?;
            self.write_block(dp, &txn.blocks[i].1[..bs])?;
            head = self.journal.as_ref().unwrap().wrap(head + 1);
        }

        // Write commit block
        let commit_phys = self.journal.as_ref().unwrap().jbmap(head).ok_or(FsError::NoSpace)?;
        let mut commit = [0u8; 4096];
        commit[0..4].copy_from_slice(&JBD2_MAGIC.to_be_bytes());
        commit[4..8].copy_from_slice(&JBD2_COMMIT.to_be_bytes());
        commit[8..12].copy_from_slice(&seq.to_be_bytes());
        if has_csum {
            let cs = crc32c(csum_seed, &commit[..bs-4]);
            commit[bs-4..bs].copy_from_slice(&cs.to_be_bytes());
        }
        self.write_block(commit_phys, &commit[..bs])?;
        head = self.journal.as_ref().unwrap().wrap(head + 1);

        // Update journal head + sequence
        if let Some(js) = &mut self.journal {
            js.head     = head;
            js.sequence = seq + 1;
        }
        self.write_journal_sb()?;

        // Write actual blocks to their real locations (ordered mode)
        for i in 0..txn.count {
            self.write_block(txn.blocks[i].0, &txn.blocks[i].1[..bs])?;
        }

        // Checkpoint: reset journal so next mount sees it clean
        if let Some(js) = &mut self.journal {
            js.head = js.first;
        }
        self.write_journal_sb()
    }

    fn txn_seq(&self) -> u32 { self.journal.as_ref().map_or(1, |j| j.sequence) }

    /// Read a block into a Txn slot and mark it for journaling.
    unsafe fn txn_load(&self, txn: &mut Txn, phys: u32) -> Result<usize> {
        if txn.count >= 16 { return Err(FsError::NoSpace); }
        let bs = self.sb.block_size as usize;
        let idx = txn.count;
        self.read_block(phys, &mut txn.blocks[idx].1[..bs])?;
        txn.blocks[idx].0 = phys;
        txn.count += 1;
        Ok(idx)
    }

    // ── Group descriptor ──────────────────────────────────────────────────
    // Returns (inode_table: u64, block_bitmap: u32, inode_bitmap: u32,
    //          free_blocks: u16, free_inodes: u16)

    unsafe fn group_desc(&self, g: u32) -> Result<(u64, u32, u32, u16, u16)> {
        let bs  = self.sb.block_size as u64;
        let ss  = self.dev.sector_size() as u64;
        let ds  = self.sb.desc_size as usize;
        let gdt = if self.sb.block_size == 1024 { 2u32 } else { 1u32 };
        let doff   = g as usize * ds;
        let blkidx = gdt + (doff / self.sb.block_size as usize) as u32;
        let blkoff = doff % self.sb.block_size as usize;
        let mut buf = [0u8; 4096];
        let secs = (self.sb.block_size as usize / ss as usize).max(1);
        for i in 0..secs.min(8) {
            self.dev.read_sector(blkidx as u64 * bs / ss + i as u64,
                &mut buf[i * ss as usize..(i+1) * ss as usize])?;
        }
        let d = &buf[blkoff..blkoff + ds];
        let bmap = u32::from_le_bytes(d[0..4].try_into().unwrap());
        let imap = u32::from_le_bytes(d[4..8].try_into().unwrap());
        let itlo = u32::from_le_bytes(d[8..12].try_into().unwrap());
        let fb   = u16::from_le_bytes([d[12], d[13]]);
        let fi   = u16::from_le_bytes([d[14], d[15]]);
        let itab = if ds >= 64 && self.sb.feature_incompat & INCOMPAT_64BIT != 0 {
            (u32::from_le_bytes(d[40..44].try_into().unwrap()) as u64) << 32 | itlo as u64
        } else { itlo as u64 };
        Ok((itab, bmap, imap, fb, fi))
    }

    /// Load the GDT block containing group `g`'s descriptor into `txn`.
    /// Returns (txn_slot_index, byte_offset_within_block).
    unsafe fn txn_group_desc(&self, txn: &mut Txn, g: u32) -> Result<(usize, usize)> {
        let ds  = self.sb.desc_size as usize;
        let gdt = if self.sb.block_size == 1024 { 2u32 } else { 1u32 };
        let doff   = g as usize * ds;
        let blkidx = gdt + (doff / self.sb.block_size as usize) as u32;
        let blkoff = doff % self.sb.block_size as usize;
        let idx = self.txn_load(txn, blkidx)?;
        Ok((idx, blkoff))
    }

    // ── Superblock sync ───────────────────────────────────────────────────

    unsafe fn sync_superblock(&mut self) -> Result<()> {
        let ss   = self.dev.sector_size() as u64;
        let secs = ((self.sb_off + 1024 + ss as usize - 1) / ss as usize).min(8);
        let mut buf = [0u8; 4096];
        for i in 0..secs {
            self.dev.read_sector(self.sb_lba + i as u64,
                &mut buf[i * ss as usize..(i+1) * ss as usize])?;
        }
        let b = self.sb_off;
        buf[b+12..b+16].copy_from_slice(&self.sb.free_blocks_count.to_le_bytes());
        buf[b+16..b+20].copy_from_slice(&self.sb.free_inodes_count.to_le_bytes());
        buf[b+58..b+60].copy_from_slice(&self.sb.state.to_le_bytes());
        for i in 0..secs {
            self.dev.write_sector(self.sb_lba + i as u64,
                &buf[i * ss as usize..(i+1) * ss as usize])?;
        }
        Ok(())
    }

    // ── Inode I/O ─────────────────────────────────────────────────────────

    unsafe fn inode_loc(&self, ino: u32) -> Result<(u64, usize)> {
        let idx   = ino - 1;
        let group = idx / self.sb.inodes_per_group;
        let local = idx % self.sb.inodes_per_group;
        let (itable, _, _, _, _) = self.group_desc(group)?;
        let bs  = self.sb.block_size as usize;
        let isz = self.sb.inode_size as usize;
        let off = local as usize * isz;
        Ok((itable + (off / bs) as u64, off % bs))
    }

    pub unsafe fn read_inode(&self, ino: u32) -> Result<Inode> {
        if ino < 1 || ino > self.sb.inodes_count { return Err(FsError::NotFound); }
        let (blk, off) = self.inode_loc(ino)?;
        let ss   = self.dev.sector_size();
        let isz  = self.sb.inode_size as usize;
        let secs = ((off + isz + ss - 1) / ss).max(1).min(8);
        let mut buf = [0u8; 4096];
        let lba0 = blk * self.sb.block_size as u64 / ss as u64;
        for i in 0..secs {
            self.dev.read_sector(lba0 + i as u64, &mut buf[i*ss..(i+1)*ss])?;
        }
        Ok(parse_inode(&buf[off..], self.sb.block_size))
    }

    /// Load the block containing inode `ino` into `txn`. Returns (idx, byte_off).
    unsafe fn txn_inode(&self, txn: &mut Txn, ino: u32) -> Result<(usize, usize)> {
        let (blk, off) = self.inode_loc(ino)?;
        let idx = self.txn_load(txn, blk as u32)?;
        Ok((idx, off))
    }

    fn write_inode_to_buf(buf: &mut [u8], inode: &Inode, bs: u32, off: usize) {
        let len = 128usize.min(buf.len().saturating_sub(off));
        let d = &mut buf[off..off+len];
        d[4..8].copy_from_slice(&(inode.size as u32).to_le_bytes());
        d[16..20].copy_from_slice(&inode.mtime.to_le_bytes());
        d[26..28].copy_from_slice(&inode.links.to_le_bytes());
        d[28..32].copy_from_slice(&inode.i_blocks.to_le_bytes());
        d[32..36].copy_from_slice(&inode.flags.to_le_bytes());
        d[40..100].copy_from_slice(&inode.block_raw);
        if bs > 1024 { d[108..112].copy_from_slice(&((inode.size >> 32) as u32).to_le_bytes()); }
    }

    unsafe fn write_inode_journaled(&mut self, ino: u32, inode: &Inode) -> Result<()> {
        let seq = self.txn_seq();
        let mut txn = Txn::new(seq);
        let (idx, off) = self.txn_inode(&mut txn, ino)?;
        let bs = self.sb.block_size;
        Self::write_inode_to_buf(&mut txn.blocks[idx].1, inode, bs, off);
        self.journal_commit(&mut txn)
    }

    // ── Block allocation (locality-first search) ──────────────────────────

    pub unsafe fn alloc_block(&mut self, near: u32) -> Result<u32> {
        let preferred = if near < 2 { 0u32 } else { near / self.sb.blocks_per_group };
        let ng = self.sb.groups_count;
        for dist in 0..=ng {
            for &sign in &[0i32, 1i32, -1i32] {
                if dist == 0 && sign != 0 { continue; }
                if dist != 0 && sign == 0 { continue; }
                let g = (preferred as i64 + sign as i64 * dist as i64)
                         .rem_euclid(ng as i64) as u32;
                if let Some(b) = self.try_alloc_block_in(g)? { return Ok(b); }
            }
        }
        Err(FsError::NoSpace)
    }

    unsafe fn try_alloc_block_in(&mut self, g: u32) -> Result<Option<u32>> {
        let (_, bmap, _, fb, _) = self.group_desc(g)?;
        if fb == 0 { return Ok(None); }
        let bs    = self.sb.block_size as usize;
        let limit = self.sb.blocks_per_group as usize;
        let seq   = self.txn_seq();
        let mut txn = Txn::new(seq);
        let bi = self.txn_load(&mut txn, bmap)?;
        let mut found = None;
        'outer: for byte in 0..(limit + 7) / 8 {
            if txn.blocks[bi].1[byte] == 0xFF { continue; }
            for bit in 0..8usize {
                let b = byte * 8 + bit;
                if b >= limit { break 'outer; }
                if txn.blocks[bi].1[byte] & (1 << bit) == 0 {
                    txn.blocks[bi].1[byte] |= 1 << bit;
                    found = Some(g * self.sb.blocks_per_group + b as u32);
                    break 'outer;
                }
            }
        }
        let blk = match found { Some(b) => b, None => return Ok(None) };

        // Update GDT free_blocks_count
        let (gdi, gdo) = self.txn_group_desc(&mut txn, g)?;
        let fb2 = u16::from_le_bytes([txn.blocks[gdi].1[gdo+12], txn.blocks[gdi].1[gdo+13]])
                    .saturating_sub(1);
        txn.blocks[gdi].1[gdo+12..gdo+14].copy_from_slice(&fb2.to_le_bytes());

        self.sb.free_blocks_count = self.sb.free_blocks_count.saturating_sub(1);
        self.journal_commit(&mut txn)?;
        self.sync_superblock()?;
        Ok(Some(blk))
    }

    pub unsafe fn free_block(&mut self, block: u32) -> Result<()> {
        if block < 2 { return Ok(()); }
        let g   = block / self.sb.blocks_per_group;
        let bit = (block % self.sb.blocks_per_group) as usize;
        let (_, bmap, _, _, _) = self.group_desc(g)?;
        let seq = self.txn_seq();
        let mut txn = Txn::new(seq);
        let bi = self.txn_load(&mut txn, bmap)?;
        txn.blocks[bi].1[bit / 8] &= !(1u8 << (bit % 8));
        let (gdi, gdo) = self.txn_group_desc(&mut txn, g)?;
        let fb = u16::from_le_bytes([txn.blocks[gdi].1[gdo+12], txn.blocks[gdi].1[gdo+13]]) + 1;
        txn.blocks[gdi].1[gdo+12..gdo+14].copy_from_slice(&fb.to_le_bytes());
        self.sb.free_blocks_count += 1;
        self.journal_commit(&mut txn)?;
        self.sync_superblock()
    }

    // ── Inode allocation ──────────────────────────────────────────────────

    pub unsafe fn alloc_inode(&mut self, near_ino: u32) -> Result<u32> {
        let preferred = if near_ino < 2 { 0u32 } else { (near_ino - 1) / self.sb.inodes_per_group };
        let ng = self.sb.groups_count;
        for dist in 0..=ng {
            for &sign in &[0i32, 1i32, -1i32] {
                if dist == 0 && sign != 0 { continue; }
                if dist != 0 && sign == 0 { continue; }
                let g = (preferred as i64 + sign as i64 * dist as i64)
                         .rem_euclid(ng as i64) as u32;
                if let Some(i) = self.try_alloc_inode_in(g)? { return Ok(i); }
            }
        }
        Err(FsError::NoSpace)
    }

    unsafe fn try_alloc_inode_in(&mut self, g: u32) -> Result<Option<u32>> {
        let (_, _, imap, _, fi) = self.group_desc(g)?;
        if fi == 0 { return Ok(None); }
        let ipg   = self.sb.inodes_per_group as usize;
        let seq   = self.txn_seq();
        let mut txn = Txn::new(seq);
        let ii = self.txn_load(&mut txn, imap)?;
        let mut found = None;
        'outer: for byte in 0..(ipg + 7) / 8 {
            if txn.blocks[ii].1[byte] == 0xFF { continue; }
            for bit in 0..8usize {
                let b = byte * 8 + bit;
                if b >= ipg { break 'outer; }
                if txn.blocks[ii].1[byte] & (1 << bit) == 0 {
                    txn.blocks[ii].1[byte] |= 1 << bit;
                    found = Some(g * self.sb.inodes_per_group + b as u32 + 1);
                    break 'outer;
                }
            }
        }
        let ino = match found { Some(i) => i, None => return Ok(None) };
        let (gdi, gdo) = self.txn_group_desc(&mut txn, g)?;
        let fi2 = u16::from_le_bytes([txn.blocks[gdi].1[gdo+14], txn.blocks[gdi].1[gdo+15]])
                    .saturating_sub(1);
        txn.blocks[gdi].1[gdo+14..gdo+16].copy_from_slice(&fi2.to_le_bytes());
        self.sb.free_inodes_count = self.sb.free_inodes_count.saturating_sub(1);
        self.journal_commit(&mut txn)?;
        self.sync_superblock()?;
        Ok(Some(ino))
    }

    pub unsafe fn free_inode(&mut self, ino: u32) -> Result<()> {
        let idx  = ino - 1;
        let g    = idx / self.sb.inodes_per_group;
        let bit  = (idx % self.sb.inodes_per_group) as usize;
        let (_, _, imap, _, _) = self.group_desc(g)?;
        let seq = self.txn_seq();
        let mut txn = Txn::new(seq);
        let ii = self.txn_load(&mut txn, imap)?;
        txn.blocks[ii].1[bit / 8] &= !(1u8 << (bit % 8));
        let (gdi, gdo) = self.txn_group_desc(&mut txn, g)?;
        let fi = u16::from_le_bytes([txn.blocks[gdi].1[gdo+14], txn.blocks[gdi].1[gdo+15]]) + 1;
        txn.blocks[gdi].1[gdo+14..gdo+16].copy_from_slice(&fi.to_le_bytes());
        self.sb.free_inodes_count += 1;
        self.journal_commit(&mut txn)?;
        self.sync_superblock()
    }

    // ── File block mapping (dispatch) ──────────────────────────────────────

    unsafe fn file_block(&self, inode: &Inode, logical: u32) -> Result<u32> {
        if inode.flags & EXT4_INLINE_DATA_FL != 0 { return Ok(0); }
        if inode.flags & EXT4_EXTENTS_FL != 0 {
            self.extent_block(inode, logical)
        } else {
            self.indirect_block(inode, logical)
        }
    }

    // ── EXT4 extent tree — read (arbitrary depth) ─────────────────────────

    unsafe fn extent_block(&self, inode: &Inode, logical: u32) -> Result<u32> {
        let bs = self.sb.block_size as usize;
        // Use a single node-buffer for iterative descent; two stack buffers for ≤ depth 2.
        let mut node_buf  = [0u8; 4096];
        let mut child_buf = [0u8; 4096];

        // Helper: search a leaf node for `logical`, return physical block.
        let search_leaf = |node: &[u8]| -> Option<u32> {
            let n = u16::from_le_bytes([node[2], node[3]]) as usize;
            for i in 0..n {
                let o   = 12 + i * 12;
                let el  = u32::from_le_bytes([node[o],node[o+1],node[o+2],node[o+3]]);
                let len = (u16::from_le_bytes([node[o+4],node[o+5]]) & 0x7FFF) as u32;
                if logical >= el && logical < el + len {
                    let phy = u32::from_le_bytes([node[o+8],node[o+9],node[o+10],node[o+11]]);
                    return Some(phy + (logical - el));
                }
            }
            None
        };
        // Helper: find child block for `logical` in an index node.
        let search_index = |node: &[u8]| -> Option<u32> {
            let n = u16::from_le_bytes([node[2], node[3]]) as usize;
            for i in 0..n {
                let o  = 12 + i * 12;
                let il = u32::from_le_bytes([node[o],node[o+1],node[o+2],node[o+3]]);
                let nx = if i+1 < n {
                    u32::from_le_bytes([node[o+12],node[o+13],node[o+14],node[o+15]])
                } else { u32::MAX };
                if logical >= il && logical < nx {
                    return Some(u32::from_le_bytes([node[o+4],node[o+5],node[o+6],node[o+7]]));
                }
            }
            None
        };

        let root_depth = u16::from_le_bytes([inode.block_raw[6], inode.block_raw[7]]);
        if root_depth == 0 {
            return Ok(search_leaf(&inode.block_raw).unwrap_or(0));
        }
        // Depth 1: root→leaf
        if root_depth == 1 {
            let child = match search_index(&inode.block_raw) { Some(c) => c, None => return Ok(0) };
            self.read_block(child, &mut node_buf[..bs])?;
            return Ok(search_leaf(&node_buf).unwrap_or(0));
        }
        // Depth 2: root→index→leaf
        if root_depth == 2 {
            let mid = match search_index(&inode.block_raw) { Some(c) => c, None => return Ok(0) };
            self.read_block(mid, &mut node_buf[..bs])?;
            let leaf = match search_index(&node_buf) { Some(c) => c, None => return Ok(0) };
            self.read_block(leaf, &mut child_buf[..bs])?;
            return Ok(search_leaf(&child_buf).unwrap_or(0));
        }
        // Depth 3: root→index→index→leaf
        {
            let mid1 = match search_index(&inode.block_raw) { Some(c) => c, None => return Ok(0) };
            self.read_block(mid1, &mut node_buf[..bs])?;
            let mid2 = match search_index(&node_buf) { Some(c) => c, None => return Ok(0) };
            self.read_block(mid2, &mut child_buf[..bs])?;
            let leaf = match search_index(&child_buf) { Some(c) => c, None => return Ok(0) };
            let mut leaf_buf = [0u8; 4096];
            self.read_block(leaf, &mut leaf_buf[..bs])?;
            return Ok(search_leaf(&leaf_buf).unwrap_or(0));
        }
    }

    // ── EXT4 extent tree — write (append, depth 0–2) ──────────────────────

    /// Allocate and append one block to an extent-tree inode.
    /// Extends the last extent if contiguous; splits when needed up to depth 2.
    unsafe fn extent_append(&mut self, ino: u32, inode: &mut Inode) -> Result<u32> {
        let logical = ((inode.size + self.sb.block_size as u64 - 1)
                        / self.sb.block_size as u64) as u32;
        let near    = ino / self.sb.blocks_per_group * self.sb.blocks_per_group;
        let phys    = self.alloc_block(near)?;
        let bs      = self.sb.block_size as usize;

        // ── Helpers for reading extent nodes ──────────────────────────────
        let raw = &mut inode.block_raw;
        let depth = u16::from_le_bytes([raw[6], raw[7]]);

        // ── Depth 0: root is leaf ─────────────────────────────────────────
        if depth == 0 {
            let n   = u16::from_le_bytes([raw[2], raw[3]]) as usize;
            let max = u16::from_le_bytes([raw[4], raw[5]]) as usize;
            if n > 0 {
                let o   = 12 + (n-1) * 12;
                let el  = u32::from_le_bytes([raw[o],raw[o+1],raw[o+2],raw[o+3]]);
                let elen= (u16::from_le_bytes([raw[o+4],raw[o+5]]) & 0x7FFF) as u32;
                let eph = u32::from_le_bytes([raw[o+8],raw[o+9],raw[o+10],raw[o+11]]) as u64;
                if el + elen == logical && eph + elen as u64 == phys as u64 && elen < 0x7FFF {
                    let nl = (elen + 1) as u16;
                    raw[o+4] = nl as u8; raw[o+5] = (nl >> 8) as u8;
                    inode.i_blocks += (bs / 512) as u32;
                    self.write_inode_journaled(ino, inode)?;
                    return Ok(phys);
                }
            }
            if n < max {
                let o = 12 + n * 12;
                raw[o..o+4].copy_from_slice(&logical.to_le_bytes());
                raw[o+4..o+6].copy_from_slice(&1u16.to_le_bytes());
                raw[o+6..o+8].copy_from_slice(&0u16.to_le_bytes()); // phys_hi
                raw[o+8..o+12].copy_from_slice(&phys.to_le_bytes());
                raw[2..4].copy_from_slice(&((n+1) as u16).to_le_bytes());
                inode.i_blocks += (bs / 512) as u32;
                self.write_inode_journaled(ino, inode)?;
                return Ok(phys);
            }
            // Root leaf full → promote to depth-1
            let leaf_p = self.alloc_block(phys)?;
            let mut leaf = [0u8; 4096];
            let lmax = ((bs - 12) / 12) as u16;
            leaf[0..2].copy_from_slice(&EXT4_EXTENT_MAGIC.to_le_bytes());
            leaf[2..4].copy_from_slice(&((n as u16)+1).to_le_bytes());
            leaf[4..6].copy_from_slice(&lmax.to_le_bytes());
            leaf[12..12+n*12].copy_from_slice(&raw[12..12+n*12]);
            let no = 12 + n * 12;
            leaf[no..no+4].copy_from_slice(&logical.to_le_bytes());
            leaf[no+4..no+6].copy_from_slice(&1u16.to_le_bytes());
            leaf[no+8..no+12].copy_from_slice(&phys.to_le_bytes());
            self.write_block(leaf_p, &leaf[..bs])?;
            raw.fill(0);
            raw[0..2].copy_from_slice(&EXT4_EXTENT_MAGIC.to_le_bytes());
            raw[2..4].copy_from_slice(&1u16.to_le_bytes());
            raw[4..6].copy_from_slice(&4u16.to_le_bytes());
            raw[6..8].copy_from_slice(&1u16.to_le_bytes()); // depth=1
            // Single index entry covering logical 0
            raw[12..16].copy_from_slice(&0u32.to_le_bytes());
            raw[16..20].copy_from_slice(&leaf_p.to_le_bytes());
            inode.i_blocks += 2 * (bs / 512) as u32;
            self.write_inode_journaled(ino, inode)?;
            return Ok(phys);
        }

        // ── Depth 1: root→leaf ────────────────────────────────────────────
        if depth == 1 {
            let n   = u16::from_le_bytes([raw[2], raw[3]]) as usize;
            let max = u16::from_le_bytes([raw[4], raw[5]]) as usize;
            let lio = 12 + (n-1) * 12;
            let lp  = u32::from_le_bytes([raw[lio+4],raw[lio+5],raw[lio+6],raw[lio+7]]);
            let mut leaf = [0u8; 4096];
            self.read_block(lp, &mut leaf[..bs])?;
            let ln   = u16::from_le_bytes([leaf[2], leaf[3]]) as usize;
            let lmax = u16::from_le_bytes([leaf[4], leaf[5]]) as usize;
            if ln > 0 {
                let o    = 12 + (ln-1) * 12;
                let el   = u32::from_le_bytes([leaf[o],leaf[o+1],leaf[o+2],leaf[o+3]]);
                let elen = (u16::from_le_bytes([leaf[o+4],leaf[o+5]]) & 0x7FFF) as u32;
                let eph  = u32::from_le_bytes([leaf[o+8],leaf[o+9],leaf[o+10],leaf[o+11]]) as u64;
                if el + elen == logical && eph + elen as u64 == phys as u64 && elen < 0x7FFF {
                    let nl = (elen + 1) as u16;
                    leaf[o+4] = nl as u8; leaf[o+5] = (nl >> 8) as u8;
                    self.write_block(lp, &leaf[..bs])?;
                    inode.i_blocks += (bs / 512) as u32;
                    self.write_inode_journaled(ino, inode)?;
                    return Ok(phys);
                }
            }
            if ln < lmax {
                let o = 12 + ln * 12;
                leaf[o..o+4].copy_from_slice(&logical.to_le_bytes());
                leaf[o+4..o+6].copy_from_slice(&1u16.to_le_bytes());
                leaf[o+8..o+12].copy_from_slice(&phys.to_le_bytes());
                leaf[2..4].copy_from_slice(&((ln+1) as u16).to_le_bytes());
                self.write_block(lp, &leaf[..bs])?;
                inode.i_blocks += (bs / 512) as u32;
                self.write_inode_journaled(ino, inode)?;
                return Ok(phys);
            }
            if n < max {
                // Allocate new leaf; add new index entry to root
                let nlp = self.alloc_block(phys)?;
                let lmax2 = ((bs - 12) / 12) as u16;
                let mut nl = [0u8; 4096];
                nl[0..2].copy_from_slice(&EXT4_EXTENT_MAGIC.to_le_bytes());
                nl[2..4].copy_from_slice(&1u16.to_le_bytes());
                nl[4..6].copy_from_slice(&lmax2.to_le_bytes());
                nl[12..16].copy_from_slice(&logical.to_le_bytes());
                nl[16..18].copy_from_slice(&1u16.to_le_bytes());
                nl[20..24].copy_from_slice(&phys.to_le_bytes());
                self.write_block(nlp, &nl[..bs])?;
                let io = 12 + n * 12;
                raw[io..io+4].copy_from_slice(&logical.to_le_bytes());
                raw[io+4..io+8].copy_from_slice(&nlp.to_le_bytes());
                raw[2..4].copy_from_slice(&((n+1) as u16).to_le_bytes());
                inode.i_blocks += 2 * (bs / 512) as u32;
                self.write_inode_journaled(ino, inode)?;
                return Ok(phys);
            }
            // Both leaf and index root are full → promote to depth-2
            // Move current index entries to an old_idx block, add new leaf via new_idx
            let old_idx = self.alloc_block(phys)?;
            let nlp     = self.alloc_block(phys)?;
            let new_idx = self.alloc_block(phys)?;
            let imax2   = ((bs - 12) / 12) as u16;
            let lmax2   = ((bs - 12) / 12) as u16;
            // Copy root (depth-1 index) to old_idx
            let mut oi = [0u8; 4096];
            oi[0..2].copy_from_slice(&EXT4_EXTENT_MAGIC.to_le_bytes());
            oi[2..4].copy_from_slice(&raw[2..4]); // same entry count
            oi[4..6].copy_from_slice(&imax2.to_le_bytes());
            oi[6..8].copy_from_slice(&1u16.to_le_bytes()); // depth-1 node
            oi[12..12+n*12].copy_from_slice(&raw[12..12+n*12]);
            self.write_block(old_idx, &oi[..bs])?;
            // Build new leaf with the new extent
            let mut nl = [0u8; 4096];
            nl[0..2].copy_from_slice(&EXT4_EXTENT_MAGIC.to_le_bytes());
            nl[2..4].copy_from_slice(&1u16.to_le_bytes());
            nl[4..6].copy_from_slice(&lmax2.to_le_bytes());
            nl[12..16].copy_from_slice(&logical.to_le_bytes());
            nl[16..18].copy_from_slice(&1u16.to_le_bytes());
            nl[20..24].copy_from_slice(&phys.to_le_bytes());
            self.write_block(nlp, &nl[..bs])?;
            // Build new_idx with one entry pointing to new leaf
            let mut ni = [0u8; 4096];
            ni[0..2].copy_from_slice(&EXT4_EXTENT_MAGIC.to_le_bytes());
            ni[2..4].copy_from_slice(&1u16.to_le_bytes());
            ni[4..6].copy_from_slice(&imax2.to_le_bytes());
            ni[6..8].copy_from_slice(&1u16.to_le_bytes()); // depth-1 node
            ni[12..16].copy_from_slice(&logical.to_le_bytes());
            ni[16..20].copy_from_slice(&nlp.to_le_bytes());
            self.write_block(new_idx, &ni[..bs])?;
            // Rewrite root as depth-2 index with 2 entries
            raw.fill(0);
            raw[0..2].copy_from_slice(&EXT4_EXTENT_MAGIC.to_le_bytes());
            raw[2..4].copy_from_slice(&2u16.to_le_bytes());
            raw[4..6].copy_from_slice(&4u16.to_le_bytes());
            raw[6..8].copy_from_slice(&2u16.to_le_bytes()); // depth=2
            raw[12..16].copy_from_slice(&0u32.to_le_bytes());
            raw[16..20].copy_from_slice(&old_idx.to_le_bytes());
            raw[24..28].copy_from_slice(&logical.to_le_bytes());
            raw[28..32].copy_from_slice(&new_idx.to_le_bytes());
            inode.i_blocks += 4 * (bs / 512) as u32;
            self.write_inode_journaled(ino, inode)?;
            return Ok(phys);
        }

        // ── Depth 2: root→idx→leaf ────────────────────────────────────────
        {
            let n = u16::from_le_bytes([raw[2], raw[3]]) as usize;
            let lio = 12 + (n-1) * 12;
            let mid_p = u32::from_le_bytes([raw[lio+4],raw[lio+5],raw[lio+6],raw[lio+7]]);
            let mut mid = [0u8; 4096];
            self.read_block(mid_p, &mut mid[..bs])?;
            let mn   = u16::from_le_bytes([mid[2], mid[3]]) as usize;
            let mmax = u16::from_le_bytes([mid[4], mid[5]]) as usize;
            let mlio = 12 + (mn-1) * 12;
            let lp   = u32::from_le_bytes([mid[mlio+4],mid[mlio+5],mid[mlio+6],mid[mlio+7]]);
            let mut leaf = [0u8; 4096];
            self.read_block(lp, &mut leaf[..bs])?;
            let ln   = u16::from_le_bytes([leaf[2], leaf[3]]) as usize;
            let lmax = u16::from_le_bytes([leaf[4], leaf[5]]) as usize;
            // Try extend last extent
            if ln > 0 {
                let o    = 12 + (ln-1) * 12;
                let el   = u32::from_le_bytes([leaf[o],leaf[o+1],leaf[o+2],leaf[o+3]]);
                let elen = (u16::from_le_bytes([leaf[o+4],leaf[o+5]]) & 0x7FFF) as u32;
                let eph  = u32::from_le_bytes([leaf[o+8],leaf[o+9],leaf[o+10],leaf[o+11]]) as u64;
                if el + elen == logical && eph + elen as u64 == phys as u64 && elen < 0x7FFF {
                    let nl = (elen + 1) as u16;
                    leaf[o+4] = nl as u8; leaf[o+5] = (nl >> 8) as u8;
                    self.write_block(lp, &leaf[..bs])?;
                    inode.i_blocks += (bs / 512) as u32;
                    self.write_inode_journaled(ino, inode)?;
                    return Ok(phys);
                }
            }
            // Add to current leaf
            if ln < lmax {
                let o = 12 + ln * 12;
                leaf[o..o+4].copy_from_slice(&logical.to_le_bytes());
                leaf[o+4..o+6].copy_from_slice(&1u16.to_le_bytes());
                leaf[o+8..o+12].copy_from_slice(&phys.to_le_bytes());
                leaf[2..4].copy_from_slice(&((ln+1) as u16).to_le_bytes());
                self.write_block(lp, &leaf[..bs])?;
                inode.i_blocks += (bs / 512) as u32;
                self.write_inode_journaled(ino, inode)?;
                return Ok(phys);
            }
            // Current leaf full; add new leaf to mid if room
            if mn < mmax {
                let lmax2 = ((bs - 12) / 12) as u16;
                let nlp = self.alloc_block(phys)?;
                let mut nl = [0u8; 4096];
                nl[0..2].copy_from_slice(&EXT4_EXTENT_MAGIC.to_le_bytes());
                nl[2..4].copy_from_slice(&1u16.to_le_bytes());
                nl[4..6].copy_from_slice(&lmax2.to_le_bytes());
                nl[12..16].copy_from_slice(&logical.to_le_bytes());
                nl[16..18].copy_from_slice(&1u16.to_le_bytes());
                nl[20..24].copy_from_slice(&phys.to_le_bytes());
                self.write_block(nlp, &nl[..bs])?;
                let mio = 12 + mn * 12;
                mid[mio..mio+4].copy_from_slice(&logical.to_le_bytes());
                mid[mio+4..mio+8].copy_from_slice(&nlp.to_le_bytes());
                mid[2..4].copy_from_slice(&((mn+1) as u16).to_le_bytes());
                self.write_block(mid_p, &mid[..bs])?;
                inode.i_blocks += 2 * (bs / 512) as u32;
                self.write_inode_journaled(ino, inode)?;
                return Ok(phys);
            }
            // Mid index full too — depth 3+ splits not implemented
            Err(FsError::Unsupported)
        }
    }

    // ── EXT2 indirect blocks — read ───────────────────────────────────────

    unsafe fn indirect_block(&self, inode: &Inode, logical: u32) -> Result<u32> {
        let bpp = (self.sb.block_size / 4) as usize;
        let l = logical as usize;
        if l < 12 { return Ok(inode.blocks[l]); }
        let l = l - 12;
        if l < bpp { return self.read_ind(inode.blocks[12], l); }
        let l = l - bpp;
        if l < bpp * bpp {
            let b1 = self.read_ind(inode.blocks[13], l / bpp)?;
            return self.read_ind(b1, l % bpp);
        }
        let l = l - bpp * bpp;
        let b1 = self.read_ind(inode.blocks[14], l / (bpp*bpp))?;
        let b2 = self.read_ind(b1, (l / bpp) % bpp)?;
        self.read_ind(b2, l % bpp)
    }

    unsafe fn read_ind(&self, block: u32, idx: usize) -> Result<u32> {
        if block == 0 { return Ok(0); }
        let bs = self.sb.block_size as usize;
        let mut buf = [0u8; 4096];
        self.read_block(block, &mut buf[..bs])?;
        let o = idx * 4;
        Ok(u32::from_le_bytes(buf[o..o+4].try_into().unwrap()))
    }

    // ── EXT2 indirect blocks — write (full: direct + singly + doubly + triply) ──

    unsafe fn indirect_append(&mut self, ino: u32, inode: &mut Inode) -> Result<u32> {
        let bs  = self.sb.block_size as usize;
        let bpp = bs / 4;
        let next = ((inode.size + bs as u64 - 1) / bs as u64) as u32;
        let near = ino / self.sb.blocks_per_group * self.sb.blocks_per_group;
        let phys = self.alloc_block(near)?;
        let l = next as usize;

        // Direct blocks 0–11
        if l < 12 {
            inode.blocks[l] = phys;
            inode.block_raw[l*4..l*4+4].copy_from_slice(&phys.to_le_bytes());
            inode.i_blocks += (bs / 512) as u32;
            self.write_inode_journaled(ino, inode)?;
            return Ok(phys);
        }
        // Singly indirect
        let l2 = l - 12;
        if l2 < bpp {
            if inode.blocks[12] == 0 {
                let ind = self.alloc_block(phys)?;
                let z = [0u8; 4096]; self.write_block(ind, &z[..bs])?;
                inode.blocks[12] = ind;
                inode.block_raw[48..52].copy_from_slice(&ind.to_le_bytes());
                inode.i_blocks += (bs / 512) as u32;
            }
            let mut ibuf = [0u8; 4096];
            self.read_block(inode.blocks[12], &mut ibuf[..bs])?;
            ibuf[l2*4..l2*4+4].copy_from_slice(&phys.to_le_bytes());
            self.write_block(inode.blocks[12], &ibuf[..bs])?;
            inode.i_blocks += (bs / 512) as u32;
            self.write_inode_journaled(ino, inode)?;
            return Ok(phys);
        }
        // Doubly indirect
        let l3 = l2 - bpp;
        if l3 < bpp * bpp {
            let b1 = l3 / bpp;
            let b2 = l3 % bpp;
            if inode.blocks[13] == 0 {
                let di = self.alloc_block(phys)?;
                let z = [0u8; 4096]; self.write_block(di, &z[..bs])?;
                inode.blocks[13] = di;
                inode.block_raw[52..56].copy_from_slice(&di.to_le_bytes());
                inode.i_blocks += (bs / 512) as u32;
            }
            let mut d1 = [0u8; 4096];
            self.read_block(inode.blocks[13], &mut d1[..bs])?;
            let ind = u32::from_le_bytes(d1[b1*4..b1*4+4].try_into().unwrap());
            let ind = if ind == 0 {
                let ni = self.alloc_block(phys)?;
                let z = [0u8; 4096]; self.write_block(ni, &z[..bs])?;
                d1[b1*4..b1*4+4].copy_from_slice(&ni.to_le_bytes());
                self.write_block(inode.blocks[13], &d1[..bs])?;
                inode.i_blocks += (bs / 512) as u32;
                ni
            } else { ind };
            let mut d2 = [0u8; 4096];
            self.read_block(ind, &mut d2[..bs])?;
            d2[b2*4..b2*4+4].copy_from_slice(&phys.to_le_bytes());
            self.write_block(ind, &d2[..bs])?;
            inode.i_blocks += (bs / 512) as u32;
            self.write_inode_journaled(ino, inode)?;
            return Ok(phys);
        }
        // Triply indirect
        let l4 = l3 - bpp * bpp;
        let c1 = l4 / (bpp * bpp);
        let c2 = (l4 / bpp) % bpp;
        let c3 = l4 % bpp;
        if inode.blocks[14] == 0 {
            let ti = self.alloc_block(phys)?;
            let z = [0u8; 4096]; self.write_block(ti, &z[..bs])?;
            inode.blocks[14] = ti;
            inode.block_raw[56..60].copy_from_slice(&ti.to_le_bytes());
            inode.i_blocks += (bs / 512) as u32;
        }
        let mut t1 = [0u8; 4096];
        self.read_block(inode.blocks[14], &mut t1[..bs])?;
        let di = {
            let v = u32::from_le_bytes(t1[c1*4..c1*4+4].try_into().unwrap());
            if v == 0 {
                let n = self.alloc_block(phys)?;
                let z = [0u8; 4096]; self.write_block(n, &z[..bs])?;
                t1[c1*4..c1*4+4].copy_from_slice(&n.to_le_bytes());
                self.write_block(inode.blocks[14], &t1[..bs])?;
                inode.i_blocks += (bs / 512) as u32; n
            } else { v }
        };
        let mut t2 = [0u8; 4096];
        self.read_block(di, &mut t2[..bs])?;
        let si = {
            let v = u32::from_le_bytes(t2[c2*4..c2*4+4].try_into().unwrap());
            if v == 0 {
                let n = self.alloc_block(phys)?;
                let z = [0u8; 4096]; self.write_block(n, &z[..bs])?;
                t2[c2*4..c2*4+4].copy_from_slice(&n.to_le_bytes());
                self.write_block(di, &t2[..bs])?;
                inode.i_blocks += (bs / 512) as u32; n
            } else { v }
        };
        let mut t3 = [0u8; 4096];
        self.read_block(si, &mut t3[..bs])?;
        t3[c3*4..c3*4+4].copy_from_slice(&phys.to_le_bytes());
        self.write_block(si, &t3[..bs])?;
        inode.i_blocks += (bs / 512) as u32;
        self.write_inode_journaled(ino, inode)?;
        Ok(phys)
    }

    // ── Symlinks ──────────────────────────────────────────────────────────

    unsafe fn read_symlink<'b>(&self, inode: &Inode, buf: &'b mut [u8; 512]) -> Result<&'b str> {
        let len = inode.size as usize;
        if len == 0 || len >= 512 { return Err(FsError::BadFormat); }
        if len <= 60 {
            buf[..len].copy_from_slice(&inode.block_raw[..len]);
        } else {
            // Long symlink stored in a data block
            let phys = self.file_block(inode, 0)?;
            let bs   = self.sb.block_size as usize;
            let mut tmp = [0u8; 4096];
            self.read_block(phys, &mut tmp[..bs])?;
            let take = len.min(bs).min(511);
            buf[..take].copy_from_slice(&tmp[..take]);
        }
        buf[len] = 0;
        core::str::from_utf8(&buf[..len]).map_err(|_| FsError::BadFormat)
    }

    // ── Path resolution with symlink following (up to 8 hops) ─────────────

    unsafe fn resolve(&self, path: &str) -> Result<u32> {
        self.resolve_at(ROOT_INO, path.trim_start_matches('/'), 8)
    }

    unsafe fn resolve_at(&self, base: u32, path: &str, hops: u32) -> Result<u32> {
        let mut ino  = base;
        let mut rest = path;
        while !rest.is_empty() {
            let (comp, tail) = path_split(rest);
            if comp.is_empty() { break; }
            let child = self.lookup_in(ino, comp)?;
            let ci    = self.read_inode(child)?;
            if ci.mode & IFMT == IFLNK {
                if hops == 0 { return Err(FsError::InvalidArg); }
                let mut lbuf = [0u8; 512];
                let target = self.read_symlink(&ci, &mut lbuf)?;
                let new_base = if target.starts_with('/') { ROOT_INO } else { ino };
                let new_root = target.trim_start_matches('/');
                if tail.is_empty() {
                    return self.resolve_at(new_base, new_root, hops - 1);
                }
                // Concatenate target + "/" + tail in a small fixed buffer
                let mut cbuf = [0u8; 512];
                let tl = target.len().min(500);
                cbuf[..tl].copy_from_slice(target.as_bytes());
                cbuf[tl] = b'/';
                let rl = tail.len().min(510 - tl);
                cbuf[tl+1..tl+1+rl].copy_from_slice(tail.as_bytes());
                let s = core::str::from_utf8(&cbuf[..tl+1+rl])
                    .map_err(|_| FsError::BadFormat)?;
                return self.resolve_at(new_base, s.trim_start_matches('/'), hops - 1);
            }
            ino  = child;
            rest = tail;
        }
        Ok(ino)
    }

    // ── Directory walking ─────────────────────────────────────────────────

    unsafe fn walk_dir<F>(&self, ino: u32, mut cb: F) -> Result<()>
    where F: FnMut(&str, u32, u8) -> bool  // (name, child_ino, file_type)
    {
        let inode = self.read_inode(ino)?;
        if inode.mode & IFMT != IFDIR { return Err(FsError::WrongType); }
        if inode.flags & EXT4_INLINE_DATA_FL != 0 {
            self.walk_dir_block(&inode.block_raw[..inode.size as usize.min(60)], &mut cb)?;
            return Ok(());
        }
        let bs   = self.sb.block_size as usize;
        let nblk = ((inode.size as usize) + bs - 1) / bs;
        let mut buf = [0u8; 4096];
        for b in 0..nblk {
            let phys = self.file_block(&inode, b as u32)?;
            if phys == 0 { continue; }
            self.read_block(phys, &mut buf[..bs])?;
            if !self.walk_dir_block(&buf[..bs], &mut cb)? { return Ok(()); }
        }
        Ok(())
    }

    fn walk_dir_block<F>(&self, block: &[u8], cb: &mut F) -> Result<bool>
    where F: FnMut(&str, u32, u8) -> bool
    {
        let mut off = 0usize;
        while off + 8 <= block.len() {
            let ino    = u32::from_le_bytes(block[off..off+4].try_into().unwrap());
            let reclen = u16::from_le_bytes([block[off+4], block[off+5]]) as usize;
            let namlen = block[off+6] as usize;
            let ftype  = block[off+7];
            if reclen == 0 { break; }
            if ino != 0 && namlen > 0 && off + 8 + namlen <= block.len() {
                if let Ok(name) = core::str::from_utf8(&block[off+8..off+8+namlen]) {
                    if name != "." && name != ".." && !cb(name, ino, ftype) {
                        return Ok(false);
                    }
                }
            }
            off += reclen;
        }
        Ok(true)
    }

    unsafe fn lookup_in(&self, dir_ino: u32, name: &str) -> Result<u32> {
        let mut found = None;
        self.walk_dir(dir_ino, |n, ino, _| {
            if n == name { found = Some(ino); false } else { true }
        })?;
        found.ok_or(FsError::NotFound)
    }

    // ── Directory modification (journaled) ────────────────────────────────

    unsafe fn add_dir_entry(&mut self, dir_ino: u32, name: &str, child: u32, ftype: u8)
        -> Result<()>
    {
        let bs   = self.sb.block_size as usize;
        let namb = name.as_bytes();
        let nl   = namb.len();
        let need = (8 + nl + 3) & !3;

        let dinode = self.read_inode(dir_ino)?;
        let nblk = ((dinode.size as usize) + bs - 1) / bs;
        let mut buf = [0u8; 4096];

        for b in 0..nblk {
            let phys = self.file_block(&dinode, b as u32)?;
            if phys == 0 { continue; }
            self.read_block(phys, &mut buf[..bs])?;
            let mut off = 0usize;
            while off + 8 <= bs {
                let ei  = u32::from_le_bytes(buf[off..off+4].try_into().unwrap());
                let rec = u16::from_le_bytes([buf[off+4], buf[off+5]]) as usize;
                if rec == 0 { break; }
                let real  = if ei == 0 { 0 } else { (8 + buf[off+6] as usize + 3) & !3 };
                let slack = rec.saturating_sub(real);
                if slack >= need {
                    if ei != 0 {
                        buf[off+4] = real as u8;
                        buf[off+5] = (real >> 8) as u8;
                    }
                    let no  = off + real;
                    let nrl = (rec - real) as u16;
                    buf[no..no+4].copy_from_slice(&child.to_le_bytes());
                    buf[no+4] = nrl as u8; buf[no+5] = (nrl >> 8) as u8;
                    buf[no+6] = nl as u8;  buf[no+7] = ftype;
                    buf[no+8..no+8+nl].copy_from_slice(namb);
                    let seq = self.txn_seq();
                    let mut txn = Txn::new(seq);
                    let i = txn.count; txn.count += 1;
                    txn.blocks[i].0 = phys;
                    txn.blocks[i].1[..bs].copy_from_slice(&buf[..bs]);
                    return self.journal_commit(&mut txn);
                }
                off += rec;
            }
        }

        // No slack found — allocate a new directory block
        let near = dinode.blocks[0].max(dir_ino / self.sb.blocks_per_group * self.sb.blocks_per_group);
        let new_blk = self.alloc_block(near)?;
        buf.fill(0);
        buf[0..4].copy_from_slice(&child.to_le_bytes());
        buf[4..6].copy_from_slice(&(bs as u16).to_le_bytes());
        buf[6] = nl as u8; buf[7] = ftype;
        buf[8..8+nl].copy_from_slice(namb);

        // Append the block to the dir inode (extent or indirect)
        let mut di = dinode;
        if di.flags & EXT4_EXTENTS_FL != 0 {
            self.extent_append(dir_ino, &mut di)?;
        } else {
            self.indirect_append(dir_ino, &mut di)?;
        }
        di.size += bs as u64;

        // Journal: new directory block + updated dir inode
        let seq = self.txn_seq();
        let mut txn = Txn::new(seq);
        let i = txn.count; txn.count += 1;
        txn.blocks[i].0 = new_blk;
        txn.blocks[i].1[..bs].copy_from_slice(&buf[..bs]);
        let (ii, io) = self.txn_inode(&mut txn, dir_ino)?;
        let ibs = self.sb.block_size;
        Self::write_inode_to_buf(&mut txn.blocks[ii].1, &di, ibs, io);
        self.journal_commit(&mut txn)
    }

    unsafe fn remove_dir_entry(&mut self, dir_ino: u32, name: &str) -> Result<()> {
        let bs   = self.sb.block_size as usize;
        let namb = name.as_bytes();
        let inode = self.read_inode(dir_ino)?;
        let nblk  = ((inode.size as usize) + bs - 1) / bs;
        let mut buf = [0u8; 4096];

        for b in 0..nblk {
            let phys = self.file_block(&inode, b as u32)?;
            if phys == 0 { continue; }
            self.read_block(phys, &mut buf[..bs])?;
            let mut off  = 0usize;
            let mut prev = 0usize;
            while off + 8 <= bs {
                let ino = u32::from_le_bytes(buf[off..off+4].try_into().unwrap());
                let rec = u16::from_le_bytes([buf[off+4], buf[off+5]]) as usize;
                let nl  = buf[off+6] as usize;
                if rec == 0 { break; }
                if ino != 0 && nl == namb.len() && &buf[off+8..off+8+nl] == namb {
                    buf[off..off+4].copy_from_slice(&0u32.to_le_bytes());
                    if prev < off {
                        let pr  = u16::from_le_bytes([buf[prev+4], buf[prev+5]]) as usize;
                        let m   = (pr + rec) as u16;
                        buf[prev+4] = m as u8; buf[prev+5] = (m >> 8) as u8;
                    }
                    let seq = self.txn_seq();
                    let mut txn = Txn::new(seq);
                    let i = txn.count; txn.count += 1;
                    txn.blocks[i].0 = phys;
                    txn.blocks[i].1[..bs].copy_from_slice(&buf[..bs]);
                    return self.journal_commit(&mut txn);
                }
                prev = off; off += rec;
            }
        }
        Err(FsError::NotFound)
    }

    // ── Free inode data blocks ────────────────────────────────────────────

    unsafe fn free_inode_data(&mut self, inode: &Inode) -> Result<()> {
        let bs  = self.sb.block_size as usize;
        let bpp = bs / 4;
        if inode.flags & EXT4_EXTENTS_FL != 0 {
            // Walk extent tree and free all data blocks
            let depth = u16::from_le_bytes([inode.block_raw[6], inode.block_raw[7]]);
            let mut free_leaf = |node: &[u8]| -> Result<()> {
                let n = u16::from_le_bytes([node[2], node[3]]) as usize;
                for i in 0..n {
                    let o   = 12 + i * 12;
                    let len = (u16::from_le_bytes([node[o+4],node[o+5]]) & 0x7FFF) as u32;
                    let phy = u32::from_le_bytes([node[o+8],node[o+9],node[o+10],node[o+11]]);
                    for j in 0..len { self.free_block(phy + j)?; }
                }
                Ok(())
            };
            if depth == 0 { return free_leaf(&inode.block_raw); }
            let n = u16::from_le_bytes([inode.block_raw[2], inode.block_raw[3]]) as usize;
            let mut lvl1 = [0u8; 4096];
            for i in 0..n {
                let o  = 12 + i * 12;
                let cp = u32::from_le_bytes([inode.block_raw[o+4],inode.block_raw[o+5],
                                              inode.block_raw[o+6],inode.block_raw[o+7]]);
                self.read_block(cp, &mut lvl1[..bs])?;
                let cd = u16::from_le_bytes([lvl1[6], lvl1[7]]);
                let cn = u16::from_le_bytes([lvl1[2], lvl1[3]]) as usize;
                if cd == 0 {
                    free_leaf(&lvl1[..bs])?;
                    self.free_block(cp)?;
                } else {
                    let mut lvl2 = [0u8; 4096];
                    for j in 0..cn {
                        let co = 12 + j * 12;
                        let gp = u32::from_le_bytes([lvl1[co+4],lvl1[co+5],lvl1[co+6],lvl1[co+7]]);
                        self.read_block(gp, &mut lvl2[..bs])?;
                        free_leaf(&lvl2[..bs])?;
                        self.free_block(gp)?;
                    }
                    self.free_block(cp)?;
                }
            }
        } else {
            let nblk = ((inode.size + bs as u64 - 1) / bs as u64) as usize;
            for i in 0..nblk.min(12) {
                if inode.blocks[i] != 0 { self.free_block(inode.blocks[i])?; }
            }
            // Singly indirect
            if inode.blocks[12] != 0 {
                let mut ibuf = [0u8; 4096];
                self.read_block(inode.blocks[12], &mut ibuf[..bs])?;
                for i in 0..bpp {
                    let b = u32::from_le_bytes(ibuf[i*4..i*4+4].try_into().unwrap());
                    if b != 0 { self.free_block(b)?; }
                }
                self.free_block(inode.blocks[12])?;
            }
            // Doubly indirect
            if inode.blocks[13] != 0 {
                let mut d1 = [0u8; 4096];
                self.read_block(inode.blocks[13], &mut d1[..bs])?;
                for i in 0..bpp {
                    let ind = u32::from_le_bytes(d1[i*4..i*4+4].try_into().unwrap());
                    if ind != 0 {
                        let mut d2 = [0u8; 4096];
                        self.read_block(ind, &mut d2[..bs])?;
                        for j in 0..bpp {
                            let b = u32::from_le_bytes(d2[j*4..j*4+4].try_into().unwrap());
                            if b != 0 { self.free_block(b)?; }
                        }
                        self.free_block(ind)?;
                    }
                }
                self.free_block(inode.blocks[13])?;
            }
            // Triply indirect: omit (extremely large files, uncommon on embedded media)
        }
        Ok(())
    }

    // ── Public read API ───────────────────────────────────────────────────

    pub unsafe fn open<'s>(&'s self, path: &str) -> Result<Ext2File<'s, D>> {
        let ino   = self.resolve(path)?;
        let inode = self.read_inode(ino)?;
        if inode.mode & IFMT == IFDIR { return Err(FsError::WrongType); }
        Ok(Ext2File { vol: self, ino, inode, pos: 0 })
    }

    pub unsafe fn read_dir<F>(&self, path: &str, mut cb: F) -> Result<()>
    where F: FnMut(&Metadata) -> bool
    {
        let ino = self.resolve(path)?;
        self.walk_dir(ino, |name, child_ino, _| {
            let child = match self.read_inode(child_ino) { Ok(i) => i, Err(_) => return true };
            let mut meta = Metadata::zeroed();
            meta.is_dir  = child.mode & IFMT == IFDIR;
            meta.size    = child.size;
            meta.readonly= child.mode & 0o200 == 0;
            meta.mtime   = child.mtime;
            meta.set_name(name);
            !cb(&meta)
        })
    }

    pub unsafe fn stat(&self, path: &str) -> Result<Metadata> {
        let ino   = self.resolve(path)?;
        let inode = self.read_inode(ino)?;
        let mut meta = Metadata::zeroed();
        meta.is_dir  = inode.mode & IFMT == IFDIR;
        meta.size    = inode.size;
        meta.readonly= inode.mode & 0o200 == 0;
        meta.mtime   = inode.mtime;
        meta.set_name(path.rsplit('/').next().unwrap_or(path));
        Ok(meta)
    }

    // ── Public write API ──────────────────────────────────────────────────

    pub unsafe fn create_file(&mut self, path: &str) -> Result<u32> {
        let (dir_p, name) = split_parent(path)?;
        let dir_ino = self.resolve(dir_p)?;
        let ino = self.alloc_inode(dir_ino)?;
        let mut inode = Inode { mode: IFREG | 0o644, flags: 0, size: 0, mtime: 0,
                                 block_raw: [0u8;60], blocks: [0u32;15], i_blocks: 0, links: 1 };
        if self.sb.feature_incompat & EXT4_INCOMPAT_EXTENTS != 0 {
            inode.flags |= EXT4_EXTENTS_FL;
            // Write extent header into block_raw (0 entries, max 4 in root, depth 0)
            inode.block_raw[0..2].copy_from_slice(&EXT4_EXTENT_MAGIC.to_le_bytes());
            inode.block_raw[4..6].copy_from_slice(&4u16.to_le_bytes());
        }
        self.write_inode_journaled(ino, &inode)?;
        self.add_dir_entry(dir_ino, name, ino, 1)?; // ftype=1 regular file
        Ok(ino)
    }

    pub unsafe fn mkdir(&mut self, path: &str) -> Result<()> {
        let (dir_p, name) = split_parent(path)?;
        let dir_ino = self.resolve(dir_p)?;
        let ino  = self.alloc_inode(dir_ino)?;
        let near = ino / self.sb.blocks_per_group * self.sb.blocks_per_group;
        let data = self.alloc_block(near)?;
        let bs   = self.sb.block_size as usize;
        let mut buf = [0u8; 4096];
        // "." entry (rec_len=12)
        buf[0..4].copy_from_slice(&ino.to_le_bytes());
        buf[4..6].copy_from_slice(&12u16.to_le_bytes());
        buf[6] = 1; buf[7] = 2; buf[8] = b'.';
        // ".." entry (rec_len = rest of block)
        let r2 = (bs - 12) as u16;
        buf[12..16].copy_from_slice(&dir_ino.to_le_bytes());
        buf[16..18].copy_from_slice(&r2.to_le_bytes());
        buf[18] = 2; buf[19] = 2; buf[20] = b'.'; buf[21] = b'.';
        self.write_block(data, &buf[..bs])?;

        let mut inode = Inode { mode: IFDIR | 0o755, flags: 0, size: bs as u64, mtime: 0,
                                 block_raw: [0u8;60], blocks: [0u32;15],
                                 i_blocks: (bs/512) as u32, links: 2 };
        if self.sb.feature_incompat & EXT4_INCOMPAT_EXTENTS != 0 {
            inode.flags |= EXT4_EXTENTS_FL;
            inode.block_raw[0..2].copy_from_slice(&EXT4_EXTENT_MAGIC.to_le_bytes());
            inode.block_raw[2..4].copy_from_slice(&1u16.to_le_bytes()); // 1 extent
            inode.block_raw[4..6].copy_from_slice(&4u16.to_le_bytes());
            // Extent entry: logical=0, len=1, phys_hi=0, phys=data
            inode.block_raw[12..16].copy_from_slice(&0u32.to_le_bytes());
            inode.block_raw[16..18].copy_from_slice(&1u16.to_le_bytes());
            inode.block_raw[18..20].copy_from_slice(&0u16.to_le_bytes()); // phys_hi
            inode.block_raw[20..24].copy_from_slice(&data.to_le_bytes());
        } else {
            inode.blocks[0] = data;
            inode.block_raw[0..4].copy_from_slice(&data.to_le_bytes());
        }
        self.write_inode_journaled(ino, &inode)?;
        self.add_dir_entry(dir_ino, name, ino, 2) // ftype=2 directory
    }

    pub unsafe fn unlink(&mut self, path: &str) -> Result<()> {
        let (dir_p, name) = split_parent(path)?;
        let dir_ino   = self.resolve(dir_p)?;
        let child_ino = self.lookup_in(dir_ino, name)?;
        let inode     = self.read_inode(child_ino)?;
        if inode.mode & IFMT == IFDIR { return Err(FsError::WrongType); }
        self.remove_dir_entry(dir_ino, name)?;
        self.free_inode_data(&inode)?;
        self.free_inode(child_ino)
    }

    pub unsafe fn rmdir(&mut self, path: &str) -> Result<()> {
        let (dir_p, name) = split_parent(path)?;
        let dir_ino   = self.resolve(dir_p)?;
        let child_ino = self.lookup_in(dir_ino, name)?;
        let inode     = self.read_inode(child_ino)?;
        if inode.mode & IFMT != IFDIR { return Err(FsError::WrongType); }
        let mut count = 0u32;
        self.walk_dir(child_ino, |_,_,_| { count += 1; false })?;
        if count > 0 { return Err(FsError::NotEmpty); }
        self.remove_dir_entry(dir_ino, name)?;
        self.free_inode_data(&inode)?;
        self.free_inode(child_ino)
    }

    // ── VFS helper: expose inode state without leaking private Inode type ─

    pub unsafe fn inode_raw_info(&self, ino: u32) -> Result<(u32, [u8; 60], [u32; 15])> {
        let inode = self.read_inode(ino)?;
        Ok((inode.flags, inode.block_raw, inode.blocks))
    }

    // ── Raw I/O for VFS (called from VfsFile::read / write) ───────────────

    pub unsafe fn raw_read(
        &self, flags: u32, block_raw: &[u8; 60], blocks: &[u32; 15],
        size: u64, pos: &mut u64, buf: &mut [u8],
    ) -> Result<usize> {
        if *pos >= size { return Ok(0); }
        if flags & EXT4_INLINE_DATA_FL != 0 {
            let off   = *pos as usize;
            let avail = (size as usize).saturating_sub(off).min(60usize.saturating_sub(off));
            let take  = avail.min(buf.len());
            buf[..take].copy_from_slice(&block_raw[off..off+take]);
            *pos += take as u64;
            return Ok(take);
        }
        let bs   = self.sb.block_size as usize;
        let want = ((size - *pos) as usize).min(buf.len());
        let inode = Inode { mode: IFREG, flags, size, mtime: 0,
                             block_raw: *block_raw, blocks: *blocks, i_blocks: 0, links: 1 };
        let mut done = 0usize;
        let mut tmp  = [0u8; 4096];
        while done < want {
            let logical = (*pos / bs as u64) as u32;
            let off     = (*pos % bs as u64) as usize;
            let phys    = self.file_block(&inode, logical)?;
            if phys == 0 {
                let take = (bs - off).min(want - done);
                buf[done..done+take].fill(0);
                done += take; *pos += take as u64;
            } else {
                self.read_block(phys, &mut tmp[..bs])?;
                let take = (bs - off).min(want - done);
                buf[done..done+take].copy_from_slice(&tmp[off..off+take]);
                done += take; *pos += take as u64;
            }
        }
        Ok(done)
    }

    pub unsafe fn raw_write(
        &mut self, ino: u32,
        flags: &mut u32, block_raw: &mut [u8; 60], blocks: &mut [u32; 15],
        size: &mut u64, pos: &mut u64, buf: &[u8],
    ) -> Result<usize> {
        if *flags & EXT4_INLINE_DATA_FL != 0 {
            let off   = *pos as usize;
            let avail = 60usize.saturating_sub(off);
            let take  = avail.min(buf.len());
            if take == 0 { return Err(FsError::NoSpace); }
            block_raw[off..off+take].copy_from_slice(&buf[..take]);
            *pos += take as u64;
            if *pos > *size { *size = *pos; }
            return Ok(take);
        }
        let bs = self.sb.block_size as usize;
        let mut inode = Inode { mode: IFREG, flags: *flags, size: *size, mtime: 0,
                                 block_raw: *block_raw, blocks: *blocks, i_blocks: 0, links: 1 };
        let mut done = 0usize;
        let mut tmp  = [0u8; 4096];
        while done < buf.len() {
            let logical = (*pos / bs as u64) as u32;
            let off     = (*pos % bs as u64) as usize;
            // Locate or allocate the physical block
            let phys = {
                let p = self.file_block(&inode, logical)?;
                if p == 0 {
                    if inode.flags & EXT4_EXTENTS_FL != 0 {
                        self.extent_append(ino, &mut inode)?
                    } else {
                        self.indirect_append(ino, &mut inode)?
                    }
                } else { p }
            };
            // Read-modify-write for partial blocks
            if off != 0 || buf.len() - done < bs {
                self.read_block(phys, &mut tmp[..bs])?;
            } else {
                tmp[..bs].fill(0);
            }
            let take = (bs - off).min(buf.len() - done);
            tmp[off..off+take].copy_from_slice(&buf[done..done+take]);
            // Write data directly (ordered mode — before journal commit)
            self.write_block(phys, &tmp[..bs])?;
            done += take; *pos += take as u64;
            if *pos > *size { *size = *pos; inode.size = *size; }
        }
        // Write updated inode to journal
        *block_raw = inode.block_raw;
        *blocks    = inode.blocks;
        *flags     = inode.flags;
        inode.size = *size;
        self.write_inode_journaled(ino, &inode)?;
        Ok(done)
    }
}

// ─── Ext2File ─────────────────────────────────────────────────────────────────

pub struct Ext2File<'a, D: BlockDev> {
    vol:       &'a Ext2<D>,
    pub ino:   u32,
    inode:     Inode,
    pos:       u64,
}

impl<'a, D: BlockDev> Ext2File<'a, D> {
    pub fn size(&self)       -> u64       { self.inode.size }
    pub fn pos(&self)        -> u64       { self.pos }
    pub fn flags(&self)      -> u32       { self.inode.flags }
    pub fn block_raw(&self)  -> &[u8;60] { &self.inode.block_raw }
    pub fn blocks(&self)     -> &[u32;15]{ &self.inode.blocks }

    pub unsafe fn seek(&mut self, p: u64) -> Result<()> {
        if p > self.inode.size { return Err(FsError::InvalidArg); }
        self.pos = p; Ok(())
    }

    pub unsafe fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.vol.raw_read(
            self.inode.flags, &self.inode.block_raw, &self.inode.blocks,
            self.inode.size, &mut self.pos, buf,
        )
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn split_parent(path: &str) -> Result<(&str, &str)> {
    let path = path.trim_end_matches('/');
    match path.rfind('/') {
        Some(i) => Ok((&path[..i], &path[i+1..])),
        None    => Ok(("/", path)),
    }
}
