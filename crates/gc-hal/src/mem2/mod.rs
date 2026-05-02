//! MEM2 — Wii extended RAM (64 MB at physical 0x10000000).
//!
//! Only available when compiled with `--features wii`.
//!
//! ## Memory layout
//!
//! ```text
//! 0x90000000  MEM2 cached start
//! 0x90400000  IOS reservation end (~4 MB for IOS)
//! 0x90400000  Homebrew-usable MEM2 start (cached)
//! 0x93FFFFFF  MEM2 end (64 MB total)
//! 0xD0000000  MEM2 uncached mirror
//! ```
//!
//! ## Usage
//!
//! ```rust,no_run
//! use gc_hal::mem2;
//! let heap_start = mem2::HEAP_START;
//! let heap_size  = mem2::HEAP_SIZE;
//! // Use heap_start..heap_start+heap_size for a second allocator heap.
//! ```

/// Start of the MEM2 cached region (physical 0x10000000 → virtual 0x90000000).
pub const MEM2_START: usize = 0x9000_0000;

/// End of MEM2 (physical 0x14000000 → virtual 0x94000000).
pub const MEM2_END: usize = 0x9400_0000;

/// Start of the uncached MEM2 mirror.
pub const MEM2_UNCACHED: usize = 0xD000_0000;

/// Bytes reserved by IOS at the start of MEM2.
pub const IOS_RESERVED: usize = 4 * 1024 * 1024; // 4 MB

/// First byte of MEM2 available to homebrew (cached).
pub const HEAP_START: usize = MEM2_START + IOS_RESERVED;

/// Total homebrew-usable MEM2 bytes.
pub const HEAP_SIZE: usize = MEM2_END - HEAP_START;

/// Convert a cached MEM2 virtual address to its uncached mirror.
#[inline(always)]
pub fn to_uncached(addr: usize) -> usize {
    debug_assert!(addr >= MEM2_START && addr < MEM2_END,
        "address not in MEM2 cached range");
    addr - MEM2_START + MEM2_UNCACHED
}

/// Convert a physical MEM2 address (0x10000000-based) to cached virtual.
#[inline(always)]
pub fn from_physical(phys: usize) -> usize {
    debug_assert!(phys >= 0x1000_0000 && phys < 0x1400_0000,
        "not a MEM2 physical address");
    phys - 0x1000_0000 + MEM2_START
}
